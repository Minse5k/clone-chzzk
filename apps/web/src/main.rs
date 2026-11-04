//! clone-chzzk 웹 앱.

mod api;

use chrono::{DateTime, Local, Utc};
use dioxus::prelude::*;

const MAIN_CSS: Asset = asset!("/assets/main.css");

#[derive(Routable, Clone, PartialEq)]
enum Route {
    #[layout(Shell)]
    #[route("/")]
    BroadcastList {},
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
                        li { key: "{broadcast.id}", class: "broadcast-card",
                            span { class: "title", "{broadcast.title}" }
                            span { class: "meta", {format_time(broadcast.created_at)} }
                        }
                    }
                }
            },
        }
    }
}

/// 등록 시각을 보는 사람의 시간대로 표시한다.
fn format_time(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}
