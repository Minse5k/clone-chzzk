//! 서버 HTTP API 통합 테스트. 실제 포트 없이 라우터에 요청을 보낸다.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use shared::Broadcast;
use tempfile::TempDir;
use tower::ServiceExt;

/// 테스트마다 임시 SQLite 파일과 임시 local 저장소를 쓰는 서버.
struct TestServer {
    dir: TempDir,
    app: Router,
}

impl TestServer {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let app = server::build_app(Self::config(&dir)).await.unwrap();
        Self { dir, app }
    }

    fn config(dir: &TempDir) -> server::Config {
        server::Config {
            database_path: dir.path().join("test.db"),
            storage_dir: dir.path().join("recordings"),
            cors_allowed_origins: vec!["http://localhost:8080".into()],
        }
    }

    async fn get(&self, uri: &str) -> (StatusCode, Vec<u8>) {
        let request = Request::get(uri).body(Body::empty()).unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, body)
    }

    async fn list_broadcasts(&self) -> Vec<Broadcast> {
        let (status, body) = self.get("/api/broadcasts").await;
        assert_eq!(status, StatusCode::OK);
        serde_json::from_slice(&body).unwrap()
    }
}

#[tokio::test]
async fn 등록된_방송이_없으면_빈_목록을_돌려준다() {
    let server = TestServer::start().await;

    assert!(server.list_broadcasts().await.is_empty());
    assert!(server.dir.path().join("recordings").is_dir());
}
