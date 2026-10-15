use anyhow::Context;
use smart_home_lab::{
    app::App,
    domain::ProjectId,
    lab::Lab,
    mqtt,
    projects::{persister::spawn_persister, store::ProjectStore},
    settings::Settings,
    web,
};
use std::{io::IsTerminal, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

/// How long open requests get to finish when the process is asked to stop.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let settings = Settings::load().context(
        "invalid configuration (check config.toml, config.local.toml and SHL_* environment variables)",
    )?;

    let store = Arc::new(ProjectStore::open(&settings.storage.dir).await?);
    let explicit = settings
        .storage
        .project
        .as_deref()
        .map(ProjectId::parse)
        .transpose()
        .context("invalid storage.project")?;
    let (project, snapshot) = store.bootstrap(explicit).await?;
    tracing::info!(
        %project,
        dir = %store.dir().display(),
        devices = snapshot.devices.len(),
        scenarios = snapshot.scenarios.len(),
        "project loaded"
    );

    let bind = settings.server.bind;
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind HTTP server to {bind}"))?;
    let address = listener.local_addr()?;

    let lab = Arc::new(Lab::with_project(project, snapshot));
    let stop_persister = CancellationToken::new();
    let persister = spawn_persister(store.clone(), lab.snapshots(), stop_persister.clone());

    let mqtt = if settings.mqtt.enabled {
        Some(mqtt::start(&settings.mqtt, lab.clone()).context("failed to start MQTT bridge")?)
    } else {
        tracing::info!("MQTT is disabled (set mqtt.enabled = true in config.toml to turn it on)");
        None
    };

    let app = App::new(lab, store);
    let shutdown = app.shutdown.clone();
    tracing::info!("server started, open http://{address} in a browser");
    let server = web::serve(listener, app);
    tokio::pin!(server);
    let outcome = tokio::select! {
        result = &mut server => result.context("HTTP server failed"),
        () = shutdown_signal() => {
            tracing::info!("shutting down");
            // Streams end, requests in flight finish; what is still open after the grace period is cut off.
            shutdown.cancel();
            match tokio::time::timeout(SHUTDOWN_GRACE, &mut server).await {
                Ok(result) => result.context("HTTP server failed"),
                Err(_) => {
                    tracing::warn!("connections still open after {SHUTDOWN_GRACE:?}, closing them");
                    Ok(())
                }
            }
        }
    };

    if let Some(mqtt) = mqtt {
        mqtt.shutdown().await;
    }
    // The persister writes the latest state once more and only then finishes.
    stop_persister.cancel();
    if let Err(error) = persister.await {
        tracing::error!(%error, "the state writer crashed");
    }
    outcome
}

/// Resolves on Ctrl-C and, on Unix, on SIGTERM (`docker stop`, `kill`, systemd).
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
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
