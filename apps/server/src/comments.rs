//! 방송별 댓글. 목록은 누구나 보고, 작성은 로그인한 사용자, 삭제는 작성자만 한다.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
};
use chrono::Utc;
use shared::{COMMENT_LIST_LIMIT, COMMENT_MAX_CHARS, Comment, NewComment};
use uuid::Uuid;

use crate::{AppState, api::ApiError, auth::AuthUser, db};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/broadcasts/{id}/comments",
            get(list_comments).post(create_comment),
        )
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
