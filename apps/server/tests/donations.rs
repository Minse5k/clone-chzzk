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
    AuthResponse, CommentEvent, DONATION_CONFIRM_PATH, DONATION_FAIL_PATH, Donation, DonationOrder,
    DonationSummary, PAYMENT_WEBHOOK_PATH, comments_path, comments_ws_path, donations_path,
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

    /// 토스페이먼츠 쪽에서 결제 상태가 바뀐 것으로 만든다. 승인 확인 요청이 서버에 닿지 못했거나,
    /// 결제가 실패·만료된 경우를 웹훅 테스트에서 재현한다.
    fn set_status(&self, payment_key: &str, status: &str) {
        self.0
            .lock()
            .unwrap()
            .payments
            .get_mut(payment_key)
            .unwrap()
            .status = status.into();
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

fn toss_config(api_base: &str) -> server::TossConfig {
    server::TossConfig {
        client_key: CLIENT_KEY.into(),
        secret_key: SECRET_KEY.into(),
        api_base: api_base.into(),
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
        let app = TestApp::with_config(|config| config.toss = Some(toss_config(&api_base))).await;
        Self::with_app(app, toss).await
    }

    /// 같은 DB·Redis를 쓰는 서버 인스턴스 둘. 후원은 `app`(A)으로 하고, 다른 인스턴스(B)를 함께 돌려준다.
    async fn cluster() -> (Self, TestApp) {
        let (toss, api_base) = FakeToss::start().await;
        let (a, b) = TestApp::cluster(|config| config.toss = Some(toss_config(&api_base))).await;
        (Self::with_app(a, toss).await, b)
    }

    async fn with_app(app: TestApp, toss: FakeToss) -> Self {
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

    /// 결제창 인증과 승인 확인까지 마쳐 후원을 확정한다.
    async fn donate(&self, amount: u64, message: &str) -> Donation {
        let order = self.order(amount, message).await;
        let payment_key = self
            .toss
            .authorize(&order.order_id, amount, OnConfirm::Approve);
        let reply = self.confirm(&payment_key, &order.order_id, amount).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.message());
        reply.json()
    }

    async fn summary(&self) -> DonationSummary {
        let reply = self.app.get(&donations_path(self.broadcast)).await;
        assert_eq!(reply.status, StatusCode::OK);
        reply.json()
    }

    /// 토스페이먼츠가 보내는 것과 같은 형식의 결제 상태 웹훅을 보낸다. 본문의 상태·금액은 서버가 믿지 않는다.
    async fn webhook(&self, payment_key: &str, claimed_status: &str) -> Reply {
        self.app
            .request(
                "POST",
                PAYMENT_WEBHOOK_PATH,
                None,
                Some(json!({
                    "eventType": "PAYMENT_STATUS_CHANGED",
                    "createdAt": "2026-11-16T10:00:00.000000",
                    "data": {
                        "paymentKey": payment_key,
                        "orderId": "본문의 주문번호는 쓰지 않는다",
                        "status": claimed_status,
                        "totalAmount": 1,
                    },
                })),
            )
            .await
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
        let status = sqlx::query_scalar("SELECT status FROM donations WHERE order_id = $1")
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
            "SELECT COUNT(*), COALESCE(SUM(amount), 0)::BIGINT FROM donations WHERE status = 'confirmed'",
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

fn webhook_result(reply: &Reply) -> String {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    reply.json::<Value>()["result"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn 사용자가_성공_화면으로_돌아오지_않아도_웹훅으로_후원이_확정된다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(7_000, "웹훅으로 확정").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 7_000, OnConfirm::Approve);
    // 결제는 승인되었지만 승인 확인 요청이 서버에 닿지 못했다.
    fixture.toss.set_status(&payment_key, "DONE");

    let reply = fixture.webhook(&payment_key, "DONE").await;

    assert_eq!(webhook_result(&reply), "confirmed");
    assert_eq!(fixture.status(&order.order_id).await, "confirmed");
    assert_eq!(fixture.confirmed_total().await, (1, 7_000));
}

#[tokio::test]
async fn 같은_웹훅이_여러_번_와도_한_번만_적립된다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(7_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 7_000, OnConfirm::Approve);
    fixture.toss.set_status(&payment_key, "DONE");

    let results = [
        webhook_result(&fixture.webhook(&payment_key, "DONE").await),
        webhook_result(&fixture.webhook(&payment_key, "DONE").await),
        webhook_result(&fixture.webhook(&payment_key, "DONE").await),
    ];

    assert_eq!(
        results,
        ["confirmed", "already_processed", "already_processed"]
    );
    assert_eq!(fixture.confirmed_total().await, (1, 7_000));
}

#[tokio::test]
async fn 승인_확인_뒤에_웹훅이_와도_한_번만_적립된다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(7_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 7_000, OnConfirm::Approve);
    assert_eq!(
        fixture
            .confirm(&payment_key, &order.order_id, 7_000)
            .await
            .status,
        StatusCode::OK
    );

    let reply = fixture.webhook(&payment_key, "DONE").await;

    assert_eq!(webhook_result(&reply), "already_processed");
    assert_eq!(fixture.confirmed_total().await, (1, 7_000));
    assert_eq!(fixture.toss.confirm_calls(), 1);
}

#[tokio::test]
async fn 웹훅_뒤에_승인_확인이_와도_같은_후원을_돌려주고_한_번만_적립된다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(7_000, "").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 7_000, OnConfirm::Approve);
    fixture.toss.set_status(&payment_key, "DONE");
    fixture.webhook(&payment_key, "DONE").await;

    // 늦게 도착한 성공 화면의 승인 확인. 결제 서비스는 이미 처리된 결제라고 답한다.
    let reply = fixture.confirm(&payment_key, &order.order_id, 7_000).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json::<Donation>().amount, 7_000);
    assert_eq!(fixture.confirmed_total().await, (1, 7_000));
}

