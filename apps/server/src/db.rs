use std::path::Path;

use anyhow::Context;
use chrono::{DateTime, SecondsFormat, Utc};
use shared::{Broadcast, BroadcastStatus, Recording};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};
use uuid::Uuid;

/// SQLite 파일을 열고(없으면 만들고) 마이그레이션을 적용한다.
pub async fn connect(path: &Path) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("DB 디렉터리를 만들 수 없습니다: {}", parent.display()))?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePool::connect_with(options)
        .await
        .with_context(|| format!("DB를 열 수 없습니다: {}", path.display()))?;
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
    id: String,
    title: String,
    owner_id: String,
    owner_username: String,
    created_at: String,
    ended_at: Option<String>,
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
        let ended_at = row.ended_at.as_deref().map(parse_time).transpose()?;
        Ok(Self {
            broadcast: Broadcast {
                id: Uuid::parse_str(&row.id)?,
                title: row.title,
                owner: shared::User {
                    id: Uuid::parse_str(&row.owner_id)?,
                    username: row.owner_username,
                },
                created_at: parse_time(&row.created_at)?,
                status: BroadcastStatus::from_ended_at(ended_at),
                ended_at,
                recording,
                live_playlist: false,
            },
            recording_location,
        })
    }
}

fn parse_time(text: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(text)?.with_timezone(&Utc))
}

/// 고정 자릿수 UTC라 문자열 정렬이 시간 순서와 같다.
fn format_time(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Micros, true)
}

const SELECT_BROADCASTS: &str = "\
    SELECT b.id, b.title, b.owner_id, u.username AS owner_username, b.created_at, b.ended_at, \
           r.storage_backend, r.object_key, r.content_type, r.size_bytes \
    FROM broadcasts b \
    JOIN users u ON u.id = b.owner_id \
    LEFT JOIN recordings r ON r.broadcast_id = b.id";

/// 방송 목록, 최신 등록순. 상태를 주면 그 상태의 방송만 돌려준다.
pub async fn list_broadcasts(
    pool: &SqlitePool,
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

pub async fn find_broadcast(
    pool: &SqlitePool,
    id: Uuid,
) -> anyhow::Result<Option<StoredBroadcast>> {
    sqlx::query_as::<_, BroadcastRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_BROADCASTS} WHERE b.id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(StoredBroadcast::try_from)
    .transpose()
}

/// 저장된 녹화본 크기의 합.
pub async fn total_recording_bytes(pool: &SqlitePool) -> anyhow::Result<u64> {
    let total: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM recordings")
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
    pool: &SqlitePool,
    new: &NewBroadcast,
    storage_quota_bytes: u64,
) -> Result<(), InsertError> {
    // BEGIN IMMEDIATE로 쓰기 잠금을 먼저 잡아, 합 확인과 기록 사이에 다른 등록이 끼지 못하게 한다.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    check_quota(&mut tx, new.recording.size_bytes, storage_quota_bytes).await?;
    let created_at = format_time(new.created_at);
    sqlx::query(
        "INSERT INTO broadcasts (id, title, owner_id, created_at, ended_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(new.id.to_string())
    .bind(&new.title)
    .bind(new.owner_id.to_string())
    .bind(&created_at)
    .bind(&created_at)
    .execute(&mut *tx)
    .await?;
    insert_recording_row(&mut tx, new.id, &new.recording, &new.recording_location).await?;
    tx.commit().await?;
    Ok(())
}

/// 종료된 라이브 방송에 녹화본을 붙인다. 방송 등록과 같은 규칙으로 저장 총량 한도를 확인한다.
pub async fn insert_recording(
    pool: &SqlitePool,
    broadcast_id: Uuid,
    recording: &Recording,
    location: &RecordingLocation,
    storage_quota_bytes: u64,
) -> Result<(), InsertError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    check_quota(&mut tx, recording.size_bytes, storage_quota_bytes).await?;
    insert_recording_row(&mut tx, broadcast_id, recording, location).await?;
    tx.commit().await?;
    Ok(())
}

