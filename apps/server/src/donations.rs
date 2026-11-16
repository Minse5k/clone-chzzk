//! 도네이션(후원). 로그인한 시청자가 라이브 중인 방송의 소유자에게 토스페이먼츠 테스트 결제로 후원한다.
//!
//! 흐름(ADR 0005):
//! 1. 주문 생성: 서버가 받는 사람(방송 소유자)과 금액을 정해 결제 대기 주문을 만든다.
//! 2. 결제창: 웹이 토스페이먼츠 결제창을 열고, 인증이 끝나면 성공 주소로 돌아온다.
//! 3. 승인 확인: 서버가 주문과 금액을 대조한 뒤 결제 승인 API를 직접 불러 결과를 확인하고 나서야 확정한다.
//!
//! 확정은 `db::confirm_donation`의 조건부 갱신으로 한 번만 일어난다.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use shared::{
    BroadcastStatus, CommentEvent, ConfirmDonation, DONATION_CONFIRM_PATH, DONATION_FAIL_PATH,
    DONATION_LIST_LIMIT, DONATION_MAX_AMOUNT, DONATION_MESSAGE_MAX_CHARS, DONATION_MIN_AMOUNT,
    Donation, DonationFailure, DonationOrder, DonationSummary, NewDonation, PAYMENT_WEBHOOK_PATH,
};
use uuid::Uuid;

use crate::{
    AppState,
    api::ApiError,
    auth::AuthUser,
    db::{self, ConfirmOutcome, DonationStatus, StoredDonation},
    payments::{self, Payment, PaymentError, TossPayments},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/broadcasts/{id}/donations",
            get(list_donations).post(create_order),
        )
        .route(DONATION_CONFIRM_PATH, post(confirm))
        .route(DONATION_FAIL_PATH, post(fail))
        .route(PAYMENT_WEBHOOK_PATH, post(webhook))
}

/// 사용자가 결제창에서 취소했다는 오류 코드. 결제창(`failUrl`)과 SDK가 각각 쓴다.
const CANCEL_CODES: [&str; 2] = ["PAY_PROCESS_CANCELED", "USER_CANCEL"];

/// 방송의 후원 내역과 합계. 로그인 없이 볼 수 있고, 확정된 후원만 센다.
async fn list_donations(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DonationSummary>, ApiError> {
    let not_found = || ApiError::not_found("방송을 찾을 수 없습니다.");
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    db::find_broadcast(&state.db, id)
        .await?
        .ok_or_else(not_found)?;
    Ok(Json(
        db::donation_summary(&state.db, id, DONATION_LIST_LIMIT).await?,
    ))
}

/// 후원 주문 생성. 로그인한 사용자가 라이브 중인 남의 방송에만 후원할 수 있다.
/// 받는 사람은 클라이언트가 아니라 서버가 방송 소유자로 정한다.
async fn create_order(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(new): Json<NewDonation>,
) -> Result<(StatusCode, Json<DonationOrder>), ApiError> {
    let payments = payments_or_unavailable(&state)?;
    let not_found = || ApiError::not_found("방송을 찾을 수 없습니다.");
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let broadcast = db::find_broadcast(&state.db, id)
        .await?
        .ok_or_else(not_found)?
        .broadcast;
    if broadcast.status != BroadcastStatus::Live {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "라이브 중인 방송에만 후원할 수 있습니다.",
        ));
    }
    if broadcast.owner.id == auth.user.id {
        return Err(bad_request("자기 방송에는 후원할 수 없습니다."));
    }
    validate_amount(new.amount)?;
    let message = validate_message(&new.message)?;

    let order = StoredDonation {
        order_id: format!("donation-{}", Uuid::now_v7()),
        broadcast_id: broadcast.id,
        donor: auth.user.clone(),
        recipient: broadcast.owner.clone(),
        amount: new.amount,
        message,
        status: DonationStatus::Pending,
        payment_key: None,
        confirmed_at: None,
    };
    db::insert_donation_order(&state.db, &order, Utc::now()).await?;
    tracing::info!(
        order = %order.order_id,
        "후원 주문: {} → {} {}원",
        order.donor.username,
        order.recipient.username,
        order.amount
    );
    Ok((
        StatusCode::CREATED,
        Json(DonationOrder {
            order_name: format!("{} 님에게 후원", order.recipient.username),
            order_id: order.order_id,
            broadcast_id: order.broadcast_id,
            amount: order.amount,
            recipient: order.recipient,
            client_key: payments.client_key.clone(),
            customer_key: format!("donor-{}", auth.user.id),
        }),
    ))
}

