//! 서버 HTTP API 통합 테스트. 실제 포트 없이 라우터에 요청을 보낸다.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::Value;
use shared::{Broadcast, recording_path};
use tempfile::TempDir;
use tower::ServiceExt;

/// 테스트마다 임시 SQLite 파일과 임시 local 저장소를 쓰는 서버.
struct TestServer {
    dir: TempDir,
    app: Router,
}

struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl TestResponse {
    fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap()
    }

    fn error_message(&self) -> String {
        self.json::<Value>()["message"].as_str().unwrap().to_owned()
    }

    fn header(&self, name: header::HeaderName) -> &str {
        self.headers.get(name).unwrap().to_str().unwrap()
    }
}

/// 방송 등록 요청의 multipart 본문 조각.
enum Part<'a> {
    Text(&'a str, &'a str),
    File { name: &'a str, file_name: &'a str, content_type: &'a str, bytes: &'a [u8] },
}

const BOUNDARY: &str = "test-boundary-7f3a";

impl TestServer {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let app = server::build_app(Self::config(&dir)).await.unwrap();
        Self { dir, app }
    }

    /// 같은 DB 파일과 저장소로 서버를 다시 띄운다.
    async fn restart(self) -> Self {
        let Self { dir, app } = self;
        drop(app);
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

    async fn send(&self, request: Request<Body>) -> TestResponse {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes().to_vec();
        TestResponse { status, headers, body }
    }

    async fn get(&self, uri: &str) -> TestResponse {
        self.send(Request::get(uri).body(Body::empty()).unwrap()).await
    }

    async fn get_range(&self, uri: &str, range: &str) -> TestResponse {
        let request = Request::get(uri).header(header::RANGE, range).body(Body::empty()).unwrap();
        self.send(request).await
    }

    async fn post_registration(&self, parts: &[Part<'_>]) -> TestResponse {
        let mut body = Vec::new();
        for part in parts {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            match part {
                Part::Text(name, value) => {
                    body.extend_from_slice(
                        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
                    );
                }
                Part::File { name, file_name, content_type, bytes } => {
                    body.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\n\
                             Content-Type: {content_type}\r\n\r\n"
                        )
                        .as_bytes(),
                    );
                    body.extend_from_slice(bytes);
                    body.extend_from_slice(b"\r\n");
                }
            }
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

        let request = Request::post("/api/broadcasts")
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={BOUNDARY}"))
            .body(Body::from(body))
            .unwrap();
        self.send(request).await
    }

    /// 제목과 mp4 녹화본으로 방송을 등록하고, 성공을 확인한 뒤 방송을 돌려준다.
    async fn register(&self, title: &str, video: &[u8]) -> Broadcast {
        let response = self
            .post_registration(&[
                Part::Text("title", title),
                Part::File { name: "recording", file_name: "clip.mp4", content_type: "video/mp4", bytes: video },
            ])
            .await;
        assert_eq!(response.status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&response.body));
        response.json()
    }

    async fn list_broadcasts(&self) -> Vec<Broadcast> {
        let response = self.get("/api/broadcasts").await;
        assert_eq!(response.status, StatusCode::OK);
        response.json()
    }

    /// local 저장소에 남아 있는 파일 수.
    fn stored_file_count(&self) -> usize {
        std::fs::read_dir(self.dir.path().join("recordings")).unwrap().count()
    }
}

/// 테스트용 녹화본 내용. 0, 1, 2, ... 255, 0, 1 ... 순서의 바이트.
fn sample_video(len: usize) -> Vec<u8> {
    (0..len).map(|i| i as u8).collect()
}

#[tokio::test]
async fn 등록된_방송이_없으면_빈_목록을_돌려준다() {
    let server = TestServer::start().await;

    assert!(server.list_broadcasts().await.is_empty());
    assert!(server.dir.path().join("recordings").is_dir());
}

