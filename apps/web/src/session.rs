//! 로그인 상태. 서버가 준 토큰과 사용자를 브라우저 저장소에 두어 새로 고쳐도 로그인이 유지된다.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use shared::User;

/// 로그인한 사용자와 그 토큰.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user: User,
}

/// 지금의 로그인 상태. 비로그인이면 `None`.
pub static SESSION: GlobalSignal<Option<Session>> = Signal::global(load);

const STORAGE_KEY: &str = "clone-chzzk.session";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn load() -> Option<Session> {
    let text = storage()?.get_item(STORAGE_KEY).ok()??;
    serde_json::from_str(&text).ok()
}

/// 로그인한 상태로 바꾼다.
pub fn sign_in(session: Session) {
    if let (Some(storage), Ok(text)) = (storage(), serde_json::to_string(&session)) {
        let _ = storage.set_item(STORAGE_KEY, &text);
    }
    *SESSION.write() = Some(session);
}

/// 로그아웃한 상태로 바꾼다. 서버가 토큰을 거절(401)했을 때도 부른다.
pub fn sign_out() {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(STORAGE_KEY);
    }
    if SESSION.peek().is_some() {
        *SESSION.write() = None;
    }
}

/// API 요청에 붙일 토큰.
pub fn token() -> Option<String> {
    SESSION.peek().as_ref().map(|session| session.token.clone())
}

/// 로그인한 사용자.
pub fn current_user() -> Option<User> {
    SESSION.read().as_ref().map(|session| session.user.clone())
}
