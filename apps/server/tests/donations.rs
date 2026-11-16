//! 도네이션 통합 테스트. 토스페이먼츠 대신 같은 응답 형식을 흉내 내는 가짜 결제 서버를 실제 포트에 띄운다.

mod common;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use common::{Reply, TestApp};
use serde_json::{Value, json};
use shared::{
    AuthResponse, DONATION_CONFIRM_PATH, DONATION_FAIL_PATH, Donation, DonationOrder,
    donations_path,
};
use uuid::Uuid;

const CLIENT_KEY: &str = "test_ck_fake_client_key";
const SECRET_KEY: &str = "test_sk_fake_secret_key";

/// 가짜 결제 서버가 기억하는 결제 하나.
#[derive(Clone)]
struct FakePayment {
    order_id: String,
    amount: u64,
    status: String,
    /// 승인 요청에 어떻게 답할지.
    on_confirm: OnConfirm,
}

#[derive(Clone)]
enum OnConfirm {
    Approve,
    /// 카드사 거절 등 토스페이먼츠 오류 응답.
    Reject {
        code: &'static str,
        message: &'static str,
    },
    /// 승인은 하지만 응답의 총액이 다르다(주문과 맞지 않는 결제).
    ApproveWithAmount(u64),
}

#[derive(Default)]
struct FakeTossState {
    payments: HashMap<String, FakePayment>,
    confirm_calls: usize,
}

/// 실제 포트에서 토스페이먼츠 코어 API(결제 승인, 결제 조회)를 흉내 내는 서버.
#[derive(Clone, Default)]
struct FakeToss(Arc<Mutex<FakeTossState>>);

impl FakeToss {
    async fn start() -> (Self, String) {
        let fake = Self::default();
        let app = Router::new()
            .route("/v1/payments/confirm", post(fake_confirm))
            .route("/v1/payments/{payment_key}", get(fake_get))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());
        (fake, format!("http://{addr}"))
    }

    /// 결제창에서 결제 인증이 끝난 상태를 만든다. 결제 키를 돌려준다.
    fn authorize(&self, order_id: &str, amount: u64, on_confirm: OnConfirm) -> String {
        let payment_key = format!("tgen_{}", Uuid::now_v7().simple());
        self.0.lock().unwrap().payments.insert(
            payment_key.clone(),
            FakePayment {
                order_id: order_id.to_owned(),
                amount,
                status: "IN_PROGRESS".into(),
                on_confirm,
            },
        );
        payment_key
    }

    fn confirm_calls(&self) -> usize {
        self.0.lock().unwrap().confirm_calls
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    let expected = format!("Basic {}", STANDARD.encode(format!("{SECRET_KEY}:")));
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some(expected.as_str())
}

fn toss_error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "code": code, "message": message }))).into_response()
}

fn payment_json(payment_key: &str, payment: &FakePayment, total_amount: u64) -> Value {
    json!({
        "paymentKey": payment_key,
        "orderId": payment.order_id,
        "status": payment.status,
        "totalAmount": total_amount,
        "method": "카드",
    })
}

async fn fake_confirm(
    State(fake): State<FakeToss>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorized(&headers) {
        return toss_error(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED_KEY",
            "인증되지 않은 키입니다.",
        );
    }
    let mut state = fake.0.lock().unwrap();
    state.confirm_calls += 1;
    let payment_key = body["paymentKey"].as_str().unwrap_or_default().to_owned();
    let Some(payment) = state.payments.get_mut(&payment_key) else {
        return toss_error(
            StatusCode::NOT_FOUND,
            "NOT_FOUND_PAYMENT",
            "존재하지 않는 결제 정보 입니다.",
        );
    };
    if payment.status == "DONE" {
        return toss_error(
            StatusCode::BAD_REQUEST,
            "ALREADY_PROCESSED_PAYMENT",
            "이미 처리된 결제 입니다.",
        );
    }
    if body["orderId"].as_str() != Some(payment.order_id.as_str())
        || body["amount"].as_u64() != Some(payment.amount)
    {
        return toss_error(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "결제 정보가 일치하지 않습니다.",
        );
    }
    match payment.on_confirm.clone() {
        OnConfirm::Approve => {
            payment.status = "DONE".into();
            let amount = payment.amount;
            Json(payment_json(&payment_key, payment, amount)).into_response()
        }
        OnConfirm::ApproveWithAmount(total) => {
            payment.status = "DONE".into();
            Json(payment_json(&payment_key, payment, total)).into_response()
        }
        OnConfirm::Reject { code, message } => {
            payment.status = "ABORTED".into();
            toss_error(StatusCode::BAD_REQUEST, code, message)
        }
    }
}

