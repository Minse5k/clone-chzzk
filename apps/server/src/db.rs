use std::path::Path;

use anyhow::Context;
use chrono::{DateTime, SecondsFormat, Utc};
use shared::{Broadcast, Recording};
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

/// 새로 등록할 방송. 녹화본은 이미 저장소에 저장된 상태여야 한다.
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
        Ok(Self {
            broadcast: Broadcast {
                id: Uuid::parse_str(&row.id)?,
                title: row.title,
                created_at: DateTime::parse_from_rfc3339(&row.created_at)?.with_timezone(&Utc),
                recording,
            },
            recording_location,
        })
    }
}

const SELECT_BROADCASTS: &str = "\
    SELECT b.id, b.title, b.created_at, \
           r.storage_backend, r.object_key, r.content_type, r.size_bytes \
    FROM broadcasts b LEFT JOIN recordings r ON r.broadcast_id = b.id";

/// 방송 목록, 최신 등록순.
pub async fn list_broadcasts(pool: &SqlitePool) -> anyhow::Result<Vec<Broadcast>> {
    sqlx::query_as::<_, BroadcastRow>(sqlx::AssertSqlSafe(format!(
        "{SELECT_BROADCASTS} ORDER BY b.created_at DESC, b.id DESC"
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
    let total: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM recordings")
        .fetch_one(&mut *tx)
        .await?;
    if u64::try_from(total)?.saturating_add(new.recording.size_bytes) > storage_quota_bytes {
        return Err(InsertError::QuotaExceeded);
    }
    sqlx::query("INSERT INTO broadcasts (id, title, created_at) VALUES (?, ?, ?)")
        .bind(new.id.to_string())
        .bind(&new.title)
        // 고정 자릿수 UTC라 문자열 정렬이 시간 순서와 같다.
        .bind(new.created_at.to_rfc3339_opts(SecondsFormat::Micros, true))
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO recordings (broadcast_id, storage_backend, object_key, content_type, size_bytes) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(new.id.to_string())
    .bind(&new.recording_location.storage_backend)
    .bind(&new.recording_location.object_key)
    .bind(&new.recording.content_type)
    .bind(i64::try_from(new.recording.size_bytes)?)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
