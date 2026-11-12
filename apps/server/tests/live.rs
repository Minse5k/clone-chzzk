//! 라이브 송출 통합 테스트. 실제 포트에 서버를 띄워 WebSocket으로 송출한다.
//! 송출 조각을 만들고 HLS로 바꾸는 데 ffmpeg가 필요하다.

use std::{net::SocketAddr, process::Stdio, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use shared::{
    Broadcast, BroadcastStatus, IngestClientMessage, IngestServerMessage, LIVE_INGEST_PATH,
    UPLOAD_POLICY_PATH, UploadPolicy, broadcast_path, broadcasts_path, live_playlist_path,
    recording_path,
};
use tempfile::TempDir;

const TEST_JWT_SECRET: &str = "test-only-jwt-secret";
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tower::ServiceExt;
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 임시 DB·저장소·라이브 디렉터리를 쓰고 실제 포트에서 요청을 받는 서버.
struct LiveServer {
    dir: TempDir,
    app: Router,
    addr: SocketAddr,
}

struct Response {
    status: StatusCode,
    content_type: String,
    body: Vec<u8>,
}

impl LiveServer {
    async fn start() -> Self {
        Self::with_config(|_| {}).await
    }

    async fn with_quota(storage_quota_bytes: u64) -> Self {
        Self::with_config(|config| config.storage_quota_bytes = storage_quota_bytes).await
    }

    /// 기본 설정을 고쳐서 서버를 띄운다.
    async fn with_config(edit: impl FnOnce(&mut server::Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Self::config(&dir);
        edit(&mut config);
        Self::serve(dir, config).await
    }

    fn config(dir: &TempDir) -> server::Config {
        server::Config {
            database_path: dir.path().join("test.db"),
            storage: server::StorageConfig::Local {
                dir: dir.path().join("recordings"),
            },
            cors_allowed_origins: vec!["http://localhost:8080".into()],
            max_upload_bytes: 100 * 1024 * 1024,
            storage_quota_bytes: 1024 * 1024 * 1024,
            web_dist_dir: None,
            live_dir: dir.path().join("live"),
            ffmpeg_path: "ffmpeg".into(),
            max_live_broadcasts: 2,
            live_idle_timeout: Duration::from_secs(20),
            jwt_secret: Some(TEST_JWT_SECRET.into()),
            jwt_ttl: std::time::Duration::from_secs(3600),
        }
    }

    async fn serve(dir: TempDir, config: server::Config) -> Self {
        let app = server::build_app(config).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app.clone()).into_future());
        Self { dir, app, addr }
    }

    async fn get(&self, uri: &str) -> Response {
        let response = self
            .app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|value| value.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        Response {
            status,
            content_type,
            body: body.to_vec(),
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, uri: &str) -> T {
        let response = self.get(uri).await;
        assert_eq!(response.status, StatusCode::OK, "{uri}");
        serde_json::from_slice(&response.body).unwrap()
    }

    async fn broadcast(&self, id: Uuid) -> Broadcast {
        self.get_json(&broadcast_path(id)).await
    }

    /// 송출이 끝난 뒤 녹화본 처리가 끝나(라이브 디렉터리가 지워질 때까지) 기다린다.
    async fn wait_until_finalized(&self, id: Uuid) -> Broadcast {
        let live_dir = self.dir.path().join("live").join(id.to_string());
        for _ in 0..200 {
            if !live_dir.exists() {
                return self.broadcast(id).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("20초 안에 녹화본 처리가 끝나지 않았습니다");
    }

    /// 다른 방송의 녹화본이 저장 총량 한도를 모두 차지한 상태로 만든다.
    async fn use_up_quota(&self) {
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(self.dir.path().join("test.db")),
        )
        .await
        .unwrap();
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO broadcasts (id, title, created_at, ended_at) VALUES (?, '큰 방송', ?, ?)",
        )
        .bind(&id)
        .bind("2026-11-12T09:00:00.000000Z")
        .bind("2026-11-12T09:00:00.000000Z")
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO recordings (broadcast_id, storage_backend, object_key, content_type, size_bytes) \
             VALUES (?, 'local', 'big.mp4', 'video/mp4', ?)",
        )
        .bind(&id)
        .bind(1024_i64 * 1024 * 1024)
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    async fn stored_recordings(&self) -> usize {
        std::fs::read_dir(self.dir.path().join("recordings"))
            .unwrap()
            .count()
    }

    /// 송출 시작을 보내고, 거부되었는지와 그 이유를 확인한다.
    async fn assert_start_rejected(&self, message: &str) {
        let mut socket = self.connect().await;
        send(&mut socket, &start_message("거부될 방송")).await;
        assert_eq!(
            receive(&mut socket).await,
            IngestServerMessage::Error {
                message: message.into()
            }
        );
    }

    async fn connect(&self) -> Socket {
        let url = format!("ws://{}{LIVE_INGEST_PATH}", self.addr);
        tokio_tungstenite::connect_async(url).await.unwrap().0
    }

    /// 송출을 시작하고 만들어진 라이브 방송을 돌려준다.
    async fn start_broadcast(&self, title: &str) -> (Socket, Broadcast) {
        let mut socket = self.connect().await;
        send(&mut socket, &start_message(title)).await;
        match receive(&mut socket).await {
            IngestServerMessage::Started { broadcast } => (socket, broadcast),
            other => panic!("송출이 시작되지 않았습니다: {other:?}"),
        }
    }

    /// 조건을 만족할 때까지 방송을 다시 조회한다.
    async fn wait_for(
        &self,
        id: Uuid,
        what: &str,
        condition: impl Fn(&Broadcast) -> bool,
    ) -> Broadcast {
        for _ in 0..200 {
            let broadcast = self.broadcast(id).await;
            if condition(&broadcast) {
                return broadcast;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("20초 안에 {what} 상태가 되지 않았습니다");
    }
}

fn start_message(title: &str) -> IngestClientMessage {
    IngestClientMessage::Start {
        title: title.into(),
        mime_type: "video/webm;codecs=vp8,opus".into(),
    }
}

async fn send(socket: &mut Socket, message: &IngestClientMessage) {
    let text = serde_json::to_string(message).unwrap();
    socket.send(Message::Text(text.into())).await.unwrap();
}

/// 서버가 보낸 다음 송출 메시지.
async fn receive(socket: &mut Socket) -> IngestServerMessage {
    let next = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                Some(Ok(_)) => continue,
                other => panic!("송출 메시지 대신 {other:?}"),
            }
        }
    });
    next.await.expect("30초 안에 서버 응답이 없습니다")
}