async fn fake_get(
    State(fake): State<FakeToss>,
    headers: HeaderMap,
    Path(payment_key): Path<String>,
) -> Response {
    if !authorized(&headers) {
        return toss_error(
            StatusCode::UNAUTHORIZED,
            "UNAUTHORIZED_KEY",
            "인증되지 않은 키입니다.",
        );
    }
    let state = fake.0.lock().unwrap();
    match state.payments.get(&payment_key) {
        Some(payment) => Json(payment_json(&payment_key, payment, payment.amount)).into_response(),
        None => toss_error(
            StatusCode::NOT_FOUND,
            "NOT_FOUND_PAYMENT",
            "존재하지 않는 결제 정보 입니다.",
        ),
    }
}

/// 라이브 방송 하나와 방송자, 시청자, 가짜 결제 서버가 있는 서버.
struct DonationFixture {
    app: TestApp,
    toss: FakeToss,
    streamer: AuthResponse,
    viewer: AuthResponse,
    broadcast: Uuid,
}

impl DonationFixture {
    async fn new() -> Self {
        let (toss, api_base) = FakeToss::start().await;
        let app = TestApp::with_config(|config| {
            config.toss = Some(server::TossConfig {
                client_key: CLIENT_KEY.into(),
                secret_key: SECRET_KEY.into(),
                api_base,
            });
        })
        .await;
        let streamer = app.signup("streamer").await;
        let viewer = app.signup("viewer").await;
        let broadcast = app
            .insert_live_broadcast(&streamer, "후원받을 라이브")
            .await;
        Self {
            app,
            toss,
            streamer,
            viewer,
            broadcast,
        }
    }

    async fn order_as(&self, token: Option<&str>, amount: u64, message: &str) -> Reply {
        self.app
            .request(
                "POST",
                &donations_path(self.broadcast),
                token,
                Some(json!({ "amount": amount, "message": message })),
            )
            .await
    }

    /// 시청자로 주문을 만들고 성공을 확인한다.
    async fn order(&self, amount: u64, message: &str) -> DonationOrder {
        let reply = self
            .order_as(Some(&self.viewer.token), amount, message)
            .await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.message());
        reply.json()
    }

    async fn confirm_as(
        &self,
        token: &str,
        payment_key: &str,
        order_id: &str,
        amount: u64,
    ) -> Reply {
        self.app
            .request(
                "POST",
                DONATION_CONFIRM_PATH,
                Some(token),
                Some(json!({ "payment_key": payment_key, "order_id": order_id, "amount": amount })),
            )
            .await
    }

    async fn confirm(&self, payment_key: &str, order_id: &str, amount: u64) -> Reply {
        let token = self.viewer.token.clone();
        self.confirm_as(&token, payment_key, order_id, amount).await
    }

    async fn fail_as(&self, token: Option<&str>, order_id: &str, code: &str) -> Reply {
        self.app
            .request(
                "POST",
                DONATION_FAIL_PATH,
                token,
                Some(json!({ "order_id": order_id, "code": code, "message": "테스트 실패" })),
            )
            .await
    }

    async fn fail(&self, order_id: &str, code: &str) -> Reply {
        let token = self.viewer.token.clone();
        self.fail_as(Some(&token), order_id, code).await
    }

    /// 방송자가 라이브를 끝낸다.
    async fn end_broadcast(&self) {
        let token = self.streamer.token.clone();
        let reply = self
            .app
            .request(
                "POST",
                &format!("/api/broadcasts/{}/end", self.broadcast),
                Some(&token),
                None,
            )
            .await;
        assert_eq!(reply.status, StatusCode::OK);
    }

    /// 주문의 현재 상태(DB 값).
    async fn status(&self, order_id: &str) -> String {
        let pool = self.app.pool().await;
        let status = sqlx::query_scalar("SELECT status FROM donations WHERE order_id = ?")
            .bind(order_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        pool.close().await;
        status
    }

    /// 확정된 후원 수와 합계(DB 값).
    async fn confirmed_total(&self) -> (i64, i64) {
        let pool = self.app.pool().await;
        let total = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(amount), 0) FROM donations WHERE status = 'confirmed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        pool.close().await;
        total
    }
}

