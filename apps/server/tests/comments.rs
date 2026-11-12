//! 방송별 댓글 통합 테스트.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::json;
use shared::{AuthResponse, Comment, CommentEvent, comment_path, comments_path, comments_ws_path};
use uuid::Uuid;

/// 방송 하나와 그 소유자, 시청자가 있는 서버.
struct CommentFixture {
    app: TestApp,
    owner: AuthResponse,
    viewer: AuthResponse,
    broadcast: Uuid,
}

impl CommentFixture {
    async fn new() -> Self {
        let app = TestApp::start().await;
        let owner = app.signup("owner").await;
        let viewer = app.signup("viewer").await;
        let broadcast = app.insert_ended_broadcast(&owner, "댓글을 받을 방송").await;
        Self {
            app,
            owner,
            viewer,
            broadcast,
        }
    }

    async fn write(&self, token: Option<&str>, body: &str) -> common::Reply {
        self.app
            .request(
                "POST",
                &comments_path(self.broadcast),
                token,
                Some(json!({ "body": body })),
            )
            .await
    }

    /// 댓글을 쓰고 성공을 확인한 뒤 만들어진 댓글을 돌려준다.
    async fn comment(&self, author: &AuthResponse, body: &str) -> Comment {
        let reply = self.write(Some(&author.token), body).await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.message());
        reply.json()
    }

    async fn list(&self) -> Vec<Comment> {
        let reply = self.app.get(&comments_path(self.broadcast)).await;
        assert_eq!(reply.status, StatusCode::OK);
        reply.json()
    }
}

#[tokio::test]
async fn 로그인한_사용자가_쓴_댓글이_작성자와_함께_오래된_순으로_보인다() {
    let fixture = CommentFixture::new().await;
    assert_eq!(fixture.list().await, vec![]);

    let first = fixture.comment(&fixture.viewer, "  첫 댓글  ").await;
    let second = fixture.comment(&fixture.owner, "두 번째 댓글").await;

    assert_eq!(first.body, "첫 댓글");
    assert_eq!(first.author, fixture.viewer.user);
    assert_eq!(first.broadcast_id, fixture.broadcast);
    assert_eq!(second.author, fixture.owner.user);
    // 목록은 로그인 없이 볼 수 있다.
    assert_eq!(fixture.list().await, vec![first, second]);
}

#[tokio::test]
async fn 로그인하지_않으면_댓글을_쓸_수_없다() {
    let fixture = CommentFixture::new().await;

    let anonymous = fixture.write(None, "비로그인 댓글").await;
    let invalid = fixture.write(Some("not-a-jwt"), "잘못된 토큰 댓글").await;

    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.message(), "로그인이 필요합니다.");
    assert_eq!(invalid.status, StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.list().await, vec![]);
}

