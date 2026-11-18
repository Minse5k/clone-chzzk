//! 여러 서버 인스턴스 통합 테스트(과제 5). 같은 PostgreSQL·Redis·공유 디렉터리를 쓰는 서버 두 개(A, B)를
//! 실제 포트에 띄우고, 요청이 어느 인스턴스로 가든 같은 상태와 실시간 알림을 보는지 확인한다.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::TestApp;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use shared::{
    AuthResponse, Broadcast, BroadcastStatus, Comment, CommentEvent, IngestClientMessage,
    IngestServerMessage, LIVE_INGEST_PATH, LOGOUT_PATH, ME_PATH, broadcast_end_path,
    broadcast_path, comment_path, comments_path, comments_ws_path, live_playlist_path,
};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn watch(app: &TestApp, broadcast: Uuid) -> Socket {
    let url = format!("ws://{}{}", app.addr, comments_ws_path(broadcast));
    tokio_tungstenite::connect_async(url).await.unwrap().0
}

/// 서버가 보낸 다음 댓글 알림.
async fn next_event(socket: &mut Socket) -> CommentEvent {
    let next = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                other => panic!("댓글 알림 대신 {other:?}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), next)
        .await
        .expect("5초 안에 댓글 알림이 오지 않았습니다")
}

async fn write_comment(app: &TestApp, user: &AuthResponse, broadcast: Uuid, body: &str) -> Comment {
    let reply = app
        .request(
            "POST",
            &comments_path(broadcast),
            Some(&user.token),
            Some(json!({ "body": body })),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED);
    reply.json()
}

async fn broadcast(app: &TestApp, id: Uuid) -> Broadcast {
    let reply = app.get(&broadcast_path(id)).await;
    assert_eq!(reply.status, StatusCode::OK);
    reply.json()
}

/// 조건을 만족할 때까지 방송을 다시 조회한다.
async fn wait_for(
    app: &TestApp,
    id: Uuid,
    what: &str,
    condition: impl Fn(&Broadcast) -> bool,
) -> Broadcast {
    for _ in 0..200 {
        let broadcast = broadcast(app, id).await;
        if condition(&broadcast) {
            return broadcast;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("20초 안에 {what} 상태가 되지 않았습니다");
}

/// `app`으로 송출을 시작하고 만들어진 라이브 방송을 돌려준다.
async fn start_broadcast(app: &TestApp, owner: &AuthResponse, title: &str) -> (Socket, Broadcast) {
    let url = format!("ws://{}{LIVE_INGEST_PATH}", app.addr);
    let mut socket = tokio_tungstenite::connect_async(url).await.unwrap().0;
    send(
        &mut socket,
        &IngestClientMessage::Start {
            token: Some(owner.token.clone()),
            title: title.into(),
            mime_type: "video/webm;codecs=vp8,opus".into(),
        },
    )
    .await;
    match receive(&mut socket).await {
        IngestServerMessage::Started { broadcast } => (socket, broadcast),
        other => panic!("송출이 시작되지 않았습니다: {other:?}"),
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
    next.await.expect("30초 안에 송출 메시지가 오지 않았습니다")
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
        .output()
        .expect("ffmpeg가 필요합니다");
    assert!(output.status.success());
    output.stdout
}

async fn send_video(socket: &mut Socket, video: &[u8]) {
    for chunk in video.chunks(16 * 1024) {
        socket
            .send(Message::Binary(chunk.to_vec().into()))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn 한_인스턴스에서_받은_토큰은_다른_인스턴스에서도_통하고_로그아웃은_모든_인스턴스에_적용된다()
 {
    let (a, b) = TestApp::cluster(|_| {}).await;
    let user = a.signup("viewer").await;

    let me = b.request("GET", ME_PATH, Some(&user.token), None).await;
    assert_eq!(me.status, StatusCode::OK);

    let logout = a
        .request("POST", LOGOUT_PATH, Some(&user.token), None)
        .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    let me = b.request("GET", ME_PATH, Some(&user.token), None).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn 다른_인스턴스로_쓴_댓글과_삭제도_실시간으로_받는다() {
    let (a, b) = TestApp::cluster(|_| {}).await;
    let streamer = a.signup("streamer").await;
    let viewer = b.signup("viewer").await;
    let id = a.insert_live_broadcast(&streamer, "두 인스턴스 방송").await;
    let mut on_a = watch(&a, id).await;
    let mut on_b = watch(&b, id).await;

    let comment = write_comment(&a, &viewer, id, "A로 쓴 댓글").await;

    for socket in [&mut on_a, &mut on_b] {
        assert_eq!(
            next_event(socket).await,
            CommentEvent::Created {
                comment: comment.clone()
            }
        );
    }
    // 목록도 어느 인스턴스로 보든 같다.
    let listed: Vec<Comment> = b.get(&comments_path(id)).await.json();
    assert_eq!(listed, vec![comment.clone()]);

    let deleted = b
        .request(
            "DELETE",
            &comment_path(id, comment.id),
            Some(&viewer.token),
            None,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    for socket in [&mut on_a, &mut on_b] {
        assert_eq!(
            next_event(socket).await,
            CommentEvent::Deleted {
                comment_id: comment.id
            }
        );
    }
}

#[tokio::test]
async fn 방송을_지우면_다른_인스턴스에_붙은_댓글_연결도_닫힌다() {
    let (a, b) = TestApp::cluster(|_| {}).await;
    let owner = a.signup("owner").await;
    let id = a.insert_ended_broadcast(&owner, "지울 방송").await;
    let mut on_b = watch(&b, id).await;

    let deleted = a
        .request("DELETE", &broadcast_path(id), Some(&owner.token), None)
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);

    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match on_b.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "5초 안에 B의 댓글 연결이 닫히지 않았습니다");
}

#[tokio::test]
async fn 다른_인스턴스에서_라이브를_보고_종료해도_송출이_끝나고_녹화본이_남는다() {
    let (a, b) = TestApp::cluster(|_| {}).await;
    let owner = a.signup("streamer").await;
    let (mut socket, started) = start_broadcast(&a, &owner, "A로 송출하는 방송").await;
    send_video(&mut socket, &sample_webm(3)).await;

    // A가 만드는 HLS 조각을 B도 공유 디렉터리에서 서빙한다.
    let live = wait_for(&b, started.id, "라이브 재생 가능", |b| {
        b.live_playlist
    })
    .await;
    assert_eq!(live.status, BroadcastStatus::Live);
    assert_eq!(
        b.get(&live_playlist_path(started.id)).await.status,
        StatusCode::OK
    );

    let ended = b
        .request(
            "POST",
            &broadcast_end_path(started.id),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(ended.status, StatusCode::OK);

    assert_eq!(
        receive(&mut socket).await,
        IngestServerMessage::Ended {
            reason: Some("소유자가 다른 기기에서 방송을 종료했습니다.".into())
        }
    );
    let recorded = wait_for(&b, started.id, "녹화본 있음", |b| {
        b.recording.is_some()
    })
    .await;
    assert_eq!(recorded.status, BroadcastStatus::Ended);
}
