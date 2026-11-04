use std::path::Path;

use anyhow::Context;
use chrono::{DateTime, Utc};
use shared::Broadcast;
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

#[derive(sqlx::FromRow)]
struct BroadcastRow {
    id: String,
    title: String,
    created_at: String,
}

impl TryFrom<BroadcastRow> for Broadcast {
    type Error = anyhow::Error;

    fn try_from(row: BroadcastRow) -> anyhow::Result<Self> {
        Ok(Self {
            id: Uuid::parse_str(&row.id)?,
            title: row.title,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)?.with_timezone(&Utc),
        })
    }
}

/// 방송 목록, 최신 등록순.
pub async fn list_broadcasts(pool: &SqlitePool) -> anyhow::Result<Vec<Broadcast>> {
    sqlx::query_as::<_, BroadcastRow>(
        "SELECT id, title, created_at FROM broadcasts ORDER BY created_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(Broadcast::try_from)
    .collect()
}
