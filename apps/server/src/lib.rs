//! clone-chzzk API 서버.

mod api;
mod auth;
mod bus;
mod comments;
mod config;
mod db;
mod donations;
mod live;
mod payments;
mod range;
mod storage;

use std::{path::Path, sync::Arc, time::Duration};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, StatusCode},
    routing::get,
};
use storage::RecordingStore;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

pub use config::{
    Config, ConnectionUrl, JwtSecret, OciStorageConfig, StorageConfig, TossConfig,
    server_addr_from_env,
};

#[derive(Clone)]
pub struct AppState {
    pub db: db::Db,
    pub store: Arc<RecordingStore>,
    pub config: Arc<Config>,
    pub live: Arc<live::LiveCapacity>,
    pub auth: Arc<auth::AuthKeys>,
    pub comments: Arc<comments::CommentHub>,
    /// 인스턴스 사이의 알림 전달(Redis Pub/Sub).
    pub bus: Arc<bus::Bus>,
    /// 후원 결제. 토스페이먼츠 키를 설정하지 않았으면 `None`이고 후원만 막는다.
    pub payments: Option<Arc<payments::TossPayments>>,
}

/// 설정으로 DB와 저장소를 준비하고 HTTP 라우터를 만든다.
/// 같은 설정으로 다시 부르면 기존 데이터를 그대로 이어서 쓴다.
pub async fn build_app(config: Config) -> anyhow::Result<Router> {
    Ok(build(config).await?.router)
}

/// 실행 중인 서버 인스턴스. 라우터와, 내려갈 때 쓰는 정상 종료 처리를 가진다.
pub struct App {
    pub router: Router,
    state: AppState,
}

impl App {
    /// 인스턴스가 내려갈 준비를 한다(SIGTERM). 헬스 체크를 실패로 바꿔 리버스 프록시가 새 요청을
    /// 보내지 않게 하고, 이 인스턴스의 송출은 이유와 함께 끝내 녹화본을 만들고, 댓글 WebSocket은
    /// 닫아 시청 화면이 다른 인스턴스로 다시 연결하게 한다. 처리 중인 HTTP 요청은 이 뒤에 axum이 마저 끝낸다.
    pub async fn prepare_shutdown(&self, timeout: Duration) {
        let stopped = self.state.live.shut_down();
        tracing::info!(
            instance = %self.state.config.instance_id,
            "종료 신호를 받았습니다. 송출 {stopped}개를 끝내고 내려갑니다"
        );
        self.state.comments.close_all();
        if !self.state.live.wait_idle(timeout).await {
            tracing::warn!("송출 마무리를 기다리다 시간이 지나 그대로 내려갑니다");
        }
    }
}

/// 서버 인스턴스를 준비한다. [`build_app`]과 같지만 정상 종료 처리도 돌려준다.
pub async fn build(config: Config) -> anyhow::Result<App> {
    let store = RecordingStore::open(&config.storage).await?;
    let db = db::connect(&config.database_url.0, config.database_max_connections).await?;
    let cors = cors_layer(&config.cors_allowed_origins)?;
    let web = match &config.web_dist_dir {
        Some(dir) => Some(web_service(dir)?),
        None => None,
    };
    let live = Arc::new(live::LiveCapacity::check(&config).await);
    let comments = Arc::new(comments::CommentHub::default());
    let bus = bus::Bus::start(
        config.redis_url.as_ref().map(|url| url.0.as_str()),
        Arc::clone(&comments),
        Arc::clone(&live),
    )
    .await?;
    let auth = auth::AuthKeys::new(&config);
    let payments = match &config.toss {
        Some(toss) => Some(Arc::new(payments::TossPayments::new(toss)?)),
        None => {
            tracing::warn!(
                "TOSS_CLIENT_KEY·TOSS_SECRET_KEY가 없어 후원 결제를 받지 않습니다. 다른 기능은 그대로 동작합니다"
            );
            None
        }
    };
    let state = AppState {
        db,
        store: Arc::new(store),
        config: Arc::new(config),
        live,
        auth: Arc::new(auth),
        comments,
        bus: Arc::new(bus),
        payments,
    };
    // 송출받던 인스턴스가 죽어 끊긴 라이브 방송을 정리한다. 다른 인스턴스가 송출받는 중인 방송은
    // 하트비트가 살아 있어 그대로 둔다. 이후로도 주기적으로 정리한다.
    live::sweep_stale(&state).await?;
    tokio::spawn(live::run_sweeper(state.clone()));
    purge_pending_deletions(&state).await?;

    let mut router = api::router()
        .route(HEALTH_PATH, get(health))
        .with_state(state.clone());
    if let Some(web) = web {
        router = router.fallback_service(web);
    }
    Ok(App {
        router: router.layer(cors).layer(TraceLayer::new_for_http()),
        state,
    })
}

/// 리버스 프록시의 헬스 체크와 오토스케일러가 보는 인스턴스 상태.
pub const HEALTH_PATH: &str = "/healthz";

/// 인스턴스 상태. 내려가는 중이면 503으로 답해 리버스 프록시가 이 인스턴스를 뺀다.
async fn health(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let shutting_down = state.live.is_shutting_down();
    let status = if shutting_down {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    let body = serde_json::json!({
        "instance": state.config.instance_id,
        "live_ingests": state.live.busy_ingests(),
        "shutting_down": shutting_down,
    });
    (status, Json(body))
}

/// 삭제 대기열의 녹화본 객체를 저장소에서 지운다. 지우지 못한 객체는 다음 시작 때 다시 시도한다.
async fn purge_pending_deletions(state: &AppState) -> anyhow::Result<()> {
    let backend = state.store.backend_name();
    for key in db::pending_deletions(&state.db, backend).await? {
        match state.store.delete(&key).await {
            Ok(()) => {
                db::remove_pending_deletion(&state.db, backend, &key).await?;
                tracing::info!("삭제된 방송의 녹화본을 지웠습니다: {key}");
            }
            Err(err) => {
                tracing::warn!("녹화본 {key}을 지우지 못해 다음 시작 때 다시 시도합니다: {err:#}")
            }
        }
    }
    Ok(())
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
