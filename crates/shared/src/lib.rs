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
}
