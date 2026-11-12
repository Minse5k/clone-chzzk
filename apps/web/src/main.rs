//! clone-chzzk 웹 앱.

mod api;
mod live;

use chrono::{DateTime, Local, Utc};
use dioxus::prelude::*;
use futures_util::StreamExt;
use gloo_timers::future::TimeoutFuture;
use live::{Facing, LiveEvent, LiveSession};
use shared::{Broadcast, BroadcastStatus, TITLE_MAX_CHARS, UploadPolicy, format_bytes};
use uuid::Uuid;
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use web_sys::MediaStream;

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
    #[route("/live")]
    LiveBroadcast {},
    #[route("/broadcasts/:id")]
    BroadcastDetail { id: String },
    #[route("/:..segments")]
    PageNotFound { segments: Vec<String> },
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
            nav { class: "actions",
                Link { class: "button live", to: Route::LiveBroadcast {}, "라이브 송출" }
                Link { class: "button", to: Route::BroadcastRegistration {}, "방송 등록" }
            }
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
            Some(Ok(list)) => {
                let (live, ended): (Vec<_>, Vec<_>) = list
                    .iter()
                    .cloned()
                    .partition(|broadcast| broadcast.status == BroadcastStatus::Live);
                rsx! {
                    BroadcastSection {
                        heading: "라이브 중",
                        empty: "지금 라이브 중인 방송이 없습니다.",
                        broadcasts: live,
                    }
                    BroadcastSection {
                        heading: "지난 방송",
                        empty: "등록된 방송이 없습니다.",
                        broadcasts: ended,
                    }
                }
            }
        }
    }
}