/// 녹화본을 더해도 저장 총량 한도를 넘지 않는지 같은 트랜잭션 안에서 확인한다.
async fn check_quota(
    tx: &mut sqlx::SqliteConnection,
    size_bytes: u64,
    storage_quota_bytes: u64,
) -> Result<(), InsertError> {
    let total: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM recordings")
        .fetch_one(&mut *tx)
        .await?;
    if u64::try_from(total)?.saturating_add(size_bytes) > storage_quota_bytes {
        return Err(InsertError::QuotaExceeded);
    }
    Ok(())
}

async fn insert_recording_row(
    tx: &mut sqlx::SqliteConnection,
    broadcast_id: Uuid,
    recording: &Recording,
    location: &RecordingLocation,
) -> Result<(), InsertError> {
    sqlx::query(
        "INSERT INTO recordings (broadcast_id, storage_backend, object_key, content_type, size_bytes) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(broadcast_id.to_string())
    .bind(&location.storage_backend)
    .bind(&location.object_key)
    .bind(&recording.content_type)
    .bind(i64::try_from(recording.size_bytes)?)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// 송출을 시작한 라이브 방송을 기록한다. 종료 시각이 없으므로 라이브 중이다.
pub async fn insert_live_broadcast(
    pool: &SqlitePool,
    id: Uuid,
    title: &str,
    owner_id: Uuid,
    started_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO broadcasts (id, title, owner_id, created_at) VALUES (?, ?, ?, ?)")
        .bind(id.to_string())
        .bind(title)
        .bind(owner_id.to_string())
        .bind(format_time(started_at))
        .execute(pool)
        .await?;
    Ok(())
}

/// 라이브 방송을 종료한다. 이미 종료된 방송이면 그대로 둔다.
pub async fn end_broadcast(
    pool: &SqlitePool,
    id: Uuid,
    ended_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE broadcasts SET ended_at = ? WHERE id = ? AND ended_at IS NULL")
        .bind(format_time(ended_at))
        .bind(id.to_string())
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
    pool: &SqlitePool,
    user: &shared::User,
    password_hash: &str,
    created_at: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT (username) DO NOTHING",
    )
    .bind(user.id.to_string())
    .bind(&user.username)
    .bind(password_hash)
    .bind(format_time(created_at))
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn find_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> anyhow::Result<Option<StoredUser>> {
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, username, password_hash FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(pool)
            .await?;
    row.map(|(id, username, password_hash)| {
        Ok(StoredUser {
            user: shared::User {
                id: Uuid::parse_str(&id)?,
                username,
            },
            password_hash,
        })
    })
    .transpose()
}

pub async fn find_user(pool: &SqlitePool, id: Uuid) -> anyhow::Result<Option<shared::User>> {
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(username.map(|username| shared::User { id, username }))
}

/// 로그아웃한 토큰을 폐기 목록에 넣고, 만료가 지나 더는 필요 없는 항목을 정리한다.
pub async fn revoke_token(
    pool: &SqlitePool,
    jti: &str,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT OR IGNORE INTO revoked_tokens (jti, expires_at) VALUES (?, ?)")
        .bind(jti)
        .bind(format_time(expires_at))
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM revoked_tokens WHERE expires_at < ?")
        .bind(format_time(Utc::now()))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn is_token_revoked(pool: &SqlitePool, jti: &str) -> anyhow::Result<bool> {
    let found: Option<i64> = sqlx::query_scalar("SELECT 1 FROM revoked_tokens WHERE jti = ?")
        .bind(jti)
        .fetch_optional(pool)
        .await?;
    Ok(found.is_some())
}

/// 방송 제목을 바꾼다.
pub async fn update_title(pool: &SqlitePool, id: Uuid, title: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE broadcasts SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id.to_string())
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
    NotFound,
}

/// 종료된 방송과 그 녹화본·댓글 행을 한 트랜잭션에서 지운다. 라이브 중인 방송은 지우지 않는다.
pub async fn delete_broadcast(pool: &SqlitePool, id: Uuid) -> anyhow::Result<DeleteOutcome> {
    let id = id.to_string();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let ended_at: Option<Option<String>> =
        sqlx::query_scalar("SELECT ended_at FROM broadcasts WHERE id = ?")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
    match ended_at {
        None => return Ok(DeleteOutcome::NotFound),
        Some(None) => return Ok(DeleteOutcome::Live),
        Some(Some(_)) => {}
    }
    let location: Option<(String, String)> = sqlx::query_as(
        "DELETE FROM recordings WHERE broadcast_id = ? RETURNING storage_backend, object_key",
    )
    .bind(&id)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM broadcasts WHERE id = ?")
        .bind(&id)
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
pub async fn enqueue_deletion(
    pool: &SqlitePool,
    location: &RecordingLocation,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO pending_object_deletions (storage_backend, object_key) VALUES (?, ?)",
    )
    .bind(&location.storage_backend)
    .bind(&location.object_key)
    .execute(pool)
    .await?;
    Ok(())
}

/// 지워야 할 녹화본 객체 중 이 저장소에 있는 것들의 키.
pub async fn pending_deletions(pool: &SqlitePool, backend: &str) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT object_key FROM pending_object_deletions WHERE storage_backend = ?",
    )
    .bind(backend)
    .fetch_all(pool)
    .await?)
}

