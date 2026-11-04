use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::Context;

/// 서버 설정. 실행 시에는 환경변수에서 읽고, 테스트에서는 직접 만든다.
#[derive(Debug, Clone)]
pub struct Config {
    /// SQLite 파일 경로. 없으면 새로 만든다.
    pub database_path: PathBuf,
    /// 녹화본 저장소.
    pub storage: StorageConfig,
    /// 브라우저 호출을 허용할 웹 출처. `*`이면 모두 허용한다.
    pub cors_allowed_origins: Vec<String>,
    /// 녹화본 하나의 최대 크기.
    pub max_upload_bytes: u64,
    /// 저장된 녹화본 크기 합의 한도. OCI 무료 한도(20GB)를 넘지 않게 여유를 둔다.
    pub storage_quota_bytes: u64,
}

/// 녹화본 저장소 백엔드. `STORAGE_BACKEND`로 고른다(기본 `local`).
#[derive(Debug, Clone)]
pub enum StorageConfig {
    /// 서버 로컬 디렉터리(`STORAGE_DIR`). 비밀 값 없이 실행된다.
    Local { dir: PathBuf },
    /// OCI Object Storage의 비공개 버킷. S3 호환 API로 접근한다.
    Oci(OciStorageConfig),
}

/// OCI Object Storage S3 호환 API 설정.
#[derive(Clone)]
pub struct OciStorageConfig {
    /// 예: `https://{namespace}.compat.objectstorage.ap-osaka-1.oraclecloud.com`
    pub endpoint: String,
    /// 예: `ap-osaka-1`
    pub region: String,
    pub bucket: String,
    /// Customer Secret Key의 Access Key.
    pub access_key_id: String,
    /// Customer Secret Key의 Secret Key.
    pub secret_access_key: String,
}

impl std::fmt::Debug for OciStorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OciStorageConfig")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key_id", &"<숨김>")
            .field("secret_access_key", &"<숨김>")
            .finish()
    }
}

impl StorageConfig {
    fn from_env() -> anyhow::Result<Self> {
        match env_or("STORAGE_BACKEND", "local").trim() {
            "local" => Ok(Self::Local { dir: env_or("STORAGE_DIR", "data/recordings").into() }),
            "oci" => {
                let mut missing = Vec::new();
                let mut required = |key: &'static str| {
                    let value = env::var(key).unwrap_or_default().trim().to_owned();
                    if value.is_empty() {
                        missing.push(key);
                    }
                    value
                };
                let config = OciStorageConfig {
                    endpoint: required("OCI_S3_ENDPOINT"),
                    region: required("OCI_REGION"),
                    bucket: required("OCI_BUCKET"),
                    access_key_id: required("OCI_ACCESS_KEY_ID"),
                    secret_access_key: required("OCI_SECRET_ACCESS_KEY"),
                };
                if !missing.is_empty() {
                    anyhow::bail!(
                        "STORAGE_BACKEND=oci에 필요한 환경변수가 없습니다: {}",
                        missing.join(", ")
                    );
                }
                Ok(Self::Oci(config))
            }
            other => anyhow::bail!("STORAGE_BACKEND는 local 또는 oci여야 합니다: {other}"),
        }
    }
}

const GIB: u64 = 1024 * 1024 * 1024;

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_path: env_or("DATABASE_PATH", "data/chzzk.db").into(),
            storage: StorageConfig::from_env()?,
            cors_allowed_origins: env_or("CORS_ALLOWED_ORIGINS", "http://localhost:8080")
                .split(',')
                .map(|origin| origin.trim().to_owned())
                .filter(|origin| !origin.is_empty())
                .collect(),
            max_upload_bytes: env_bytes("MAX_UPLOAD_BYTES", 2 * GIB)?,
            storage_quota_bytes: env_bytes("STORAGE_QUOTA_BYTES", 15 * GIB)?,
        })
    }
}

fn env_bytes(key: &str, default: u64) -> anyhow::Result<u64> {
    match env::var(key) {
        Ok(value) => value
            .trim()
            .parse()
            .with_context(|| format!("{key}는 바이트 단위 정수여야 합니다: {value}")),
        Err(_) => Ok(default),
    }
}

/// 서버가 바인딩할 주소. `SERVER_ADDR`, 기본값 `0.0.0.0:3000`.
pub fn server_addr_from_env() -> anyhow::Result<SocketAddr> {
    env_or("SERVER_ADDR", "0.0.0.0:3000")
        .parse()
        .context("SERVER_ADDR는 `호스트:포트` 형식이어야 합니다")
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_owned())
}
