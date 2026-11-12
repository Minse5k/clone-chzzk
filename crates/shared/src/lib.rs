//! 서버와 웹이 함께 쓰는 API 타입.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 방송 하나. 목록과 상세 화면에 보이는 단위다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Broadcast {
    pub id: Uuid,
    pub title: String,
    /// 등록 시각. 라이브 방송은 송출을 시작한 시각이다.
    pub created_at: DateTime<Utc>,
    pub status: BroadcastStatus,
    /// 방송이 끝난 시각. 라이브 중이면 `None`.
    pub ended_at: Option<DateTime<Utc>>,
    /// 종료된 방송에 남는 영상. 아직 녹화본이 없는 방송이면 `None`.
    pub recording: Option<Recording>,
    /// 라이브 재생 목록(HLS)이 있어 지금 볼 수 있는지. 송출 직후 첫 조각이 만들어지기 전에는 `false`.
    #[serde(default)]
    pub live_playlist: bool,
}

/// 방송이 지금 송출 중인지 여부. `Live`에서 `Ended`로만 바뀐다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BroadcastStatus {
    Live,
    Ended,
}

impl BroadcastStatus {
    /// 종료 시각으로 정한 상태. 종료 시각이 없으면 라이브 중이다.
    pub fn from_ended_at(ended_at: Option<DateTime<Utc>>) -> Self {
        match ended_at {
            None => Self::Live,
            Some(_) => Self::Ended,
        }
    }

    /// API 질의 값(`live`/`ended`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Ended => "ended",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "live" => Some(Self::Live),
            "ended" => Some(Self::Ended),
            _ => None,
        }
    }

    /// 화면에 보이는 이름.
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "라이브 중",
            Self::Ended => "종료",
        }
    }
}

/// 녹화본의 공개 정보. 저장 위치는 서버만 안다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub content_type: String,
    pub size_bytes: u64,
}

/// 방송 등록 전에 확인하는 업로드 한도.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UploadPolicy {
    /// 녹화본 하나의 최대 크기.
    pub max_upload_bytes: u64,
    /// 저장 총량 한도까지 남은 크기.
    pub remaining_bytes: u64,
}

/// 방송 제목의 최대 글자 수(앞뒤 공백 제거 후).
pub const TITLE_MAX_CHARS: usize = 100;

/// 사람이 읽기 쉬운 크기 표기. 예: `2GB`, `1.5GB`, `300MB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = format!("{value:.1}");
    format!("{}{}", text.trim_end_matches(".0"), UNITS[unit])
}

/// 방송 등록 요청(multipart)의 필드 이름.
pub mod registration_fields {
    pub const TITLE: &str = "title";
    pub const RECORDING: &str = "recording";
}

/// 방송 목록 API 경로. 상태를 주면 그 상태의 방송만 돌려준다.
pub fn broadcasts_path(status: Option<BroadcastStatus>) -> String {
    match status {
        None => "/api/broadcasts".into(),
        Some(status) => format!("/api/broadcasts?status={}", status.as_str()),
    }
}

/// 방송 상세 API 경로.
pub fn broadcast_path(id: Uuid) -> String {
    format!("/api/broadcasts/{id}")
}

/// 업로드 한도 API 경로.
pub const UPLOAD_POLICY_PATH: &str = "/api/upload-policy";

/// 녹화본 재생 API 경로. Range 요청을 지원한다.
pub fn recording_path(id: Uuid) -> String {
    format!("/api/broadcasts/{id}/recording")
}

/// 라이브 재생 목록(HLS) 경로. 같은 디렉터리의 조각 파일은 재생 목록 기준 상대 경로로 받는다.
pub fn live_playlist_path(id: Uuid) -> String {
    format!("/api/broadcasts/{id}/live/{LIVE_PLAYLIST_FILE}")
}

/// 라이브 재생 목록 파일 이름.
pub const LIVE_PLAYLIST_FILE: &str = "index.m3u8";

/// 송출 WebSocket 경로.
///
/// 송출자가 연결을 열고 [`IngestClientMessage::Start`]를 보내면 서버가 라이브 방송을 만들고
/// [`IngestServerMessage::Started`]로 답한다. 이후 바이너리 메시지는 모두 브라우저
/// MediaRecorder가 만든 녹화 조각이다. [`IngestClientMessage::End`]를 보내거나 연결이 끊기면
/// 방송이 종료된다. 연결의 수명이 곧 라이브 방송의 수명이다.
pub const LIVE_INGEST_PATH: &str = "/api/live/ingest";

/// 송출자가 보내는 텍스트 메시지.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IngestClientMessage {
    /// 송출 시작. 연결 후 첫 메시지여야 한다.
    Start {
        title: String,
        /// MediaRecorder 녹화 형식. 예: `video/webm;codecs=vp8,opus`, `video/mp4`.
        mime_type: String,
    },
    /// 송출 종료. 마지막 녹화 조각을 보낸 뒤에 보낸다.
    End,
}

/// 서버가 송출자에게 보내는 텍스트 메시지.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IngestServerMessage {
    /// 라이브 방송이 만들어졌다. 이제 녹화 조각을 보내면 된다.
    Started { broadcast: Broadcast },
    /// 방송이 종료되었다. 서버가 먼저 끝낸 경우 `reason`에 이유가 있다.
    Ended { reason: Option<String> },
    /// 송출을 시작할 수 없다. 방송은 만들어지지 않았다.
    Error { message: String },
}

/// 로그인한 사용자. 방송 소유자와 댓글 작성자로도 쓰인다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    /// 아이디. 화면에 보이는 이름으로도 쓴다.
    pub username: String,
}

/// 회원가입·로그인 요청.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// 회원가입·로그인 응답. 이후 요청의 `Authorization: Bearer {token}`에 쓴다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}

/// 아이디 길이(글자 수). 영문 소문자·숫자·밑줄만 쓸 수 있고, 대문자는 소문자로 바꿔 저장한다.
pub const USERNAME_MIN_CHARS: usize = 3;
pub const USERNAME_MAX_CHARS: usize = 20;
/// 비밀번호 길이(글자 수).
pub const PASSWORD_MIN_CHARS: usize = 8;
pub const PASSWORD_MAX_CHARS: usize = 72;

/// 회원가입 API 경로. 가입하면 바로 로그인된 토큰을 돌려준다.
pub const SIGNUP_PATH: &str = "/api/auth/signup";
/// 로그인 API 경로.
pub const LOGIN_PATH: &str = "/api/auth/login";
/// 로그아웃 API 경로. 요청에 쓴 토큰을 폐기한다.
pub const LOGOUT_PATH: &str = "/api/auth/logout";
/// 내 정보 API 경로. 토큰이 아직 유효한지 확인할 때도 쓴다.
pub const ME_PATH: &str = "/api/auth/me";
