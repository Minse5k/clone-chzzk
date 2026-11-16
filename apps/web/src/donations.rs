//! 도네이션(후원) 화면: 라이브 방송 상세의 후원 폼, 토스페이먼츠 결제위젯 연동, 결제 결과 화면.
//!
//! 결제창이 성공을 보여 줘도 후원은 아직 확정되지 않는다. 성공 화면이 서버의 승인 확인 API를 부르고,
//! 서버가 토스페이먼츠 결제 승인 API로 확인한 결과만 후원으로 확정된다.

use dioxus::prelude::*;
use shared::{
    Broadcast, BroadcastStatus, ConfirmDonation, DONATION_MAX_AMOUNT, DONATION_MESSAGE_MAX_CHARS,
    DONATION_MIN_AMOUNT, DONATION_PRESET_AMOUNTS, Donation, DonationFailure, DonationOrder,
};
use uuid::Uuid;

use crate::{LoginRequired, Route, api, session};

/// 토스페이먼츠 JavaScript SDK(v2). 결제위젯(주문서형)으로 결제 수단을 고르고 결제를 요청한다. 후원할 때만 불러온다.
const TOSS_SDK_URL: &str = "https://js.tosspayments.com/v2/standard";

/// 결제창을 여는 동안 기억해 두는 주문. 결제창에서 취소하면 토스페이먼츠가 주문번호를 돌려주지 않아
/// 실패 화면이 이 값으로 주문을 찾는다.
pub const PENDING_ORDER_KEY: &str = "clone-chzzk.pending-donation";

/// 원 단위 금액 표기. 예: `5,000원`.
pub fn format_won(amount: u64) -> String {
    let digits = amount.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out.push('원');
    out
}

/// 라이브 방송 상세의 후원 영역. 라이브 중이고 로그인했고 남의 방송일 때만 후원 폼을 보여 준다.
/// 버튼을 숨기는 것과 상관없이 서버가 같은 조건을 다시 검사한다.
#[component]
pub fn DonationPanel(broadcast: Broadcast) -> Element {
    if broadcast.status != BroadcastStatus::Live {
        return rsx! {};
    }
    let content = match session::current_user() {
        None => rsx! { LoginRequired { action: "후원하려면" } },
        Some(user) if user.id == broadcast.owner.id => rsx! {
            p { class: "notice", "내 방송에는 후원할 수 없습니다. 시청자의 후원은 아래 후원 내역에 나타납니다." }
        },
        Some(_) => {
            rsx! { DonationForm { broadcast_id: broadcast.id, recipient: broadcast.owner.username.clone() } }
        }
    };
    rsx! {
        section { class: "donation-panel",
            h2 { "후원하기" }
            {content}
        }
    }
}