/// 결제 승인 확인. 결제창이 성공 주소로 돌려준 값을 받아, 서버가 주문과 대조하고
/// 결제 승인 API로 결과를 확인한 뒤에만 후원을 확정한다.
///
/// 같은 요청이 다시 와도(새로고침, 재시도) 이미 확정된 후원을 그대로 돌려줄 뿐 다시 적립하지 않는다.
async fn confirm(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(request): Json<ConfirmDonation>,
) -> Result<Json<Donation>, ApiError> {
    let payments = payments_or_unavailable(&state)?;
    let order = find_own_order(&state, &request.order_id, &auth).await?;

    match order.status {
        DonationStatus::Pending => {}
        // 같은 결제의 승인 확인이 다시 왔다. 결제 API를 다시 부르지 않고 확정된 후원을 돌려준다.
        DonationStatus::Confirmed
            if order.payment_key.as_deref() == Some(request.payment_key.as_str()) =>
        {
            return confirmed(&order);
        }
        DonationStatus::Confirmed => {
            return Err(conflict("이미 다른 결제로 확정된 후원 주문입니다."));
        }
        DonationStatus::Failed | DonationStatus::Canceled => {
            return Err(conflict(
                "이미 실패하거나 취소된 결제입니다. 다시 후원해 주세요.",
            ));
        }
    }

    // 성공 주소의 금액은 브라우저에서 바뀔 수 있다. 주문 금액과 다르면 승인하지 않는다.
    if request.amount != order.amount {
        let reason = "결제 금액이 주문 금액과 달라 결제를 승인하지 않았습니다.";
        close(&state, &order, DonationStatus::Failed, reason).await?;
        return Err(bad_request(reason));
    }

    // 결제 인증과 승인 사이에 방송이 끝났다. 승인하지 않으면 결제 인증은 만료되어 돈이 나가지 않는다.
    let broadcast_live = db::find_broadcast(&state.db, order.broadcast_id)
        .await?
        .is_some_and(|stored| stored.broadcast.status == BroadcastStatus::Live);
    if !broadcast_live {
        let reason = "방송이 끝나 결제를 승인하지 않았습니다. 돈은 나가지 않습니다.";
        close(&state, &order, DonationStatus::Canceled, reason).await?;
        return Err(conflict(reason));
    }

    let payment = match payments
        .confirm(&request.payment_key, &order.order_id, order.amount)
        .await
    {
        Ok(payment) => payment,
        // 같은 결제를 이미 승인했다(동시에 온 승인 확인 등). 결제를 조회해 같은 경로로 확정한다.
        Err(PaymentError::Rejected { code, .. }) if code == payments::ALREADY_PROCESSED => payments
            .get(&request.payment_key)
            .await
            .map_err(payment_unavailable)?,
        Err(PaymentError::Rejected { code, message }) => {
            tracing::info!(order = %order.order_id, "결제 승인 거절: {code} {message}");
            let reason = format!("결제가 승인되지 않았습니다: {message}");
            close(&state, &order, DonationStatus::Failed, &reason).await?;
            return Err(bad_request(reason));
        }
        Err(err) => return Err(payment_unavailable(err)),
    };

    let (donation, _) = finalize(&state, &order, &payment).await?;
    Ok(Json(donation))
}

