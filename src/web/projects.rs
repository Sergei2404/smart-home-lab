use super::{
    error::ApiError,
    extract::{ApiJson, ApiPath},
};
use crate::{
    domain::ProjectId,
    projects::service::{Listing, ProjectService, Template},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// `from` says where the content comes from (see `Template`); omitted means empty. Unknown keys are refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewProject {
    name: String,
    #[serde(default)]
    from: Template,
}

pub async fn list(State(projects): State<Arc<ProjectService>>) -> Result<Json<Listing>, ApiError> {
    Ok(Json(projects.list().await?))
}

pub async fn create(
    State(projects): State<Arc<ProjectService>>,
    ApiJson(body): ApiJson<NewProject>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let id = projects.create(&body.name, body.from).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

pub async fn activate(
    State(projects): State<Arc<ProjectService>>,
    ApiPath(id): ApiPath<ProjectId>,
) -> Result<Json<Value>, ApiError> {
    let active = projects.activate(&id).await?;
    Ok(Json(json!({ "active": active })))
}

pub async fn remove(
    State(projects): State<Arc<ProjectService>>,
    ApiPath(id): ApiPath<ProjectId>,
) -> Result<StatusCode, ApiError> {
    projects.remove(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The project as a downloadable file, in the same format the import accepts.
pub async fn export(
    State(projects): State<Arc<ProjectService>>,
    ApiPath(id): ApiPath<ProjectId>,
) -> Result<Response, ApiError> {
    let snapshot = projects.export(&id).await?;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{id}.json\""))
        .expect("a project id is plain ASCII");
    // Pretty-printed, like the files on disk: this is a file people open, edit and share.
    let body = serde_json::to_vec_pretty(&snapshot)
        .map_err(|error| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        body,
    )
        .into_response())
}