#[tokio::test]
async fn 웹훅_본문이_완료라고_해도_조회한_결제가_끝나지_않았으면_확정하지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(7_000, "").await;
    // 결제창 인증만 끝나고 승인되지 않은 결제. 누군가 본문을 꾸며 DONE이라고 보냈다.
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 7_000, OnConfirm::Approve);

    let reply = fixture.webhook(&payment_key, "DONE").await;

    assert_eq!(webhook_result(&reply), "ignored");
    assert_eq!(fixture.status(&order.order_id).await, "pending");
    assert_eq!(fixture.confirmed_total().await, (0, 0));
}

#[tokio::test]
async fn 조회한_결제_금액이_주문과_다르면_웹훅으로도_확정하지_않는다() {
    let fixture = DonationFixture::new().await;
    let order = fixture.order(7_000, "").await;
    // 같은 주문번호로 다른 금액이 결제된 경우.
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 100, OnConfirm::Approve);
    fixture.toss.set_status(&payment_key, "DONE");

    let reply = fixture.webhook(&payment_key, "DONE").await;

    assert_eq!(webhook_result(&reply), "closed");
    assert_eq!(fixture.status(&order.order_id).await, "failed");
    assert_eq!(fixture.confirmed_total().await, (0, 0));
}

#[tokio::test]
async fn 결제가_실패하거나_만료됐다는_웹훅은_주문을_끝낸다() {
    let fixture = DonationFixture::new().await;
    let aborted = fixture.order(7_000, "").await;
    let canceled = fixture.order(7_000, "").await;
    let aborted_key = fixture
        .toss
        .authorize(&aborted.order_id, 7_000, OnConfirm::Approve);
    let canceled_key = fixture
        .toss
        .authorize(&canceled.order_id, 7_000, OnConfirm::Approve);
    fixture.toss.set_status(&aborted_key, "EXPIRED");
    fixture.toss.set_status(&canceled_key, "CANCELED");

    assert_eq!(
        webhook_result(&fixture.webhook(&aborted_key, "EXPIRED").await),
        "closed"
    );
    assert_eq!(
        webhook_result(&fixture.webhook(&canceled_key, "CANCELED").await),
        "closed"
    );

    assert_eq!(fixture.status(&aborted.order_id).await, "failed");
    assert_eq!(fixture.status(&canceled.order_id).await, "canceled");
}

#[tokio::test]
async fn 모르는_결제와_다른_이벤트는_200으로_무시한다() {
    let fixture = DonationFixture::new().await;

    let unknown_payment = fixture.webhook("tgen_unknown", "DONE").await;
    let other_event = fixture
        .app
        .request(
            "POST",
            PAYMENT_WEBHOOK_PATH,
            None,
            Some(json!({ "eventType": "DEPOSIT_CALLBACK", "data": {} })),
        )
        .await;

    assert_eq!(webhook_result(&unknown_payment), "ignored");
    assert_eq!(webhook_result(&other_event), "ignored");
}

