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
    pub created_at: DateTime<Utc>,
    pub recording: Recording,
    pub recording_location: RecordingLocation,
}

#[derive(sqlx::FromRow)]
struct BroadcastRow {
    id: String,
    title: String,
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
    SELECT b.id, b.title, b.created_at, b.ended_at, \
           r.storage_backend, r.object_key, r.content_type, r.size_bytes \
    FROM broadcasts b LEFT JOIN recordings r ON r.broadcast_id = b.id";

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
    sqlx::query("INSERT INTO broadcasts (id, title, created_at, ended_at) VALUES (?, ?, ?, ?)")
        .bind(new.id.to_string())
        .bind(&new.title)
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
    started_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO broadcasts (id, title, created_at) VALUES (?, ?, ?)")
        .bind(id.to_string())
        .bind(title)
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