/// 브라우저 MediaRecorder 대신 ffmpeg로 만든 webm(VP8 + Opus) 송출 영상.
fn sample_webm(seconds: u32) -> Vec<u8> {
    let output = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-t", &seconds.to_string()])
        .args(["-c:v", "libvpx", "-deadline", "realtime", "-c:a", "libopus"])
        .args(["-f", "webm", "pipe:1"])
        .stderr(Stdio::inherit())
        .output()
        .expect("ffmpeg가 필요합니다");
    assert!(output.status.success());
    output.stdout
}

/// MediaRecorder처럼 영상을 여러 조각으로 나눠 보낸다.
async fn send_video(socket: &mut Socket, video: &[u8]) {
    for chunk in video.chunks(16 * 1024) {
        socket
            .send(Message::Binary(chunk.to_vec().into()))
            .await
            .unwrap();
    }
}

fn first_segment(playlist: &str) -> &str {
    playlist
        .lines()
        .find(|line| line.ends_with(".ts"))
        .expect("재생 목록에 조각이 없습니다")
}

#[tokio::test]
async fn 송출하면_라이브_방송이_생기고_hls로_시청되며_종료하면_종료된다() {
    let server = LiveServer::start().await;

    let (mut socket, started) = server.start_broadcast("  첫 라이브  ").await;
    assert_eq!(started.title, "첫 라이브");
    assert_eq!(started.status, BroadcastStatus::Live);
    assert_eq!(started.ended_at, None);

    let live: Vec<Broadcast> = server
        .get_json(&broadcasts_path(Some(BroadcastStatus::Live)))
        .await;
    assert_eq!(
        live.iter().map(|b| b.id).collect::<Vec<_>>(),
        vec![started.id]
    );

    send_video(&mut socket, &sample_webm(5)).await;
    server
        .wait_for(started.id, "라이브 재생 가능", |b| b.live_playlist)
        .await;

    let playlist = server.get(&live_playlist_path(started.id)).await;
    assert_eq!(playlist.status, StatusCode::OK);
    assert_eq!(playlist.content_type, "application/vnd.apple.mpegurl");
    let playlist = String::from_utf8(playlist.body).unwrap();
    assert!(playlist.starts_with("#EXTM3U"), "{playlist}");
    let segment = server
        .get(&format!(
            "/api/broadcasts/{}/live/{}",
            started.id,
            first_segment(&playlist)
        ))
        .await;
    assert_eq!(segment.status, StatusCode::OK);
    assert_eq!(segment.content_type, "video/mp2t");
    assert!(!segment.body.is_empty());

    send(&mut socket, &IngestClientMessage::End).await;
    assert_eq!(
        receive(&mut socket).await,
        IngestServerMessage::Ended { reason: None }
    );

    let ended = server.broadcast(started.id).await;
    assert_eq!(ended.status, BroadcastStatus::Ended);
    assert!(ended.ended_at.unwrap() >= ended.created_at);
    let live: Vec<Broadcast> = server
        .get_json(&broadcasts_path(Some(BroadcastStatus::Live)))
        .await;
    assert!(live.is_empty());
}

