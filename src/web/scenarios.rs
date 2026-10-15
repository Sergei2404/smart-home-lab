use super::{
    error::ApiError,
    extract::{ApiJson, ApiPath},
};
use crate::{
    domain::{Scenario, ScenarioId, Step},
    lab::Lab,
    runner::{RunInfo, ScenarioRunner},
};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioBody {
    name: String,
    #[serde(default)]
    repeat: bool,
    steps: Vec<Step>,
}

pub async fn list(State(lab): State<Arc<Lab>>) -> Json<Vec<Scenario>> {
    Json(lab.scenarios())
}

pub async fn create(
    State(lab): State<Arc<Lab>>,
    ApiJson(body): ApiJson<ScenarioBody>,
) -> Result<(StatusCode, Json<Scenario>), ApiError> {
    let scenario = lab.add_scenario(&body.name, body.repeat, body.steps)?;
    Ok((StatusCode::CREATED, Json(scenario)))
}

pub async fn update(
    State(lab): State<Arc<Lab>>,
    ApiPath(id): ApiPath<ScenarioId>,
    ApiJson(body): ApiJson<ScenarioBody>,
) -> Result<Json<Scenario>, ApiError> {
    Ok(Json(lab.update_scenario(
        &id,
        &body.name,
        body.repeat,
        body.steps,
    )?))
}

pub async fn remove(
    State(runner): State<Arc<ScenarioRunner>>,
    ApiPath(id): ApiPath<ScenarioId>,
) -> Result<StatusCode, ApiError> {
    runner.remove_scenario(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn run(
    State(runner): State<Arc<ScenarioRunner>>,
    ApiPath(id): ApiPath<ScenarioId>,
) -> Result<StatusCode, ApiError> {
    runner.run(&id)?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn stop(
    State(runner): State<Arc<ScenarioRunner>>,
    ApiPath(id): ApiPath<ScenarioId>,
) -> Result<StatusCode, ApiError> {
    runner.stop(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn runs(State(runner): State<Arc<ScenarioRunner>>) -> Json<Vec<RunInfo>> {
    Json(runner.runs())
}
