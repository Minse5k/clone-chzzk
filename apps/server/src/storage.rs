//! 녹화본 저장소. 백엔드마다 같은 연산(스트리밍 저장, 범위 읽기, 삭제)을 제공한다.

use std::{
    io::{self, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::Context;
use axum::body::{Body, Bytes};
use futures_util::{Stream, StreamExt};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use tokio_util::io::ReaderStream;

/// 녹화본 안의 바이트 구간. `len`은 1 이상이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub len: u64,
}

pub enum RecordingStore {
    Local(LocalStore),
}

impl RecordingStore {
    /// DB의 `storage_backend` 값.
    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
        }
    }

    /// 스트림을 끝까지 저장하고 저장한 바이트 수를 돌려준다.
    /// 실패하면 쓰다 만 객체를 남기지 않는다.
    pub async fn put<S>(&self, key: &str, stream: S) -> io::Result<u64>
    where
        S: Stream<Item = io::Result<Bytes>> + Send,
    {
        match self {
            Self::Local(store) => store.put(key, stream).await,
        }
    }

    pub async fn read_range(&self, key: &str, range: ByteRange) -> anyhow::Result<Body> {
        match self {
            Self::Local(store) => store.read_range(key, range).await,
        }
    }

    /// 객체를 지운다. 없는 객체여도 성공으로 본다.
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        match self {
            Self::Local(store) => store.delete(key).await,
        }
    }
}

/// 서버 로컬 디렉터리에 녹화본을 두는 저장소.
pub struct LocalStore {
    dir: PathBuf,
}

impl LocalStore {
    pub async fn open(dir: &Path) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| format!("저장 디렉터리를 만들 수 없습니다: {}", dir.display()))?;
        Ok(Self { dir: dir.to_owned() })
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    async fn put<S>(&self, key: &str, stream: S) -> io::Result<u64>
    where
        S: Stream<Item = io::Result<Bytes>> + Send,
    {
        // 다 쓴 뒤에 이름을 바꿔서, 중간에 실패해도 완성되지 않은 녹화본이 보이지 않게 한다.
        let partial = self.path(&format!("{key}.part"));
        let result = async {
            let mut file = File::create(&partial).await?;
            let mut written = 0;
            let mut stream = std::pin::pin!(stream);
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                file.write_all(&chunk).await?;
                written += chunk.len() as u64;
            }
            file.sync_all().await?;
            tokio::fs::rename(&partial, self.path(key)).await?;
            Ok(written)
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        result
    }

    async fn read_range(&self, key: &str, range: ByteRange) -> anyhow::Result<Body> {
        let mut file = File::open(self.path(key))
            .await
            .with_context(|| format!("녹화본을 열 수 없습니다: {key}"))?;
        file.seek(SeekFrom::Start(range.start)).await?;
        Ok(Body::from_stream(ReaderStream::new(file.take(range.len))))
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        match tokio::fs::remove_file(self.path(key)).await {
            Err(err) if err.kind() != io::ErrorKind::NotFound => {
                Err(err).with_context(|| format!("녹화본을 지울 수 없습니다: {key}"))
            }
            _ => Ok(()),
        }
    }
}