#[tokio::test]
async fn 주문을_만들면_받는_사람은_방송_소유자이고_결제창에_쓸_값을_돌려준다() {
    let fixture = DonationFixture::new().await;

    let order = fixture.order(5_000, "  응원합니다  ").await;

    assert_eq!(order.recipient, fixture.streamer.user);
    assert_eq!(order.amount, 5_000);
    assert_eq!(order.broadcast_id, fixture.broadcast);
    assert_eq!(order.client_key, CLIENT_KEY);
    assert_eq!(order.order_name, "streamer 님에게 후원");
    assert!((6..=64).contains(&order.order_id.len()));
    assert!(
        order
            .order_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    );
    assert_eq!(fixture.status(&order.order_id).await, "pending");
    // 주문만으로는 후원이 생기지 않는다.
    assert_eq!(fixture.confirmed_total().await, (0, 0));
}

#[tokio::test]
async fn 주문은_로그인한_사용자가_라이브_중인_남의_방송에_규칙에_맞는_금액으로만_만든다() {
    let fixture = DonationFixture::new().await;
    let streamer = fixture.streamer.token.clone();
    let viewer = fixture.viewer.token.clone();

    let anonymous = fixture.order_as(None, 1_000, "").await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    let own = fixture.order_as(Some(&streamer), 1_000, "").await;
    assert_eq!(own.status, StatusCode::BAD_REQUEST);
    assert_eq!(own.message(), "자기 방송에는 후원할 수 없습니다.");

    for amount in [0, 999, 1_000_001] {
        let reply = fixture.order_as(Some(&viewer), amount, "").await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{amount}");
        assert_eq!(
            reply.message(),
            "후원 금액은 1,000원 이상 1,000,000원 이하로 입력해 주세요."
        );
    }
    let long = fixture
        .order_as(Some(&viewer), 1_000, &"가".repeat(101))
        .await;
    assert_eq!(long.status, StatusCode::BAD_REQUEST);
    assert_eq!(long.message(), "후원 메시지는 100자 이하로 입력해 주세요.");

    let ended = fixture
        .app
        .insert_ended_broadcast(&fixture.streamer, "끝난 방송")
        .await;
    let reply = fixture
        .app
        .request(
            "POST",
            &donations_path(ended),
            Some(&viewer),
            Some(json!({ "amount": 1_000 })),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.message(), "라이브 중인 방송에만 후원할 수 있습니다.");

    let missing = fixture
        .app
        .request(
            "POST",
            &donations_path(Uuid::now_v7()),
            Some(&viewer),
            Some(json!({ "amount": 1_000 })),
        )
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    // 경계값은 받는다.
    fixture.order(1_000, "").await;
    fixture.order(1_000_000, &"가".repeat(100)).await;
}

#[tokio::test]
async fn 결제_서비스를_설정하지_않으면_후원만_503이다() {
    let app = TestApp::start().await;
    let streamer = app.signup("streamer").await;
    let viewer = app.signup("viewer").await;
    let broadcast = app.insert_live_broadcast(&streamer, "라이브").await;

    let reply = app
        .request(
            "POST",
            &donations_path(broadcast),
            Some(&viewer.token),
            Some(json!({ "amount": 1_000 })),
        )
        .await;

    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        reply.message(),
        "결제 서비스가 설정되지 않아 지금은 후원할 수 없습니다."
    );
}

#[tokio::test]
async fn 결제_승인을_확인하면_후원이_확정된다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "응원합니다").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 5_000, OnConfirm::Approve);

    let reply = fixture.confirm(&payment_key, &order.order_id, 5_000).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.message());
    let donation: Donation = reply.json();
    assert_eq!(donation.order_id, order.order_id);
    assert_eq!(donation.donor, fixture.viewer.user);
    assert_eq!(donation.recipient, fixture.streamer.user);
    assert_eq!(donation.amount, 5_000);
    assert_eq!(donation.message, "응원합니다");
    assert_eq!(fixture.status(&order.order_id).await, "confirmed");
    assert_eq!(fixture.confirmed_total().await, (1, 5_000));
    assert_eq!(fixture.toss.confirm_calls(), 1);
}

