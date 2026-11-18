//! PostgreSQL 접근. 모든 서버 인스턴스가 같은 DB를 함께 쓴다(ADR 0006).

use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use shared::{Broadcast, BroadcastStatus, Recording};
use sqlx::{PgConnection, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

/// 서버가 쓰는 DB 연결 풀.
pub type Db = PgPool;

/// 저장 총량을 확인하고 녹화본을 기록하는 트랜잭션끼리 줄을 세우는 advisory lock 키.
const QUOTA_LOCK: i64 = 0x6368_7a7a_6b01;
/// 라이브 방송 수를 세고 새 라이브를 기록하는 트랜잭션끼리 줄을 세우는 advisory lock 키.
const LIVE_LOCK: i64 = 0x6368_7a7a_6b02;

/// DB에 연결하고 마이그레이션을 적용한다. 여러 인스턴스가 동시에 시작해도
/// 마이그레이터가 잠금을 잡아 한 번만 적용된다.
pub async fn connect(url: &str, max_connections: u32) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect(url)
        .await
        .context("DB(DATABASE_URL)에 연결할 수 없습니다")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("DB 마이그레이션 실패")?;
    Ok(pool)
}

/// 녹화본이 저장소의 어디에 있는지.
#[derive(Debug, Clone)]
pub struct RecordingLocation {
    pub storage_backend: String,
    pub object_key: String,
}

/// DB에 저장된 방송. API로 내보내는 정보에 녹화본 위치가 더해진다.
pub struct StoredBroadcast {
    pub broadcast: Broadcast,
    pub recording_location: Option<RecordingLocation>,
}

/// 새로 등록할 방송. 방송 등록으로 생긴 방송은 등록 시각에 이미 종료된 방송이다.
/// 녹화본은 이미 저장소에 저장된 상태여야 한다.
pub struct NewBroadcast {
    pub id: Uuid,
    pub title: String,
    pub owner_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub recording: Recording,
    pub recording_location: RecordingLocation,
}

#[derive(sqlx::FromRow)]
struct BroadcastRow {
    id: Uuid,
    title: String,
    owner_id: Uuid,
    owner_username: String,
    created_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    storage_backend: Option<String>,
    object_key: Option<String>,
    content_type: Option<String>,
    size_bytes: Option<i64>,
}

impl TryFrom<BroadcastRow> for StoredBroadcast {
    type Error = anyhow::Error;

    fn try_from(row: BroadcastRow) -> anyhow::Result<Self> {
        let (recording, recording_location) = match (
            row.storage_backend,
            row.object_key,
            row.content_type,
            row.size_bytes,
        ) {
            (Some(storage_backend), Some(object_key), Some(content_type), Some(size_bytes)) => (
                Some(Recording {
                    content_type,
                    size_bytes: size_bytes.try_into()?,
                }),
                Some(RecordingLocation {
                    storage_backend,
                    object_key,
                }),
            ),
            _ => (None, None),
        };
        let ended_at = row.ended_at;
        Ok(Self {
            broadcast: Broadcast {
                id: row.id,
                title: row.title,
                owner: shared::User {
                    id: row.owner_id,
                    username: row.owner_username,
                },
                created_at: row.created_at,
                status: BroadcastStatus::from_ended_at(ended_at),
                ended_at,
                recording,
                live_playlist: false,
            },
            recording_location,
        })
    }
}

const SELECT_BROADCASTS: &str = "\
    SELECT b.id, b.title, b.owner_id, u.username AS owner_username, b.created_at, b.ended_at, \
           r.storage_backend, r.object_key, r.content_type, r.size_bytes \
    FROM broadcasts b \
    JOIN users u ON u.id = b.owner_id \
    LEFT JOIN recordings r ON r.broadcast_id = b.id";

/// 방송 목록, 최신 등록순. 상태를 주면 그 상태의 방송만 돌려준다.
pub async fn list_broadcasts(
    pool: &Db,
    status: Option<BroadcastStatus>,
) -> anyhow::Result<Vec<Broadcast>> {
    let filter = match status {
        None => "",
        Some(BroadcastStatus::Live) => "WHERE b.ended_at IS NULL",
        Some(BroadcastStatus::Ended) => "WHERE b.ended_at IS NOT NULL",
    };
    sqlx::query_as::<_, BroadcastRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_BROADCASTS} {filter} ORDER BY b.created_at DESC, b.id DESC"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| Ok(StoredBroadcast::try_from(row)?.broadcast))
    .collect()
}