#[tokio::test]
async fn 종료된_라이브_방송은_녹화본으로_다시_볼_수_있다() {
    let server = LiveServer::start().await;
    let (mut socket, started) = server.start_broadcast("녹화될 라이브").await;
    send_video(&mut socket, &sample_webm(4)).await;
    server
        .wait_for(started.id, "라이브 재생 가능", |b| b.live_playlist)
        .await;
    send(&mut socket, &IngestClientMessage::End).await;
    receive(&mut socket).await;

    let ended = server
        .wait_for(started.id, "녹화본 있음", |b| b.recording.is_some())
        .await;

    let recording = ended.recording.as_ref().unwrap();
    assert_eq!(recording.content_type, "video/mp4");
    let video = server.get(&recording_path(started.id)).await;
    assert_eq!(video.status, StatusCode::OK);
    assert_eq!(video.content_type, "video/mp4");
    assert_eq!(video.body.len() as u64, recording.size_bytes);
    assert_eq!(&video.body[4..8], b"ftyp");

    // 녹화본을 만든 뒤에는 라이브 재생 목록이 남지 않는다.
    let finalized = server.wait_until_finalized(started.id).await;
    assert!(!finalized.live_playlist);
    let playlist = server.get(&live_playlist_path(started.id)).await;
    assert_eq!(playlist.status, StatusCode::NOT_FOUND);

    // 녹화본 크기가 저장 총량 사용량에 들어간다.
    let policy: UploadPolicy = server.get_json(UPLOAD_POLICY_PATH).await;
    assert_eq!(
        policy.remaining_bytes,
        1024 * 1024 * 1024 - recording.size_bytes
    );
}

#[tokio::test]
async fn 영상을_보내지_않고_끝난_라이브_방송은_녹화본_없이_종료된다() {
    let server = LiveServer::start().await;
    let (mut socket, started) = server.start_broadcast("빈 방송").await;

    send(&mut socket, &IngestClientMessage::End).await;
    receive(&mut socket).await;

    let ended = server.wait_until_finalized(started.id).await;
    assert_eq!(ended.status, BroadcastStatus::Ended);
    assert_eq!(ended.recording, None);
    assert!(!ended.live_playlist);
    assert_eq!(server.stored_recordings().await, 0);
}

#[tokio::test]
async fn 송출_도중_저장_공간이_다_차면_녹화본을_남기지_않는다() {
    let server = LiveServer::start().await;
    let (mut socket, started) = server.start_broadcast("한도 넘는 라이브").await;
    send_video(&mut socket, &sample_webm(3)).await;
    server
        .wait_for(started.id, "라이브 재생 가능", |b| b.live_playlist)
        .await;

    // 송출 도중 다른 방송 등록이 남은 저장 공간을 모두 썼다.
    server.use_up_quota().await;
    send(&mut socket, &IngestClientMessage::End).await;
    receive(&mut socket).await;

    let ended = server.wait_until_finalized(started.id).await;
    assert_eq!(ended.status, BroadcastStatus::Ended);
    assert_eq!(ended.recording, None);
    assert_eq!(server.stored_recordings().await, 0);
}