#[component]
fn DonationForm(broadcast_id: Uuid, recipient: String) -> Element {
    let mut amount = use_signal(|| DONATION_PRESET_AMOUNTS[0].to_string());
    let mut message = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    // 만들어진 주문. 있으면 결제 수단 선택 UI(토스페이먼츠 결제위젯)를 보여 준다.
    let mut order = use_signal(|| None::<DonationOrder>);

    // 주문이 생기면 결제위젯을 그린다.
    use_effect(move || {
        if let Some(order) = order.read().as_ref() {
            render_payment_widget(order);
        }
    });

    let create_order = move |event: FormEvent| async move {
        event.prevent_default();
        if busy() {
            return;
        }
        let Ok(value) = amount().trim().replace(',', "").parse::<u64>() else {
            error.set(Some("후원 금액을 숫자로 입력해 주세요.".into()));
            return;
        };
        if !(DONATION_MIN_AMOUNT..=DONATION_MAX_AMOUNT).contains(&value) {
            error.set(Some(format!(
                "후원 금액은 {} 이상 {} 이하로 입력해 주세요.",
                format_won(DONATION_MIN_AMOUNT),
                format_won(DONATION_MAX_AMOUNT)
            )));
            return;
        }
        busy.set(true);
        error.set(None);
        match api::create_donation_order(broadcast_id, value, message().trim()).await {
            Ok(created) => {
                remember_order(&created);
                order.set(Some(created));
            }
            Err(text) => error.set(Some(text)),
        }
        busy.set(false);
    };

    let pay = move |_| async move {
        let Some(current) = order() else { return };
        busy.set(true);
        error.set(None);
        // 결제를 마치면 성공·실패 주소로 페이지가 바뀐다. 바뀌지 않고 돌아왔다면(결제창 닫기 등) 실패다.
        if let Err(failure) = request_payment(&current).await {
            error.set(Some(failure_text(&failure.code, &failure.message)));
            report_failure(&current.order_id, &failure).await;
            // 끝난 주문으로는 다시 결제할 수 없으므로 금액 고르기부터 다시 한다.
            order.set(None);
        }
        busy.set(false);
    };

    let choose_again = move |_| async move {
        if let Some(current) = order() {
            let canceled = PaymentFailure {
                code: "USER_CANCEL".into(),
                message: String::new(),
            };
            report_failure(&current.order_id, &canceled).await;
        }
        order.set(None);
        error.set(None);
    };

    if let Some(current) = order() {
        return rsx! {
            div { class: "donation-checkout",
                p {
                    strong { "{current.recipient.username}" }
                    " 님에게 "
                    strong { "{format_won(current.amount)}" }
                    " 후원 · 결제 수단을 고르세요(테스트 결제라 실제 돈은 나가지 않습니다)."
                }
                div { id: PAYMENT_METHODS_ID }
                div { id: AGREEMENT_ID }
                if let Some(text) = error() {
                    p { class: "notice error", role: "alert", "{text}" }
                }
                div { class: "button-row",
                    button { class: "live", disabled: busy(), onclick: pay,
                        if busy() { "결제창을 여는 중…" } else { "{format_won(current.amount)} 결제하기" }
                    }
                    button { class: "secondary", disabled: busy(), onclick: choose_again, "금액 다시 고르기" }
                }
            }
        };
    }

    rsx! {
        form { class: "donation-form", onsubmit: create_order,
            p { class: "hint", "받는 사람: " strong { "{recipient}" } " · 토스페이먼츠 테스트 결제(실제 돈은 나가지 않습니다)" }
            div { class: "button-row amount-presets",
                for preset in DONATION_PRESET_AMOUNTS {
                    button {
                        r#type: "button",
                        class: if amount() == preset.to_string() { "" } else { "secondary" },
                        disabled: busy(),
                        onclick: move |_| amount.set(preset.to_string()),
                        "{format_won(preset)}"
                    }
                }
            }
            label { r#for: "donation-amount", "금액(원)" }
            input {
                id: "donation-amount",
                r#type: "number",
                inputmode: "numeric",
                min: DONATION_MIN_AMOUNT as i64,
                max: DONATION_MAX_AMOUNT as i64,
                step: 100,
                value: "{amount}",
                disabled: busy(),
                oninput: move |event| amount.set(event.value()),
            }
            label { r#for: "donation-message", "메시지 (선택)" }
            input {
                id: "donation-message",
                r#type: "text",
                maxlength: DONATION_MESSAGE_MAX_CHARS as i64,
                placeholder: "{DONATION_MESSAGE_MAX_CHARS}자까지",
                value: "{message}",
                disabled: busy(),
                oninput: move |event| message.set(event.value()),
            }
            if let Some(text) = error() {
                p { class: "notice error", role: "alert", "{text}" }
            }
            button { r#type: "submit", class: "live", disabled: busy(),
                if busy() { "주문을 만드는 중…" } else { "다음: 결제 수단 고르기" }
            }
        }
    }
}

/// 결제위젯이 결제 수단 선택 UI와 약관 UI를 그릴 자리.
const PAYMENT_METHODS_ID: &str = "toss-payment-methods";
const AGREEMENT_ID: &str = "toss-agreement";

/// 결제창이 결제를 끝내지 못하고 돌려준 이유.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PaymentFailure {
    pub code: String,
    pub message: String,
}

/// 결제 실패 코드를 사용자에게 보여 줄 문장으로 바꾼다.
pub fn failure_text(code: &str, message: &str) -> String {
    match code {
        "PAY_PROCESS_CANCELED" | "USER_CANCEL" => {
            "결제를 취소했습니다. 후원은 이루어지지 않았습니다.".into()
        }
        _ if message.is_empty() => {
            format!("결제에 실패했습니다({code}). 후원은 이루어지지 않았습니다.")
        }
        _ => format!("결제에 실패했습니다: {message} 후원은 이루어지지 않았습니다."),
    }
}

/// 결제 실패·취소를 서버에 알려 결제 대기 주문을 끝낸다. 알리지 못해도 주문은 승인되지 않으므로 화면 안내는 그대로 둔다.
async fn report_failure(order_id: &str, failure: &PaymentFailure) {
    let failure = DonationFailure {
        order_id: order_id.to_owned(),
        code: failure.code.clone(),
        message: failure.message.clone(),
    };
    if let Err(text) = api::fail_donation(&failure).await {
        dioxus::logger::tracing::warn!("결제 실패를 알리지 못했습니다: {text}");
    }
}

fn remember_order(order: &DonationOrder) {
    if let (Some(storage), Ok(text)) = (session_storage(), serde_json::to_string(order)) {
        let _ = storage.set_item(PENDING_ORDER_KEY, &text);
    }
}

/// 결제창을 열기 전에 기억해 둔 주문.
pub fn remembered_order() -> Option<DonationOrder> {
    let text = session_storage()?.get_item(PENDING_ORDER_KEY).ok()??;
    serde_json::from_str(&text).ok()
}

fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok()?
}

