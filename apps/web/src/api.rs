//! 서버 HTTP API 호출.

use gloo_net::http::{Request, RequestBuilder, Response};
use serde::{Deserialize, de::DeserializeOwned};
use shared::{
    AuthResponse, Broadcast, Credentials, LOGIN_PATH, LOGOUT_PATH, ME_PATH, SIGNUP_PATH,
    UPLOAD_POLICY_PATH, UpdateBroadcast, UploadPolicy, User, broadcast_end_path, broadcast_path,
    live_playlist_path, recording_path, registration_fields,
};
use uuid::Uuid;
use web_sys::{File, FormData};

use crate::session;

/// 서버 주소. 웹을 빌드할 때 `API_BASE_URL`로 정한다.
/// 비워 두면 웹을 서빙한 곳과 같은 출처로 호출한다(배포 시).
const API_BASE_URL: &str = match option_env!("API_BASE_URL") {
    Some(url) => url,
    None => "http://localhost:3000",
};

fn url(path: &str) -> String {
    format!("{}{path}", API_BASE_URL.trim_end_matches('/'))
}

/// 사용자에게 그대로 보여 줄 수 있는 한국어 오류 메시지.
pub type ApiResult<T> = Result<T, String>;

pub async fn list_broadcasts() -> ApiResult<Vec<Broadcast>> {
    read_json(Request::get(&url("/api/broadcasts")).send().await).await
}

/// 방송 하나. 없는 방송(형식이 틀린 ID 포함)이면 `None`.
pub async fn get_broadcast(id: &str) -> ApiResult<Option<Broadcast>> {
    let Ok(id) = Uuid::parse_str(id) else {
        return Ok(None);
    };
    let response = Request::get(&url(&broadcast_path(id)))
        .send()
        .await
        .map_err(|_| "서버에 연결할 수 없습니다.".to_owned())?;
    if response.status() == 404 {
        return Ok(None);
    }
    read_json(Ok(response)).await.map(Some)
}

pub async fn get_upload_policy() -> ApiResult<UploadPolicy> {
    read_json(Request::get(&url(UPLOAD_POLICY_PATH)).send().await).await
}

/// 녹화본 재생 주소. `<video src>`에 그대로 쓴다.
pub fn recording_url(id: Uuid) -> String {
    url(&recording_path(id))
}

/// 라이브 재생 목록(HLS) 주소.
pub fn live_playlist_url(id: Uuid) -> String {
    url(&live_playlist_path(id))
}

/// WebSocket 주소. 같은 출처로 서빙될 때는(API 주소가 비어 있으면) 지금 열린 주소를 쓴다.
/// HTTPS로 열렸으면 `wss://`가 된다.
pub fn ws_url(path: &str) -> String {
    let base = if API_BASE_URL.is_empty() {
        web_sys::window()
            .and_then(|window| window.location().origin().ok())
            .unwrap_or_default()
    } else {
        API_BASE_URL.trim_end_matches('/').to_owned()
    };
    let base = match base.split_once("://") {
        Some(("https", rest)) => format!("wss://{rest}"),
        Some(("http", rest)) => format!("ws://{rest}"),
        _ => base,
    };
    format!("{base}{path}")
}

/// 제목과 녹화본으로 방송을 등록한다. 파일은 메모리에 읽지 않고 브라우저가 그대로 전송한다.
pub async fn register_broadcast(title: &str, recording: &File) -> ApiResult<Broadcast> {
    let form = FormData::new().map_err(|_| "업로드를 준비할 수 없습니다.".to_owned())?;
    form.append_with_str(registration_fields::TITLE, title)
        .and_then(|()| {
            form.append_with_blob_and_filename(
                registration_fields::RECORDING,
                recording,
                &recording.name(),
            )
        })
        .map_err(|_| "업로드를 준비할 수 없습니다.".to_owned())?;
    let request = authorized(Request::post(&url("/api/broadcasts")))
        .body(form)
        .map_err(|_| "업로드를 준비할 수 없습니다.".to_owned())?;
    let response = request.send().await.map_err(|_| {
        // 서버가 한도 초과 등으로 업로드 도중 연결을 끊으면 응답 대신 네트워크 오류가 된다.
        "업로드에 실패했습니다. 파일 크기와 네트워크 상태를 확인하고 다시 시도해 주세요.".to_owned()
    })?;
    read_json(Ok(response)).await
}

