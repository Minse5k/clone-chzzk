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
    broadcast_path, broadcasts_path, live_playlist_path,
};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tower::ServiceExt;
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 임시 DB·저장소·라이브 디렉터리를 쓰고 실제 포트에서 요청을 받는 서버.
struct LiveServer {
    _dir: TempDir,
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
        let dir = tempfile::tempdir().unwrap();
        let config = server::Config {
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
        };
        let app = server::build_app(config).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app.clone()).into_future());
        Self {
            _dir: dir,
            app,
            addr,
        }
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
    // 송출이 끝나면 재생 목록이 끝맺어진다.
    let playlist = server.get(&live_playlist_path(started.id)).await;
    assert!(
        String::from_utf8(playlist.body)
            .unwrap()
            .contains("#EXT-X-ENDLIST")
    );
    let live: Vec<Broadcast> = server
        .get_json(&broadcasts_path(Some(BroadcastStatus::Live)))
        .await;
    assert!(live.is_empty());
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