#[tokio::test]
async fn 같은_승인_확인을_다시_보내도_한_번만_적립되고_결제_승인도_다시_부르지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(10_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 10_000, OnConfirm::Approve);
    let first: Donation = fixture
        .confirm(&payment_key, &order.order_id, 10_000)
        .await
        .json();

    for _ in 0..3 {
        let again = fixture.confirm(&payment_key, &order.order_id, 10_000).await;
        assert_eq!(again.status, StatusCode::OK);
        assert_eq!(again.json::<Donation>(), first);
    }

    assert_eq!(fixture.confirmed_total().await, (1, 10_000));
    assert_eq!(fixture.toss.confirm_calls(), 1);
}

#[tokio::test]
async fn 동시에_온_승인_확인도_한_건만_확정된다() {
    let fixture = Arc::new(DonationFixture::new().await);
    let order = fixture.order(3_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 3_000, OnConfirm::Approve);

    let tasks = (0..5).map(|_| {
        let fixture = Arc::clone(&fixture);
        let payment_key = payment_key.clone();
        let order_id = order.order_id.clone();
        tokio::spawn(async move { fixture.confirm(&payment_key, &order_id, 3_000).await.status })
    });
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }

    assert_eq!(fixture.confirmed_total().await, (1, 3_000));
}

#[tokio::test]
async fn 성공_주소의_금액이_주문과_다르면_결제를_승인하지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    // 브라우저에서 금액을 1,000원으로 바꿔 보낸 경우.
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 1_000, OnConfirm::Approve);

    let reply = fixture.confirm(&payment_key, &order.order_id, 1_000).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        reply.message(),
        "결제 금액이 주문 금액과 달라 결제를 승인하지 않았습니다."
    );
    assert_eq!(fixture.toss.confirm_calls(), 0);
    assert_eq!(fixture.status(&order.order_id).await, "failed");
    assert_eq!(fixture.confirmed_total().await, (0, 0));
}

#[tokio::test]
async fn 결제_승인이_거절되면_실패로_남고_후원은_생기지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    let payment_key = fixture.toss.authorize(
        &order.order_id,
        5_000,
        OnConfirm::Reject {
            code: "REJECT_CARD_COMPANY",
            message: "카드사에서 결제를 거절했습니다.",
        },
    );

    let reply = fixture.confirm(&payment_key, &order.order_id, 5_000).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        reply.message(),
        "결제가 승인되지 않았습니다: 카드사에서 결제를 거절했습니다."
    );
    assert_eq!(fixture.status(&order.order_id).await, "failed");
    assert_eq!(fixture.confirmed_total().await, (0, 0));
    // 실패한 주문은 다시 확인해도 확정되지 않는다.
    let again = fixture.confirm(&payment_key, &order.order_id, 5_000).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn 승인_응답의_금액이_주문과_다르면_확정하지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    let payment_key =
        fixture
            .toss
            .authorize(&order.order_id, 5_000, OnConfirm::ApproveWithAmount(500));

    let reply = fixture.confirm(&payment_key, &order.order_id, 5_000).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.message().contains("결제 결과가 주문과 맞지 않아"));
    assert_eq!(fixture.status(&order.order_id).await, "failed");
    assert_eq!(fixture.confirmed_total().await, (0, 0));
}

#[tokio::test]
async fn 다른_주문의_결제_키로는_확정할_수_없다() {
    let fixture = DonationFixture::new().await;
    let first = fixture.order(5_000, "").await;
    let second = fixture.order(5_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&first.order_id, 5_000, OnConfirm::Approve);

    // 첫 주문의 결제 키를 두 번째 주문에 붙여 보낸다. 결제 서비스가 주문번호 불일치로 거절한다.
    let reply = fixture.confirm(&payment_key, &second.order_id, 5_000).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(fixture.status(&second.order_id).await, "failed");
    assert_eq!(fixture.confirmed_total().await, (0, 0));
}

