use crate::{
    lab::LabError,
    projects::{service::ProjectError, store::StoreError},
    runner::RunError,
};
use axum::{
    Json,
    extract::rejection::{JsonRejection, PathRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

/// Every error leaves the API as `{"error": "..."}`, axum's own rejections included.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl From<LabError> for ApiError {
    fn from(error: LabError) -> Self {
        let status = match error {
            LabError::NotFound { .. } => StatusCode::NOT_FOUND,
            LabError::AlreadyExists { .. }
            | LabError::LimitReached { .. }
            | LabError::DeviceInUse { .. } => StatusCode::CONFLICT,
            LabError::UnknownDevice(_) | LabError::Invalid(_) => StatusCode::BAD_REQUEST,
        };
        Self::new(status, error.to_string())
    }
}

impl From<RunError> for ApiError {
    fn from(error: RunError) -> Self {
        match error {
            RunError::Scenario(error) => error.into(),
            conflict @ (RunError::AlreadyRunning | RunError::ProjectSwitching) => {
                Self::new(StatusCode::CONFLICT, conflict.to_string())
            }
        }
    }
}

impl From<ProjectError> for ApiError {
    fn from(error: ProjectError) -> Self {
        let status = match &error {
            ProjectError::Store(StoreError::NotFound(_)) => StatusCode::NOT_FOUND,
            ProjectError::Store(StoreError::AlreadyExists(_)) | ProjectError::Active(_) => {
                StatusCode::CONFLICT
            }
            ProjectError::Store(StoreError::Unusable { .. })
            | ProjectError::UnknownPreset(_)
            | ProjectError::Name(_)
            | ProjectError::BadImport(_) => StatusCode::BAD_REQUEST,
            ProjectError::Store(StoreError::Io(_)) | ProjectError::Internal => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        // An I/O error can contain paths and OS details: keep them in the log, not in the answer.
        let message = match &error {
            ProjectError::Store(StoreError::Io(source)) => {
                tracing::error!(error = ?source, "project storage failed");
                "the project storage failed, see the server log".to_owned()
            }
            other => other.to_string(),
        };
        Self::new(status, message)
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        // A body that parses but does not fit is a plain bad request, like every other invalid input.
        let status = match rejection.status() {
            StatusCode::UNPROCESSABLE_ENTITY => StatusCode::BAD_REQUEST,
            other => other,
        };
        Self::new(status, rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        Self::new(rejection.status(), rejection.body_text())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
