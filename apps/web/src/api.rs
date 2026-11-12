//! 서버 HTTP API 호출.

use gloo_net::http::{Request, Response};
use serde::{Deserialize, de::DeserializeOwned};
use shared::{
    Broadcast, UPLOAD_POLICY_PATH, UploadPolicy, broadcast_path, live_playlist_path,
    recording_path, registration_fields,
};
use uuid::Uuid;
use web_sys::{File, FormData};

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
    let request = Request::post(&url("/api/broadcasts"))
        .body(form)
        .map_err(|_| "업로드를 준비할 수 없습니다.".to_owned())?;
    let response = request.send().await.map_err(|_| {
        // 서버가 한도 초과 등으로 업로드 도중 연결을 끊으면 응답 대신 네트워크 오류가 된다.
        "업로드에 실패했습니다. 파일 크기와 네트워크 상태를 확인하고 다시 시도해 주세요.".to_owned()
    })?;
    read_json(Ok(response)).await
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
        return Err(match response.json::<ErrorBody>().await {
            Ok(body) => body.message,
            Err(_) => format!("요청이 실패했습니다 (HTTP {}).", response.status()),
        });
    }
    response
        .json()
        .await
        .map_err(|_| "서버 응답을 해석할 수 없습니다.".to_owned())
}
