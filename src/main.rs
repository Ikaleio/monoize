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

    match state.user_store.cleanup_pending_request_logs().await {
        Ok(n) if n > 0 => tracing::info!(count = n, "cleaned up stale pending request logs"),
        Ok(_) => {}
        Err(e) => tracing::warn!("failed to cleanup pending request logs: {e}"),
    }

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

    let shutdown_state = state.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_state))
        .await
        .map_err(|err| {
            AppError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "serve_failed",
                err.to_string(),
            )
        })?;

    match state.user_store.cleanup_pending_request_logs().await {
        Ok(n) if n > 0 => tracing::info!(count = n, "finalized pending request logs on shutdown"),
        Ok(_) => {}
        Err(e) => tracing::warn!("failed to cleanup pending request logs on shutdown: {e}"),
    }

    Ok(())
}

async fn shutdown_signal(state: monoize::app::AppState) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl+c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => { tracing::info!("received SIGINT, shutting down"); }
        _ = terminate => { tracing::info!("received SIGTERM, shutting down"); }
    }

    match state.user_store.cleanup_pending_request_logs().await {
        Ok(n) if n > 0 => {
            tracing::info!(count = n, "finalized in-flight pending request logs");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("failed to cleanup pending request logs: {e}"),
    }
}
