//! 토스페이먼츠 코어 API 클라이언트. 후원 결제의 승인과 조회만 쓴다.
//!
//! 결제 결과는 화면이나 웹훅 본문이 아니라 이 클라이언트로 토스페이먼츠에 직접 물어 확인한다(ADR 0005).

use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde_json::json;

use crate::config::TossConfig;

/// 결제 API 응답을 기다리는 최대 시간. 토스페이먼츠는 승인에 최대 60초까지 걸릴 수 있다고 안내한다.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// 결제가 끝나 돈이 나간 상태.
pub const STATUS_DONE: &str = "DONE";
/// 결제 승인이 이미 처리되었다는 오류 코드. 같은 결제를 다시 승인하려 할 때 온다.
pub const ALREADY_PROCESSED: &str = "ALREADY_PROCESSED_PAYMENT";

pub struct TossPayments {
    http: Client,
    api_base: String,
    /// `Basic base64(시크릿키:)`
    authorization: String,
    pub client_key: String,
}

/// 토스페이먼츠가 알려 준 결제 상태 중 후원 확정에 필요한 값.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payment {
    pub payment_key: String,
    pub order_id: String,
    /// `READY`, `IN_PROGRESS`, `DONE`, `CANCELED`, `ABORTED`, `EXPIRED` 등.
    pub status: String,
    pub total_amount: u64,
}

#[derive(Debug)]
pub enum PaymentError {
    /// 토스페이먼츠가 요청을 거절했다(카드 거절, 금액 오류, 이미 처리됨 등).
    Rejected { code: String, message: String },
    /// 토스페이먼츠에 연결할 수 없거나 응답을 읽을 수 없다. 다시 시도할 수 있다.
    Unavailable(anyhow::Error),
}

#[derive(Deserialize)]
struct ErrorBody {
    code: String,
    message: String,
}

impl TossPayments {
    pub fn new(config: &TossConfig) -> anyhow::Result<Self> {
        let http = Client::builder().timeout(REQUEST_TIMEOUT).build()?;
        Ok(Self {
            http,
            api_base: config.api_base.trim_end_matches('/').to_owned(),
            // 시크릿 키 뒤에 콜론을 붙여(비밀번호 없음) base64로 인코딩한다.
            authorization: format!(
                "Basic {}",
                STANDARD.encode(format!("{}:", config.secret_key))
            ),
            client_key: config.client_key.clone(),
        })
    }

    /// 결제 승인. 결제창 인증이 끝난 결제를 실제로 승인하고 결과를 돌려준다.
    pub async fn confirm(
        &self,
        payment_key: &str,
        order_id: &str,
        amount: u64,
    ) -> Result<Payment, PaymentError> {
        let request = self
            .http
            .post(format!("{}/v1/payments/confirm", self.api_base))
            .json(&json!({ "paymentKey": payment_key, "orderId": order_id, "amount": amount }));
        self.send(request).await
    }

    /// `paymentKey`로 결제 조회.
    pub async fn get(&self, payment_key: &str) -> Result<Payment, PaymentError> {
        let request = self.http.get(format!(
            "{}/v1/payments/{}",
            self.api_base,
            urlencoding(payment_key)
        ));
        self.send(request).await
    }

    async fn send(&self, request: RequestBuilder) -> Result<Payment, PaymentError> {
        let response = request
            .header(reqwest::header::AUTHORIZATION, &self.authorization)
            .send()
            .await
            .map_err(|err| PaymentError::Unavailable(err.into()))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .map_err(|err| PaymentError::Unavailable(err.into()));
        }
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            return Err(PaymentError::Unavailable(anyhow::anyhow!(
                "토스페이먼츠 응답 {status}"
            )));
        }
        match response.json::<ErrorBody>().await {
            Ok(body) => Err(PaymentError::Rejected {
                code: body.code,
                message: body.message,
            }),
            Err(err) => Err(PaymentError::Unavailable(
                anyhow::Error::from(err).context(format!("토스페이먼츠 오류 응답 {status}")),
            )),
        }
    }
}

/// 경로에 넣을 값을 퍼센트 인코딩한다. 결제 키는 영문·숫자지만 다른 문자가 와도 경로를 벗어나지 않게 한다.
fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
