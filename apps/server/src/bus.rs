//! 서버 인스턴스 사이의 알림 전달(ADR 0006).
//!
//! 댓글·후원 알림, 방송 삭제, 송출 종료 요청은 어느 인스턴스에서 생기든 모든 인스턴스에 전해져야 한다.
//! 시청자의 댓글 WebSocket이나 송출 연결이 다른 인스턴스에 붙어 있을 수 있기 때문이다.
//! `REDIS_URL`이 있으면 Redis Pub/Sub로 보내고, 모든 인스턴스(보낸 인스턴스 포함)가 구독으로 받아
//! 자기 연결에 나눈다. 없으면 인스턴스 하나로 보고 프로세스 안에서 바로 나눈다.

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use futures_util::StreamExt;
use redis::{AsyncCommands, aio::ConnectionManager};
use serde::{Deserialize, Serialize};
use shared::CommentEvent;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{comments::CommentHub, live::LiveCapacity};

/// 방송별 알림 채널 이름의 앞부분. 뒤에 방송 ID가 붙는다.
const BROADCAST_CHANNEL_PREFIX: &str = "chzzk:broadcast:";
/// 방송 삭제·송출 종료 요청을 보내는 채널.
const CONTROL_CHANNEL: &str = "chzzk:control";
/// 처음 구독을 기다리는 시간. 넘으면 서버를 시작하지 않는다.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(10);
/// 구독 연결이 끊긴 뒤 다시 연결하기 전에 기다리는 시간.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// 방송 알림 외에 모든 인스턴스에 전하는 신호.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Control {
    /// 방송이 지워졌다. 그 방송의 댓글 WebSocket 연결을 닫는다.
    BroadcastDeleted { broadcast_id: Uuid },
    /// 소유자가 방송을 종료했다. 그 방송을 송출받는 인스턴스가 송출을 끝낸다.
    StopIngest { broadcast_id: Uuid },
}

/// 알림을 받아 이 인스턴스의 연결에 나누는 곳.
#[derive(Clone)]
struct LocalTargets {
    comments: Arc<CommentHub>,
    live: Arc<LiveCapacity>,
}

impl LocalTargets {
    fn apply(&self, control: Control) {
        match control {
            Control::BroadcastDeleted { broadcast_id } => self.comments.close(broadcast_id),
            Control::StopIngest { broadcast_id } => {
                if self.live.request_stop(broadcast_id) {
                    tracing::info!(broadcast = %broadcast_id, "이 인스턴스가 받던 송출에 종료를 알렸습니다");
                }
            }
        }
    }

    /// Redis로 받은 메시지를 해석해 나눈다. 모르는 메시지는 버린다.
    fn dispatch(&self, channel: &str, payload: &str) {
        let result = if channel == CONTROL_CHANNEL {
            serde_json::from_str(payload)
                .map(|control| self.apply(control))
                .map_err(anyhow::Error::from)
        } else if let Some(id) = channel.strip_prefix(BROADCAST_CHANNEL_PREFIX) {
            Uuid::parse_str(id)
                .map_err(anyhow::Error::from)
                .and_then(|id| Ok((id, serde_json::from_str(payload)?)))
                .map(|(id, event)| self.comments.deliver(id, event))
        } else {
            Ok(())
        };
        if let Err(err) = result {
            tracing::warn!(channel, "알 수 없는 알림 메시지를 버립니다: {err:#}");
        }
    }
}

pub struct Bus {
    local: LocalTargets,
    redis: Option<ConnectionManager>,
}

