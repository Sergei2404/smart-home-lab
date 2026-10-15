use super::{
    error::ApiError,
    extract::{ApiJson, ApiPath},
};
use crate::{
    domain::{Device, DeviceId, DeviceKind, State as DeviceState},
    lab::{Lab, Source},
};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use std::sync::Arc;

// Request bodies refuse unknown keys: a misspelled optional key would otherwise be silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewDevice {
    name: String,
    kind: DeviceKind,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewState {
    state: DeviceState,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewAvailability {
    online: bool,
}

pub async fn list(State(lab): State<Arc<Lab>>) -> Json<Vec<Device>> {
    Json(lab.devices())
}

pub async fn create(
    State(lab): State<Arc<Lab>>,
    ApiJson(body): ApiJson<NewDevice>,
) -> Result<(StatusCode, Json<Device>), ApiError> {
    let device = lab.add_device(&body.name, body.kind)?;
    Ok((StatusCode::CREATED, Json(device)))
}

pub async fn remove(
    State(lab): State<Arc<Lab>>,
    ApiPath(id): ApiPath<DeviceId>,
) -> Result<StatusCode, ApiError> {
    lab.remove_device(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_state(
    State(lab): State<Arc<Lab>>,
    ApiPath(id): ApiPath<DeviceId>,
    ApiJson(body): ApiJson<NewState>,
) -> Result<Json<Device>, ApiError> {
    Ok(Json(lab.set_state(&id, body.state, Source::Ui)?))
}

pub async fn set_availability(
    State(lab): State<Arc<Lab>>,
    ApiPath(id): ApiPath<DeviceId>,
    ApiJson(body): ApiJson<NewAvailability>,
) -> Result<Json<Device>, ApiError> {
    Ok(Json(lab.set_online(&id, body.online, Source::Ui)?))
}