/// 같은 상태의 방송 묶음. 비어 있으면 안내 문구를 보여 준다.
#[component]
fn BroadcastSection(heading: String, empty: String, broadcasts: Vec<Broadcast>) -> Element {
    rsx! {
        section { class: "broadcast-section",
            h2 { "{heading}" }
            if broadcasts.is_empty() {
                p { class: "notice", "{empty}" }
            } else {
                ul { class: "broadcast-list",
                    for broadcast in broadcasts {
                        li { key: "{broadcast.id}",
                            Link {
                                class: "broadcast-card",
                                to: Route::BroadcastDetail { id: broadcast.id.to_string() },
                                StatusBadge { status: broadcast.status }
                                span { class: "title", "{broadcast.title}" }
                                span { class: "meta", {format_time(broadcast.created_at)} }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 방송 상태 배지. 라이브 중이면 눈에 띄게 표시한다.
#[component]
fn StatusBadge(status: BroadcastStatus) -> Element {
    let class = match status {
        BroadcastStatus::Live => "badge live",
        BroadcastStatus::Ended => "badge ended",
    };
    rsx! { span { class, {status.label()} } }
}

/// 라이브 방송이면 상태와 재생 목록이 바뀌었는지 이 간격으로 다시 확인한다.
const LIVE_REFRESH_MILLIS: u32 = 3000;

#[component]
fn BroadcastDetail(id: ReadSignal<String>) -> Element {
    let mut broadcast = use_resource(move || async move { api::get_broadcast(&id()).await });

    // 라이브 중이거나 녹화본을 만드는 중인 방송은 주기적으로 다시 불러온다.
    // 다시 불러오는 동안에도 이전 값이 남아 있어 플레이어가 다시 만들어지지 않는다.
    use_future(move || async move {
        loop {
            TimeoutFuture::new(LIVE_REFRESH_MILLIS).await;
            let changing = matches!(&*broadcast.peek(), Some(Ok(Some(b))) if may_change(b));
            if changing {
                broadcast.restart();
            }
        }
    });

    match &*broadcast.read() {
        None => rsx! { p { class: "notice", "불러오는 중…" } },
        Some(Err(message)) => rsx! { p { class: "notice error", "{message}" } },
        Some(Ok(None)) => rsx! {
            NotFound { message: "방송을 찾을 수 없습니다. 공유 URL이 올바른지 확인해 주세요." }
        },
        Some(Ok(Some(broadcast))) => rsx! {
            article { class: "broadcast-detail",
                Link { class: "back", to: Route::BroadcastList {}, "← 목록으로" }
                if broadcast.recording.is_some() {
                    video {
                        class: "player",
                        src: api::recording_url(broadcast.id),
                        controls: true,
                        preload: "metadata",
                    }
                } else if broadcast.live_playlist {
                    LivePlayer { id: broadcast.id }
                    if broadcast.status == BroadcastStatus::Ended {
                        p { class: "notice", role: "status",
                            "방송이 종료되었습니다. 녹화본을 준비하고 있습니다. 준비되면 이 화면에서 바로 다시 볼 수 있습니다."
                        }
                    }
                } else if broadcast.status == BroadcastStatus::Live {
                    p { class: "notice player-placeholder", "방송을 준비하고 있습니다. 잠시 후 영상이 나옵니다…" }
                } else {
                    p { class: "notice", "이 방송에는 녹화본이 없습니다." }
                }
                div { class: "title-row",
                    StatusBadge { status: broadcast.status }
                    h1 { "{broadcast.title}" }
                }
                p { class: "meta",
                    match broadcast.ended_at {
                        None => rsx! { "송출 시작 " {format_time(broadcast.created_at)} },
                        // 방송 등록으로 생긴 방송은 등록 시각에 이미 종료된 방송이다.
                        Some(ended_at) if ended_at == broadcast.created_at => rsx! {
                            "등록 " {format_time(broadcast.created_at)}
                        },
                        Some(ended_at) => rsx! {
                            "방송 " {format_time(broadcast.created_at)} " ~ " {format_time(ended_at)}
                        },
                    }
                }
                ShareUrl { id: broadcast.id.to_string() }
            }
        },
    }
}

/// 다시 불러오면 바뀔 수 있는 방송: 라이브 중이거나, 끝났지만 녹화본을 아직 만드는 중이다.
fn may_change(broadcast: &Broadcast) -> bool {
    broadcast.status == BroadcastStatus::Live
        || (broadcast.recording.is_none() && broadcast.live_playlist)
}

const LIVE_PLAYER_ID: &str = "live-player";

/// 라이브 방송 플레이어(HLS). Safari처럼 HLS를 바로 재생하는 브라우저는 그대로 쓰고,
/// 나머지는 hls.js로 재생한다. 자동 재생이 막히지 않도록 소리를 끈 채 시작한다.
#[component]
fn LivePlayer(id: Uuid) -> Element {
    let src = api::live_playlist_url(id);
    rsx! {
        video {
            id: LIVE_PLAYER_ID,
            class: "player",
            controls: true,
            autoplay: true,
            muted: true,
            "playsinline": true,
            onmounted: move |_| attach_hls(LIVE_PLAYER_ID, &src),
        }
        p { class: "hint", "소리가 들리지 않으면 플레이어의 음소거를 해제해 주세요." }
    }
}

/// hls.js 버전은 고정한다.
const HLS_JS_URL: &str = "https://cdn.jsdelivr.net/npm/hls.js@1.5.20/dist/hls.min.js";

fn attach_hls(video_id: &str, src: &str) {
    let script = r#"
        const [videoId, src, hlsUrl] = await dioxus.recv();
        const video = document.getElementById(videoId);
        if (!video) return;
        video.muted = true;
        if (video._hls) { video._hls.destroy(); video._hls = null; }
        if (video.canPlayType("application/vnd.apple.mpegurl")) {
            video.src = src;
        } else {
            if (!window.Hls) {
                await new Promise((resolve, reject) => {
                    const tag = document.createElement("script");
                    tag.src = hlsUrl;
                    tag.onload = resolve;
                    tag.onerror = reject;
                    document.head.appendChild(tag);
                });
            }
            if (!window.Hls.isSupported()) return;
            const hls = new window.Hls({ liveSyncDurationCount: 2 });
            hls.loadSource(src);
            hls.attachMedia(video);
            video._hls = hls;
        }
        video.play().catch(() => {});
    "#;
    let eval = document::eval(script);
    let _ = eval.send((video_id, src, HLS_JS_URL));
}

/// 공유 URL과 복사 버튼. 이 주소를 받은 누구나 로그인 없이 방송을 볼 수 있다.
#[component]
fn ShareUrl(id: String) -> Element {
    let share_url = share_url(&id);
    let mut copied = use_signal(|| None::<bool>);
    let copy = {
        let share_url = share_url.clone();
        move |_| {
            let share_url = share_url.clone();
            async move { copied.set(Some(copy_to_clipboard(&share_url).await)) }
        }
    };

    rsx! {
        section { class: "share",
            label { r#for: "share-url", "공유 URL" }
            div { class: "share-row",
                input {
                    id: "share-url",
                    r#type: "text",
                    readonly: true,
                    value: "{share_url}",
                    onfocus: move |_| select_share_url(),
                }
                button { r#type: "button", onclick: copy, "복사" }
            }
            match copied() {
                Some(true) => rsx! { p { class: "hint", role: "status", "공유 URL을 복사했습니다." } },
                Some(false) => rsx! {
                    p { class: "notice error", role: "status", "자동 복사가 막혀 있습니다. 주소를 직접 복사해 주세요." }
                },
                None => rsx! {},
            }
        }
    }
}

/// 지금 열린 웹 주소를 기준으로 만든 방송 상세 화면의 절대 주소.
fn share_url(id: &str) -> String {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    format!("{origin}{}", Route::BroadcastDetail { id: id.to_owned() })
}

/// 클립보드에 복사한다. HTTP로 배포된 주소처럼 클립보드 API를 쓸 수 없는
/// 보안되지 않은 출처에서는 선택 후 복사 명령으로 대신한다.
async fn copy_to_clipboard(text: &str) -> bool {
    let script = r#"
        const text = await dioxus.recv();
        if (window.isSecureContext && navigator.clipboard) {
            try { await navigator.clipboard.writeText(text); return true; } catch (_) {}
        }
        const area = document.createElement("textarea");
        area.value = text;
        area.style.position = "fixed";
        area.style.opacity = "0";
        document.body.appendChild(area);
        area.select();
        let ok = false;
        try { ok = document.execCommand("copy"); } catch (_) {}
        area.remove();
        return ok;
    "#;
    let eval = document::eval(script);
    if eval.send(text).is_err() {
        return false;
    }
    eval.join::<bool>().await.unwrap_or(false)
}

fn select_share_url() {
    if let Some(input) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("share-url"))
        .and_then(|element| element.dyn_into::<HtmlInputElement>().ok())
    {
        input.select();
    }
}

#[component]
fn PageNotFound(segments: Vec<String>) -> Element {
    rsx! { NotFound { message: "요청한 페이지가 없습니다." } }
}

#[component]
fn NotFound(message: String) -> Element {
    rsx! {
        section { class: "not-found",
            h1 { "찾을 수 없음" }
            p { class: "notice", "{message}" }
            Link { class: "button", to: Route::BroadcastList {}, "방송 목록으로" }
        }
    }
}

const PREVIEW_ID: &str = "live-preview";

/// 송출 화면의 진행 단계.
#[derive(Debug, Clone, PartialEq)]
enum LivePhase {
    /// 송출 전. 카메라가 켜져 있을 수도 있다.
    Ready,
    /// 서버에 연결해 방송을 만드는 중.
    Connecting,
    /// 송출 중. `started_at`은 브라우저 시계 기준 밀리초.
    Live {
        broadcast: Broadcast,
        started_at: f64,
    },
    /// 종료를 요청하고 서버의 답을 기다리는 중.
    Stopping { broadcast: Broadcast },
    /// 방송이 끝났다. 서버나 연결 문제로 끝났다면 이유가 있다.
    Ended {
        broadcast: Broadcast,
        reason: Option<String>,
    },
}

impl LivePhase {
    fn broadcast(&self) -> Option<&Broadcast> {
        match self {
            Self::Live { broadcast, .. }
            | Self::Stopping { broadcast }
            | Self::Ended { broadcast, .. } => Some(broadcast),
            Self::Ready | Self::Connecting => None,
        }
    }
}

/// 휴대폰 카메라·마이크로 라이브 방송을 송출하는 화면.
#[component]
fn LiveBroadcast() -> Element {
    let mut title = use_signal(String::new);
    let mut camera = use_signal(|| None::<MediaStream>);
    let mut facing = use_signal(|| Facing::User);
    let mut opening_camera = use_signal(|| false);
    let mut phase = use_signal(|| LivePhase::Ready);
    let mut session = use_signal(|| None::<LiveSession>);
    let mut error = use_signal(|| None::<String>);
    let mut now = use_signal(js_sys::Date::now);

    // 카메라가 바뀌면 미리보기에 붙인다.
    use_effect(move || live::show_preview(PREVIEW_ID, camera.read().as_ref()));

    // 송출 경과 시간 표시용 시계.
    use_future(move || async move {
        loop {
            TimeoutFuture::new(1000).await;
            now.set(js_sys::Date::now());
        }
    });

    // 화면을 떠나면 송출을 끊고(방송 종료) 카메라를 끈다.
    use_drop(move || {
        session.write().take();
        if let Some(stream) = camera.write().take() {
            live::stop_camera(&stream);
        }
    });

    let open_camera = move |_| async move {
        opening_camera.set(true);
        error.set(None);
        if let Some(old) = camera.write().take() {
            live::stop_camera(&old);
        }
        match live::open_camera(facing()).await {
            Ok(stream) => camera.set(Some(stream)),
            Err(message) => error.set(Some(message)),
        }
        opening_camera.set(false);
    };

    let start = move |_| {
        let Some(stream) = camera() else { return };
        if title().trim().is_empty() {
            error.set(Some("제목을 입력해 주세요.".into()));
            return;
        }
        let (sender, mut events) = futures_channel::mpsc::unbounded();
        match LiveSession::start(&stream, title().trim(), sender) {
            Ok(started) => session.set(Some(started)),
            Err(message) => {
                error.set(Some(message));
                return;
            }
        }
        error.set(None);
        phase.set(LivePhase::Connecting);
        spawn(async move {
            while let Some(event) = events.next().await {
                let current = phase();
                match event {
                    LiveEvent::Started(broadcast) => phase.set(LivePhase::Live {
                        broadcast,
                        started_at: js_sys::Date::now(),
                    }),
                    LiveEvent::Ended(reason) => {
                        if let Some(broadcast) = current.broadcast() {
                            phase.set(LivePhase::Ended {
                                broadcast: broadcast.clone(),
                                reason,
                            });
                        }
                        session.set(None);
                        break;
                    }
                    LiveEvent::Rejected(message) => {
                        error.set(Some(message));
                        phase.set(LivePhase::Ready);
                        session.set(None);
                        break;
                    }
                    LiveEvent::Disconnected => {
                        match current.broadcast() {
                            Some(broadcast) => phase.set(LivePhase::Ended {
                                broadcast: broadcast.clone(),
                                reason: Some("서버와의 연결이 끊겨 방송이 종료되었습니다.".into()),
                            }),
                            None => {
                                error.set(Some(
                                    "서버에 연결할 수 없습니다. 주소와 네트워크를 확인해 주세요."
                                        .into(),
                                ));
                                phase.set(LivePhase::Ready);
                            }
                        }
                        session.set(None);
                        break;
                    }
                }
            }
        });
    };

    let stop = move |_| {
        if let LivePhase::Live { broadcast, .. } = phase() {
            if let Some(session) = &*session.read() {
                session.stop();
            }
            phase.set(LivePhase::Stopping { broadcast });
        }
    };

    let switch_camera = move |event| {
        facing.set(facing().other());
        open_camera(event)
    };

    let current = phase();
    let broadcasting = matches!(
        current,
        LivePhase::Connecting | LivePhase::Live { .. } | LivePhase::Stopping { .. }
    );

    rsx! {
        h1 { "라이브 송출" }
        section { class: "live-studio",
            div { class: "preview-frame",
                video {
                    id: PREVIEW_ID,
                    class: "preview",
                    autoplay: true,
                    muted: true,
                    "playsinline": true,
                }
                if camera.read().is_none() {
                    p { class: "preview-empty", "카메라가 꺼져 있습니다" }
                }
                match &current {
                    LivePhase::Live { started_at, .. } => rsx! {
                        span { class: "badge live on-air", "LIVE {format_elapsed(now() - started_at)}" }
                    },
                    LivePhase::Connecting => rsx! { span { class: "badge ended on-air", "연결 중…" } },
                    LivePhase::Stopping { .. } => rsx! { span { class: "badge ended on-air", "종료 중…" } },
                    _ => rsx! {},
                }
            }

            div { class: "live-controls",
                label { r#for: "live-title", "제목" }
                input {
                    id: "live-title",
                    r#type: "text",
                    maxlength: TITLE_MAX_CHARS as i64,
                    placeholder: "방송 제목",
                    value: "{title}",
                    disabled: broadcasting,
                    oninput: move |event| title.set(event.value()),
                }
                div { class: "button-row",
                    if camera.read().is_none() {
                        button {
                            r#type: "button",
                            disabled: opening_camera(),
                            onclick: open_camera,
                            if opening_camera() { "카메라 켜는 중…" } else { "카메라·마이크 켜기" }
                        }
                    } else if !broadcasting {
                        button { r#type: "button", class: "live", onclick: start, "방송 시작" }
                        button {
                            r#type: "button",
                            class: "secondary",
                            disabled: opening_camera(),
                            onclick: switch_camera,
                            "카메라 전환"
                        }
                    } else {
                        button {
                            r#type: "button",
                            class: "danger",
                            disabled: !matches!(current, LivePhase::Live { .. }),
                            onclick: stop,
                            "방송 종료"
                        }
                    }
                }
                if let Some(message) = error() {
                    p { class: "notice error", role: "alert", "{message}" }
                }
                match &current {
                    LivePhase::Ready if camera.read().is_none() => rsx! {
                        p { class: "hint",
                            "카메라·마이크 켜기를 누르면 브라우저가 권한을 묻습니다. 허용하면 미리보기가 나오고 방송을 시작할 수 있습니다."
                        }
                    },
                    LivePhase::Live { broadcast, .. } => rsx! {
                        p { class: "hint", role: "status",
                            "송출 중입니다. 시청 화면: "
                            Link { to: Route::BroadcastDetail { id: broadcast.id.to_string() }, new_tab: true, "새 탭에서 열기" }
                        }
                    },
                    LivePhase::Ended { broadcast, reason } => rsx! {
                        div { class: "ended-box", role: "status",
                            p { "방송이 종료되었습니다." }
                            if let Some(reason) = reason {
                                p { class: "notice error", "{reason}" }
                            }
                            div { class: "button-row",
                                Link {
                                    class: "button",
                                    to: Route::BroadcastDetail { id: broadcast.id.to_string() },
                                    "방송 보기"
                                }
                                button {
                                    r#type: "button",
                                    class: "secondary",
                                    onclick: move |_| phase.set(LivePhase::Ready),
                                    "새 방송 준비"
                                }
                            }
                        }
                    },
                    _ => rsx! {},
                }
            }
        }
    }
}

/// 송출 경과 시간. 예: `03:25`, `1:02:03`.
fn format_elapsed(millis: f64) -> String {
    let total = (millis.max(0.0) / 1000.0) as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
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
        let current_policy = policy
            .read()
            .as_ref()
            .and_then(|p| p.as_ref().ok())
            .cloned();
        if let Err(message) = check_registration(&title(), file.as_ref(), current_policy.as_ref()) {
            error.set(Some(message));
            return;
        }
        let Some(file) = file else { return };
        submitting.set(true);
        error.set(None);
        match api::register_broadcast(&title(), &file).await {
            Ok(broadcast) => {
                navigator.push(Route::BroadcastDetail {
                    id: broadcast.id.to_string(),
                });
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
            return Err(format!(
                "녹화본은 {} 이하만 등록할 수 있습니다.",
                format_bytes(policy.max_upload_bytes)
            ));
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
    time.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}
