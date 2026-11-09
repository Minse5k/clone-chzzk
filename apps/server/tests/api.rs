//! 서버 HTTP API 통합 테스트. 실제 포트 없이 라우터에 요청을 보낸다.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::Value;
use shared::{
    Broadcast, BroadcastStatus, UPLOAD_POLICY_PATH, UploadPolicy, broadcasts_path, recording_path,
};
use tempfile::TempDir;
use tower::ServiceExt;

/// 테스트마다 임시 SQLite 파일과 임시 local 저장소를 쓰는 서버.
struct TestServer {
    dir: TempDir,
    limits: Limits,
    app: Router,
}

/// 테스트 서버의 업로드 한도.
#[derive(Clone, Copy)]
struct Limits {
    max_upload_bytes: u64,
    storage_quota_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_upload_bytes: 1024 * 1024,
            storage_quota_bytes: 10 * 1024 * 1024,
        }
    }
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
    File {
        name: &'a str,
        file_name: &'a str,
        content_type: &'a str,
        bytes: &'a [u8],
    },
}

const BOUNDARY: &str = "test-boundary-7f3a";

impl TestServer {
    async fn start() -> Self {
        Self::with_limits(Limits::default()).await
    }

    async fn with_limits(limits: Limits) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let app = server::build_app(Self::config(&dir, limits)).await.unwrap();
        Self { dir, limits, app }
    }

    /// 같은 DB 파일과 저장소로 서버를 다시 띄운다.
    async fn restart(self) -> Self {
        let Self { dir, limits, app } = self;
        drop(app);
        let app = server::build_app(Self::config(&dir, limits)).await.unwrap();
        Self { dir, limits, app }
    }

    fn config(dir: &TempDir, limits: Limits) -> server::Config {
        server::Config {
            database_path: dir.path().join("test.db"),
            storage: server::StorageConfig::Local {
                dir: dir.path().join("recordings"),
            },
            cors_allowed_origins: vec!["http://localhost:8080".into()],
            max_upload_bytes: limits.max_upload_bytes,
            storage_quota_bytes: limits.storage_quota_bytes,
            web_dist_dir: None,
        }
    }

    async fn send(&self, request: Request<Body>) -> TestResponse {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        TestResponse {
            status,
            headers,
            body,
        }
    }

    async fn get(&self, uri: &str) -> TestResponse {
        self.send(Request::get(uri).body(Body::empty()).unwrap())
            .await
    }

    async fn get_range(&self, uri: &str, range: &str) -> TestResponse {
        let request = Request::get(uri)
            .header(header::RANGE, range)
            .body(Body::empty())
            .unwrap();
        self.send(request).await
    }

    async fn post_registration(&self, parts: &[Part<'_>]) -> TestResponse {
        let mut body = Vec::new();
        for part in parts {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            match part {
                Part::Text(name, value) => {
                    body.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                        )
                        .as_bytes(),
                    );
                }
                Part::File {
                    name,
                    file_name,
                    content_type,
                    bytes,
                } => {
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
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body))
            .unwrap();
        self.send(request).await
    }

    /// 제목과 mp4 녹화본으로 방송을 등록하고, 성공을 확인한 뒤 방송을 돌려준다.
    async fn register(&self, title: &str, video: &[u8]) -> Broadcast {
        let response = self
            .post_registration(&[
                Part::Text("title", title),
                Part::File {
                    name: "recording",
                    file_name: "clip.mp4",
                    content_type: "video/mp4",
                    bytes: video,
                },
            ])
            .await;
        assert_eq!(
            response.status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        response.json()
    }

    async fn list_broadcasts(&self) -> Vec<Broadcast> {
        let response = self.get("/api/broadcasts").await;
        assert_eq!(response.status, StatusCode::OK);
        response.json()
    }

    /// 거부되어야 하는 등록을 보내고, 상태 코드와 메시지를 확인한 뒤
    /// 목록과 저장소에 아무것도 남지 않았는지 확인한다.
    async fn assert_rejected(&self, parts: &[Part<'_>], status: StatusCode, message: &str) {
        let before_list = self.list_broadcasts().await;
        let before_files = self.stored_file_count();

        let response = self.post_registration(parts).await;

        assert_eq!(
            response.status,
            status,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        assert_eq!(response.error_message(), message);
        assert_eq!(self.list_broadcasts().await, before_list);
        assert_eq!(self.stored_file_count(), before_files);
    }

    /// 테스트 DB에 직접 연결한다. API로 아직 만들 수 없는 상태를 준비할 때 쓴다.
    async fn db(&self) -> sqlx::SqlitePool {
        sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(self.dir.path().join("test.db")),
        )
        .await
        .unwrap()
    }

    /// 송출 중인(종료 시각이 없는) 방송을 DB에 직접 넣는다.
    async fn insert_live_broadcast(&self, title: &str) -> uuid::Uuid {
        let id = uuid::Uuid::now_v7();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        sqlx::query("INSERT INTO broadcasts (id, title, created_at) VALUES (?, ?, ?)")
            .bind(id.to_string())
            .bind(title)
            .bind(now)
            .execute(&self.db().await)
            .await
            .unwrap();
        id
    }

    async fn list_by_status(&self, status: BroadcastStatus) -> Vec<Broadcast> {
        let response = self.get(&broadcasts_path(Some(status))).await;
        assert_eq!(response.status, StatusCode::OK);
        response.json()
    }

    /// local 저장소에 남아 있는 파일 수.
    fn stored_file_count(&self) -> usize {
        std::fs::read_dir(self.dir.path().join("recordings"))
            .unwrap()
            .count()
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
            Part::File {
                name: "recording",
                file_name: "clip.webm",
                content_type: "video/webm",
                bytes: b"webm",
            },
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
    assert_eq!(
        open_ended.header(header::CONTENT_RANGE),
        "bytes 900-999/1000"
    );
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

fn mp4(bytes: &[u8]) -> Part<'_> {
    Part::File {
        name: "recording",
        file_name: "clip.mp4",
        content_type: "video/mp4",
        bytes,
    }
}

#[tokio::test]
async fn 제목이_없거나_공백이거나_너무_길면_400이다() {
    let server = TestServer::start().await;
    let video = sample_video(100);
    let too_long = "가".repeat(101);

    server
        .assert_rejected(
            &[mp4(&video)],
            StatusCode::BAD_REQUEST,
            "제목을 입력해 주세요.",
        )
        .await;
    server
        .assert_rejected(
            &[Part::Text("title", "   "), mp4(&video)],
            StatusCode::BAD_REQUEST,
            "제목을 입력해 주세요.",
        )
        .await;
    server
        .assert_rejected(
            &[Part::Text("title", &too_long), mp4(&video)],
            StatusCode::BAD_REQUEST,
            "제목은 100자 이하로 입력해 주세요.",
        )
        .await;
    // 녹화본이 제목보다 먼저 와서 이미 저장된 경우에도 정리된다.
    server
        .assert_rejected(
            &[mp4(&video), Part::Text("title", " ")],
            StatusCode::BAD_REQUEST,
            "제목을 입력해 주세요.",
        )
        .await;
}

#[tokio::test]
async fn 제목은_앞뒤_공백을_빼고_100자까지_등록된다() {
    let server = TestServer::start().await;
    let title = "가".repeat(100);

    let created = server
        .register(&format!("  {title}  "), &sample_video(10))
        .await;

    assert_eq!(created.title, title);
}

#[tokio::test]
async fn 녹화본이_없으면_400이다() {
    let server = TestServer::start().await;

    server
        .assert_rejected(
            &[Part::Text("title", "녹화본 없음")],
            StatusCode::BAD_REQUEST,
            "녹화본 파일을 선택해 주세요.",
        )
        .await;
}

#[tokio::test]
async fn mp4나_webm이_아니면_415다() {
    let server = TestServer::start().await;

    for (file_name, content_type) in [
        ("clip.mov", "video/quicktime"),
        ("notes.txt", "text/plain"),
        ("clip", "video/x-msvideo"),
    ] {
        server
            .assert_rejected(
                &[
                    Part::Text("title", "형식 테스트"),
                    Part::File {
                        name: "recording",
                        file_name,
                        content_type,
                        bytes: b"data",
                    },
                ],
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "mp4 또는 webm 영상만 등록할 수 있습니다.",
            )
            .await;
    }
}

#[tokio::test]
async fn 녹화본이_개별_상한을_넘으면_413이고_상한과_같으면_등록된다() {
    let server = TestServer::with_limits(Limits {
        max_upload_bytes: 2048,
        storage_quota_bytes: 1024 * 1024,
    })
    .await;

    server
        .assert_rejected(
            &[Part::Text("title", "너무 큼"), mp4(&sample_video(2049))],
            StatusCode::PAYLOAD_TOO_LARGE,
            "녹화본은 2KB 이하만 등록할 수 있습니다.",
        )
        .await;

    let created = server.register("딱 맞음", &sample_video(2048)).await;
    assert_eq!(created.recording.unwrap().size_bytes, 2048);
}

#[tokio::test]
async fn 저장_총량_한도를_넘으면_507이다() {
    let server = TestServer::with_limits(Limits {
        max_upload_bytes: 1000,
        storage_quota_bytes: 1500,
    })
    .await;
    let message = "저장 공간 한도에 도달해 더 이상 방송을 등록할 수 없습니다.";

    server.register("첫 방송", &sample_video(1000)).await;
    // 남은 500바이트보다 크다. 개별 상한(1000)보다는 작아도 저장 총량 때문에 거부된다.
    server
        .assert_rejected(
            &[Part::Text("title", "두 번째"), mp4(&sample_video(600))],
            StatusCode::INSUFFICIENT_STORAGE,
            message,
        )
        .await;

    server.register("남은 만큼", &sample_video(500)).await;
    server
        .assert_rejected(
            &[Part::Text("title", "꽉 참"), mp4(&sample_video(1))],
            StatusCode::INSUFFICIENT_STORAGE,
            message,
        )
        .await;
}

#[tokio::test]
async fn 업로드_한도는_남은_저장_공간을_반영한다() {
    let server = TestServer::with_limits(Limits {
        max_upload_bytes: 1000,
        storage_quota_bytes: 1500,
    })
    .await;

    let before: UploadPolicy = server.get(UPLOAD_POLICY_PATH).await.json();
    assert_eq!(
        before,
        UploadPolicy {
            max_upload_bytes: 1000,
            remaining_bytes: 1500
        }
    );

    server.register("방송", &sample_video(400)).await;

    let after: UploadPolicy = server.get(UPLOAD_POLICY_PATH).await.json();
    assert_eq!(
        after,
        UploadPolicy {
            max_upload_bytes: 1000,
            remaining_bytes: 1100
        }
    );
}

/// 빌드된 웹 정적 파일을 흉내 낸 디렉터리.
fn fake_web_dist(dir: &TempDir) -> std::path::PathBuf {
    let dist = dir.path().join("web");
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::write(
        dist.join("index.html"),
        "<!doctype html><title>clone-chzzk</title>",
    )
    .unwrap();
    std::fs::write(dist.join("assets/app.js"), "console.log('app')").unwrap();
    dist
}

#[tokio::test]
async fn 배포_시_서버가_웹_화면도_서빙하고_모르는_경로는_진입_html로_답한다() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TestServer::config(&dir, Limits::default());
    config.web_dist_dir = Some(fake_web_dist(&dir));
    let server = TestServer {
        app: server::build_app(config).await.unwrap(),
        limits: Limits::default(),
        dir,
    };
    let created = server.register("공유할 방송", &sample_video(10)).await;

    for path in [
        "/".to_owned(),
        format!("/broadcasts/{}", created.id),
        "/broadcasts/new".to_owned(),
    ] {
        let page = server.get(&path).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert!(
            page.header(header::CONTENT_TYPE).starts_with("text/html"),
            "{path}"
        );
        assert_eq!(
            page.body, b"<!doctype html><title>clone-chzzk</title>",
            "{path}"
        );
    }

    let asset = server.get("/assets/app.js").await;
    assert_eq!(asset.status, StatusCode::OK);
    assert_eq!(asset.body, b"console.log('app')");

    // API 경로는 웹 화면으로 대체되지 않는다.
    assert_eq!(server.list_broadcasts().await, vec![created]);
    let unknown_api = server.get("/api/nope").await;
    assert_eq!(unknown_api.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown_api.error_message(), "없는 API입니다.");
}

#[tokio::test]
async fn 웹_정적_파일_디렉터리에_index_html이_없으면_시작하지_않는다() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TestServer::config(&dir, Limits::default());
    config.web_dist_dir = Some(dir.path().join("empty"));

    let error = server::build_app(config).await.err().unwrap();

    assert!(error.to_string().contains("index.html"), "{error}");
}