#[tokio::test]
async fn 남의_주문은_승인_확인할_수_없다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 5_000, OnConfirm::Approve);
    let stranger = fixture.app.signup("stranger").await;

    let other = fixture
        .confirm_as(&stranger.token, &payment_key, &order.order_id, 5_000)
        .await;
    let anonymous = fixture
        .app
        .request(
            "POST",
            DONATION_CONFIRM_PATH,
            None,
            Some(
                json!({ "payment_key": payment_key, "order_id": order.order_id, "amount": 5_000 }),
            ),
        )
        .await;
    let missing = fixture
        .confirm(&payment_key, "donation-missing", 5_000)
        .await;

    assert_eq!(other.status, StatusCode::FORBIDDEN);
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(fixture.toss.confirm_calls(), 0);
    assert_eq!(fixture.status(&order.order_id).await, "pending");
}

#[tokio::test]
async fn 후원을_받은_방송은_삭제할_수_없다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 5_000, OnConfirm::Approve);
    fixture.confirm(&payment_key, &order.order_id, 5_000).await;
    let token = fixture.streamer.token.clone();
    let end = fixture
        .app
        .request(
            "POST",
            &format!("/api/broadcasts/{}/end", fixture.broadcast),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(end.status, StatusCode::OK);

    let reply = fixture
        .app
        .request(
            "DELETE",
            &format!("/api/broadcasts/{}", fixture.broadcast),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(
        reply.message(),
        "후원을 받은 방송은 후원 내역을 지키기 위해 삭제할 수 없습니다."
    );
}

#[tokio::test]
async fn 결제창에서_취소하면_주문이_취소되고_후원은_생기지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;

    let reply = fixture.fail(&order.order_id, "PAY_PROCESS_CANCELED").await;

    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert_eq!(fixture.status(&order.order_id).await, "canceled");
    assert_eq!(fixture.confirmed_total().await, (0, 0));
    // 같은 통지가 다시 와도 그대로다.
    assert_eq!(
        fixture
            .fail(&order.order_id, "PAY_PROCESS_CANCELED")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.status(&order.order_id).await, "canceled");
}

#[tokio::test]
async fn 결제가_실패하면_주문이_실패로_남고_다시_승인_확인해도_확정되지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 5_000, OnConfirm::Approve);

    let reply = fixture.fail(&order.order_id, "REJECT_CARD_COMPANY").await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert_eq!(fixture.status(&order.order_id).await, "failed");

    let confirm = fixture.confirm(&payment_key, &order.order_id, 5_000).await;
    assert_eq!(confirm.status, StatusCode::CONFLICT);
    assert_eq!(fixture.toss.confirm_calls(), 0);
    assert_eq!(fixture.confirmed_total().await, (0, 0));
}

#[tokio::test]
async fn 확정된_후원은_실패_통지로_바뀌지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 5_000, OnConfirm::Approve);
    fixture.confirm(&payment_key, &order.order_id, 5_000).await;

    let reply = fixture.fail(&order.order_id, "PAY_PROCESS_CANCELED").await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(fixture.status(&order.order_id).await, "confirmed");
    assert_eq!(fixture.confirmed_total().await, (1, 5_000));
}

#[tokio::test]
async fn 남의_주문은_실패로_바꿀_수_없다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    let stranger = fixture.app.signup("stranger").await;

    let other = fixture
        .fail_as(
            Some(&stranger.token),
            &order.order_id,
            "PAY_PROCESS_CANCELED",
        )
        .await;
    let anonymous = fixture
        .fail_as(None, &order.order_id, "PAY_PROCESS_CANCELED")
        .await;

    assert_eq!(other.status, StatusCode::FORBIDDEN);
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.status(&order.order_id).await, "pending");
}

#[tokio::test]
async fn 결제_중에_방송이_끝나면_결제를_승인하지_않고_주문을_취소한다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(5_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 5_000, OnConfirm::Approve);
    fixture.end_broadcast().await;

    let reply = fixture.confirm(&payment_key, &order.order_id, 5_000).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(
        reply.message(),
        "방송이 끝나 결제를 승인하지 않았습니다. 돈은 나가지 않습니다."
    );
    assert_eq!(fixture.toss.confirm_calls(), 0);
    assert_eq!(fixture.status(&order.order_id).await, "canceled");
    assert_eq!(fixture.confirmed_total().await, (0, 0));
}
