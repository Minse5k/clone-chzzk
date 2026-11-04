//! clone-chzzk 웹 앱.

mod api;

use chrono::{DateTime, Local, Utc};
use dioxus::prelude::*;
use shared::{TITLE_MAX_CHARS, UploadPolicy, format_bytes};
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;

const MAIN_CSS: Asset = asset!("/assets/main.css");

#[derive(Routable, Clone, PartialEq)]
// 변형 이름이 곧 화면 컴포넌트 이름이다.
#[allow(clippy::enum_variant_names)]
enum Route {
    #[layout(Shell)]
    #[route("/")]
    BroadcastList {},
    #[route("/broadcasts/new")]
    BroadcastRegistration {},
    #[route("/broadcasts/:id")]
    BroadcastDetail { id: String },
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Title { "clone-chzzk" }
        document::Stylesheet { href: MAIN_CSS }
        Router::<Route> {}
    }
}

#[component]
fn Shell() -> Element {
    rsx! {
        header { class: "topbar",
            Link { class: "brand", to: Route::BroadcastList {}, "clone-chzzk" }
            Link { class: "button", to: Route::BroadcastRegistration {}, "방송 등록" }
        }
        main { class: "page", Outlet::<Route> {} }
    }
}

#[component]
fn BroadcastList() -> Element {
    let broadcasts = use_resource(api::list_broadcasts);

    rsx! {
        h1 { "방송 목록" }
        match &*broadcasts.read() {
            None => rsx! { p { class: "notice", "불러오는 중…" } },
            Some(Err(message)) => rsx! { p { class: "notice error", "{message}" } },
            Some(Ok(list)) if list.is_empty() => rsx! {
                p { class: "notice", "등록된 방송이 없습니다." }
            },
            Some(Ok(list)) => rsx! {
                ul { class: "broadcast-list",
                    for broadcast in list {
                        li { key: "{broadcast.id}",
                            Link {
                                class: "broadcast-card",
                                to: Route::BroadcastDetail { id: broadcast.id.to_string() },
                                span { class: "title", "{broadcast.title}" }
                                span { class: "meta", {format_time(broadcast.created_at)} }
                            }
                        }
                    }
                }
            },
        }
    }
}

#[component]
fn BroadcastDetail(id: ReadSignal<String>) -> Element {
    let broadcast = use_resource(move || async move { api::get_broadcast(&id()).await });

    match &*broadcast.read() {
        None => rsx! { p { class: "notice", "불러오는 중…" } },
        Some(Err(message)) => rsx! { p { class: "notice error", "{message}" } },
        Some(Ok(broadcast)) => rsx! {
            article { class: "broadcast-detail",
                if broadcast.recording.is_some() {
                    video {
                        class: "player",
                        src: api::recording_url(broadcast.id),
                        controls: true,
                        preload: "metadata",
                    }
                } else {
                    p { class: "notice", "이 방송에는 녹화본이 없습니다." }
                }
                h1 { "{broadcast.title}" }
                p { class: "meta", "등록 " {format_time(broadcast.created_at)} }
            }
        },
    }
}

const RECORDING_INPUT_ID: &str = "recording-file";

#[component]
fn BroadcastRegistration() -> Element {
    let mut title = use_signal(String::new);
    let mut submitting = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut policy = use_resource(api::get_upload_policy);
    let navigator = use_navigator();

    let submit = move |event: FormEvent| async move {
        event.prevent_default();
        if submitting() {
            return;
        }
        let file = selected_recording();
        // 서버도 같은 검사를 하지만, 큰 파일을 다 올린 뒤에 거부되지 않도록 먼저 확인한다.
        let current_policy = policy.read().as_ref().and_then(|p| p.as_ref().ok()).cloned();
        if let Err(message) = check_registration(&title(), file.as_ref(), current_policy.as_ref()) {
            error.set(Some(message));
            return;
        }
        let Some(file) = file else { return };
        submitting.set(true);
        error.set(None);
        match api::register_broadcast(&title(), &file).await {
            Ok(broadcast) => {
                navigator.push(Route::BroadcastDetail { id: broadcast.id.to_string() });
            }
            Err(message) => {
                error.set(Some(message));
                // 다른 등록으로 남은 저장 공간이 바뀌었을 수 있다.
                policy.restart();
            }
        }
        submitting.set(false);
    };

    rsx! {
        h1 { "방송 등록" }
        form { class: "registration-form", onsubmit: submit,
            label { r#for: "title", "제목" }
            input {
                id: "title",
                r#type: "text",
                maxlength: TITLE_MAX_CHARS as i64,
                placeholder: "방송 제목",
                value: "{title}",
                disabled: submitting(),
                oninput: move |event| title.set(event.value()),
            }
            label { r#for: RECORDING_INPUT_ID, "녹화본 (mp4, webm)" }
            input {
                id: RECORDING_INPUT_ID,
                r#type: "file",
                accept: "video/mp4,video/webm,.mp4,.webm",
                disabled: submitting(),
            }
            if let Some(Ok(policy)) = &*policy.read() {
                p { class: "hint",
                    "파일 하나당 최대 {format_bytes(policy.max_upload_bytes)} · 남은 저장 공간 {format_bytes(policy.remaining_bytes)}"
                }
            }
            if let Some(message) = error() {
                p { class: "notice error", role: "alert", "{message}" }
            }
            button { r#type: "submit", disabled: submitting(),
                if submitting() { "업로드 중…" } else { "등록" }
            }
        }
    }
}

/// 서버에 보내기 전에 걸러낼 수 있는 등록 오류. 문구는 서버와 같다.
fn check_registration(
    title: &str,
    file: Option<&web_sys::File>,
    policy: Option<&UploadPolicy>,
) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("제목을 입력해 주세요.".into());
    }
    let Some(file) = file else {
        return Err("녹화본 파일을 선택해 주세요.".into());
    };
    let name = file.name().to_ascii_lowercase();
    if !(name.ends_with(".mp4") || name.ends_with(".webm")) {
        return Err("mp4 또는 webm 영상만 등록할 수 있습니다.".into());
    }
    if let Some(policy) = policy {
        let size = file.size() as u64;
        if size > policy.max_upload_bytes {
            return Err(format!("녹화본은 {} 이하만 등록할 수 있습니다.", format_bytes(policy.max_upload_bytes)));
        }
        if size > policy.remaining_bytes {
            return Err("저장 공간 한도에 도달해 더 이상 방송을 등록할 수 없습니다.".into());
        }
    }
    Ok(())
}

/// 파일 입력에서 고른 녹화본. 내용을 읽지 않고 브라우저의 `File` 핸들만 가져온다.
fn selected_recording() -> Option<web_sys::File> {
    web_sys::window()?
        .document()?
        .get_element_by_id(RECORDING_INPUT_ID)?
        .dyn_into::<HtmlInputElement>()
        .ok()?
        .files()?
        .get(0)
}

/// 등록 시각을 보는 사람의 시간대로 표시한다.
fn format_time(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}