pub async fn find_broadcast(pool: &Db, id: Uuid) -> anyhow::Result<Option<StoredBroadcast>> {
    sqlx::query_as::<_, BroadcastRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_BROADCASTS} WHERE b.id = $1"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(StoredBroadcast::try_from)
    .transpose()
}

/// 저장된 녹화본 크기의 합.
pub async fn total_recording_bytes(pool: &Db) -> anyhow::Result<u64> {
    let total: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0)::BIGINT FROM recordings")
            .fetch_one(pool)
            .await?;
    Ok(total.try_into()?)
}

#[derive(Debug)]
pub enum InsertError {
    /// 기록하면 녹화본 크기 합이 저장 총량 한도를 넘는다.
    QuotaExceeded,
    Other(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for InsertError {
    fn from(err: E) -> Self {
        Self::Other(err.into())
    }
}

/// 방송과 녹화본을 한 트랜잭션으로 기록한다.
/// 동시에 들어온 등록끼리도 저장 총량 한도를 넘지 않도록 같은 트랜잭션 안에서 합을 다시 확인한다.
pub async fn insert_broadcast(
    pool: &Db,
    new: &NewBroadcast,
    storage_quota_bytes: u64,
) -> Result<(), InsertError> {
    let mut tx = pool.begin().await?;
    check_quota(&mut tx, new.recording.size_bytes, storage_quota_bytes).await?;
    sqlx::query(
        "INSERT INTO broadcasts (id, title, owner_id, created_at, ended_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(new.id)
    .bind(&new.title)
    .bind(new.owner_id)
    .bind(new.created_at)
    .bind(new.created_at)
    .execute(&mut *tx)
    .await?;
    insert_recording_row(&mut tx, new.id, &new.recording, &new.recording_location).await?;
    tx.commit().await?;
    Ok(())
}

/// 종료된 라이브 방송에 녹화본을 붙인다. 방송 등록과 같은 규칙으로 저장 총량 한도를 확인한다.
pub async fn insert_recording(
    pool: &Db,
    broadcast_id: Uuid,
    recording: &Recording,
    location: &RecordingLocation,
    storage_quota_bytes: u64,
) -> Result<(), InsertError> {
    let mut tx = pool.begin().await?;
    check_quota(&mut tx, recording.size_bytes, storage_quota_bytes).await?;
    insert_recording_row(&mut tx, broadcast_id, recording, location).await?;
    tx.commit().await?;
    Ok(())
}

/// 녹화본을 더해도 저장 총량 한도를 넘지 않는지 같은 트랜잭션 안에서 확인한다.
async fn check_quota(
    tx: &mut PgConnection,
    size_bytes: u64,
    storage_quota_bytes: u64,
) -> Result<(), InsertError> {
    // 동시에 기록하는 트랜잭션이 서로의 합을 보지 못해 함께 한도를 넘는 일을 막는다.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(QUOTA_LOCK)
        .execute(&mut *tx)
        .await?;
    let total: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0)::BIGINT FROM recordings")
            .fetch_one(&mut *tx)
            .await?;
    if u64::try_from(total)?.saturating_add(size_bytes) > storage_quota_bytes {
        return Err(InsertError::QuotaExceeded);
    }
    Ok(())
}

async fn insert_recording_row(
    tx: &mut PgConnection,
    broadcast_id: Uuid,
    recording: &Recording,
    location: &RecordingLocation,
) -> Result<(), InsertError> {
    sqlx::query(
        "INSERT INTO recordings (broadcast_id, storage_backend, object_key, content_type, size_bytes) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(broadcast_id)
    .bind(&location.storage_backend)
    .bind(&location.object_key)
    .bind(&recording.content_type)
    .bind(i64::try_from(recording.size_bytes)?)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// 송출을 시작한 라이브 방송을 기록한다. 종료 시각이 없으므로 라이브 중이다.
/// 서비스 전체의 라이브 방송이 이미 `max_live`개면 기록하지 않고 `Ok(false)`를 돌려준다.
/// 여러 인스턴스가 동시에 시작해도 advisory lock으로 줄을 세워 한도를 넘지 않는다.
pub async fn insert_live_broadcast(
    pool: &Db,
    id: Uuid,
    title: &str,
    owner_id: Uuid,
    started_at: DateTime<Utc>,
    instance: &str,
    max_live: usize,
) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(LIVE_LOCK)
        .execute(&mut *tx)
        .await?;
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM broadcasts WHERE ended_at IS NULL")
        .fetch_one(&mut *tx)
        .await?;
    if usize::try_from(live)? >= max_live {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO broadcasts (id, title, owner_id, created_at, ingest_instance, ingest_heartbeat_at) \
         VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(id)
    .bind(title)
    .bind(owner_id)
    .bind(started_at)
    .bind(instance)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// 송출 중인 인스턴스가 살아 있음을 기록한다. 방송이 이미 종료되었으면(다른 인스턴스에서 소유자가
/// 끝냈거나, 하트비트가 끊겨 정리되었으면) `Ok(false)`.
pub async fn touch_ingest(pool: &Db, id: Uuid, instance: &str) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE broadcasts SET ingest_heartbeat_at = now(), ingest_instance = $2 \
         WHERE id = $1 AND ended_at IS NULL",
    )
    .bind(id)
    .bind(instance)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 하트비트가 `timeout`보다 오래 끊긴 라이브 방송을 종료하고 그 ID를 돌려준다.
/// 조건부 갱신이라 여러 인스턴스가 함께 찾아도 방송 하나는 한 인스턴스만 종료시킨다.
/// 하트비트 기록과 비교 모두 DB의 시계(`now()`)를 써서 인스턴스마다 시계가 달라도 어긋나지 않는다.
pub async fn end_stale_live_broadcasts(pool: &Db, timeout: Duration) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "UPDATE broadcasts SET ended_at = now() \
         WHERE ended_at IS NULL AND ingest_heartbeat_at < now() - make_interval(secs => $1) \
         RETURNING id",
    )
    .bind(timeout.as_secs_f64())
    .fetch_all(pool)
    .await?)
}

/// 라이브 방송을 종료한다. 이미 종료된 방송이면 그대로 둔다.
pub async fn end_broadcast(pool: &Db, id: Uuid, ended_at: DateTime<Utc>) -> anyhow::Result<()> {
    sqlx::query("UPDATE broadcasts SET ended_at = $1 WHERE id = $2 AND ended_at IS NULL")
        .bind(ended_at)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 로그인할 때 확인하는 사용자 정보.
pub struct StoredUser {
    pub user: shared::User,
    pub password_hash: String,
}

/// 새 사용자를 기록한다. 같은 아이디가 이미 있으면 `Ok(false)`.
pub async fn insert_user(
    pool: &Db,
    user: &shared::User,
    password_hash: &str,
    created_at: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO users (id, username, password_hash, created_at) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (username) DO NOTHING",
    )
    .bind(user.id)
    .bind(&user.username)
    .bind(password_hash)
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn find_user_by_username(
    pool: &Db,
    username: &str,
) -> anyhow::Result<Option<StoredUser>> {
    let row: Option<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, username, password_hash FROM users WHERE username = $1")
            .bind(username)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id, username, password_hash)| StoredUser {
        user: shared::User { id, username },
        password_hash,
    }))
}

