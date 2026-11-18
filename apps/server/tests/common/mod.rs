//! 통합 테스트가 함께 쓰는 서버 준비와 요청 도우미.

// 테스트 파일마다 쓰는 도우미가 달라, 한 파일에서 쓰지 않는 함수가 있어도 경고하지 않는다.
#![allow(dead_code)]

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use shared::{AuthResponse, SIGNUP_PATH};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

pub const TEST_JWT_SECRET: &str = "test-only-jwt-secret";

/// 여러 인스턴스 테스트가 쓰는 Redis 주소.
pub fn test_redis_url() -> String {
    std::env::var("TEST_REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into())
}

/// 테스트용 PostgreSQL 서버 주소. 이 서버에 테스트마다 새 데이터베이스를 만든다.
fn test_database_server_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".into())
}

/// 테스트 하나가 쓰는 PostgreSQL 데이터베이스. 사라질 때 데이터베이스도 지운다.
pub struct TestDatabase {
    pub url: String,
    name: String,
}

impl TestDatabase {
    pub async fn create() -> Self {
        let server_url = test_database_server_url();
        let name = format!("chzzk_test_{}", Uuid::now_v7().simple());
        let mut admin = connect_admin(&server_url).await;
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut admin)
            .await
            .unwrap();
        let base = server_url
            .rsplit_once('/')
            .map(|(base, _)| base)
            .expect("TEST_DATABASE_URL은 postgres://.../<db> 형식이어야 합니다");
        Self {
            url: format!("{base}/{name}"),
            name,
        }
    }

    /// 테스트 DB에 직접 연결한다. API로 만들 수 없는 상태를 준비하거나 저장된 값을 확인할 때 쓴다.
    pub async fn pool(&self) -> sqlx::PgPool {
        sqlx::PgPool::connect(&self.url).await.unwrap()
    }
}

async fn connect_admin(url: &str) -> sqlx::PgConnection {
    use sqlx::Connection;
    sqlx::PgConnection::connect(url).await.unwrap_or_else(|err| {
        panic!(
            "테스트용 PostgreSQL({url})에 연결할 수 없습니다: {err}. README의 「테스트」를 보고 띄우세요"
        )
    })
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let server_url = test_database_server_url();
        let name = self.name.clone();
        // Drop에서는 기다릴 수 없어, 별도 스레드의 런타임에서 데이터베이스를 지운다.
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let mut admin = connect_admin(&server_url).await;
                let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
                )))
                .execute(&mut admin)
                .await;
            });
        })
        .join();
    }
}

/// 테스트마다 임시 디렉터리와 새 DB를 쓰는 기본 설정.
pub fn test_config(dir: &TempDir, db: &TestDatabase) -> server::Config {
    server::Config {
        database_url: db.url.as_str().into(),
        database_max_connections: 5,
        redis_url: None,
        storage: server::StorageConfig::Local {
            dir: dir.path().join("recordings"),
        },
        cors_allowed_origins: vec!["http://localhost:8080".into()],
        max_upload_bytes: 1024 * 1024,
        storage_quota_bytes: 10 * 1024 * 1024,
        web_dist_dir: None,
        live_dir: dir.path().join("live"),
        ffmpeg_path: "ffmpeg".into(),
        max_live_broadcasts: 2,
        instance_id: format!("test-{}", Uuid::now_v7().simple()),
        live_heartbeat_interval: std::time::Duration::from_secs(5),
        live_heartbeat_timeout: std::time::Duration::from_secs(30),
        live_idle_timeout: std::time::Duration::from_secs(20),
        jwt_secret: Some(TEST_JWT_SECRET.into()),
        jwt_ttl: std::time::Duration::from_secs(3600),
        toss: None,
    }
}

/// 실제 포트에서도 요청을 받는 테스트 서버. HTTP 요청은 라우터에 바로 보내고,
/// WebSocket은 `addr`로 연결한다.
pub struct TestApp {
    /// 정상 종료 처리를 부를 수 있는 서버 인스턴스.
    pub server: Arc<server::App>,
    /// 녹화본·라이브 조각 디렉터리. 여러 인스턴스 테스트에서는 공유 볼륨처럼 인스턴스끼리 함께 쓴다.
    pub dir: Arc<TempDir>,
    pub db: Arc<TestDatabase>,
    pub app: Router,
    pub addr: SocketAddr,
}

pub struct Reply {
    pub status: StatusCode,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|err| panic!("{err}: {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn message(&self) -> String {
        self.json::<Value>()["message"].as_str().unwrap().to_owned()
    }
}

impl TestApp {
    pub async fn start() -> Self {
        Self::with_config(|_| {}).await
    }

    pub async fn with_config(edit: impl FnOnce(&mut server::Config)) -> Self {
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let db = Arc::new(TestDatabase::create().await);
        let mut config = test_config(&dir, &db);
        edit(&mut config);
        Self::serve(dir, db, config).await
    }

