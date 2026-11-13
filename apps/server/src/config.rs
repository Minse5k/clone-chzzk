use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

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
    /// 빌드된 웹 정적 파일 디렉터리. 지정하면 서버가 웹 화면도 함께 서빙한다(배포 시).
    pub web_dist_dir: Option<PathBuf>,
    /// 라이브 방송의 HLS 조각을 만드는 디렉터리. 방송마다 하위 디렉터리를 쓴다.
    pub live_dir: PathBuf,
    /// 송출을 HLS로 바꾸는 ffmpeg 실행 파일.
    pub ffmpeg_path: PathBuf,
    /// 동시에 진행할 수 있는 라이브 방송 수. 송출마다 ffmpeg가 CPU로 인코딩하므로 VM에 맞춘다.
    pub max_live_broadcasts: usize,
    /// 송출 중에 녹화 조각이 이 시간 동안 오지 않으면 방송을 종료한다.
    pub live_idle_timeout: Duration,
    /// 로그인 토큰(JWT)의 서명 키. 비우면 서버를 시작할 때마다 임시 키를 만든다(재시작하면 모두 로그아웃).
    pub jwt_secret: Option<JwtSecret>,
    /// 로그인 토큰의 유효 시간.
    pub jwt_ttl: Duration,
}

/// 로그인 토큰 서명 키. 로그나 디버그 출력에 값이 드러나지 않게 감싼다.
#[derive(Clone)]
pub struct JwtSecret(pub String);

impl std::fmt::Debug for JwtSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JwtSecret(<숨김>)")
    }
}

impl From<&str> for JwtSecret {
    fn from(secret: &str) -> Self {
        Self(secret.to_owned())
    }
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
            "local" => Ok(Self::Local {
                dir: env_or("STORAGE_DIR", "data/recordings").into(),
            }),
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
            web_dist_dir: env::var("WEB_DIST_DIR")
                .ok()
                .filter(|dir| !dir.trim().is_empty())
                .map(PathBuf::from),
            live_dir: env_or("LIVE_DIR", "data/live").into(),
            ffmpeg_path: env_or("FFMPEG_PATH", "ffmpeg").into(),
            max_live_broadcasts: env_number("MAX_LIVE_BROADCASTS", 2)?,
            live_idle_timeout: Duration::from_secs(env_number("LIVE_IDLE_TIMEOUT_SECS", 20)?),
            jwt_secret: env::var("JWT_SECRET")
                .ok()
                .map(|secret| secret.trim().to_owned())
                .filter(|secret| !secret.is_empty())
                .map(JwtSecret),
            jwt_ttl: Duration::from_secs(env_number("JWT_TTL_SECS", 24 * 60 * 60)?),
        })
    }
}

fn env_number<T: std::str::FromStr>(key: &str, default: T) -> anyhow::Result<T> {
    match env::var(key) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("{key}는 0 이상의 정수여야 합니다: {value}")),
        Err(_) => Ok(default),
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
