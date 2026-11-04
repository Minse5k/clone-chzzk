use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::Context;

/// 서버 설정. 실행 시에는 환경변수에서 읽고, 테스트에서는 직접 만든다.
#[derive(Debug, Clone)]
pub struct Config {
    /// SQLite 파일 경로. 없으면 새로 만든다.
    pub database_path: PathBuf,
    /// local 저장소가 녹화본을 두는 디렉터리.
    pub storage_dir: PathBuf,
    /// 브라우저 호출을 허용할 웹 출처. `*`이면 모두 허용한다.
    pub cors_allowed_origins: Vec<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            database_path: env_or("DATABASE_PATH", "data/chzzk.db").into(),
            storage_dir: env_or("STORAGE_DIR", "data/recordings").into(),
            cors_allowed_origins: env_or("CORS_ALLOWED_ORIGINS", "http://localhost:8080")
                .split(',')
                .map(|origin| origin.trim().to_owned())
                .filter(|origin| !origin.is_empty())
                .collect(),
        })
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
