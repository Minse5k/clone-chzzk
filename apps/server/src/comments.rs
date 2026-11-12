//! 방송별 댓글. 목록은 누구나 보고, 작성은 로그인한 사용자, 삭제는 작성자만 한다.

use std::{collections::HashMap, sync::Mutex};

use axum::{
    Json, Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::Response,
    routing::{delete, get},
};
use chrono::Utc;
use shared::{COMMENT_LIST_LIMIT, COMMENT_MAX_CHARS, Comment, CommentEvent, NewComment};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{AppState, api::ApiError, auth::AuthUser, db};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/broadcasts/{id}/comments",
            get(list_comments).post(create_comment),
        )
        .route("/api/broadcasts/{id}/comments/ws", get(comment_feed))
        .route(
            "/api/broadcasts/{id}/comments/{comment_id}",
            delete(delete_comment),
        )
}

/// 방송의 댓글 목록, 오래된 순. 로그인 없이 볼 수 있다.
async fn list_comments(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Comment>>, ApiError> {
    let id = existing_broadcast(&state, &id).await?;
    Ok(Json(
        db::list_comments(&state.db, id, COMMENT_LIST_LIMIT).await?,
    ))
}

/// 댓글 작성. 로그인한 사용자만 쓸 수 있고, 그 사용자가 작성자가 된다.
async fn create_comment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(new): Json<NewComment>,
) -> Result<(StatusCode, Json<Comment>), ApiError> {
    let broadcast_id = existing_broadcast(&state, &id).await?;
    let comment = Comment {
        id: Uuid::now_v7(),
        broadcast_id,
        author: auth.user,
        body: validate_body(&new.body)?,
        created_at: Utc::now(),
    };
    if !db::insert_comment(&state.db, &comment).await? {
        return Err(broadcast_not_found());
    }
    state.comments.publish(
        broadcast_id,
        CommentEvent::Created {
            comment: comment.clone(),
        },
    );
    Ok((StatusCode::CREATED, Json(comment)))
}

/// 댓글 삭제. 작성자만 지울 수 있다.
///
/// 판정 순서: 로그인하지 않았으면 401, 없는 방송·댓글이면 404, 작성자가 아니면 403.
async fn delete_comment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, comment_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let broadcast_id = existing_broadcast(&state, &id).await?;
    let not_found = || ApiError::not_found("댓글을 찾을 수 없습니다.");
    let comment_id = Uuid::parse_str(&comment_id).map_err(|_| not_found())?;
    let comment = db::find_comment(&state.db, broadcast_id, comment_id)
        .await?
        .ok_or_else(not_found)?;
    if comment.author.id != auth.user.id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "댓글을 쓴 사용자만 지울 수 있습니다.",
        ));
    }
    db::delete_comment(&state.db, comment.id).await?;
    state.comments.publish(
        broadcast_id,
        CommentEvent::Deleted {
            comment_id: comment.id,
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// 방송이 있는지 확인하고 ID를 돌려준다. ID 형식이 틀려도 없는 방송과 똑같이 404로 답한다.
async fn existing_broadcast(state: &AppState, id: &str) -> Result<Uuid, ApiError> {
    let id = Uuid::parse_str(id).map_err(|_| broadcast_not_found())?;
    match db::find_broadcast(&state.db, id).await? {
        Some(_) => Ok(id),
        None => Err(broadcast_not_found()),
    }
}

fn broadcast_not_found() -> ApiError {
    ApiError::not_found("방송을 찾을 수 없습니다.")
}

fn validate_body(body: &str) -> Result<String, ApiError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "댓글 내용을 입력해 주세요.",
        ));
    }
    if body.chars().count() > COMMENT_MAX_CHARS {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("댓글은 {COMMENT_MAX_CHARS}자 이하로 입력해 주세요."),
        ));
    }
    Ok(body.to_owned())
}

/// 방송별 댓글 알림 채널. 댓글 WebSocket 연결마다 구독하고, 댓글 API가 처리를 마친 뒤 알린다.
#[derive(Default)]
pub struct CommentHub {
    channels: Mutex<HashMap<Uuid, broadcast::Sender<CommentEvent>>>,
}

/// 구독자가 늦어 쌓아 둘 수 있는 알림 수. 넘치면 오래된 알림부터 건너뛴다.
const CHANNEL_CAPACITY: usize = 64;

impl CommentHub {
    fn subscribe(&self, broadcast_id: Uuid) -> broadcast::Receiver<CommentEvent> {
        self.lock()
            .entry(broadcast_id)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .subscribe()
    }

    /// 구독자에게 알린다. 구독자가 없으면 채널을 정리한다.
    fn publish(&self, broadcast_id: Uuid, event: CommentEvent) {
        let mut channels = self.lock();
        if let Some(sender) = channels.get(&broadcast_id)
            && sender.send(event).is_err()
        {
            channels.remove(&broadcast_id);
        }
    }

    /// 구독이 끝났을 때 남은 구독자가 없으면 채널을 정리한다.
    fn release(&self, broadcast_id: Uuid) {
        let mut channels = self.lock();
        if channels
            .get(&broadcast_id)
            .is_some_and(|sender| sender.receiver_count() == 0)
        {
            channels.remove(&broadcast_id);
        }
    }

    /// 방송이 지워졌다. 채널을 없애 그 방송의 댓글 WebSocket 연결이 모두 닫히게 한다.
    pub fn close(&self, broadcast_id: Uuid) {
        self.lock().remove(&broadcast_id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, broadcast::Sender<CommentEvent>>> {
        self.channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 댓글 실시간 WebSocket. 읽기 전용이라 로그인하지 않아도 연결할 수 있다.
async fn comment_feed(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let broadcast_id = existing_broadcast(&state, &id).await?;
    // 연결을 받기 전에 구독해, 연결 직후에 쓰인 댓글도 놓치지 않게 한다.
    let events = state.comments.subscribe(broadcast_id);
    Ok(ws.on_upgrade(move |socket| async move {
        relay_events(socket, events).await;
        state.comments.release(broadcast_id);
    }))
}

async fn relay_events(mut socket: WebSocket, mut events: broadcast::Receiver<CommentEvent>) {
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    let text = serde_json::to_string(&event).expect("댓글 알림은 항상 JSON으로 바뀐다");
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                // 너무 늦은 구독자는 놓친 알림을 건너뛴다. 웹은 다시 연결할 때 목록을 새로 받는다.
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!("댓글 알림 {skipped}개를 건너뜁니다");
                }
                // 방송이 지워졌다.
                Err(broadcast::error::RecvError::Closed) => {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                // 시청자가 보내는 메시지는 쓰지 않는다(Ping에는 axum이 알아서 답한다).
                Some(Ok(_)) => {}
            },
        }
    }
}