pub async fn find_user(pool: &Db, id: Uuid) -> anyhow::Result<Option<shared::User>> {
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(username.map(|username| shared::User { id, username }))
}

/// 로그아웃한 토큰을 폐기 목록에 넣고, 만료가 지나 더는 필요 없는 항목을 정리한다.
pub async fn revoke_token(pool: &Db, jti: &str, expires_at: DateTime<Utc>) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO revoked_tokens (jti, expires_at) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(jti)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM revoked_tokens WHERE expires_at < $1")
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn is_token_revoked(pool: &Db, jti: &str) -> anyhow::Result<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1::BIGINT FROM revoked_tokens WHERE jti = $1")
            .bind(jti)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// 방송 제목을 바꾼다.
pub async fn update_title(pool: &Db, id: Uuid, title: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE broadcasts SET title = $1 WHERE id = $2")
        .bind(title)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 방송 삭제 결과.
pub enum DeleteOutcome {
    /// 지웠다. 녹화본이 있었다면 저장소에서 지워야 할 위치를 돌려준다.
    Deleted(Option<RecordingLocation>),
    /// 라이브 중이라 지우지 않았다.
    Live,
    /// 확정된 후원이 있어 지우지 않았다. 후원 내역은 돈이 오간 기록이라 남긴다.
    HasDonations,
    NotFound,
}

/// 종료된 방송과 그 녹화본·댓글 행을 한 트랜잭션에서 지운다. 라이브 중인 방송은 지우지 않는다.
pub async fn delete_broadcast(pool: &Db, id: Uuid) -> anyhow::Result<DeleteOutcome> {
    let mut tx = pool.begin().await?;
    // 방송 행을 잠가, 지우는 동안 같은 방송의 종료·삭제가 끼지 못하게 한다.
    let ended_at: Option<Option<DateTime<Utc>>> =
        sqlx::query_scalar("SELECT ended_at FROM broadcasts WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match ended_at {
        None => return Ok(DeleteOutcome::NotFound),
        Some(None) => return Ok(DeleteOutcome::Live),
        Some(Some(_)) => {}
    }
    let confirmed_donations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM donations WHERE broadcast_id = $1 AND status = 'confirmed'",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if confirmed_donations > 0 {
        return Ok(DeleteOutcome::HasDonations);
    }
    // 확정되지 않은 주문(결제 대기·실패·취소)은 방송과 함께 지운다.
    sqlx::query("DELETE FROM donations WHERE broadcast_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let location: Option<(String, String)> = sqlx::query_as(
        "DELETE FROM recordings WHERE broadcast_id = $1 RETURNING storage_backend, object_key",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM broadcasts WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted(location.map(
        |(storage_backend, object_key)| RecordingLocation {
            storage_backend,
            object_key,
        },
    )))
}

