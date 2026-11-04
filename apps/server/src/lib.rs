//! clone-chzzk API 서버.

mod api;
mod config;
mod db;
mod range;
mod storage;

use std::sync::Arc;

use anyhow::Context;
use axum::{Router, http::HeaderValue};
use sqlx::SqlitePool;
use storage::RecordingStore;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    trace::TraceLayer,
};

pub use config::{Config, OciStorageConfig, StorageConfig, server_addr_from_env};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub store: Arc<RecordingStore>,
    pub config: Arc<Config>,
}

/// 설정으로 DB와 저장소를 준비하고 HTTP 라우터를 만든다.
/// 같은 설정으로 다시 부르면 기존 데이터를 그대로 이어서 쓴다.
pub async fn build_app(config: Config) -> anyhow::Result<Router> {
    let store = RecordingStore::open(&config.storage).await?;
    let db = db::connect(&config.database_path).await?;
    let cors = cors_layer(&config.cors_allowed_origins)?;
    let state = AppState { db, store: Arc::new(store), config: Arc::new(config) };

    Ok(api::router()
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http()))
}

fn cors_layer(origins: &[String]) -> anyhow::Result<CorsLayer> {
    let allow_origin = if origins.iter().any(|origin| origin == "*") {
        AllowOrigin::any()
    } else {
        let origins = origins
            .iter()
            .map(|origin| HeaderValue::from_str(origin))
            .collect::<Result<Vec<_>, _>>()
            .context("CORS_ALLOWED_ORIGINS에 잘못된 출처가 있습니다")?;
        AllowOrigin::list(origins)
    };
    Ok(CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods(Any)
        .allow_headers(Any))
}
