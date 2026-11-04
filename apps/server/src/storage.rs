//! 녹화본 저장소. 백엔드마다 같은 연산(스트리밍 저장, 범위 읽기, 삭제)을 제공한다.

use std::{
    io::{self, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::Context;
use axum::body::{Body, Bytes};
use futures_util::{Stream, StreamExt};
use object_store::{
    GetOptions, GetRange, ObjectStore, ObjectStoreExt, WriteMultipart,
    aws::{AmazonS3, AmazonS3Builder},
    path::Path as ObjectPath,
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use tokio_util::io::ReaderStream;

use crate::config::{OciStorageConfig, StorageConfig};

/// 녹화본 안의 바이트 구간. `len`은 1 이상이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub len: u64,
}

pub enum RecordingStore {
    Local(LocalStore),
    Oci(OciStore),
}

impl RecordingStore {
    /// 설정한 백엔드를 연다. 접근할 수 없으면 서버가 시작하지 않도록 오류를 돌려준다.
    pub async fn open(config: &StorageConfig) -> anyhow::Result<Self> {
        Ok(match config {
            StorageConfig::Local { dir } => Self::Local(LocalStore::open(dir).await?),
            StorageConfig::Oci(oci) => Self::Oci(OciStore::open(oci).await?),
        })
    }

    /// DB의 `storage_backend` 값.
    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Oci(_) => "oci",
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
            Self::Oci(store) => store.put(key, stream).await,
        }
    }

    pub async fn read_range(&self, key: &str, range: ByteRange) -> anyhow::Result<Body> {
        match self {
            Self::Local(store) => store.read_range(key, range).await,
            Self::Oci(store) => store.read_range(key, range).await,
        }
    }

    /// 객체를 지운다. 없는 객체여도 성공으로 본다.
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        match self {
            Self::Local(store) => store.delete(key).await,
            Self::Oci(store) => store.delete(key).await,
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
        Ok(Self {
            dir: dir.to_owned(),
        })
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

/// OCI Object Storage 비공개 버킷에 녹화본을 두는 저장소(S3 호환 API).
pub struct OciStore {
    s3: AmazonS3,
}

impl OciStore {
    /// 업로드를 이 크기의 파트로 나눠 보낸다. 업로드 하나가 쓰는 메모리는 대략 이 값 × 동시 파트 수다.
    const PART_BYTES: usize = 8 * 1024 * 1024;
    const MAX_CONCURRENT_PARTS: usize = 2;
    /// 객체 키 앞에 붙여 버킷 안에서 녹화본을 한곳에 모은다.
    const PREFIX: &'static str = "recordings";

    async fn open(config: &OciStorageConfig) -> anyhow::Result<Self> {
        let s3 = AmazonS3Builder::new()
            .with_endpoint(&config.endpoint)
            .with_region(&config.region)
            .with_bucket_name(&config.bucket)
            .with_access_key_id(&config.access_key_id)
            .with_secret_access_key(&config.secret_access_key)
            // OCI S3 호환 API는 경로 방식(엔드포인트/버킷/키)만 지원한다.
            .with_virtual_hosted_style_request(false)
            .with_allow_http(config.endpoint.starts_with("http://"))
            .build()
            .context("OCI 저장소 설정이 올바르지 않습니다")?;
        let store = Self { s3 };
        store.check_access(config).await?;
        Ok(store)
    }

    /// 녹화본 접두어 아래를 나열해 본다. 버킷이 없거나 키가 틀리면 실패한다.
    /// (없는 객체 조회는 버킷이 없을 때도 "없음"으로 답해 확인에 쓸 수 없다.)
    async fn check_access(&self, config: &OciStorageConfig) -> anyhow::Result<()> {
        let prefix = ObjectPath::from(Self::PREFIX);
        match self.s3.list_with_delimiter(Some(&prefix)).await {
            Ok(_) => Ok(()),
            Err(err) => Err(err).with_context(|| {
                format!(
                    "OCI 버킷 `{}`에 접근할 수 없습니다. OCI_S3_ENDPOINT, OCI_REGION, OCI_BUCKET, 키를 확인하세요",
                    config.bucket
                )
            }),
        }
    }

    fn path(&self, key: &str) -> ObjectPath {
        ObjectPath::from(format!("{}/{key}", Self::PREFIX))
    }

    async fn put<S>(&self, key: &str, stream: S) -> io::Result<u64>
    where
        S: Stream<Item = io::Result<Bytes>> + Send,
    {
        let upload = self
            .s3
            .put_multipart(&self.path(key))
            .await
            .map_err(io::Error::other)?;
        let mut writer = WriteMultipart::new_with_chunk_size(upload, Self::PART_BYTES);
        let mut written = 0;
        let mut stream = std::pin::pin!(stream);
        // 멀티파트 업로드는 끝내거나 중단하기 전까지 버킷에 객체로 보이지 않는다.
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(err) => {
                    let _ = writer.abort().await;
                    return Err(err);
                }
            };
            if let Err(err) = writer.wait_for_capacity(Self::MAX_CONCURRENT_PARTS).await {
                let _ = writer.abort().await;
                return Err(io::Error::other(err));
            }
            written += chunk.len() as u64;
            writer.put(chunk);
        }
        writer.finish().await.map_err(io::Error::other)?;
        Ok(written)
    }

    async fn read_range(&self, key: &str, range: ByteRange) -> anyhow::Result<Body> {
        let options = GetOptions {
            range: Some(GetRange::Bounded(range.start..range.start + range.len)),
            ..Default::default()
        };
        let result = self
            .s3
            .get_opts(&self.path(key), options)
            .await
            .with_context(|| format!("OCI에서 녹화본을 읽을 수 없습니다: {key}"))?;
        Ok(Body::from_stream(result.into_stream()))
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        match self.s3.delete(&self.path(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(err) => {
                Err(err).with_context(|| format!("OCI에서 녹화본을 지울 수 없습니다: {key}"))
            }
        }
    }
}
