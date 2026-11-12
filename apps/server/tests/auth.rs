//! 회원가입·로그인·로그아웃 통합 테스트.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use shared::{AuthResponse, LOGIN_PATH, LOGOUT_PATH, ME_PATH, SIGNUP_PATH, User};
use tempfile::TempDir;
use tower::ServiceExt;

const TEST_JWT_SECRET: &str = "test-only-jwt-secret";

struct AuthServer {
    _dir: TempDir,
    app: Router,
}

struct Reply {
    status: StatusCode,
    body: Vec<u8>,
}

impl Reply {
    fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap()
    }

    fn message(&self) -> String {
        self.json::<Value>()["message"].as_str().unwrap().to_owned()
    }
}

impl AuthServer {
    async fn start() -> Self {
        Self::with_secret(Some(TEST_JWT_SECRET)).await
    }

    async fn with_secret(secret: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = server::Config {
            database_path: dir.path().join("test.db"),
            storage: server::StorageConfig::Local {
                dir: dir.path().join("recordings"),
            },
            cors_allowed_origins: vec!["http://localhost:8080".into()],
            max_upload_bytes: 1024 * 1024,
            storage_quota_bytes: 10 * 1024 * 1024,
            web_dist_dir: None,
            live_dir: dir.path().join("live"),
            ffmpeg_path: "ffmpeg".into(),
            max_live_broadcasts: 2,
            live_idle_timeout: std::time::Duration::from_secs(20),
            jwt_secret: secret.map(Into::into),
            jwt_ttl: std::time::Duration::from_secs(3600),
        };
        let app = server::build_app(config).await.unwrap();
        Self { _dir: dir, app }
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        Reply { status, body }
    }

    async fn signup(&self, username: &str, password: &str) -> Reply {
        let body = json!({ "username": username, "password": password });
        self.request("POST", SIGNUP_PATH, None, Some(body)).await
    }

    async fn login(&self, username: &str, password: &str) -> Reply {
        let body = json!({ "username": username, "password": password });
        self.request("POST", LOGIN_PATH, None, Some(body)).await
    }

    async fn me(&self, token: Option<&str>) -> Reply {
        self.request("GET", ME_PATH, token, None).await
    }
}

#[tokio::test]
async fn 회원가입하면_201과_토큰을_돌려주고_그_토큰으로_내_정보를_본다() {
    let server = AuthServer::start().await;

    let reply = server.signup("Streamer_1", "password123").await;

    assert_eq!(reply.status, StatusCode::CREATED);
    let auth: AuthResponse = reply.json();
    // 아이디는 소문자로 바꿔 저장한다.
    assert_eq!(auth.user.username, "streamer_1");
    let me = server.me(Some(&auth.token)).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json::<User>(), auth.user);
}

#[tokio::test]
async fn 같은_아이디는_대소문자가_달라도_다시_가입할_수_없다() {
    let server = AuthServer::start().await;
    assert_eq!(
        server.signup("viewer", "password123").await.status,
        StatusCode::CREATED
    );

    let reply = server.signup("VIEWER", "another-pass").await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.message(), "이미 쓰고 있는 아이디입니다.");
}

