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
