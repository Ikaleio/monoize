use monoize::error::AppError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    let is_replica = state.node.is_replica();
    state.user_store.spawn_background_tasks_for_role(is_replica);

    if !is_replica {
        monoize::price_sync::spawn_auto_sync_scheduler(
            state.http.clone(),
            state.model_price_store.clone(),
            state.model_registry_store.clone(),
            state.settings_store.clone(),
            state.background_shutdown.clone(),
        );
    }

    if !is_replica {
        // PRP11: retention/pending-log deletion is a primary responsibility.
        match state.user_store.cleanup_pending_request_logs().await {
            Ok(n) if n > 0 => tracing::info!(count = n, "cleaned up stale pending request logs"),
            Ok(_) => {}
            Err(e) => tracing::warn!("failed to cleanup pending request logs: {e}"),
        }

        match state.user_store.cleanup_expired_request_logs().await {
            Ok(n) if n > 0 => tracing::info!(count = n, "cleaned up expired request logs"),
            Ok(_) => {}
            Err(e) => tracing::warn!("failed to cleanup expired request logs: {e}"),
        }
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

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(state.background_shutdown.clone()))
    .await
    .map_err(|err| {
        AppError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "serve_failed",
            err.to_string(),
        )
    })?;

    let terminal_tasks = state.request_log_tasks.active_count();
    if terminal_tasks > 0 {
        tracing::info!(terminal_tasks, "waiting for terminal request-log tasks");
    }
    state.request_log_tasks.wait_for_idle().await;
    if is_replica {
        // M6: one best-effort shipment attempt; durable spool covers the rest.
        if let Some(metering) = state.metering.as_ref() {
            metering
                .final_ship(
                    &state.user_store.request_log_batcher_clone(),
                    &state.user_store.last_used_batcher_clone(),
                )
                .await;
        }
    } else {
        state.user_store.flush_all_batchers().await;
    }

    if !is_replica {
        match state.user_store.cleanup_pending_request_logs().await {
            Ok(n) if n > 0 => {
                tracing::info!(count = n, "finalized pending request logs on shutdown")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("failed to cleanup pending request logs on shutdown: {e}"),
        }
    }

    Ok(())
}

async fn shutdown_signal(background_shutdown: Arc<AtomicBool>) {
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

    let signal = tokio::select! {
        _ = ctrl_c => "SIGINT",
        _ = terminate => "SIGTERM",
    };
    background_shutdown.store(true, Ordering::Release);
    tracing::info!(signal, "received shutdown signal");
}
