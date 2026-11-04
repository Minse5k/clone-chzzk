//! 서버와 웹이 함께 쓰는 API 타입.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 방송 하나. 목록과 상세 화면에 보이는 단위다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Broadcast {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    /// 종료된 방송에 남는 영상. 아직 녹화본이 없는 방송이면 `None`.
    pub recording: Option<Recording>,
}

/// 녹화본의 공개 정보. 저장 위치는 서버만 안다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub content_type: String,
    pub size_bytes: u64,
}

/// 방송 등록 요청(multipart)의 필드 이름.
pub mod registration_fields {
    pub const TITLE: &str = "title";
    pub const RECORDING: &str = "recording";
}

/// 방송 상세 API 경로.
pub fn broadcast_path(id: Uuid) -> String {
    format!("/api/broadcasts/{id}")
}

/// 녹화본 재생 API 경로. Range 요청을 지원한다.
pub fn recording_path(id: Uuid) -> String {
    format!("/api/broadcasts/{id}/recording")
}
