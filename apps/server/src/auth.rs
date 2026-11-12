//! 회원가입·로그인·로그아웃과 요청의 로그인 사용자 확인. 설계 근거는 ADR 0004.
//!
//! 로그인하면 서버가 서명한 JWT를 돌려주고, 클라이언트는 이후 요청의 `Authorization: Bearer` 헤더에 붙인다.
//! 로그아웃한 토큰은 폐기 목록에 올려 만료 전이라도 쓸 수 없게 한다.

use std::{sync::OnceLock, time::Duration};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    extract::{FromRequestParts, State},
    http::{StatusCode, header, request::Parts},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use shared::{
    AuthResponse, Credentials, LOGIN_PATH, LOGOUT_PATH, ME_PATH, PASSWORD_MAX_CHARS,
    PASSWORD_MIN_CHARS, SIGNUP_PATH, USERNAME_MAX_CHARS, USERNAME_MIN_CHARS, User,
};
use uuid::Uuid;

use crate::{AppState, Config, api::ApiError, db};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(SIGNUP_PATH, post(signup))
        .route(LOGIN_PATH, post(login))
        .route(LOGOUT_PATH, post(logout))
        .route(ME_PATH, get(me))
}

/// 토큰 서명·검증 키와 유효 시간.
pub struct AuthKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
    ttl: Duration,
}

impl AuthKeys {
    pub fn new(config: &Config) -> Self {
        let secret = match &config.jwt_secret {
            Some(secret) => secret.0.as_bytes().to_vec(),
            None => {
                tracing::warn!(
                    "JWT_SECRET이 없어 임시 서명 키를 만듭니다. 서버를 다시 시작하면 모든 사용자가 로그아웃됩니다"
                );
                let mut secret = vec![0u8; 32];
                OsRng.fill_bytes(&mut secret);
                secret
            }
        };
        let mut validation = Validation::new(Algorithm::HS256);
        // 만료 시각을 그대로 지킨다. 기본값(60초)만큼 늦게 만료되지 않게 한다.
        validation.leeway = 0;
        validation.set_required_spec_claims(&["exp", "sub", "jti"]);
        Self {
            encoding: EncodingKey::from_secret(&secret),
            decoding: DecodingKey::from_secret(&secret),
            validation,
            ttl: config.jwt_ttl,
        }
    }

    fn issue(&self, user: &User) -> anyhow::Result<String> {
        let now = Utc::now().timestamp();
        let claims = Claims {
            sub: user.id.to_string(),
            name: user.username.clone(),
            iat: now,
            exp: now + i64::try_from(self.ttl.as_secs())?,
            jti: Uuid::now_v7().to_string(),
        };
        Ok(jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &self.encoding,
        )?)
    }
}

/// 토큰에 담는 내용.
#[derive(Serialize, Deserialize)]
struct Claims {
    /// 사용자 ID.
    sub: String,
    /// 아이디. 확인용이며, 요청을 처리할 때는 DB의 사용자 정보를 쓴다.
    name: String,
    iat: i64,
    exp: i64,
    /// 토큰 ID. 로그아웃할 때 이 값으로 폐기한다.
    jti: String,
}

/// 요청을 보낸 로그인 사용자. 핸들러 인자로 받으면 로그인을 요구하게 된다.
/// 토큰이 없거나 잘못되었거나 만료·폐기되었으면 401로 거절한다.
pub struct AuthUser {
    pub user: User,
    jti: String,
    expires_at: DateTime<Utc>,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = bearer_token(parts).ok_or_else(login_required)?;
        authenticate(state, token).await
    }
}

fn bearer_token(parts: &Parts) -> Option<&str> {
    let value = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

/// 토큰을 검증해 로그인 사용자를 찾는다. HTTP 헤더를 쓸 수 없는 WebSocket도 이 함수를 쓴다.
pub async fn authenticate(state: &AppState, token: &str) -> Result<AuthUser, ApiError> {
    let claims =
        jsonwebtoken::decode::<Claims>(token, &state.auth.decoding, &state.auth.validation)
            .map_err(|_| invalid_token())?
            .claims;
    if db::is_token_revoked(&state.db, &claims.jti).await? {
        return Err(invalid_token());
    }
    let id = Uuid::parse_str(&claims.sub).map_err(|_| invalid_token())?;
    // 다른 DB에서 발급한 토큰이 같은 키로 서명되었더라도 없는 사용자면 거절한다.
    let user = db::find_user(&state.db, id)
        .await?
        .ok_or_else(invalid_token)?;
    let expires_at = DateTime::from_timestamp(claims.exp, 0).ok_or_else(invalid_token)?;
    Ok(AuthUser {
        user,
        jti: claims.jti,
        expires_at,
    })
}

pub fn login_required() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "로그인이 필요합니다.")
}

