use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let config = server::Config::from_env()?;
    let addr = server::server_addr_from_env()?;
    let instance = config.instance_id.clone();
    let app = server::build(config).await?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%instance, "서버 시작: http://{addr}");
    let router = app.router.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            app.prepare_shutdown(SHUTDOWN_TIMEOUT).await;
        })
        .await?;
    tracing::info!(%instance, "서버 종료");
    Ok(())
}

/// 종료 신호를 받은 뒤 송출 마무리를 기다리는 최대 시간. compose의 stop_grace_period보다 짧게 둔다.
const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// SIGTERM(docker stop, 스케일 인) 또는 Ctrl+C.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::warn!("SIGTERM을 받을 수 없습니다: {err}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
