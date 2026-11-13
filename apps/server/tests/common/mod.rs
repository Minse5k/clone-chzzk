//! 통합 테스트가 함께 쓰는 서버 준비와 요청 도우미.

// 테스트 파일마다 쓰는 도우미가 달라, 한 파일에서 쓰지 않는 함수가 있어도 경고하지 않는다.
#![allow(dead_code)]

use std::net::SocketAddr;

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

/// 테스트마다 임시 디렉터리를 쓰는 기본 설정.
pub fn test_config(dir: &TempDir) -> server::Config {
    server::Config {
        database_path: dir.path().join("test.db"),
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
        live_idle_timeout: std::time::Duration::from_secs(20),
        jwt_secret: Some(TEST_JWT_SECRET.into()),
        jwt_ttl: std::time::Duration::from_secs(3600),
    }
}

/// 실제 포트에서도 요청을 받는 테스트 서버. HTTP 요청은 라우터에 바로 보내고,
/// WebSocket은 `addr`로 연결한다.
pub struct TestApp {
    pub dir: TempDir,
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
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config(&dir);
        edit(&mut config);
        let app = server::build_app(config).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app.clone()).into_future());
        Self { dir, app, addr }
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

    /// 녹화본 없이 종료된 방송을 DB에 직접 만든다. 방송 등록·송출 경로와 상관없는 테스트의 준비용.
    pub async fn insert_ended_broadcast(&self, owner: &AuthResponse, title: &str) -> Uuid {
        let id = Uuid::now_v7();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(self.dir.path().join("test.db")),
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO broadcasts (id, title, owner_id, created_at, ended_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(title)
        .bind(owner.user.id.to_string())
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
        id
    }
}