#[tokio::test]
async fn 결제_서비스에_물을_수_없으면_웹훅을_다시_보내도록_5xx로_답한다() {
    let app = TestApp::with_config(|config| {
        config.toss = Some(server::TossConfig {
            client_key: CLIENT_KEY.into(),
            secret_key: SECRET_KEY.into(),
            // 아무도 듣지 않는 주소.
            api_base: "http://127.0.0.1:9".into(),
        });
    })
    .await;

    let reply = app
        .request(
            "POST",
            PAYMENT_WEBHOOK_PATH,
            None,
            Some(json!({ "eventType": "PAYMENT_STATUS_CHANGED", "data": { "paymentKey": "tgen_x" } })),
        )
        .await;

    assert_eq!(reply.status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn 후원_내역은_확정된_후원만_최신순으로_담고_합계와_건수를_돌려준다() {
    let fixture = DonationFixture::new().await;
    assert_eq!(
        fixture.summary().await,
        DonationSummary {
            total_amount: 0,
            count: 0,
            donations: vec![],
        }
    );

    let first = fixture.donate(1_000, "첫 후원").await;
    let second = fixture.donate(5_000, "두 번째").await;
    // 확정되지 않은 주문들: 결제 대기, 취소, 실패.
    fixture.order(9_000, "대기").await;
    let canceled = fixture.order(9_000, "").await;
    fixture
        .fail(&canceled.order_id, "PAY_PROCESS_CANCELED")
        .await;
    let failed = fixture.order(9_000, "").await;
    fixture.fail(&failed.order_id, "REJECT_CARD_COMPANY").await;

    let summary = fixture.summary().await;

    assert_eq!(summary.total_amount, 6_000);
    assert_eq!(summary.count, 2);
    assert_eq!(summary.donations, vec![second, first]);
    // 내역은 로그인 없이 보고, 없는 방송은 404다.
    assert_eq!(
        fixture
            .app
            .get(&donations_path(Uuid::now_v7()))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn 끝난_방송에서도_후원_내역과_합계를_본다() {
    let fixture = DonationFixture::new().await;
    fixture.donate(3_000, "").await;
    fixture.end_broadcast().await;

    let summary = fixture.summary().await;

    assert_eq!((summary.total_amount, summary.count), (3_000, 1));
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_event(socket: &mut Socket) -> CommentEvent {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    let next = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                Some(Ok(_)) => continue,
                other => panic!("방송 알림 대신 {other:?}"),
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), next)
        .await
        .expect("5초 안에 방송 알림이 오지 않았습니다")
}

#[tokio::test]
async fn 후원이_확정되면_시청_화면에_한_번만_알린다() {
    let fixture = DonationFixture::new().await;
    let url = format!(
        "ws://{}{}",
        fixture.app.addr,
        comments_ws_path(fixture.broadcast)
    );
    let mut socket = tokio_tungstenite::connect_async(url).await.unwrap().0;
    let order = fixture.order(5_000, "실시간 후원").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 5_000, OnConfirm::Approve);

    let donation: Donation = fixture
        .confirm(&payment_key, &order.order_id, 5_000)
        .await
        .json();
    // 같은 결제의 승인 확인과 웹훅이 다시 와도 알림은 더 가지 않는다.
    fixture.confirm(&payment_key, &order.order_id, 5_000).await;
    webhook_result(&fixture.webhook(&payment_key, "DONE").await);
    // 다음 알림이 이 댓글이면 그 사이에 중복 후원 알림이 없었던 것이다.
    let comment = fixture
        .app
        .request(
            "POST",
            &comments_path(fixture.broadcast),
            Some(&fixture.viewer.token),
            Some(json!({ "body": "알림 경계" })),
        )
        .await;
    assert_eq!(comment.status, StatusCode::CREATED);

    assert_eq!(
        next_event(&mut socket).await,
        CommentEvent::Donated { donation }
    );
    assert!(matches!(
        next_event(&mut socket).await,
        CommentEvent::Created { .. }
    ));
}

#[tokio::test]
async fn 다른_인스턴스에서_확정된_후원도_시청_화면에_알린다() {
    let (fixture, other) = DonationFixture::cluster().await;
    let url = format!("ws://{}{}", other.addr, comments_ws_path(fixture.broadcast));
    let mut socket = tokio_tungstenite::connect_async(url).await.unwrap().0;
    let order = fixture.order(3_000, "다른 인스턴스로 후원").await;
    let payment_key = fixture
        .toss
        .authorize(&order.order_id, 3_000, OnConfirm::Approve);

    let donation: Donation = fixture
        .confirm(&payment_key, &order.order_id, 3_000)
        .await
        .json();

    assert_eq!(
        next_event(&mut socket).await,
        CommentEvent::Donated { donation }
    );
    // 합계도 어느 인스턴스로 보든 같다.
    let summary: shared::DonationSummary =
        other.get(&donations_path(fixture.broadcast)).await.json();
    assert_eq!((summary.total_amount, summary.count), (3_000, 1));
}