/// 녹화본 객체를 삭제 대기열에 넣는다. 저장소에서 바로 지우지 못했을 때 쓴다.
pub async fn enqueue_deletion(pool: &Db, location: &RecordingLocation) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO pending_object_deletions (storage_backend, object_key) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(&location.storage_backend)
    .bind(&location.object_key)
    .execute(pool)
    .await?;
    Ok(())
}

/// 지워야 할 녹화본 객체 중 이 저장소에 있는 것들의 키.
pub async fn pending_deletions(pool: &Db, backend: &str) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT object_key FROM pending_object_deletions WHERE storage_backend = $1",
    )
    .bind(backend)
    .fetch_all(pool)
    .await?)
}

/// 저장소에서 지운 객체를 삭제 대기열에서 뺀다.
pub async fn remove_pending_deletion(
    pool: &Db,
    backend: &str,
    object_key: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM pending_object_deletions WHERE storage_backend = $1 AND object_key = $2",
    )
    .bind(backend)
    .bind(object_key)
    .execute(pool)
    .await?;
    Ok(())
}

/// 댓글을 기록한다. 방송이 없으면(그사이 삭제됨) `Ok(false)`.
pub async fn insert_comment(pool: &Db, comment: &shared::Comment) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO comments (id, broadcast_id, author_id, body, created_at) \
         SELECT $1, id, $2, $3, $4 FROM broadcasts WHERE id = $5",
    )
    .bind(comment.id)
    .bind(comment.author.id)
    .bind(&comment.body)
    .bind(comment.created_at)
    .bind(comment.broadcast_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[derive(sqlx::FromRow)]
struct CommentRow {
    id: Uuid,
    broadcast_id: Uuid,
    author_id: Uuid,
    author_username: String,
    body: String,
    created_at: DateTime<Utc>,
}

impl From<CommentRow> for shared::Comment {
    fn from(row: CommentRow) -> Self {
        Self {
            id: row.id,
            broadcast_id: row.broadcast_id,
            author: shared::User {
                id: row.author_id,
                username: row.author_username,
            },
            body: row.body,
            created_at: row.created_at,
        }
    }
}

const SELECT_COMMENTS: &str = "\
    SELECT c.id, c.broadcast_id, c.author_id, u.username AS author_username, c.body, c.created_at \
    FROM comments c JOIN users u ON u.id = c.author_id";

/// 방송의 댓글, 오래된 순. 많으면 최근 `limit`개만 돌려준다.
pub async fn list_comments(
    pool: &Db,
    broadcast_id: Uuid,
    limit: usize,
) -> anyhow::Result<Vec<shared::Comment>> {
    let mut comments = sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_COMMENTS} WHERE c.broadcast_id = $1 ORDER BY c.created_at DESC, c.id DESC LIMIT $2"
    )))
    .bind(broadcast_id)
    .bind(i64::try_from(limit)?)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(shared::Comment::from)
    .collect::<Vec<_>>();
    comments.reverse();
    Ok(comments)
}