/// 결제 실패·취소 통지. 결제창의 실패 주소(또는 결제창을 닫은 웹)가 보낸다.
/// 결제 대기 중인 주문만 실패·취소로 끝내고, 이미 확정된 후원은 바꾸지 않는다.
async fn fail(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(failure): Json<DonationFailure>,
) -> Result<StatusCode, ApiError> {
    let order = find_own_order(&state, &failure.order_id, &auth).await?;
    match order.status {
        DonationStatus::Pending => {
            let (status, reason) = if CANCEL_CODES.contains(&failure.code.as_str()) {
                (
                    DonationStatus::Canceled,
                    "사용자가 결제를 취소했습니다.".to_owned(),
                )
            } else {
                (
                    DonationStatus::Failed,
                    format!("결제 실패({}): {}", failure.code, failure.message),
                )
            };
            close(&state, &order, status, &reason).await?;
            Ok(StatusCode::NO_CONTENT)
        }
        // 이미 실패·취소로 끝난 주문이다(같은 통지가 다시 왔다).
        DonationStatus::Failed | DonationStatus::Canceled => Ok(StatusCode::NO_CONTENT),
        DonationStatus::Confirmed => Err(conflict("이미 확정된 후원은 실패로 바꿀 수 없습니다.")),
    }
}

/// 웹훅을 처리한 결과. 토스페이먼츠는 상태 코드만 보지만, 직접 확인할 때 무엇이 일어났는지 알 수 있게 담는다.
#[derive(serde::Serialize)]
struct WebhookResult {
    /// `confirmed`(새로 확정), `already_processed`(이미 처리됨), `closed`(실패·취소로 끝냄), `ignored`(처리할 것 없음)
    result: &'static str,
}

fn webhook_result(result: &'static str) -> (StatusCode, Json<WebhookResult>) {
    (StatusCode::OK, Json(WebhookResult { result }))
}

/// 토스페이먼츠 결제 상태 웹훅(`PAYMENT_STATUS_CHANGED`).
///
/// 웹훅에는 서명이 없어 본문을 믿지 않는다. 본문에서는 결제 키만 꺼내 결제 조회 API로 지금 상태를 다시 묻고,
/// 그 결과로 승인 확인과 같은 확정 경로를 탄다. 같은 통지가 여러 번 와도(재전송) 조건부 갱신 때문에 한 번만 적립된다.
/// 처리할 것이 없으면 200으로 답해 재전송을 멈추고, 결제 서비스에 묻지 못했으면 5xx로 답해 다시 보내게 한다.
async fn webhook(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<WebhookResult>), ApiError> {
    if body["eventType"].as_str() != Some("PAYMENT_STATUS_CHANGED") {
        return Ok(webhook_result("ignored"));
    }
    let Some(payment_key) = body["data"]["paymentKey"].as_str() else {
        return Ok(webhook_result("ignored"));
    };
    let payments = payments_or_unavailable(&state)?;
    let payment = match payments.get(payment_key).await {
        Ok(payment) => payment,
        Err(PaymentError::Rejected { code, message }) => {
            tracing::warn!("웹훅의 결제를 조회할 수 없어 무시합니다: {code} {message}");
            return Ok(webhook_result("ignored"));
        }
        Err(err) => return Err(payment_unavailable(err)),
    };
    // 주문번호도 본문이 아니라 조회 결과에서 가져온다.
    let Some(order) = db::find_donation(&state.db, &payment.order_id).await? else {
        return Ok(webhook_result("ignored"));
    };
    if order.status != DonationStatus::Pending {
        if order.status == DonationStatus::Confirmed && payment.status != payments::STATUS_DONE {
            // 확정된 후원의 결제가 취소되었다(개발자센터에서 취소 등). 환불 처리는 범위 밖이라 기록만 남긴다.
            tracing::warn!(order = %order.order_id, "확정된 후원의 결제 상태가 {}로 바뀌었습니다", payment.status);
        }
        return Ok(webhook_result("already_processed"));
    }

    match payment.status.as_str() {
        // 돈이 나간 결제다. 방송이 그사이 끝났어도 확정한다(승인 확인 요청이 확정 직전에 끊긴 경우).
        payments::STATUS_DONE => match finalize(&state, &order, &payment).await {
            Ok((_, true)) => Ok(webhook_result("confirmed")),
            Ok((_, false)) => Ok(webhook_result("already_processed")),
            Err(err) => {
                tracing::warn!(order = %order.order_id, "웹훅으로 후원을 확정하지 않았습니다: {}", err.message());
                Ok(webhook_result("closed"))
            }
        },
        "ABORTED" | "EXPIRED" => {
            let reason = format!("결제가 끝나지 않았습니다({}).", payment.status);
            close(&state, &order, DonationStatus::Failed, &reason).await?;
            Ok(webhook_result("closed"))
        }
        "CANCELED" | "PARTIAL_CANCELED" => {
            close(
                &state,
                &order,
                DonationStatus::Canceled,
                "결제가 취소되었습니다.",
            )
            .await?;
            Ok(webhook_result("closed"))
        }
        // 결제 진행 중(READY, IN_PROGRESS 등). 아직 처리할 것이 없다.
        _ => Ok(webhook_result("ignored")),
    }
}