#[tokio::test]
async fn 비었거나_너무_긴_댓글은_400이다() {
    let fixture = CommentFixture::new().await;
    let token = fixture.viewer.token.clone();

    for (body, message) in [
        ("   ".to_owned(), "댓글 내용을 입력해 주세요."),
        ("가".repeat(301), "댓글은 300자 이하로 입력해 주세요."),
    ] {
        let reply = fixture.write(Some(&token), &body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(reply.message(), message);
    }
    // 300자까지는 쓸 수 있다.
    let longest = fixture.comment(&fixture.viewer, &"가".repeat(300)).await;
    assert_eq!(fixture.list().await, vec![longest]);
}

#[tokio::test]
async fn 없는_방송의_댓글은_404다() {
    let fixture = CommentFixture::new().await;
    let missing = Uuid::now_v7();

    let list = fixture.app.get(&comments_path(missing)).await;
    let write = fixture
        .app
        .request(
            "POST",
            &comments_path(missing),
            Some(&fixture.viewer.token),
            Some(json!({ "body": "댓글" })),
        )
        .await;

    assert_eq!(list.status, StatusCode::NOT_FOUND);
    assert_eq!(write.status, StatusCode::NOT_FOUND);
    assert_eq!(
        fixture
            .app
            .get("/api/broadcasts/not-a-uuid/comments")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn 작성자만_댓글을_지울_수_있다() {
    let fixture = CommentFixture::new().await;
    let comment = fixture.comment(&fixture.viewer, "지울 댓글").await;
    let kept = fixture.comment(&fixture.owner, "남을 댓글").await;
    let path = comment_path(fixture.broadcast, comment.id);

    let anonymous = fixture.app.request("DELETE", &path, None, None).await;
    // 방송 소유자여도 다른 사람의 댓글은 지울 수 없다.
    let other = fixture
        .app
        .request("DELETE", &path, Some(&fixture.owner.token), None)
        .await;
    let author = fixture
        .app
        .request("DELETE", &path, Some(&fixture.viewer.token), None)
        .await;

    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(other.status, StatusCode::FORBIDDEN);
    assert_eq!(other.message(), "댓글을 쓴 사용자만 지울 수 있습니다.");
    assert_eq!(author.status, StatusCode::NO_CONTENT);
    assert_eq!(fixture.list().await, vec![kept]);
    // 이미 지운 댓글은 없다.
    let again = fixture
        .app
        .request("DELETE", &path, Some(&fixture.viewer.token), None)
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn 다른_방송의_댓글_경로로는_지울_수_없다() {
    let fixture = CommentFixture::new().await;
    let comment = fixture.comment(&fixture.viewer, "이 방송의 댓글").await;
    let other_broadcast = fixture
        .app
        .insert_ended_broadcast(&fixture.owner, "다른 방송")
        .await;

    let reply = fixture
        .app
        .request(
            "DELETE",
            &comment_path(other_broadcast, comment.id),
            Some(&fixture.viewer.token),
            None,
        )
        .await;

    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(fixture.list().await, vec![comment]);
}

#[tokio::test]
async fn 방송을_지우면_댓글도_지워진다() {
    let fixture = CommentFixture::new().await;
    fixture
        .comment(&fixture.viewer, "방송과 함께 사라질 댓글")
        .await;

    let deleted = fixture
        .app
        .request(
            "DELETE",
            &format!("/api/broadcasts/{}", fixture.broadcast),
            Some(&fixture.owner.token),
            None,
        )
        .await;

    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(
        fixture
            .app
            .get(&comments_path(fixture.broadcast))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(fixture.app.dir.path().join("test.db")),
    )
    .await
    .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM comments")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 비로그인 시청자로 댓글 WebSocket에 연결한다.
async fn watch(app: &TestApp, broadcast: Uuid) -> Socket {
    let url = format!("ws://{}{}", app.addr, comments_ws_path(broadcast));
    tokio_tungstenite::connect_async(url).await.unwrap().0
}

/// 서버가 보낸 다음 댓글 알림.
async fn next_event(socket: &mut Socket) -> CommentEvent {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    let next = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                other => panic!("댓글 알림 대신 {other:?}"),
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), next)
        .await
        .expect("5초 안에 댓글 알림이 오지 않았습니다")
}

#[tokio::test]
async fn 댓글_웹소켓으로_다른_사용자의_댓글_작성과_삭제를_바로_받는다() {
    let fixture = CommentFixture::new().await;
    let mut first = watch(&fixture.app, fixture.broadcast).await;
    let mut second = watch(&fixture.app, fixture.broadcast).await;

    let comment = fixture.comment(&fixture.viewer, "실시간 댓글").await;

    for socket in [&mut first, &mut second] {
        assert_eq!(
            next_event(socket).await,
            CommentEvent::Created {
                comment: comment.clone()
            }
        );
    }

    let deleted = fixture
        .app
        .request(
            "DELETE",
            &comment_path(fixture.broadcast, comment.id),
            Some(&fixture.viewer.token),
            None,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(
        next_event(&mut first).await,
        CommentEvent::Deleted {
            comment_id: comment.id
        }
    );
}

#[tokio::test]
async fn 다른_방송의_댓글은_알리지_않는다() {
    let fixture = CommentFixture::new().await;
    let other = fixture
        .app
        .insert_ended_broadcast(&fixture.owner, "다른 방송")
        .await;
    let mut socket = watch(&fixture.app, fixture.broadcast).await;

    let reply = fixture
        .app
        .request(
            "POST",
            &comments_path(other),
            Some(&fixture.viewer.token),
            Some(json!({ "body": "다른 방송 댓글" })),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let mine = fixture.comment(&fixture.viewer, "이 방송 댓글").await;

    // 먼저 쓴 다른 방송의 댓글은 오지 않고, 이 방송의 댓글만 온다.
    assert_eq!(
        next_event(&mut socket).await,
        CommentEvent::Created { comment: mine }
    );
}

#[tokio::test]
async fn 없는_방송의_댓글_웹소켓은_404다() {
    let app = TestApp::start().await;
    let url = format!("ws://{}{}", app.addr, comments_ws_path(Uuid::now_v7()));

    let error = tokio_tungstenite::connect_async(url).await.unwrap_err();

    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        other => panic!("404 대신 {other:?}"),
    }
}

#[tokio::test]
async fn 방송을_지우면_댓글_웹소켓이_닫힌다() {
    use futures_util::StreamExt;
    let fixture = CommentFixture::new().await;
    let mut socket = watch(&fixture.app, fixture.broadcast).await;

    let deleted = fixture
        .app
        .request(
            "DELETE",
            &format!("/api/broadcasts/{}", fixture.broadcast),
            Some(&fixture.owner.token),
            None,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);

    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => return,
                Some(Ok(_)) => continue,
                Some(Err(_)) => return,
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "방송을 지웠는데 댓글 웹소켓이 닫히지 않았습니다"
    );
}
