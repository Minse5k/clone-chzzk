use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;
use shared::Broadcast;

use crate::{AppState, db};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/broadcasts", get(list_broadcasts))
}

async fn list_broadcasts(State(state): State<AppState>) -> Result<Json<Vec<Broadcast>>, ApiError> {
    Ok(Json(db::list_broadcasts(&state.db).await?))
}

/// API 오류. 본문에 사용자에게 보여 줄 한국어 메시지를 담는다.
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "message": self.message }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!("{err:#}");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "서버 오류가 발생했습니다. 잠시 후 다시 시도해 주세요.".into(),
        }
    }
}