/// 토스페이먼츠가 알려 준 결제 결과를 주문과 대조하고, 맞으면 후원을 확정한다.
/// 승인 확인과 웹훅이 모두 이 함수를 쓴다. 이번 호출로 새로 확정했으면 `true`와 함께 돌려주고,
/// 그때만 시청 화면에 후원 알림을 보낸다.
pub(crate) async fn finalize(
    state: &AppState,
    order: &StoredDonation,
    payment: &Payment,
) -> Result<(Donation, bool), ApiError> {
    if let Err(problem) = verify(order, payment) {
        // 돈이 나간 결제가 주문과 다르면 사람이 확인해야 하므로 오류로 남긴다.
        tracing::error!(
            order = %order.order_id,
            payment = %payment.payment_key,
            "결제 결과가 주문과 맞지 않아 후원을 확정하지 않습니다: {problem}"
        );
        let reason = format!("결제 결과가 주문과 맞지 않아 후원을 확정하지 않았습니다({problem}).");
        close(state, order, DonationStatus::Failed, &reason).await?;
        return Err(bad_request(reason));
    }

    let outcome =
        db::confirm_donation(&state.db, &order.order_id, &payment.payment_key, Utc::now()).await?;
    match outcome {
        ConfirmOutcome::Confirmed => {
            tracing::info!(
                order = %order.order_id,
                "후원 확정: {} → {} {}원",
                order.donor.username,
                order.recipient.username,
                order.amount
            );
        }
        ConfirmOutcome::NotPending => {
            tracing::info!(order = %order.order_id, "이미 처리된 후원 주문이라 다시 확정하지 않습니다");
        }
        ConfirmOutcome::PaymentKeyTaken => {
            tracing::error!(
                order = %order.order_id,
                payment = %payment.payment_key,
                "이 결제는 이미 다른 후원 주문에 확정되어 있습니다"
            );
            return Err(conflict("이미 다른 후원에 쓰인 결제입니다."));
        }
    }

    // 지금 상태를 다시 읽어 돌려준다. 다른 요청이 먼저 확정했어도 같은 후원이 나온다.
    let current = db::find_donation(&state.db, &order.order_id)
        .await?
        .ok_or_else(|| ApiError::not_found("후원 주문을 찾을 수 없습니다."))?;
    if current.payment_key.as_deref() != Some(payment.payment_key.as_str()) {
        return Err(conflict("이미 처리된 후원 주문입니다."));
    }
    let Json(donation) = confirmed(&current)?;
    let newly_confirmed = outcome == ConfirmOutcome::Confirmed;
    if newly_confirmed {
        state.comments.publish(
            donation.broadcast_id,
            CommentEvent::Donated {
                donation: donation.clone(),
            },
        );
    }
    Ok((donation, newly_confirmed))
}