#[tokio::test]
async fn 방송_등록으로_만든_방송은_종료_상태다() {
    let server = TestServer::start().await;

    let created = server.register("녹화 방송", &sample_video(10)).await;

    assert_eq!(created.status, BroadcastStatus::Ended);
    assert_eq!(created.ended_at, Some(created.created_at));
    let detail = server.get(&format!("/api/broadcasts/{}", created.id)).await;
    assert_eq!(detail.json::<Broadcast>(), created);
}

#[tokio::test]
async fn 종료_시각이_없는_방송은_라이브_중으로_조회된다() {
    let server = TestServer::start().await;
    let id = server.insert_live_broadcast("송출 중인 방송").await;

    let detail: Broadcast = server.get(&format!("/api/broadcasts/{id}")).await.json();

    assert_eq!(detail.status, BroadcastStatus::Live);
    assert_eq!(detail.ended_at, None);
    assert_eq!(detail.recording, None);
}

#[tokio::test]
async fn 방송_목록을_상태로_거를_수_있다() {
    let server = TestServer::start().await;
    let old = server.register("지난 방송", &sample_video(10)).await;
    let live_id = server.insert_live_broadcast("라이브 방송").await;
    let newer = server
        .register("더 최근 지난 방송", &sample_video(10))
        .await;

    let live = server.list_by_status(BroadcastStatus::Live).await;
    assert_eq!(live.iter().map(|b| b.id).collect::<Vec<_>>(), vec![live_id]);

    assert_eq!(
        server.list_by_status(BroadcastStatus::Ended).await,
        vec![newer.clone(), old.clone()]
    );

    let all = server.list_broadcasts().await;
    assert_eq!(
        all.iter().map(|b| b.id).collect::<Vec<_>>(),
        vec![newer.id, live_id, old.id]
    );
}

