use monoize::error::AppError;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,monoize=debug")),
        )
        .json()
        .init();

    if let Err(err) = run().await {
        eprintln!("error: {}", err.message);
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AppError> {
    let state = monoize::app::load_state().await?;
    let app = monoize::app::build_app(state.clone());
    let addr: std::net::SocketAddr =
        state
            .runtime
            .listen
            .parse()
            .map_err(|err: std::net::AddrParseError| {
                AppError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "listen_invalid",
                    err.to_string(),
                )
            })?;
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "listen_failed",
            err.to_string(),
        )
    })?;
    tracing::info!("listening on {}", addr);
    axum::serve(listener, app).await.map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "serve_failed",
            err.to_string(),
        )
    })?;
    Ok(())
}
