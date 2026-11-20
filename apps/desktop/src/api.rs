//! 배포된 서버와 주고받는 부분. 화면과 떨어뜨려 단위 테스트로 확인한다.

use std::{sync::OnceLock, time::Duration};

use shared::{Broadcast, BroadcastStatus};
use uuid::Uuid;

/// 빌드할 때 `CHZZK_SERVER_URL`을 주면 그 주소가, 없으면 배포 주소가 기본 서버다.
pub const DEFAULT_SERVER_URL: &str = match option_env!("CHZZK_SERVER_URL") {
    Some(url) => url,
    None => "https://mins.work",
};

/// 앱을 켤 때의 서버 주소. 실행 환경의 `CHZZK_SERVER_URL`이 빌드 때 정한 기본값보다 우선한다.
pub fn initial_server_url() -> String {
    std::env::var("CHZZK_SERVER_URL")
        .ok()
        .map(|url| normalize_server_url(&url))
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| normalize_server_url(DEFAULT_SERVER_URL))
}

/// 사용자가 입력한 서버 주소를 `https://host[:port]` 꼴로 정리한다. 스킴이 없으면 HTTPS로 본다.
pub fn normalize_server_url(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    }
}

/// 서버 주소와 API 경로를 잇는다.
pub fn url(server: &str, path: &str) -> String {
    format!("{}{path}", server.trim_end_matches('/'))
}

/// 방송 하나를 어떻게 재생할지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playback {
    /// 라이브 방송. HLS 재생 목록 주소.
    Live(String),
    /// 종료된 방송의 녹화본 주소.
    Recording(String),
    /// 아직 볼 수 없지만 곧 볼 수 있다(송출 직후, 녹화본을 만드는 중).
    Preparing(&'static str),
    /// 볼 영상이 없다.
    Unavailable,
}

pub fn playback(server: &str, broadcast: &Broadcast) -> Playback {
    match broadcast.status {
        BroadcastStatus::Live if broadcast.live_playlist => {
            Playback::Live(url(server, &shared::live_playlist_path(broadcast.id)))
        }
        BroadcastStatus::Live => {
            Playback::Preparing("송출을 준비하고 있습니다. 잠시 뒤 새로고침해 주세요.")
        }
        BroadcastStatus::Ended if broadcast.recording.is_some() => {
            Playback::Recording(url(server, &shared::recording_path(broadcast.id)))
        }
        BroadcastStatus::Ended if broadcast.live_playlist => {
            Playback::Preparing("녹화본을 만들고 있습니다. 잠시 뒤 새로고침해 주세요.")
        }
        BroadcastStatus::Ended => Playback::Unavailable,
    }
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("chzzk-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP 클라이언트를 만들 수 없습니다")
    })
}

/// 방송 목록. 상태를 주면 그 상태의 방송만 받는다.
pub async fn list_broadcasts(
    server: &str,
    status: Option<BroadcastStatus>,
) -> Result<Vec<Broadcast>, String> {
    let response = client()
        .get(url(server, &shared::broadcasts_path(status)))
        .send()
        .await
        .map_err(|err| format!("서버({server})에 연결하지 못했습니다: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "방송 목록을 받지 못했습니다 (HTTP {})",
            response.status()
        ));
    }
    response
        .json()
        .await
        .map_err(|err| format!("방송 목록을 읽지 못했습니다: {err}"))
}

/// 방송 하나. 목록을 받은 뒤 상태가 바뀌었을 수 있어 재생 직전에 다시 받는다.
pub async fn get_broadcast(server: &str, id: Uuid) -> Result<Broadcast, String> {
    let response = client()
        .get(url(server, &shared::broadcast_path(id)))
        .send()
        .await
        .map_err(|err| format!("서버({server})에 연결하지 못했습니다: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "방송을 받지 못했습니다 (HTTP {})",
            response.status()
        ));
    }
    response
        .json()
        .await
        .map_err(|err| format!("방송을 읽지 못했습니다: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use shared::{Recording, User};

    fn broadcast(status: BroadcastStatus, recording: bool, live_playlist: bool) -> Broadcast {
        Broadcast {
            id: Uuid::nil(),
            title: "t".into(),
            owner: User {
                id: Uuid::nil(),
                username: "u".into(),
            },
            created_at: Utc::now(),
            status,
            ended_at: None,
            recording: recording.then(|| Recording {
                content_type: "video/webm".into(),
                size_bytes: 1,
            }),
            live_playlist,
        }
    }

    #[test]
    fn 서버_주소를_정리한다() {
        assert_eq!(normalize_server_url(" mins.work/ "), "https://mins.work");
        assert_eq!(
            normalize_server_url("http://localhost:8088/"),
            "http://localhost:8088"
        );
        assert_eq!(
            normalize_server_url("https://mins.work"),
            "https://mins.work"
        );
        assert_eq!(normalize_server_url("  "), "");
    }

    #[test]
    fn 기본_서버는_배포_주소다() {
        assert!(
            DEFAULT_SERVER_URL.starts_with("https://") || DEFAULT_SERVER_URL.starts_with("http://")
        );
    }

    #[test]
    fn 라이브는_hls로_재생한다() {
        let id = Uuid::nil();
        assert_eq!(
            playback(
                "https://mins.work/",
                &broadcast(BroadcastStatus::Live, false, true)
            ),
            Playback::Live(format!(
                "https://mins.work/api/broadcasts/{id}/live/index.m3u8"
            ))
        );
    }

    #[test]
    fn 첫_조각_전_라이브는_준비_중이다() {
        assert!(matches!(
            playback(
                "https://mins.work",
                &broadcast(BroadcastStatus::Live, false, false)
            ),
            Playback::Preparing(_)
        ));
    }

    #[test]
    fn 종료된_방송은_녹화본을_재생한다() {
        let id = Uuid::nil();
        assert_eq!(
            playback(
                "https://mins.work",
                &broadcast(BroadcastStatus::Ended, true, false)
            ),
            Playback::Recording(format!("https://mins.work/api/broadcasts/{id}/recording"))
        );
    }

    #[test]
    fn 녹화본을_만드는_중이면_준비_중이다() {
        assert!(matches!(
            playback(
                "https://mins.work",
                &broadcast(BroadcastStatus::Ended, false, true)
            ),
            Playback::Preparing(_)
        ));
        assert_eq!(
            playback(
                "https://mins.work",
                &broadcast(BroadcastStatus::Ended, false, false)
            ),
            Playback::Unavailable
        );
    }
}