/// 토스페이먼츠 결제위젯으로 결제 수단 선택 UI와 약관 UI를 그린다. 금액은 서버가 만든 주문 금액이다.
fn render_payment_widget(order: &DonationOrder) {
    let script = r#"
        const [sdkUrl, order, methodsSelector, agreementSelector] = await dioxus.recv();
        if (!window.TossPayments) {
            await new Promise((resolve, reject) => {
                const tag = document.createElement("script");
                tag.src = sdkUrl;
                tag.onload = resolve;
                tag.onerror = reject;
                document.head.appendChild(tag);
            });
        }
        const widgets = window.TossPayments(order.client_key).widgets({ customerKey: order.customer_key });
        await widgets.setAmount({ currency: "KRW", value: order.amount });
        await Promise.all([
            widgets.renderPaymentMethods({ selector: methodsSelector, variantKey: "DEFAULT" }),
            widgets.renderAgreement({ selector: agreementSelector, variantKey: "AGREEMENT" }),
        ]);
        window.__chzzkPaymentWidgets = widgets;
    "#;
    let eval = document::eval(script);
    let _ = eval.send((
        TOSS_SDK_URL,
        order,
        format!("#{PAYMENT_METHODS_ID}"),
        format!("#{AGREEMENT_ID}"),
    ));
}

/// 결제를 요청한다. 결제를 마치면 성공·실패 주소로 페이지가 바뀌므로 이 함수는 돌아오지 않는다.
/// 결제창을 열 수 없거나 사용자가 닫으면 이유를 돌려준다.
async fn request_payment(order: &DonationOrder) -> Result<(), PaymentFailure> {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    let script = r#"
        const [order, successUrl, failUrl] = await dioxus.recv();
        try {
            const widgets = window.__chzzkPaymentWidgets;
            if (!widgets) throw { code: "WIDGET_NOT_READY", message: "결제 수단을 불러오는 중입니다. 잠시 후 다시 눌러 주세요." };
            await widgets.requestPayment({
                orderId: order.order_id,
                orderName: order.order_name,
                successUrl,
                failUrl,
            });
            dioxus.send(null);
        } catch (error) {
            dioxus.send({ code: String(error?.code ?? "UNKNOWN"), message: String(error?.message ?? "") });
        }
    "#;
    let mut eval = document::eval(script);
    let _ = eval.send((
        order,
        format!("{origin}/donations/success"),
        format!("{origin}/donations/fail"),
    ));
    match eval.recv::<Option<PaymentFailure>>().await {
        Ok(None) => Ok(()),
        Ok(Some(failure)) => Err(failure),
        Err(_) => Err(PaymentFailure {
            code: "UNKNOWN".into(),
            message: "결제창을 열 수 없습니다.".into(),
        }),
    }
}