/// 방송에 속한 댓글 하나.
pub async fn find_comment(
    pool: &Db,
    broadcast_id: Uuid,
    comment_id: Uuid,
) -> anyhow::Result<Option<shared::Comment>> {
    Ok(sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_COMMENTS} WHERE c.id = $1 AND c.broadcast_id = $2"
    )))
    .bind(comment_id)
    .bind(broadcast_id)
    .fetch_optional(pool)
    .await?
    .map(shared::Comment::from))
}

pub async fn delete_comment(pool: &Db, comment_id: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM comments WHERE id = $1")
        .bind(comment_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 후원 주문의 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DonationStatus {
    /// 결제 대기. 결제창에서 결제 중이거나 승인 확인을 기다린다.
    Pending,
    /// 결제가 승인되어 확정된 후원.
    Confirmed,
    Failed,
    Canceled,
}

impl DonationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        Ok(match value {
            "pending" => Self::Pending,
            "confirmed" => Self::Confirmed,
            "failed" => Self::Failed,
            "canceled" => Self::Canceled,
            other => anyhow::bail!("알 수 없는 후원 상태: {other}"),
        })
    }
}

/// DB에 저장된 후원 주문.
#[derive(Debug, Clone)]
pub struct StoredDonation {
    pub order_id: String,
    pub broadcast_id: Uuid,
    pub donor: shared::User,
    pub recipient: shared::User,
    pub amount: u64,
    pub message: String,
    pub status: DonationStatus,
    pub payment_key: Option<String>,
    pub confirmed_at: Option<DateTime<Utc>>,
}

impl StoredDonation {
    /// 확정된 후원이면 API로 내보내는 값.
    pub fn confirmed(&self) -> Option<shared::Donation> {
        if self.status != DonationStatus::Confirmed {
            return None;
        }
        Some(shared::Donation {
            order_id: self.order_id.clone(),
            broadcast_id: self.broadcast_id,
            donor: self.donor.clone(),
            recipient: self.recipient.clone(),
            amount: self.amount,
            message: self.message.clone(),
            confirmed_at: self.confirmed_at?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct DonationRow {
    order_id: String,
    broadcast_id: Uuid,
    donor_id: Uuid,
    donor_username: String,
    recipient_id: Uuid,
    recipient_username: String,
    amount: i64,
    message: String,
    status: String,
    payment_key: Option<String>,
    confirmed_at: Option<DateTime<Utc>>,
}

impl TryFrom<DonationRow> for StoredDonation {
    type Error = anyhow::Error;

    fn try_from(row: DonationRow) -> anyhow::Result<Self> {
        Ok(Self {
            order_id: row.order_id,
            broadcast_id: row.broadcast_id,
            donor: shared::User {
                id: row.donor_id,
                username: row.donor_username,
            },
            recipient: shared::User {
                id: row.recipient_id,
                username: row.recipient_username,
            },
            amount: row.amount.try_into()?,
            message: row.message,
            status: DonationStatus::parse(&row.status)?,
            payment_key: row.payment_key,
            confirmed_at: row.confirmed_at,
        })
    }
}

const SELECT_DONATIONS: &str = "\
    SELECT d.order_id, d.broadcast_id, d.donor_id, donor.username AS donor_username, \
           d.recipient_id, recipient.username AS recipient_username, d.amount, d.message, \
           d.status, d.payment_key, d.confirmed_at \
    FROM donations d \
    JOIN users donor ON donor.id = d.donor_id \
    JOIN users recipient ON recipient.id = d.recipient_id";

/// 결제 대기 상태의 후원 주문을 만든다.
pub async fn insert_donation_order(
    pool: &Db,
    order: &StoredDonation,
    created_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO donations \
         (order_id, broadcast_id, recipient_id, donor_id, amount, message, status, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)",
    )
    .bind(&order.order_id)
    .bind(order.broadcast_id)
    .bind(order.recipient.id)
    .bind(order.donor.id)
    .bind(i64::try_from(order.amount)?)
    .bind(&order.message)
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_donation(pool: &Db, order_id: &str) -> anyhow::Result<Option<StoredDonation>> {
    sqlx::query_as::<_, DonationRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_DONATIONS} WHERE d.order_id = $1"
    )))
    .bind(order_id)
    .fetch_optional(pool)
    .await?
    .map(StoredDonation::try_from)
    .transpose()
}

