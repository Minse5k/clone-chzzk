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
    let app = server::build_app(config).await?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("서버 시작: http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