#[tokio::test]
async fn 규칙에_맞지_않는_아이디와_비밀번호는_400이다() {
    let server = AuthServer::start().await;
    let username_message = "아이디는 3~20자의 영문 소문자, 숫자, 밑줄(_)만 쓸 수 있습니다.";
    let password_message = "비밀번호는 8~72자로 입력해 주세요.";

    for (username, password, message) in [
        ("ab", "password123", username_message),
        ("a".repeat(21).as_str(), "password123", username_message),
        ("한글아이디", "password123", username_message),
        ("with space", "password123", username_message),
        ("valid_name", "short", password_message),
        ("valid_name", "p".repeat(73).as_str(), password_message),
    ]
    .map(|(u, p, m)| (u.to_owned(), p.to_owned(), m))
    {
        let reply = server.signup(&username, &password).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{username} / {password}"
        );
        assert_eq!(reply.message(), message);
    }
    // 거절된 가입은 사용자를 만들지 않는다.
    assert_eq!(
        server.login("valid_name", "short").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn 로그인하면_새_토큰을_돌려준다() {
    let server = AuthServer::start().await;
    let signed_up: AuthResponse = server.signup("streamer", "password123").await.json();

    let reply = server.login("Streamer", "password123").await;

    assert_eq!(reply.status, StatusCode::OK);
    let auth: AuthResponse = reply.json();
    assert_eq!(auth.user, signed_up.user);
    assert_eq!(server.me(Some(&auth.token)).await.status, StatusCode::OK);
}

#[tokio::test]
async fn 틀린_비밀번호와_없는_아이디는_같은_메시지로_401이다() {
    let server = AuthServer::start().await;
    server.signup("streamer", "password123").await;

    let wrong_password = server.login("streamer", "password124").await;
    let unknown_user = server.login("nobody", "password123").await;

    assert_eq!(wrong_password.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_user.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        wrong_password.message(),
        "아이디 또는 비밀번호가 올바르지 않습니다."
    );
    assert_eq!(unknown_user.message(), wrong_password.message());
}

#[tokio::test]
async fn 토큰_없이_내_정보를_보면_401이다() {
    let server = AuthServer::start().await;

    let reply = server.me(None).await;

    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply.message(), "로그인이 필요합니다.");
}

#[tokio::test]
async fn 로그아웃한_토큰은_다시_쓸_수_없고_다른_토큰은_그대로_쓴다() {
    let server = AuthServer::start().await;
    let first: AuthResponse = server.signup("streamer", "password123").await.json();
    let second: AuthResponse = server.login("streamer", "password123").await.json();

    let reply = server
        .request("POST", LOGOUT_PATH, Some(&first.token), None)
        .await;

    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    let me = server.me(Some(&first.token)).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        me.message(),
        "로그인이 만료되었거나 올바르지 않습니다. 다시 로그인해 주세요."
    );
    assert_eq!(server.me(Some(&second.token)).await.status, StatusCode::OK);
    // 이미 로그아웃한 토큰으로 다시 로그아웃할 수도 없다.
    let again = server
        .request("POST", LOGOUT_PATH, Some(&first.token), None)
        .await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
}

/// 테스트가 직접 서명한 토큰. 서버와 같은 형식의 내용을 담는다.
fn forge_token(secret: &str, user: &User, exp_offset_secs: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = json!({
        "sub": user.id.to_string(),
        "name": user.username,
        "iat": now,
        "exp": now + exp_offset_secs,
        "jti": uuid::Uuid::now_v7().to_string(),
    });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

#[tokio::test]
async fn 만료되었거나_다른_키로_서명했거나_형식이_틀린_토큰은_401이다() {
    let server = AuthServer::start().await;
    let auth: AuthResponse = server.signup("streamer", "password123").await.json();

    let valid = forge_token(TEST_JWT_SECRET, &auth.user, 60);
    let expired = forge_token(TEST_JWT_SECRET, &auth.user, -1);
    let other_key = forge_token("another-secret", &auth.user, 60);
    let unknown_user = forge_token(
        TEST_JWT_SECRET,
        &User {
            id: uuid::Uuid::now_v7(),
            username: "ghost".into(),
        },
        60,
    );

    assert_eq!(server.me(Some(&valid)).await.status, StatusCode::OK);
    for token in [expired, other_key, unknown_user, "not-a-jwt".into()] {
        assert_eq!(
            server.me(Some(&token)).await.status,
            StatusCode::UNAUTHORIZED,
            "{token}"
        );
    }
}

#[tokio::test]
async fn 서명_키를_정하지_않아도_임시_키로_로그인된다() {
    let server = AuthServer::with_secret(None).await;

    let auth: AuthResponse = server.signup("streamer", "password123").await.json();

    assert_eq!(server.me(Some(&auth.token)).await.status, StatusCode::OK);
    // 임시 키는 테스트가 아는 키가 아니다.
    let forged = forge_token(TEST_JWT_SECRET, &auth.user, 60);
    assert_eq!(
        server.me(Some(&forged)).await.status,
        StatusCode::UNAUTHORIZED
    );
}
