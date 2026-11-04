//! 서버 HTTP API 호출.

use gloo_net::http::Request;
use serde::Deserialize;
use shared::Broadcast;

/// 서버 주소. 웹을 빌드할 때 `API_BASE_URL`로 정한다.
/// 비워 두면 웹을 서빙한 곳과 같은 출처로 호출한다(배포 시).
const API_BASE_URL: &str = match option_env!("API_BASE_URL") {
    Some(url) => url,
    None => "http://localhost:3000",
};

fn url(path: &str) -> String {
    format!("{}{path}", API_BASE_URL.trim_end_matches('/'))
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

/// 사용자에게 그대로 보여 줄 수 있는 한국어 오류 메시지.
pub type ApiResult<T> = Result<T, String>;

pub async fn list_broadcasts() -> ApiResult<Vec<Broadcast>> {
    let response = Request::get(&url("/api/broadcasts"))
        .send()
        .await
        .map_err(|_| "서버에 연결할 수 없습니다.".to_owned())?;
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
