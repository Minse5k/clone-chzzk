//! 방송 상세의 댓글 영역.

use dioxus::prelude::*;
use futures_channel::mpsc::{UnboundedSender, unbounded};
use futures_util::StreamExt;
use gloo_timers::future::TimeoutFuture;
use shared::{COMMENT_MAX_CHARS, Comment, CommentEvent, comments_ws_path};
use uuid::Uuid;
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{CloseEvent, MessageEvent, WebSocket};

use crate::{LoginRequired, api, format_time, session};

/// 실시간 연결이 끊기면 이만큼 기다렸다가 다시 연결한다.
const RECONNECT_MILLIS: u32 = 3000;

/// 방송의 댓글 목록과 작성란. 목록은 누구나 보고, 작성은 로그인한 사용자, 삭제는 작성자만 한다.
#[component]
pub fn CommentSection(broadcast_id: Uuid) -> Element {
    let comments = use_signal(Vec::<Comment>::new);
    let load_error = use_signal(|| None::<String>);
    let mut draft = use_signal(String::new);
    let mut sending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // 댓글 실시간 연결. 연결될 때마다 목록을 새로 받아 끊긴 동안 놓친 댓글을 채우고,
    // 끊기면 잠시 뒤 다시 연결한다. 화면을 떠나면 이 작업이 멈추며 연결도 닫힌다.
    use_future(move || async move {
        loop {
            let (sender, mut events) = unbounded();
            let feed = CommentFeed::connect(broadcast_id, sender);
            if feed.is_none() {
                // 실시간 연결을 만들 수 없어도 목록은 보여 준다.
                refresh(broadcast_id, comments, load_error).await;
            }
            while let Some(event) = events.next().await {
                match event {
                    FeedEvent::Opened => refresh(broadcast_id, comments, load_error).await,
                    FeedEvent::Comment(CommentEvent::Created { comment }) => {
                        add_comment(comments, comment)
                    }
                    FeedEvent::Comment(CommentEvent::Deleted { comment_id }) => {
                        remove_comment(comments, comment_id)
                    }
                    FeedEvent::Closed => break,
                }
            }
            drop(feed);
            TimeoutFuture::new(RECONNECT_MILLIS).await;
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

/// 댓글 목록을 새로 받는다.
async fn refresh(
    broadcast_id: Uuid,
    mut comments: Signal<Vec<Comment>>,
    mut load_error: Signal<Option<String>>,
) {
    match api::list_comments(broadcast_id).await {
        Ok(list) => {
            comments.set(list);
            load_error.set(None);
        }
        Err(message) => load_error.set(Some(message)),
    }
}

enum FeedEvent {
    /// 연결되었다. 이제부터의 댓글 알림은 놓치지 않는다.
    Opened,
    Comment(CommentEvent),
    /// 연결이 끊겼다(방송 삭제, 네트워크, 서버 재시작).
    Closed,
}

/// 댓글 실시간 WebSocket 연결. 버리면 연결을 닫는다.
struct CommentFeed {
    socket: WebSocket,
    _on_open: Closure<dyn FnMut()>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
}

impl CommentFeed {
    fn connect(broadcast_id: Uuid, events: UnboundedSender<FeedEvent>) -> Option<Self> {
        let socket = WebSocket::new(&api::ws_url(&comments_ws_path(broadcast_id))).ok()?;
        let on_open = {
            let events = events.clone();
            Closure::<dyn FnMut()>::new(move || {
                let _ = events.unbounded_send(FeedEvent::Opened);
            })
        };
        let on_message = {
            let events = events.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                if let Some(event) = event
                    .data()
                    .as_string()
                    .and_then(|text| serde_json::from_str(&text).ok())
                {
                    let _ = events.unbounded_send(FeedEvent::Comment(event));
                }
            })
        };
        let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |_: CloseEvent| {
            let _ = events.unbounded_send(FeedEvent::Closed);
        });
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        Some(Self {
            socket,
            _on_open: on_open,
            _on_message: on_message,
            _on_close: on_close,
        })
    }
}

impl Drop for CommentFeed {
    fn drop(&mut self) {
        // 버린 클로저를 브라우저가 부르지 않도록 먼저 떼어 낸다.
        self.socket.set_onopen(None);
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        let _ = self.socket.close();
    }
}