/// 결제창의 성공 주소. 쿼리의 결제 키·주문번호·금액을 서버에 넘겨 승인을 확인받고 결과를 보여 준다.
/// 이 화면에 왔다는 것만으로는 후원이 확정되지 않는다.
#[component]
pub fn DonationSuccess(query: String) -> Element {
    let params = parse_query(&query);
    let result = use_resource(move || {
        let params = params.clone();
        async move {
            let request = ConfirmDonation {
                payment_key: params.get("paymentKey").cloned().unwrap_or_default(),
                order_id: params.get("orderId").cloned().unwrap_or_default(),
                amount: params
                    .get("amount")
                    .and_then(|amount| amount.parse().ok())
                    .unwrap_or_default(),
            };
            if request.payment_key.is_empty() || request.order_id.is_empty() {
                return Err("결제 정보가 없습니다. 결제창에서 다시 시도해 주세요.".to_owned());
            }
            api::confirm_donation(&request).await
        }
    });
    let back = remembered_order().map(|order| order.broadcast_id);

    rsx! {
        h1 { "후원 결제" }
        match &*result.read() {
            None => rsx! { p { class: "notice", role: "status", "결제를 확인하고 있습니다…" } },
            Some(Ok(donation)) => rsx! { DonationReceipt { donation: donation.clone() } },
            Some(Err(text)) => rsx! {
                p { class: "notice error", role: "alert", "{text}" }
                BackToBroadcast { broadcast_id: back }
            },
        }
    }
}

/// 결제창의 실패 주소. 결제 실패·취소를 서버에 알리고 이유를 보여 준다.
/// 사용자가 취소하면 토스페이먼츠가 주문번호를 주지 않으므로 결제창을 열기 전에 기억해 둔 주문을 쓴다.
#[component]
pub fn DonationFail(query: String) -> Element {
    let params = parse_query(&query);
    let code = params
        .get("code")
        .cloned()
        .unwrap_or_else(|| "UNKNOWN".into());
    let message = params.get("message").cloned().unwrap_or_default();
    let remembered = remembered_order();
    let order_id = params
        .get("orderId")
        .cloned()
        .or_else(|| remembered.as_ref().map(|order| order.order_id.clone()));
    let back = remembered.map(|order| order.broadcast_id);
    let text = failure_text(&code, &message);

    use_future(move || {
        let (order_id, code, message) = (order_id.clone(), code.clone(), message.clone());
        async move {
            if let Some(order_id) = order_id {
                report_failure(&order_id, &PaymentFailure { code, message }).await;
            }
        }
    });

    rsx! {
        h1 { "후원 결제" }
        p { class: "notice error", role: "alert", "{text}" }
        BackToBroadcast { broadcast_id: back }
    }
}

#[component]
fn DonationReceipt(donation: Donation) -> Element {
    rsx! {
        div { class: "ended-box",
            p { strong { "후원이 완료되었습니다." } }
            p { "{donation.recipient.username} 님에게 {format_won(donation.amount)}을 후원했습니다." }
            if !donation.message.is_empty() {
                p { class: "hint", "메시지: {donation.message}" }
            }
            BackToBroadcast { broadcast_id: Some(donation.broadcast_id) }
        }
    }
}

#[component]
pub fn BackToBroadcast(broadcast_id: Option<Uuid>) -> Element {
    rsx! {
        div { class: "button-row",
            match broadcast_id {
                Some(id) => rsx! {
                    Link { class: "button", to: Route::BroadcastDetail { id: id.to_string() }, "방송으로 돌아가기" }
                },
                None => rsx! {
                    Link { class: "button", to: Route::BroadcastList {}, "방송 목록으로" }
                },
            }
        }
    }
}

/// `a=1&b=2` 형태의 쿼리를 이름별 값으로 나눈다. 값은 퍼센트 인코딩을 푼다.
pub fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    query
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let decode = |text: &str| {
                js_sys::decode_uri_component(&text.replace('+', " "))
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_else(|| text.to_owned())
            };
            (!key.is_empty()).then(|| (decode(key), decode(value)))
        })
        .collect()
}