    /// 같은 DB·Redis·공유 디렉터리를 쓰는 서버 인스턴스 두 개(A, B)를 띄운다.
    pub async fn cluster(edit: impl Fn(&mut server::Config)) -> (Self, Self) {
        let first = Self::with_config(|config| {
            config.redis_url = Some(test_redis_url().as_str().into());
            edit(config);
        })
        .await;
        let second = first.peer(edit).await;
        (first, second)
    }

    /// 이 서버와 같은 DB·Redis·공유 디렉터리를 쓰는 인스턴스를 하나 더 띄운다.
    pub async fn peer(&self, edit: impl Fn(&mut server::Config)) -> Self {
        let mut config = test_config(&self.dir, &self.db);
        config.redis_url = Some(test_redis_url().as_str().into());
        edit(&mut config);
        Self::serve(Arc::clone(&self.dir), Arc::clone(&self.db), config).await
    }

    async fn serve(dir: Arc<TempDir>, db: Arc<TestDatabase>, config: server::Config) -> Self {
        let server = Arc::new(server::build(config).await.unwrap());
        let app = server.router.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app.clone()).into_future());
        Self {
            server,
            dir,
            db,
            app,
            addr,
        }
    }

    /// 주어진 토큰으로(없으면 비로그인으로) 요청을 보낸다. 본문이 있으면 JSON으로 보낸다.
    pub async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        Reply { status, body }
    }

    pub async fn get(&self, uri: &str) -> Reply {
        self.request("GET", uri, None, None).await
    }

    /// 사용자를 가입시키고 로그인 정보를 돌려준다. 비밀번호는 모두 `password123`.
    pub async fn signup(&self, username: &str) -> AuthResponse {
        let body = json!({ "username": username, "password": "password123" });
        let reply = self.request("POST", SIGNUP_PATH, None, Some(body)).await;
        assert_eq!(reply.status, StatusCode::CREATED, "{username} 가입 실패");
        reply.json()
    }

    /// 라이브 중인(종료 시각이 없는) 방송을 DB에 직접 만든다. 송출 없이 라이브 상태가 필요한 테스트의 준비용.
    pub async fn insert_live_broadcast(&self, owner: &AuthResponse, title: &str) -> Uuid {
        insert_broadcast(&self.db, owner.user.id, title, false).await
    }

    /// 테스트 DB에 직접 연결한다.
    pub async fn pool(&self) -> sqlx::PgPool {
        self.db.pool().await
    }

    /// 녹화본 없이 종료된 방송을 DB에 직접 만든다. 방송 등록·송출 경로와 상관없는 테스트의 준비용.
    pub async fn insert_ended_broadcast(&self, owner: &AuthResponse, title: &str) -> Uuid {
        insert_broadcast(&self.db, owner.user.id, title, true).await
    }
}

/// 방송을 DB에 직접 만든다. `ended`면 만든 시각에 이미 종료된 방송이다.
pub async fn insert_broadcast(db: &TestDatabase, owner_id: Uuid, title: &str, ended: bool) -> Uuid {
    let id = Uuid::now_v7();
    let now = chrono::Utc::now();
    let pool = db.pool().await;
    sqlx::query(
        "INSERT INTO broadcasts (id, title, owner_id, created_at, ended_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(title)
    .bind(owner_id)
    .bind(now)
    .bind(ended.then_some(now))
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    id
}

/// 송출받던 인스턴스(`instance`)가 `heartbeat_ago` 전에 마지막으로 하트비트를 남긴 라이브 방송을
/// DB에 직접 만든다. 송출받던 인스턴스가 죽은 상황을 재현할 때 쓴다. 시각은 DB 시계 기준이다.
pub async fn insert_ingesting_broadcast(
    db: &TestDatabase,
    owner_id: Uuid,
    title: &str,
    instance: &str,
    heartbeat_ago: std::time::Duration,
) -> Uuid {
    let id = Uuid::now_v7();
    let pool = db.pool().await;
    sqlx::query(
        "INSERT INTO broadcasts (id, title, owner_id, created_at, ingest_instance, ingest_heartbeat_at) \
         VALUES ($1, $2, $3, now() - make_interval(secs => $4), $5, now() - make_interval(secs => $4))",
    )
    .bind(id)
    .bind(title)
    .bind(owner_id)
    .bind(heartbeat_ago.as_secs_f64())
    .bind(instance)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    id
}

/// ffmpeg로 3초짜리 HLS 조각을 `dir`에 만든다. 송출받던 인스턴스가 남긴 라이브 조각을 재현한다.
pub fn write_hls_fragments(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let status = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-t", "3", "-c:v", "libx264", "-c:a", "aac"])
        .args(["-f", "hls", "-hls_time", "1", "-hls_list_size", "0"])
        .arg("-hls_segment_filename")
        .arg(dir.join("seg%05d.ts"))
        .arg(dir.join("index.m3u8"))
        .status()
        .expect("ffmpeg가 필요합니다");
    assert!(status.success());
}