/// 후원 확정 시도의 결과.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// 이번 요청으로 새로 확정했다. 실시간 알림은 이때만 보낸다.
    Confirmed,
    /// 결제 대기 상태가 아니어서 바꾸지 않았다(이미 확정, 실패, 취소).
    NotPending,
    /// 같은 결제 키로 이미 다른 주문이 확정되어 있다.
    PaymentKeyTaken,
}

/// 결제 대기 중인 주문을 확정한다. 같은 주문에 승인 확인과 웹훅이 여러 번 와도
/// `status = 'pending'` 조건 때문에 한 번만 확정되고, 결제 키 유일 제약 때문에
/// 같은 결제가 두 주문에 확정되지도 않는다.
pub async fn confirm_donation(
    pool: &Db,
    order_id: &str,
    payment_key: &str,
    confirmed_at: DateTime<Utc>,
) -> anyhow::Result<ConfirmOutcome> {
    let result = sqlx::query(
        "UPDATE donations SET status = 'confirmed', payment_key = $1, confirmed_at = $2 \
         WHERE order_id = $3 AND status = 'pending'",
    )
    .bind(payment_key)
    .bind(confirmed_at)
    .bind(order_id)
    .execute(pool)
    .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => Ok(ConfirmOutcome::Confirmed),
        Ok(_) => Ok(ConfirmOutcome::NotPending),
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            Ok(ConfirmOutcome::PaymentKeyTaken)
        }
        Err(err) => Err(err.into()),
    }
}

/// 결제 대기 중인 주문을 실패나 취소로 끝낸다. 이미 확정된 주문은 바꾸지 않는다.
/// 바꿨으면 `true`.
pub async fn close_donation(
    pool: &Db,
    order_id: &str,
    status: DonationStatus,
    reason: &str,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        matches!(status, DonationStatus::Failed | DonationStatus::Canceled),
        "후원 주문은 실패나 취소로만 끝낼 수 있습니다"
    );
    let result = sqlx::query(
        "UPDATE donations SET status = $1, failure_reason = $2 \
         WHERE order_id = $3 AND status = 'pending'",
    )
    .bind(status.as_str())
    .bind(reason)
    .bind(order_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 방송의 확정 후원 합계·건수와 최근 후원 목록.
pub async fn donation_summary(
    pool: &Db,
    broadcast_id: Uuid,
    limit: usize,
) -> anyhow::Result<shared::DonationSummary> {
    let id = broadcast_id;
    let (total, count): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(amount), 0)::BIGINT, COUNT(*) FROM donations \
         WHERE broadcast_id = $1 AND status = 'confirmed'",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    let donations = sqlx::query_as::<_, DonationRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_DONATIONS} WHERE d.broadcast_id = $1 AND d.status = 'confirmed' \
         ORDER BY d.confirmed_at DESC, d.order_id DESC LIMIT $2"
    )))
    .bind(id)
    .bind(i64::try_from(limit)?)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| {
        StoredDonation::try_from(row)?
            .confirmed()
            .ok_or_else(|| anyhow::anyhow!("확정 후원에 확정 시각이 없습니다"))
    })
    .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(shared::DonationSummary {
        total_amount: total.try_into()?,
        count: count.try_into()?,
        donations,
    })
}