#[tokio::test]
async fn 방송을_등록하면_201과_방송을_돌려주고_상세로_조회된다() {
    let server = TestServer::start().await;
    let video = sample_video(1000);

    let created = server.register("  첫 방송  ", &video).await;

    assert_eq!(created.title, "첫 방송");
    let recording = created.recording.as_ref().unwrap();
    assert_eq!(recording.content_type, "video/mp4");
    assert_eq!(recording.size_bytes, 1000);

    let detail = server.get(&format!("/api/broadcasts/{}", created.id)).await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(detail.json::<Broadcast>(), created);
}

#[tokio::test]
async fn 방송_목록은_최신_등록순이다() {
    let server = TestServer::start().await;

    let first = server.register("먼저 올린 방송", &sample_video(10)).await;
    let second = server.register("나중에 올린 방송", &sample_video(10)).await;

    assert_eq!(server.list_broadcasts().await, vec![second, first]);
}

#[tokio::test]
async fn webm_녹화본도_등록된다() {
    let server = TestServer::start().await;

    let response = server
        .post_registration(&[
            Part::Text("title", "webm 방송"),
            Part::File { name: "recording", file_name: "clip.webm", content_type: "video/webm", bytes: b"webm" },
        ])
        .await;

    assert_eq!(response.status, StatusCode::CREATED);
    let created: Broadcast = response.json();
    assert_eq!(created.recording.unwrap().content_type, "video/webm");
}

#[tokio::test]
async fn 녹화본_전체_요청은_원본_바이트를_돌려준다() {
    let server = TestServer::start().await;
    let video = sample_video(1000);
    let created = server.register("재생 테스트", &video).await;

    let response = server.get(&recording_path(created.id)).await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.header(header::CONTENT_TYPE), "video/mp4");
    assert_eq!(response.header(header::ACCEPT_RANGES), "bytes");
    assert_eq!(response.header(header::CONTENT_LENGTH), "1000");
    assert_eq!(response.body, video);
}

#[tokio::test]
async fn 녹화본_range_요청은_206과_해당_구간을_돌려준다() {
    let server = TestServer::start().await;
    let video = sample_video(1000);
    let created = server.register("탐색 테스트", &video).await;
    let path = recording_path(created.id);

    let middle = server.get_range(&path, "bytes=100-199").await;
    assert_eq!(middle.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(middle.header(header::CONTENT_RANGE), "bytes 100-199/1000");
    assert_eq!(middle.header(header::CONTENT_LENGTH), "100");
    assert_eq!(middle.body, video[100..200]);

    let open_ended = server.get_range(&path, "bytes=900-").await;
    assert_eq!(open_ended.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(open_ended.header(header::CONTENT_RANGE), "bytes 900-999/1000");
    assert_eq!(open_ended.body, video[900..]);

    let suffix = server.get_range(&path, "bytes=-50").await;
    assert_eq!(suffix.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(suffix.header(header::CONTENT_RANGE), "bytes 950-999/1000");
    assert_eq!(suffix.body, video[950..]);

    let past_end = server.get_range(&path, "bytes=5000-").await;
    assert_eq!(past_end.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(past_end.header(header::CONTENT_RANGE), "bytes */1000");
}

#[tokio::test]
async fn 없는_방송의_상세와_녹화본은_404다() {
    let server = TestServer::start().await;
    let missing = "0190b6a0-0000-7000-8000-000000000000";

    for uri in [
        format!("/api/broadcasts/{missing}"),
        format!("/api/broadcasts/{missing}/recording"),
        "/api/broadcasts/not-a-uuid".to_owned(),
    ] {
        let response = server.get(&uri).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(response.error_message(), "방송을 찾을 수 없습니다.");
    }
}

#[tokio::test]
async fn 서버를_다시_시작해도_방송과_녹화본이_남아_있다() {
    let server = TestServer::start().await;
    let video = sample_video(500);
    let created = server.register("재시작 테스트", &video).await;

    let server = server.restart().await;

    assert_eq!(server.list_broadcasts().await, vec![created.clone()]);
    let detail = server.get(&format!("/api/broadcasts/{}", created.id)).await;
    assert_eq!(detail.json::<Broadcast>(), created);
    let recording = server.get(&recording_path(created.id)).await;
    assert_eq!(recording.body, video);
    assert_eq!(server.stored_file_count(), 1);
}