/// 저장소에서 지운 객체를 삭제 대기열에서 뺀다.
pub async fn remove_pending_deletion(
    pool: &SqlitePool,
    backend: &str,
    object_key: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM pending_object_deletions WHERE storage_backend = ? AND object_key = ?",
    )
    .bind(backend)
    .bind(object_key)
    .execute(pool)
    .await?;
    Ok(())
}

/// 댓글을 기록한다. 방송이 없으면(그사이 삭제됨) `Ok(false)`.
pub async fn insert_comment(pool: &SqlitePool, comment: &shared::Comment) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO comments (id, broadcast_id, author_id, body, created_at) \
         SELECT ?, id, ?, ?, ? FROM broadcasts WHERE id = ?",
    )
    .bind(comment.id.to_string())
    .bind(comment.author.id.to_string())
    .bind(&comment.body)
    .bind(format_time(comment.created_at))
    .bind(comment.broadcast_id.to_string())
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[derive(sqlx::FromRow)]
struct CommentRow {
    id: String,
    broadcast_id: String,
    author_id: String,
    author_username: String,
    body: String,
    created_at: String,
}

impl TryFrom<CommentRow> for shared::Comment {
    type Error = anyhow::Error;

    fn try_from(row: CommentRow) -> anyhow::Result<Self> {
        Ok(Self {
            id: Uuid::parse_str(&row.id)?,
            broadcast_id: Uuid::parse_str(&row.broadcast_id)?,
            author: shared::User {
                id: Uuid::parse_str(&row.author_id)?,
                username: row.author_username,
            },
            body: row.body,
            created_at: parse_time(&row.created_at)?,
        })
    }
}

const SELECT_COMMENTS: &str = "\
    SELECT c.id, c.broadcast_id, c.author_id, u.username AS author_username, c.body, c.created_at \
    FROM comments c JOIN users u ON u.id = c.author_id";

/// 방송의 댓글, 오래된 순. 많으면 최근 `limit`개만 돌려준다.
pub async fn list_comments(
    pool: &SqlitePool,
    broadcast_id: Uuid,
    limit: usize,
) -> anyhow::Result<Vec<shared::Comment>> {
    let mut comments = sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_COMMENTS} WHERE c.broadcast_id = ? ORDER BY c.created_at DESC, c.id DESC LIMIT ?"
    )))
    .bind(broadcast_id.to_string())
    .bind(i64::try_from(limit)?)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(shared::Comment::try_from)
    .collect::<anyhow::Result<Vec<_>>>()?;
    comments.reverse();
    Ok(comments)
}

/// 방송에 속한 댓글 하나.
pub async fn find_comment(
    pool: &SqlitePool,
    broadcast_id: Uuid,
    comment_id: Uuid,
) -> anyhow::Result<Option<shared::Comment>> {
    sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_COMMENTS} WHERE c.id = ? AND c.broadcast_id = ?"
    )))
    .bind(comment_id.to_string())
    .bind(broadcast_id.to_string())
    .fetch_optional(pool)
    .await?
    .map(shared::Comment::try_from)
    .transpose()
}

pub async fn delete_comment(pool: &SqlitePool, comment_id: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM comments WHERE id = ?")
        .bind(comment_id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}
