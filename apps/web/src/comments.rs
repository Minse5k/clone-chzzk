//! 방송 상세의 댓글 영역.

use dioxus::prelude::*;
use shared::{COMMENT_MAX_CHARS, Comment};
use uuid::Uuid;

use crate::{LoginRequired, api, format_time, session};

/// 방송의 댓글 목록과 작성란. 목록은 누구나 보고, 작성은 로그인한 사용자, 삭제는 작성자만 한다.
#[component]
pub fn CommentSection(broadcast_id: Uuid) -> Element {
    let mut comments = use_signal(Vec::<Comment>::new);
    let mut load_error = use_signal(|| None::<String>);
    let mut draft = use_signal(String::new);
    let mut sending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    use_future(move || async move {
        match api::list_comments(broadcast_id).await {
            Ok(list) => {
                comments.set(list);
                load_error.set(None);
            }
            Err(message) => load_error.set(Some(message)),
        }
    });

    let submit = move |event: FormEvent| async move {
        event.prevent_default();
        if sending() {
            return;
        }
        let body = draft().trim().to_owned();
        if body.is_empty() {
            error.set(Some("댓글 내용을 입력해 주세요.".into()));
            return;
        }
        sending.set(true);
        match api::create_comment(broadcast_id, &body).await {
            Ok(comment) => {
                add_comment(comments, comment);
                draft.set(String::new());
                error.set(None);
            }
            Err(message) => error.set(Some(message)),
        }
        sending.set(false);
    };

    let user = session::current_user();
    let count = comments.read().len();

    rsx! {
        section { class: "comments",
            h2 { "댓글 {count}" }
            if let Some(message) = load_error() {
                p { class: "notice error", "{message}" }
            }
            if user.is_some() {
                form { class: "comment-form", onsubmit: submit,
                    textarea {
                        "aria-label": "댓글",
                        rows: 2,
                        maxlength: COMMENT_MAX_CHARS as i64,
                        placeholder: "댓글을 남겨 보세요 ({COMMENT_MAX_CHARS}자까지)",
                        value: "{draft}",
                        disabled: sending(),
                        oninput: move |event| draft.set(event.value()),
                    }
                    button { r#type: "submit", disabled: sending(), "등록" }
                }
            } else {
                LoginRequired { action: "댓글을 쓰려면" }
            }
            if let Some(message) = error() {
                p { class: "notice error", role: "alert", "{message}" }
            }
            if count == 0 && load_error().is_none() {
                p { class: "notice", "아직 댓글이 없습니다." }
            }
            ul { class: "comment-list",
                for comment in comments() {
                    CommentItem {
                        key: "{comment.id}",
                        mine: user.as_ref().is_some_and(|user| user.id == comment.author.id),
                        comment: comment.clone(),
                        on_deleted: move |id| remove_comment(comments, id),
                        on_error: move |message| error.set(Some(message)),
                    }
                }
            }
        }
    }
}

/// 댓글 하나. 내 댓글이면 삭제 버튼을 보여 준다(삭제 권한은 서버가 다시 확인한다).
#[component]
fn CommentItem(
    comment: Comment,
    mine: bool,
    on_deleted: EventHandler<Uuid>,
    on_error: EventHandler<String>,
) -> Element {
    let mut deleting = use_signal(|| false);
    let broadcast_id = comment.broadcast_id;
    let id = comment.id;
    let delete = move |_| async move {
        deleting.set(true);
        match api::delete_comment(broadcast_id, id).await {
            Ok(()) => on_deleted.call(id),
            Err(message) => {
                on_error.call(message);
                deleting.set(false);
            }
        }
    };

    rsx! {
        li { class: "comment",
            div { class: "comment-meta",
                span { class: "author", "{comment.author.username}" }
                span { class: "time", {format_time(comment.created_at)} }
                if mine {
                    button {
                        class: "link",
                        disabled: deleting(),
                        onclick: delete,
                        "삭제"
                    }
                }
            }
            p { class: "comment-body", "{comment.body}" }
        }
    }
}

/// 목록에 댓글을 더한다. 이미 있는 댓글이면(실시간 알림과 작성 응답이 둘 다 온 경우) 그대로 둔다.
pub fn add_comment(mut comments: Signal<Vec<Comment>>, comment: Comment) {
    let mut list = comments.write();
    if list.iter().all(|existing| existing.id != comment.id) {
        list.push(comment);
    }
}

pub fn remove_comment(mut comments: Signal<Vec<Comment>>, id: Uuid) {
    comments.write().retain(|comment| comment.id != id);
}