fn invalid_token() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "로그인이 만료되었거나 올바르지 않습니다. 다시 로그인해 주세요.",
    )
}

/// 회원가입. 가입하면 바로 로그인된 토큰을 돌려준다.
async fn signup(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<(StatusCode, Json<AuthResponse>), ApiError> {
    let username = validate_username(&credentials.username)?;
    validate_password(&credentials.password)?;
    let password_hash = hash_password(credentials.password).await?;
    let user = User {
        id: Uuid::now_v7(),
        username,
    };
    if !db::insert_user(&state.db, &user, &password_hash, Utc::now()).await? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "이미 쓰고 있는 아이디입니다.",
        ));
    }
    tracing::info!(user = %user.id, "회원가입: {}", user.username);
    let token = state.auth.issue(&user)?;
    Ok((StatusCode::CREATED, Json(AuthResponse { token, user })))
}

/// 로그인. 아이디가 없는지 비밀번호가 틀렸는지 드러나지 않게 같은 메시지로 거절한다.
async fn login(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<Json<AuthResponse>, ApiError> {
    let wrong = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "아이디 또는 비밀번호가 올바르지 않습니다.",
        )
    };
    let username = credentials.username.trim().to_lowercase();
    let stored = db::find_user_by_username(&state.db, &username).await?;
    // 없는 아이디도 비밀번호를 확인하는 만큼 시간을 들여, 응답 시간으로 아이디 존재를 알 수 없게 한다.
    let hash = stored.as_ref().map(|stored| stored.password_hash.clone());
    let verified = verify_password(credentials.password, hash).await?;
    let user = match stored {
        Some(stored) if verified => stored.user,
        _ => return Err(wrong()),
    };
    let token = state.auth.issue(&user)?;
    Ok(Json(AuthResponse { token, user }))
}

/// 로그아웃. 요청에 쓴 토큰을 폐기해 만료 전이라도 다시 쓸 수 없게 한다.
async fn logout(State(state): State<AppState>, auth: AuthUser) -> Result<StatusCode, ApiError> {
    db::revoke_token(&state.db, &auth.jti, auth.expires_at).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// 내 정보. 웹이 저장해 둔 토큰이 아직 유효한지 확인할 때 쓴다.
async fn me(auth: AuthUser) -> Json<User> {
    Json(auth.user)
}

/// 아이디 규칙을 확인하고, 소문자로 바꾼 아이디를 돌려준다.
fn validate_username(username: &str) -> Result<String, ApiError> {
    let username = username.trim().to_lowercase();
    let length = username.chars().count();
    let valid = (USERNAME_MIN_CHARS..=USERNAME_MAX_CHARS).contains(&length)
        && username
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "아이디는 {USERNAME_MIN_CHARS}~{USERNAME_MAX_CHARS}자의 영문 소문자, 숫자, 밑줄(_)만 쓸 수 있습니다."
            ),
        ));
    }
    Ok(username)
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    let length = password.chars().count();
    if !(PASSWORD_MIN_CHARS..=PASSWORD_MAX_CHARS).contains(&length) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("비밀번호는 {PASSWORD_MIN_CHARS}~{PASSWORD_MAX_CHARS}자로 입력해 주세요."),
        ));
    }
    Ok(())
}

/// 비밀번호를 argon2id로 해시한다. CPU를 많이 쓰므로 비동기 작업 스레드를 막지 않게 따로 돌린다.
async fn hash_password(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|err| anyhow::anyhow!("비밀번호 해시 실패: {err}"))
    })
    .await?
}

/// 비밀번호가 해시와 맞는지 확인한다. 해시가 없으면(없는 아이디) 맞지 않는 해시와 비교해 같은 시간을 쓴다.
async fn verify_password(password: String, hash: Option<String>) -> anyhow::Result<bool> {
    tokio::task::spawn_blocking(move || {
        let hash = hash.as_deref().unwrap_or_else(|| dummy_hash());
        let hash = PasswordHash::new(hash)
            .map_err(|err| anyhow::anyhow!("저장된 비밀번호 해시를 읽을 수 없습니다: {err}"))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok())
    })
    .await?
}

/// 없는 아이디로 로그인할 때 비교에 쓰는 해시. 어떤 비밀번호와도 맞지 않는다.
fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(Uuid::now_v7().as_bytes(), &salt)
            .expect("임의 값의 해시는 항상 만들 수 있다")
            .to_string()
    })
}