#[tokio::test]
async fn 송출_연결이_끊기면_방송이_종료된다() {
    let server = LiveServer::start().await;
    let (mut socket, started) = server.start_broadcast("끊길 방송").await;
    send_video(&mut socket, &sample_webm(1)).await;

    drop(socket);

    server
        .wait_for(started.id, "종료", |b| b.status == BroadcastStatus::Ended)
        .await;
}

#[tokio::test]
async fn 제목이_규칙에_맞지_않으면_송출을_시작하지_않는다() {
    let server = LiveServer::start().await;

    for (title, message) in [
        ("   ", "제목을 입력해 주세요.".to_owned()),
        (
            &"가".repeat(101),
            "제목은 100자 이하로 입력해 주세요.".to_owned(),
        ),
    ] {
        let mut socket = server.connect().await;
        send(&mut socket, &start_message(title)).await;
        assert_eq!(
            receive(&mut socket).await,
            IngestServerMessage::Error { message }
        );
    }

    let all: Vec<Broadcast> = server.get_json(&broadcasts_path(None)).await;
    assert!(all.is_empty());
}

#[tokio::test]
async fn 시작_요청_없이_영상부터_보내면_송출을_시작하지_않는다() {
    let server = LiveServer::start().await;
    let mut socket = server.connect().await;

    socket
        .send(Message::Binary(vec![0u8; 16].into()))
        .await
        .unwrap();

    assert_eq!(
        receive(&mut socket).await,
        IngestServerMessage::Error {
            message: "송출 시작 요청을 먼저 보내야 합니다.".into()
        }
    );
    let all: Vec<Broadcast> = server.get_json(&broadcasts_path(None)).await;
    assert!(all.is_empty());
}

#[tokio::test]
async fn 라이브_재생_경로는_정해진_파일만_서빙한다() {
    let server = LiveServer::start().await;
    let (mut socket, started) = server.start_broadcast("경로 확인").await;
    send_video(&mut socket, &sample_webm(3)).await;
    server
        .wait_for(started.id, "라이브 재생 가능", |b| b.live_playlist)
        .await;

    for file in ["test.db", "..%2F..%2Ftest.db", "seg.ts", "index.m3u8.tmp"] {
        let response = server
            .get(&format!("/api/broadcasts/{}/live/{file}", started.id))
            .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{file}");
    }
    let unknown = server.get(&live_playlist_path(Uuid::now_v7())).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let bad_id = server
        .get("/api/broadcasts/없는-방송/live/index.m3u8")
        .await;
    assert_eq!(bad_id.status, StatusCode::NOT_FOUND);

    send(&mut socket, &IngestClientMessage::End).await;
    receive(&mut socket).await;
}

#[tokio::test]
async fn 동시_라이브_방송_수를_넘으면_송출을_시작하지_않는다() {
    let server = LiveServer::with_config(|config| config.max_live_broadcasts = 1).await;
    let (mut first, started) = server.start_broadcast("먼저 시작한 방송").await;

    server
        .assert_start_rejected(
            "지금은 동시에 진행할 수 있는 라이브 방송 수(1개)가 모두 차 있습니다. 잠시 후 다시 시도해 주세요.",
        )
        .await;
    let all: Vec<Broadcast> = server.get_json(&broadcasts_path(None)).await;
    assert_eq!(
        all.iter().map(|b| b.id).collect::<Vec<_>>(),
        vec![started.id]
    );

    // 먼저 시작한 방송이 끝나면 자리가 난다.
    send(&mut first, &IngestClientMessage::End).await;
    receive(&mut first).await;
    let (mut second, _) = server.start_broadcast("다음 방송").await;
    send(&mut second, &IngestClientMessage::End).await;
    receive(&mut second).await;
}

