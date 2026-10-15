//! Checks applied to requests before a handler sees them.

use super::error::ApiError;
use crate::lab::Lab;
use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// The name of the project a page was showing when it sent the request.
const PROJECT_HEADER: &str = "x-project";

fn changes_state(request: &Request) -> bool {
    !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    )
}

/// Browsers attach `Origin` to cross-site requests. Run/stop are body-less POSTs, which a page on
/// any other site could fire without a CORS preflight, so changes must come from this very origin
/// (or from a client that sends no `Origin` at all, such as curl).
pub async fn same_origin(request: Request, next: Next) -> Response {
    if changes_state(&request)
        && let Some(origin) = request.headers().get(header::ORIGIN)
        && !origin_matches_host(origin, request.headers().get(header::HOST))
    {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "cross-origin requests are not allowed",
        )
        .into_response();
    }
    next.run(request).await
}

/// Devices and scenarios are addressed by id alone and two projects may both have a `lamp`, so a page
/// that missed a switch could change the wrong project. The UI names the project it shows in
/// `X-Project`; a change that names another one than the one running gets 409. Requests without the
/// header (curl, scripts) pass. It guards stale pages, not a switch starting a microsecond later.
/// Only the routes addressed by id are wrapped in it (see `router`).
pub async fn same_project(State(lab): State<Arc<Lab>>, request: Request, next: Next) -> Response {
    if changes_state(&request)
        && let Some(shown) = request.headers().get(PROJECT_HEADER)
    {
        let running = lab.project();
        if shown.as_bytes() != running.as_str().as_bytes() {
            return ApiError::new(
                StatusCode::CONFLICT,
                format!(
                    "this page was showing another project than the one that is running ({running}); it has been refreshed, please try again"
                ),
            )
            .into_response();
        }
    }
    next.run(request).await
}

fn origin_matches_host(origin: &HeaderValue, host: Option<&HeaderValue>) -> bool {
    let (Ok(origin), Some(Ok(host))) = (origin.to_str(), host.map(HeaderValue::to_str)) else {
        return false;
    };
    origin
        .split_once("://")
        .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host))
}
