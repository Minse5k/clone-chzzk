use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Context;

/// 서버 설정. 실행 시에는 환경변수에서 읽고, 테스트에서는 직접 만든다.
#[derive(Debug, Clone)]
pub struct Config {
    /// PostgreSQL 연결 주소. 모든 서버 인스턴스가 같은 DB를 쓴다.
    pub database_url: ConnectionUrl,
    /// 인스턴스 하나가 여는 DB 연결 수의 상한.
    pub database_max_connections: u32,
    /// 인스턴스 사이에 알림을 전하는 Redis 주소. 없으면 인스턴스 하나로 보고 프로세스 안에서만 전한다.
    pub redis_url: Option<ConnectionUrl>,
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
    /// 서비스 전체(모든 인스턴스 합)에서 동시에 진행할 수 있는 라이브 방송 수.
    /// 송출마다 ffmpeg가 CPU로 인코딩하므로 머신에 맞춘다.
    pub max_live_broadcasts: usize,
    /// 이 서버 인스턴스의 이름. 라이브 방송을 어느 인스턴스가 송출받는지 기록할 때 쓴다.
    pub instance_id: String,
    /// 송출 중인 인스턴스가 하트비트를 갱신하고, 끊긴 라이브 방송을 찾는 주기.
    pub live_heartbeat_interval: Duration,
    /// 하트비트가 이만큼 끊긴 라이브 방송은 송출받던 인스턴스가 죽은 것으로 보고 종료한다.
    pub live_heartbeat_timeout: Duration,
    /// 송출 중에 녹화 조각이 이 시간 동안 오지 않으면 방송을 종료한다.
    pub live_idle_timeout: Duration,
    /// 로그인 토큰(JWT)의 서명 키. 비우면 서버를 시작할 때마다 임시 키를 만든다(재시작하면 모두 로그아웃).
    pub jwt_secret: Option<JwtSecret>,
    /// 로그인 토큰의 유효 시간.
    pub jwt_ttl: Duration,
    /// 후원 결제에 쓰는 토스페이먼츠 테스트 키. 없으면 후원만 막고 나머지 기능은 그대로 쓴다.
    pub toss: Option<TossConfig>,
}

/// 토스페이먼츠 설정. 테스트 키(`test_`로 시작)만 쓴다.
#[derive(Clone)]
pub struct TossConfig {
    /// 결제창 SDK를 초기화하는 클라이언트 키. 웹에 내려 준다.
    pub client_key: String,
    /// 결제 승인·조회 API를 부르는 시크릿 키. 서버에만 둔다.
    pub secret_key: String,
    /// 결제 API 주소. 기본 `https://api.tosspayments.com`, 테스트에서는 가짜 결제 서버 주소.
    pub api_base: String,
}

impl std::fmt::Debug for TossConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TossConfig")
            .field("client_key", &self.client_key)
            .field("secret_key", &"<숨김>")
            .field("api_base", &self.api_base)
            .finish()
    }
}

impl TossConfig {
    fn from_env() -> anyhow::Result<Option<Self>> {
        let client_key = env::var("TOSS_CLIENT_KEY")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let secret_key = env::var("TOSS_SECRET_KEY")
            .unwrap_or_default()
            .trim()
            .to_owned();
        match (client_key.is_empty(), secret_key.is_empty()) {
            (true, true) => Ok(None),
            (false, false) => Ok(Some(Self {
                client_key,
                secret_key,
                api_base: env_or("TOSS_API_BASE", "https://api.tosspayments.com"),
            })),
            _ => anyhow::bail!("TOSS_CLIENT_KEY와 TOSS_SECRET_KEY는 함께 지정해야 합니다"),
        }
    }
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

/// DB·Redis 연결 주소. 비밀번호가 들어 있을 수 있으므로 로그나 디버그 출력에 드러나지 않게 감싼다.
#[derive(Clone)]
pub struct ConnectionUrl(pub String);

impl std::fmt::Debug for ConnectionUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectionUrl(<숨김>)")
    }
}

impl From<&str> for ConnectionUrl {
    fn from(url: &str) -> Self {
        Self(url.to_owned())
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
            database_url: env::var("DATABASE_URL")
                .ok()
                .map(|url| url.trim().to_owned())
                .filter(|url| !url.is_empty())
                .map(ConnectionUrl)
                .context(
                    "DATABASE_URL이 필요합니다 (예: postgres://chzzk:<비밀번호>@localhost:5432/chzzk)",
                )?,
            database_max_connections: env_number("DATABASE_MAX_CONNECTIONS", 10)?,
            redis_url: env::var("REDIS_URL")
                .ok()
                .map(|url| url.trim().to_owned())
                .filter(|url| !url.is_empty())
                .map(ConnectionUrl),
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
            instance_id: instance_id_from_env(),
            live_heartbeat_interval: Duration::from_secs(env_number(
                "LIVE_HEARTBEAT_INTERVAL_SECS",
                5,
            )?),
            live_heartbeat_timeout: Duration::from_secs(env_number(
                "LIVE_HEARTBEAT_TIMEOUT_SECS",
                30,
            )?),
            live_idle_timeout: Duration::from_secs(env_number("LIVE_IDLE_TIMEOUT_SECS", 20)?),
            jwt_secret: env::var("JWT_SECRET")
                .ok()
                .map(|secret| secret.trim().to_owned())
                .filter(|secret| !secret.is_empty())
                .map(JwtSecret),
            jwt_ttl: Duration::from_secs(env_number("JWT_TTL_SECS", 24 * 60 * 60)?),
            toss: TossConfig::from_env()?,
        })
    }
}

/// `INSTANCE_ID`, 없으면 호스트 이름(컨테이너에서는 컨테이너 ID), 그것도 없으면 무작위 이름.
fn instance_id_from_env() -> String {
    ["INSTANCE_ID", "HOSTNAME"]
        .iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| format!("instance-{}", uuid::Uuid::now_v7().simple()))
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