/// 결제 결과가 주문과 같은지 확인한다: 승인 완료, 같은 주문번호, 같은 금액.
fn verify(order: &StoredDonation, payment: &Payment) -> Result<(), String> {
    if payment.order_id != order.order_id {
        return Err(format!("주문번호 {}", payment.order_id));
    }
    if payment.status != payments::STATUS_DONE {
        return Err(format!("결제 상태 {}", payment.status));
    }
    if payment.total_amount != order.amount {
        return Err(format!(
            "결제 금액 {}원, 주문 금액 {}원",
            payment.total_amount, order.amount
        ));
    }
    Ok(())
}

fn confirmed(order: &StoredDonation) -> Result<Json<Donation>, ApiError> {
    order
        .confirmed()
        .map(Json)
        .ok_or_else(|| conflict("확정되지 않은 후원 주문입니다."))
}

/// 결제 대기 중인 주문을 실패·취소로 끝낸다. 다른 요청이 먼저 확정했으면 그대로 둔다.
async fn close(
    state: &AppState,
    order: &StoredDonation,
    status: DonationStatus,
    reason: &str,
) -> Result<(), ApiError> {
    if db::close_donation(&state.db, &order.order_id, status, reason).await? {
        tracing::info!(order = %order.order_id, "후원 주문 {}: {reason}", status.as_str());
    }
    Ok(())
}

/// 로그인한 사용자 자신의 주문을 찾는다. 남의 주문이면 403.
async fn find_own_order(
    state: &AppState,
    order_id: &str,
    auth: &AuthUser,
) -> Result<StoredDonation, ApiError> {
    let order = db::find_donation(&state.db, order_id)
        .await?
        .ok_or_else(|| ApiError::not_found("후원 주문을 찾을 수 없습니다."))?;
    if order.donor.id != auth.user.id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "주문한 사용자만 결제를 확인할 수 있습니다.",
        ));
    }
    Ok(order)
}

fn payments_or_unavailable(state: &AppState) -> Result<&TossPayments, ApiError> {
    state.payments.as_deref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "결제 서비스가 설정되지 않아 지금은 후원할 수 없습니다.",
        )
    })
}

fn payment_unavailable(err: PaymentError) -> ApiError {
    match err {
        PaymentError::Unavailable(err) => tracing::warn!("토스페이먼츠 요청 실패: {err:#}"),
        PaymentError::Rejected { code, message } => {
            tracing::warn!("토스페이먼츠 조회 거절: {code} {message}")
        }
    }
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "결제 서비스와 통신하지 못했습니다. 잠시 후 다시 시도해 주세요.",
    )
}

fn validate_amount(amount: u64) -> Result<(), ApiError> {
    if !(DONATION_MIN_AMOUNT..=DONATION_MAX_AMOUNT).contains(&amount) {
        return Err(bad_request(format!(
            "후원 금액은 {}원 이상 {}원 이하로 입력해 주세요.",
            format_won(DONATION_MIN_AMOUNT),
            format_won(DONATION_MAX_AMOUNT)
        )));
    }
    Ok(())
}

fn validate_message(message: &str) -> Result<String, ApiError> {
    let message = message.trim();
    if message.chars().count() > DONATION_MESSAGE_MAX_CHARS {
        return Err(bad_request(format!(
            "후원 메시지는 {DONATION_MESSAGE_MAX_CHARS}자 이하로 입력해 주세요."
        )));
    }
    Ok(message.to_owned())
}

/// 천 단위 쉼표. 예: `1,000`.
fn format_won(amount: u64) -> String {
    let digits = amount.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

fn conflict(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, message)
}