impl Bus {
    /// 알림 전달을 준비한다. Redis를 쓰면 구독이 시작된 뒤에 돌려줘, 이후의 알림을 놓치지 않게 한다.
    pub async fn start(
        redis_url: Option<&str>,
        comments: Arc<CommentHub>,
        live: Arc<LiveCapacity>,
    ) -> anyhow::Result<Self> {
        let local = LocalTargets { comments, live };
        let Some(url) = redis_url else {
            tracing::info!("REDIS_URL이 없어 알림을 이 인스턴스 안에서만 전합니다(인스턴스 1개용)");
            return Ok(Self { local, redis: None });
        };
        let client = redis::Client::open(url).context("REDIS_URL 형식이 잘못되었습니다")?;
        let publisher = tokio::time::timeout(SUBSCRIBE_TIMEOUT, client.get_connection_manager())
            .await
            .context("Redis(REDIS_URL)에 연결하지 못했습니다(시간 초과)")?
            .context("Redis(REDIS_URL)에 연결할 수 없습니다")?;

        let (subscribed, ready) = oneshot::channel();
        tokio::spawn(subscribe_forever(client, local.clone(), subscribed));
        tokio::time::timeout(SUBSCRIBE_TIMEOUT, ready)
            .await
            .context("Redis 구독을 시작하지 못했습니다(시간 초과)")?
            .context("Redis 구독 작업이 끝났습니다")?;
        tracing::info!("Redis Pub/Sub로 인스턴스 사이에 알림을 전합니다");
        Ok(Self {
            local,
            redis: Some(publisher),
        })
    }

    /// 방송의 댓글 WebSocket 구독자 모두에게 알린다(어느 인스턴스에 붙어 있든).
    pub async fn publish(&self, broadcast_id: Uuid, event: CommentEvent) {
        let payload = serde_json::to_string(&event).expect("댓글 알림은 항상 JSON으로 바뀐다");
        let channel = format!("{BROADCAST_CHANNEL_PREFIX}{broadcast_id}");
        if !self.send(&channel, payload).await {
            self.local.comments.deliver(broadcast_id, event);
        }
    }

    /// 방송이 지워졌음을 알려, 모든 인스턴스가 그 방송의 댓글 WebSocket을 닫게 한다.
    pub async fn broadcast_deleted(&self, broadcast_id: Uuid) {
        self.control(Control::BroadcastDeleted { broadcast_id })
            .await;
    }

    /// 방송 송출을 끝내라고 알린다. 그 방송을 송출받는 인스턴스가 송출을 끝내고 녹화본을 만든다.
    pub async fn stop_ingest(&self, broadcast_id: Uuid) {
        self.control(Control::StopIngest { broadcast_id }).await;
    }

    async fn control(&self, control: Control) {
        let payload = serde_json::to_string(&control).expect("제어 신호는 항상 JSON으로 바뀐다");
        if !self.send(CONTROL_CHANNEL, payload).await {
            self.local.apply(control);
        }
    }

    /// Redis로 보낸다. Redis를 쓰지 않거나 보내지 못했으면 `false`를 돌려주고,
    /// 부른 쪽이 이 인스턴스 안에서라도 전한다.
    async fn send(&self, channel: &str, payload: String) -> bool {
        let Some(redis) = &self.redis else {
            return false;
        };
        let mut redis = redis.clone();
        match redis.publish::<_, _, i64>(channel, payload).await {
            Ok(_) => true,
            Err(err) => {
                tracing::warn!(
                    channel,
                    "Redis로 알림을 보내지 못해 이 인스턴스에만 전합니다: {err}"
                );
                false
            }
        }
    }
}

/// Redis 구독을 유지한다. 연결이 끊기면 다시 연결해 구독한다.
async fn subscribe_forever(
    client: redis::Client,
    local: LocalTargets,
    subscribed: oneshot::Sender<()>,
) {
    let mut subscribed = Some(subscribed);
    loop {
        match subscribe_once(&client, &local, &mut subscribed).await {
            Ok(()) => tracing::warn!("Redis 구독이 끊겼습니다. 다시 연결합니다"),
            Err(err) => tracing::warn!("Redis 구독에 실패했습니다. 다시 연결합니다: {err:#}"),
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn subscribe_once(
    client: &redis::Client,
    local: &LocalTargets,
    subscribed: &mut Option<oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub
        .psubscribe(format!("{BROADCAST_CHANNEL_PREFIX}*"))
        .await?;
    pubsub.subscribe(CONTROL_CHANNEL).await?;
    if let Some(subscribed) = subscribed.take() {
        let _ = subscribed.send(());
    }
    let mut messages = pubsub.into_on_message();
    while let Some(message) = messages.next().await {
        match message.get_payload::<String>() {
            Ok(payload) => local.dispatch(message.get_channel_name(), &payload),
            Err(err) => tracing::warn!("Redis 메시지를 읽을 수 없습니다: {err}"),
        }
    }
    Ok(())
}
