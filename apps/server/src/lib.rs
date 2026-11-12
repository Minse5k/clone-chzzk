//! clone-chzzk API 서버.

mod api;
mod config;
mod db;
mod live;
mod range;
mod storage;

use std::{path::Path, sync::Arc};

use anyhow::Context;
use axum::{Router, http::HeaderValue};
use sqlx::SqlitePool;
use storage::RecordingStore;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

pub use config::{Config, OciStorageConfig, StorageConfig, server_addr_from_env};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub store: Arc<RecordingStore>,
    pub config: Arc<Config>,
    pub live: Arc<live::LiveCapacity>,
}

/// 설정으로 DB와 저장소를 준비하고 HTTP 라우터를 만든다.
/// 같은 설정으로 다시 부르면 기존 데이터를 그대로 이어서 쓴다.
pub async fn build_app(config: Config) -> anyhow::Result<Router> {
    let store = RecordingStore::open(&config.storage).await?;
    let db = db::connect(&config.database_path).await?;
    let cors = cors_layer(&config.cors_allowed_origins)?;
    let web = match &config.web_dist_dir {
        Some(dir) => Some(web_service(dir)?),
        None => None,
    };
    let live = live::LiveCapacity::check(&config).await;
    let state = AppState {
        db,
        store: Arc::new(store),
        config: Arc::new(config),
        live: Arc::new(live),
    };
    // 서버가 송출 도중에 멈췄다면 라이브 중으로 남은 방송을 정리한다.
    live::recover(&state).await?;

    let mut router = api::router().with_state(state);
    if let Some(web) = web {
        router = router.fallback_service(web);
    }
    Ok(router.layer(cors).layer(TraceLayer::new_for_http()))
}

/// 빌드된 웹 정적 파일. 파일이 없는 경로는 웹 진입 HTML로 답해,
/// 공유 URL(`/broadcasts/{id}`)로 바로 들어오거나 새로고침해도 웹 라우터가 화면을 그린다.
fn web_service(dir: &Path) -> anyhow::Result<ServeDir<ServeFile>> {
    let index = dir.join("index.html");
    anyhow::ensure!(
        index.is_file(),
        "WEB_DIST_DIR에 index.html이 없습니다: {} (`dx build -p web --release` 결과의 public 디렉터리를 지정하세요)",
        dir.display()
    );
    Ok(ServeDir::new(dir).fallback(ServeFile::new(index)))
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
