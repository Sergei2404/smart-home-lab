mod devices;
mod error;
mod events;
mod extract;
mod guards;
mod projects;
mod scenarios;

use crate::{app::App, lab::Lab, projects::service::ProjectService, runner::ScenarioRunner};
use axum::{
    Router,
    extract::{DefaultBodyLimit, FromRef},
    http::{StatusCode, header},
    middleware,
    response::{Html, IntoResponse},
    routing::{delete, get, post, put},
};
use error::ApiError;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// The largest request body (axum's default, made explicit); a project at its limits fits, more gets 413.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

const INDEX_HTML: &str = include_str!("../static/index.html");
const APP_JS: &str = include_str!("../static/app.js");
const STYLE_CSS: &str = include_str!("../static/style.css");

impl FromRef<App> for Arc<ProjectService> {
    fn from_ref(app: &App) -> Self {
        app.projects.clone()
    }
}

impl FromRef<App> for Arc<Lab> {
    fn from_ref(app: &App) -> Self {
        app.lab.clone()
    }
}

impl FromRef<App> for Arc<ScenarioRunner> {
    fn from_ref(app: &App) -> Self {
        app.runner.clone()
    }
}

impl FromRef<App> for CancellationToken {
    fn from_ref(app: &App) -> Self {
        app.shutdown.clone()
    }
}

pub fn router(app: App) -> Router {
    // The routes that address devices and scenarios by id: a page has to show the running project.
    let by_id = Router::new()
        .route("/api/devices", get(devices::list).post(devices::create))
        .route("/api/devices/{id}", delete(devices::remove))
        .route("/api/devices/{id}/state", put(devices::set_state))
        .route(
            "/api/devices/{id}/availability",
            put(devices::set_availability),
        )
        .route(
            "/api/scenarios",
            get(scenarios::list).post(scenarios::create),
        )
        .route(
            "/api/scenarios/{id}",
            put(scenarios::update).delete(scenarios::remove),
        )
        .route("/api/scenarios/{id}/run", post(scenarios::run))
        .route("/api/scenarios/{id}/stop", post(scenarios::stop))
        .route("/api/runs", get(scenarios::runs))
        .route_layer(middleware::from_fn_with_state(
            app.lab.clone(),
            guards::same_project,
        ));

    Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route(
            "/app.js",
            get(|| async { asset("text/javascript; charset=utf-8", APP_JS) }),
        )
        .route(
            "/style.css",
            get(|| async { asset("text/css; charset=utf-8", STYLE_CSS) }),
        )
        .route("/health", get(|| async { StatusCode::OK }))
        .merge(by_id)
        .route("/api/projects", get(projects::list).post(projects::create))
        .route("/api/projects/{id}", delete(projects::remove))
        .route("/api/projects/{id}/activate", post(projects::activate))
        .route("/api/projects/{id}/export", get(projects::export))
        .route("/api/events", get(events::stream))
        .route("/api/log", get(events::log))
        .route("/api/status", get(events::status))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "no such endpoint") })
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "this method is not allowed for this endpoint",
            )
        })
        .layer(middleware::from_fn(guards::same_origin))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(app)
}

/// Serves until `app.shutdown` is cancelled, then lets requests in flight finish.
pub async fn serve(listener: TcpListener, app: App) -> std::io::Result<()> {
    let shutdown = app.shutdown.clone();
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
}

fn asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, content_type)], body)
}