/// 로그인했으면 요청에 토큰을 붙인다.
fn authorized(request: RequestBuilder) -> RequestBuilder {
    match session::token() {
        Some(token) => request.header("Authorization", &format!("Bearer {token}")),
        None => request,
    }
}

/// 회원가입. 성공하면 바로 로그인된다.
pub async fn signup(credentials: &Credentials) -> ApiResult<AuthResponse> {
    send_credentials(SIGNUP_PATH, credentials).await
}

pub async fn login(credentials: &Credentials) -> ApiResult<AuthResponse> {
    send_credentials(LOGIN_PATH, credentials).await
}

async fn send_credentials(path: &str, credentials: &Credentials) -> ApiResult<AuthResponse> {
    let request = Request::post(&url(path))
        .json(credentials)
        .map_err(|_| "요청을 준비할 수 없습니다.".to_owned())?;
    read_json(request.send().await).await
}

/// 내 정보. 저장해 둔 토큰이 아직 유효한지 확인한다(401이면 로그아웃 상태가 된다).
pub async fn me() -> ApiResult<User> {
    read_json(authorized(Request::get(&url(ME_PATH))).send().await).await
}

/// 서버에서 지금 토큰을 폐기한다.
pub async fn logout() -> ApiResult<()> {
    let response = authorized(Request::post(&url(LOGOUT_PATH)))
        .send()
        .await
        .map_err(|_| "서버에 연결할 수 없습니다.".to_owned())?;
    if response.ok() {
        Ok(())
    } else {
        Err(error_message(response).await)
    }
}

/// 방송 제목을 바꾼다(소유자만).
pub async fn update_broadcast(id: Uuid, title: &str) -> ApiResult<Broadcast> {
    let request = authorized(Request::patch(&url(&broadcast_path(id))))
        .json(&UpdateBroadcast {
            title: title.to_owned(),
        })
        .map_err(|_| "요청을 준비할 수 없습니다.".to_owned())?;
    read_json(request.send().await).await
}

/// 방송을 지운다(소유자만, 종료된 방송만).
pub async fn delete_broadcast(id: Uuid) -> ApiResult<()> {
    let response = authorized(Request::delete(&url(&broadcast_path(id))))
        .send()
        .await
        .map_err(|_| "서버에 연결할 수 없습니다.".to_owned())?;
    if response.ok() {
        Ok(())
    } else {
        Err(error_message(response).await)
    }
}

/// 라이브 방송을 종료한다(소유자만). 송출하던 기기가 아니어도 된다.
pub async fn end_broadcast(id: Uuid) -> ApiResult<Broadcast> {
    read_json(
        authorized(Request::post(&url(&broadcast_end_path(id))))
            .send()
            .await,
    )
    .await
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

async fn read_json<T: DeserializeOwned>(
    response: Result<Response, gloo_net::Error>,
) -> ApiResult<T> {
    let response = response.map_err(|_| "서버에 연결할 수 없습니다.".to_owned())?;
    if !response.ok() {
        return Err(error_message(response).await);
    }
    response
        .json()
        .await
        .map_err(|_| "서버 응답을 해석할 수 없습니다.".to_owned())
}

/// 실패한 응답의 안내 문구. 서버가 토큰을 거절(401)했으면 로그아웃 상태로 바꾼다.
async fn error_message(response: Response) -> String {
    if response.status() == 401 {
        session::sign_out();
    }
    match response.json::<ErrorBody>().await {
        Ok(body) => body.message,
        Err(_) => format!("요청이 실패했습니다 (HTTP {}).", response.status()),
    }
}
