use super::{
    presets::{self, PresetInfo},
    store::{ProjectEntry, ProjectStore, StoreError},
};
use crate::{
    domain::{DomainError, ProjectId, Snapshot},
    lab::{EventKind, Lab},
    runner::ScenarioRunner,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("unknown preset {0:?}")]
    UnknownPreset(String),
    #[error("project {0} is the active one; switch to another project first")]
    Active(ProjectId),
    #[error("invalid project name: {0}")]
    Name(#[from] DomainError),
    #[error("{0}")]
    BadImport(String),
    /// A bug, not a condition the caller can do anything about; the details are in the log.
    #[error("the project switch failed unexpectedly, see the server log")]
    Internal,
}

/// Where a new project's content comes from, and the `from` field of `POST /api/projects`:
/// `{"preset": "dacha"}`, `{"copy_of": "default"}` or `{"snapshot": {…}}`, one key at most.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Template {
    #[default]
    Empty,
    Preset(String),
    CopyOf(ProjectId),
    /// An uploaded project file.
    #[serde(rename = "snapshot")]
    Import(Value),
}

/// Everything the project picker needs.
#[derive(Debug, Serialize)]
pub struct Listing {
    pub active: ProjectId,
    pub projects: Vec<ProjectEntry>,
    pub presets: Vec<PresetInfo>,
}

/// Creating, switching and removing projects, serialized by one lock.
pub struct ProjectService {
    store: Arc<ProjectStore>,
    lab: Arc<Lab>,
    runner: Arc<ScenarioRunner>,
    busy: Mutex<()>,
}

impl ProjectService {
    pub fn new(store: Arc<ProjectStore>, lab: Arc<Lab>, runner: Arc<ScenarioRunner>) -> Self {
        Self {
            store,
            lab,
            runner,
            busy: Mutex::new(()),
        }
    }

    pub async fn list(&self) -> Result<Listing, ProjectError> {
        let mut projects = self.store.list().await.map_err(StoreError::Io)?;
        // The active project's file can lag the live state; read the pair after the slow part so they match.
        let live = self.lab.persisted();
        for entry in projects.iter_mut().filter(|entry| entry.id == live.project) {
            entry.devices = live.snapshot.devices.len();
            entry.scenarios = live.snapshot.scenarios.len();
            entry.error = None;
        }
        Ok(Listing {
            active: live.project,
            projects,
            presets: presets::infos().to_vec(),
        })
    }

    /// Creates a project without switching to it.
    pub async fn create(&self, name: &str, source: Template) -> Result<ProjectId, ProjectError> {
        let id = ProjectId::from_name(name)?;
        let _busy = self.busy.lock().await;
        let snapshot = match source {
            Template::Empty => Snapshot::default(),
            Template::Preset(preset) => presets::find(&preset)
                .ok_or(ProjectError::UnknownPreset(preset))?
                .snapshot(),
            Template::CopyOf(from) => {
                let live = self.lab.persisted();
                if from == live.project {
                    live.snapshot
                } else {
                    self.store.load(&from).await?
                }
            }
            Template::Import(value) => parse_import(value)?,
        };
        self.store.create(&id, &snapshot).await?;
        self.lab.report(EventKind::ProjectsChanged);
        Ok(id)
    }

    /// Makes `id` the project the lab works on. The target is read and checked, and the project being
    /// left is saved, before anything is stopped, so a broken file or a failing disk changes nothing.
    /// Then runs are stopped, the contents swapped and the old project saved once more.
    ///
    /// It runs as a task of its own: a client that gives up halfway makes the server drop the handler,
    /// and that must not leave a half-done switch.
    pub async fn activate(self: &Arc<Self>, id: &ProjectId) -> Result<ProjectId, ProjectError> {
        let this = Arc::clone(self);
        let id = id.clone();
        match tokio::spawn(async move { this.switch_to(&id).await }).await {
            Ok(result) => result,
            Err(error) => {
                tracing::error!(%error, "the project switch task failed");
                Err(ProjectError::Internal)
            }
        }
    }

    async fn switch_to(&self, id: &ProjectId) -> Result<ProjectId, ProjectError> {
        let _busy = self.busy.lock().await;
        if *id == self.lab.project() {
            return Ok(id.clone());
        }
        let target = self.store.load(id).await?;

        // Before the runs are stopped: if this fails nothing has been touched yet.
        let leaving = self.lab.persisted();
        self.store
            .save(&leaving.project, &leaving.snapshot)
            .await
            .map_err(StoreError::Io)?;

        let _no_runs = self.runner.begin_switch().await;
        let (previous, last_state) = self.lab.replace(id.clone(), target);
        // Written even if nothing changed: a branch taken only sometimes is where a lost change would hide.
        if let Err(error) = self.store.save(&previous, &last_state).await {
            tracing::error!(%error, project = %previous, "the last changes of the previous project were not saved");
        }
        if let Err(error) = self.store.set_active(id).await {
            tracing::error!(%error, "could not remember the active project");
        }
        self.lab.report(EventKind::ProjectsChanged);
        Ok(id.clone())
    }

    pub async fn remove(&self, id: &ProjectId) -> Result<(), ProjectError> {
        let _busy = self.busy.lock().await;
        if *id == self.lab.project() {
            return Err(ProjectError::Active(id.clone()));
        }
        self.store.remove(id).await?;
        self.lab.report(EventKind::ProjectsChanged);
        Ok(())
    }

    /// A project as it would be saved (the live state for the active one). It does not wait for a switch:
    /// a project that just became active is read from its file, a few milliseconds behind at most.
    pub async fn export(&self, id: &ProjectId) -> Result<Snapshot, ProjectError> {
        let live = self.lab.persisted();
        if *id == live.project {
            return Ok(live.snapshot);
        }
        Ok(self.store.load(id).await?)
    }
}

/// An uploaded file must at least look like a project: an object with devices and/or scenarios.
/// Without this check any JSON object would silently become an empty project.
fn parse_import(value: Value) -> Result<Snapshot, ProjectError> {
    let looks_like_a_project = value
        .as_object()
        .is_some_and(|object| object.contains_key("devices") || object.contains_key("scenarios"));
    if !looks_like_a_project {
        return Err(ProjectError::BadImport(
            "this is not a project file: expected an object with \"devices\" and/or \"scenarios\""
                .to_owned(),
        ));
    }
    serde_json::from_value(value).map_err(|error| {
        ProjectError::BadImport(format!("this is not a valid project file: {error}"))
    })
}

#[cfg(test)]
mod tests;