#[tokio::test]
async fn ffmpeg가_없으면_송출만_거부하고_녹화방송은_그대로_쓴다() {
    let server =
        LiveServer::with_config(|config| config.ffmpeg_path = "/없는/경로/ffmpeg".into()).await;

    server
        .assert_start_rejected(
            "서버에 ffmpeg가 없어 라이브 송출을 할 수 없습니다. 녹화방송 등록과 시청은 그대로 쓸 수 있습니다.",
        )
        .await;
    let all: Vec<Broadcast> = server.get_json(&broadcasts_path(None)).await;
    assert!(all.is_empty());
    let policy = server.get(UPLOAD_POLICY_PATH).await;
    assert_eq!(policy.status, StatusCode::OK);
}

#[tokio::test]
async fn 저장_공간이_없으면_송출을_시작하지_않는다() {
    let server = LiveServer::with_quota(0).await;

    server
        .assert_start_rejected("저장 공간 한도에 도달해 더 이상 방송을 시작할 수 없습니다.")
        .await;
}

#[tokio::test]
async fn 녹화본_크기_상한을_넘으면_서버가_방송을_종료한다() {
    let server = LiveServer::with_config(|config| config.max_upload_bytes = 64 * 1024).await;
    let (mut socket, started) = server.start_broadcast("큰 방송").await;

    send_video(&mut socket, &sample_webm(3)).await;

    assert_eq!(
        receive(&mut socket).await,
        IngestServerMessage::Ended {
            reason: Some("녹화본 크기 상한(64KB)에 닿아 방송을 종료했습니다.".into())
        }
    );
    let ended = server.broadcast(started.id).await;
    assert_eq!(ended.status, BroadcastStatus::Ended);
}

#[tokio::test]
async fn 영상이_한동안_오지_않으면_서버가_방송을_종료한다() {
    let server =
        LiveServer::with_config(|config| config.live_idle_timeout = Duration::from_secs(1)).await;
    let (mut socket, started) = server.start_broadcast("멈춘 방송").await;

    assert_eq!(
        receive(&mut socket).await,
        IngestServerMessage::Ended {
            reason: Some("1초 동안 영상이 오지 않아 방송을 종료했습니다.".into())
        }
    );
    let ended = server.broadcast(started.id).await;
    assert_eq!(ended.status, BroadcastStatus::Ended);
}

#[tokio::test]
async fn 서버가_송출_도중_멈췄다면_다시_시작할_때_방송을_종료하고_남은_영상을_녹화본으로_만든다() {
    let dir = tempfile::tempdir().unwrap();
    let config = LiveServer::config(&dir);

    // 서버가 멈추기 전 상태: 라이브 중인 방송 둘. 하나는 HLS 조각이 남아 있고 하나는 없다.
    drop(server::build_app(config.clone()).await.unwrap());
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&config.database_path),
    )
    .await
    .unwrap();
    let with_video = Uuid::now_v7();
    let without_video = Uuid::now_v7();
    for (id, title) in [
        (with_video, "조각이 남은 방송"),
        (without_video, "조각이 없는 방송"),
    ] {
        sqlx::query("INSERT INTO broadcasts (id, title, created_at) VALUES (?, ?, ?)")
            .bind(id.to_string())
            .bind(title)
            .bind("2026-11-12T09:00:00.000000Z")
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
    let live_dir = config.live_dir.join(with_video.to_string());
    std::fs::create_dir_all(&live_dir).unwrap();
    let status = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-t", "3", "-c:v", "libx264", "-c:a", "aac"])
        .args(["-f", "hls", "-hls_time", "1", "-hls_list_size", "0"])
        .arg("-hls_segment_filename")
        .arg(live_dir.join("seg%05d.ts"))
        .arg(live_dir.join("index.m3u8"))
        .status()
        .unwrap();
    assert!(status.success());

    let server = LiveServer::serve(dir, config).await;

    let recovered = server.broadcast(with_video).await;
    assert_eq!(recovered.status, BroadcastStatus::Ended);
    assert_eq!(recovered.recording.unwrap().content_type, "video/mp4");
    assert!(!live_dir.exists());
    let empty = server.broadcast(without_video).await;
    assert_eq!(empty.status, BroadcastStatus::Ended);
    assert_eq!(empty.recording, None);
}
