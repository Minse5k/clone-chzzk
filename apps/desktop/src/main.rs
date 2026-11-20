//! 치지직 클론 데스크톱 시청 클라이언트(Windows·macOS).
//!
//! 배포된 서버(기본 `https://mins.work`)의 방송 목록을 보여 주고, 고른 방송을 앱 안에서 재생한다.
//! 라이브는 HLS(macOS WKWebView는 바로, Windows WebView2는 hls.js로), 종료된 방송은 녹화본을 재생한다.

mod api;

use api::Playback;
use chrono::Local;
use dioxus::prelude::*;
use shared::{Broadcast, BroadcastStatus};
use uuid::Uuid;

const STYLE: Asset = asset!("/assets/main.css");

fn main() {
    // `--check [서버]`: 창을 띄우지 않고 서버의 방송 목록과 재생 주소를 확인한다(CI·설치 확인용).
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|arg| arg == "--check") {
        let server = args
            .get(pos + 1)
            .map(|url| api::normalize_server_url(url))
            .unwrap_or_else(api::initial_server_url);
        std::process::exit(check(&server));
    }

    let window = dioxus::desktop::WindowBuilder::new()
        .with_title("치지직 클론")
        .with_inner_size(dioxus::desktop::LogicalSize::new(1180.0, 780.0));
    let config = dioxus::desktop::Config::new().with_window(window);
    dioxus::LaunchBuilder::desktop()
        .with_cfg(config)
        .launch(App);
}

/// 서버에서 방송 목록을 받아 방송마다 어떻게 재생할지 출력한다. 성공하면 0, 실패하면 1.
fn check(server: &str) -> i32 {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("비동기 런타임을 만들 수 없습니다");
    match runtime.block_on(api::list_broadcasts(server, None)) {
        Ok(list) => {
            println!("서버 {server}: 방송 {}개", list.len());
            for broadcast in &list {
                let how = match api::playback(server, broadcast) {
                    Playback::Live(url) => format!("라이브 HLS {url}"),
                    Playback::Recording(url) => format!("녹화본 {url}"),
                    Playback::Preparing(message) => message.to_owned(),
                    Playback::Unavailable => "볼 영상 없음".to_owned(),
                };
                println!(
                    "- [{}] {} → {how}",
                    broadcast.status.label(),
                    broadcast.title
                );
            }
            0
        }
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

#[component]
fn App() -> Element {
    let mut server = use_signal(api::initial_server_url);
    let mut server_input = use_signal(|| server.peek().clone());
    let mut filter = use_signal(|| None::<BroadcastStatus>);
    let mut selected = use_signal(|| None::<Uuid>);
    let mut reload = use_signal(|| 0u32);

    let broadcasts = use_resource(move || async move {
        let server = server();
        let filter = filter();
        reload();
        api::list_broadcasts(&server, filter).await
    });

    rsx! {
        document::Stylesheet { href: STYLE }
        header { class: "topbar",
            span { class: "brand", "치지직 클론" }
            form {
                class: "server",
                onsubmit: move |event: FormEvent| {
                    event.prevent_default();
                    let next = api::normalize_server_url(&server_input());
                    if !next.is_empty() {
                        server_input.set(next.clone());
                        selected.set(None);
                        server.set(next);
                    }
                },
                label { r#for: "server-url", "서버" }
                input {
                    id: "server-url",
                    value: "{server_input}",
                    oninput: move |event| server_input.set(event.value()),
                }
                button { r#type: "submit", "연결" }
            }
        }
        main { class: "layout",
            section { class: "list",
                nav { class: "tabs",
                    for (label, value) in [("전체", None), ("라이브", Some(BroadcastStatus::Live)), ("종료", Some(BroadcastStatus::Ended))] {
                        button {
                            class: if filter() == value { "tab active" } else { "tab" },
                            onclick: move |_| filter.set(value),
                            "{label}"
                        }
                    }
                    button { class: "tab refresh", onclick: move |_| reload += 1, "새로고침" }
                }
                match &*broadcasts.read() {
                    None => rsx! { p { class: "hint", "불러오는 중…" } },
                    Some(Err(message)) => rsx! { p { class: "error", "{message}" } },
                    Some(Ok(list)) if list.is_empty() => rsx! { p { class: "hint", "방송이 없습니다." } },
                    Some(Ok(list)) => rsx! {
                        ul { class: "cards",
                            for broadcast in list.iter().cloned() {
                                BroadcastCard {
                                    key: "{broadcast.id}",
                                    active: selected() == Some(broadcast.id),
                                    onselect: move |id| selected.set(Some(id)),
                                    broadcast,
                                }
                            }
                        }
                    },
                }
            }
            section { class: "viewer",
                match selected() {
                    None => rsx! { p { class: "hint", "왼쪽 목록에서 볼 방송을 고르세요." } },
                    Some(id) => rsx! { Viewer { key: "{id}-{reload}", server: server(), id } },
                }
            }
        }
    }
}

#[component]
fn BroadcastCard(broadcast: Broadcast, active: bool, onselect: EventHandler<Uuid>) -> Element {
    let id = broadcast.id;
    let created = broadcast
        .created_at
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M");
    rsx! {
        li {
            class: if active { "card active" } else { "card" },
            onclick: move |_| onselect.call(id),
            div { class: "card-head",
                StatusBadge { status: broadcast.status }
                span { class: "title", "{broadcast.title}" }
            }
            p { class: "meta", "{broadcast.owner.username} · {created}" }
        }
    }
}

#[component]
fn StatusBadge(status: BroadcastStatus) -> Element {
    let class = match status {
        BroadcastStatus::Live => "badge live",
        BroadcastStatus::Ended => "badge",
    };
    rsx! { span { class, "{status.label()}" } }
}

/// 고른 방송의 재생 화면. 재생 직전에 방송을 다시 받아 지금 상태로 재생한다.
#[component]
fn Viewer(server: String, id: Uuid) -> Element {
    let detail = use_resource({
        let server = server.clone();
        move || {
            let server = server.clone();
            async move { api::get_broadcast(&server, id).await }
        }
    });
    match &*detail.read() {
        None => rsx! { p { class: "hint", "불러오는 중…" } },
        Some(Err(message)) => rsx! { p { class: "error", "{message}" } },
        Some(Ok(broadcast)) => {
            let player = match api::playback(&server, broadcast) {
                Playback::Live(src) => rsx! { LivePlayer { src } },
                Playback::Recording(src) => rsx! {
                    video { class: "player", controls: true, autoplay: true, src }
                },
                Playback::Preparing(message) => rsx! { p { class: "hint", "{message}" } },
                Playback::Unavailable => {
                    rsx! { p { class: "hint", "이 방송에는 볼 수 있는 영상이 없습니다." } }
                }
            };
            rsx! {
                div { class: "viewer-head",
                    StatusBadge { status: broadcast.status }
                    h1 { "{broadcast.title}" }
                }
                p { class: "meta", "방송한 사람: {broadcast.owner.username}" }
                {player}
            }
        }
    }
}

const LIVE_PLAYER_ID: &str = "live-player";

/// hls.js 버전은 웹과 같은 것으로 고정한다.
const HLS_JS_URL: &str = "https://cdn.jsdelivr.net/npm/hls.js@1.5.20/dist/hls.min.js";

/// 라이브 플레이어. HLS를 바로 재생하는 웹뷰(macOS WKWebView)는 그대로 쓰고,
/// 나머지(Windows WebView2)는 hls.js로 재생한다. 자동 재생이 막히지 않도록 소리를 끈 채 시작한다.
#[component]
fn LivePlayer(src: String) -> Element {
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