#[tokio::test]
async fn 잘못된_상태로_방송_목록을_요청하면_400이다() {
    let server = TestServer::start().await;

    let response = server.get("/api/broadcasts?status=onair").await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        response.error_message(),
        "방송 상태는 live 또는 ended만 쓸 수 있습니다."
    );
}

#[tokio::test]
async fn 상태_도입_전에_등록된_방송은_마이그레이션_후_종료_상태가_된다() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    // 과제 1 시점의 스키마(마이그레이션 0001, 0002)로 방송 하나를 남긴다.
    let mut migrator = sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
        .await
        .unwrap();
    migrator.migrations = migrator
        .migrations
        .iter()
        .filter(|m| m.version <= 2)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    migrator.run(&pool).await.unwrap();
    let id = uuid::Uuid::now_v7();
    let created_at = "2026-11-01T09:00:00.000000Z";
    sqlx::query("INSERT INTO broadcasts (id, title, created_at) VALUES (?, ?, ?)")
        .bind(id.to_string())
        .bind("과제 1 방송")
        .bind(created_at)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let app = server::build_app(TestServer::config(&dir, Limits::default()))
        .await
        .unwrap();
    let server = TestServer {
        dir,
        limits: Limits::default(),
        app,
    };

    let detail: Broadcast = server.get(&format!("/api/broadcasts/{id}")).await.json();
    assert_eq!(detail.status, BroadcastStatus::Ended);
    assert_eq!(detail.ended_at, Some(detail.created_at));
    assert_eq!(detail.created_at.to_rfc3339(), "2026-11-01T09:00:00+00:00");
}
