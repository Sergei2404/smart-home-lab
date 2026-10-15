use super::file::{LoadError, ProjectFile};
use crate::domain::{ProjectId, Snapshot};
use anyhow::Context;
use serde::Serialize;
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tokio::sync::Mutex;

const ACTIVE_FILE: &str = ".active";
const EXTENSION: &str = "json";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("project {0} not found")]
    NotFound(ProjectId),
    #[error("project {0} already exists")]
    AlreadyExists(ProjectId),
    /// The file's content breaks the rules the API enforces. `reason` never contains the path.
    #[error("project {id} cannot be used: {reason}")]
    Unusable { id: ProjectId, reason: String },
    #[error(transparent)]
    Io(#[from] anyhow::Error),
}

/// One row of the project list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectEntry {
    pub id: ProjectId,
    pub devices: usize,
    pub scenarios: usize,
    /// Set when the file cannot be used, so the user can see it and delete it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A folder of project files, `<dir>/<id>.json`, and a marker `<dir>/.active` naming the project to
/// open next time. All writes take one lock, so the persister and the project service never write
/// the same temporary file at once.
pub struct ProjectStore {
    dir: PathBuf,
    write: Mutex<()>,
}

impl ProjectStore {
    pub async fn open(dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let dir = dir.into();
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("failed to create the projects folder {}", dir.display()))?;
        Ok(Self {
            dir,
            write: Mutex::new(()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the file of a project is (for logs and start-up messages, never for API answers).
    pub fn path_of(&self, id: &ProjectId) -> PathBuf {
        self.dir.join(format!("{id}.{EXTENSION}"))
    }

    fn file(&self, id: &ProjectId) -> ProjectFile {
        ProjectFile::new(self.path_of(id))
    }

    fn active_path(&self) -> PathBuf {
        self.dir.join(ACTIVE_FILE)
    }

    /// The ids of the project files in alphabetical order, without reading them. Files whose name is
    /// not a valid project id (notes, backups, temporary files) and folders are ignored.
    async fn ids(&self) -> anyhow::Result<Vec<ProjectId>> {
        let mut ids = Vec::new();
        let mut dir = tokio::fs::read_dir(&self.dir)
            .await
            .with_context(|| format!("failed to read {}", self.dir.display()))?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| ProjectId::parse(stem).ok())
            else {
                continue;
            };
            // `metadata` follows symbolic links, so a linked project file is still a file.
            if tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_file())
            {
                ids.push(id);
            }
        }
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(ids)
    }

    /// The project files, sorted; one that cannot be used is listed with its error so it can be deleted.
    pub async fn list(&self) -> anyhow::Result<Vec<ProjectEntry>> {
        let mut entries = Vec::new();
        for id in self.ids().await? {
            entries.push(match self.load(&id).await {
                Ok(snapshot) => ProjectEntry {
                    devices: snapshot.devices.len(),
                    scenarios: snapshot.scenarios.len(),
                    error: None,
                    id,
                },
                // Deleted since the folder was read.
                Err(StoreError::NotFound(_)) => continue,
                Err(StoreError::Unusable { reason, .. }) => ProjectEntry {
                    devices: 0,
                    scenarios: 0,
                    error: Some(reason),
                    id,
                },
                Err(error) => {
                    tracing::warn!(%error, project = %id, "a project file could not be read");
                    ProjectEntry {
                        devices: 0,
                        scenarios: 0,
                        error: Some("the file could not be read, see the server log".to_owned()),
                        id,
                    }
                }
            });
        }
        Ok(entries)
    }

    /// Reads and validates a project. A project that does not exist is `NotFound`, never an empty one.
    pub async fn load(&self, id: &ProjectId) -> Result<Snapshot, StoreError> {
        let file = self.file(id);
        file.load().await.map_err(|error| match error {
            LoadError::Missing => StoreError::NotFound(id.clone()),
            LoadError::Read(source) => StoreError::Io(
                anyhow::Error::new(source)
                    .context(format!("failed to read {}", file.path().display())),
            ),
            LoadError::Invalid(reason) => StoreError::Unusable {
                id: id.clone(),
                reason,
            },
        })
    }

    /// Writes (or replaces) a project file atomically.
    pub async fn save(&self, id: &ProjectId, snapshot: &Snapshot) -> anyhow::Result<()> {
        let _write = self.write.lock().await;
        self.file(id).save(snapshot).await
    }

    /// Writes what `latest` returns to the file of the project it names. `latest` runs only once the
    /// write lock is held, so a write that waited behind another (the final save of a project just left)
    /// writes what is newest by then and never puts an older state over a newer one.
    pub async fn save_latest(
        &self,
        latest: impl FnOnce() -> (ProjectId, Snapshot),
    ) -> anyhow::Result<()> {
        let _write = self.write.lock().await;
        let (project, snapshot) = latest();
        self.file(&project)
            .save(&snapshot)
            .await
            .with_context(|| format!("failed to save project {project}"))
    }

    /// Creates a new project; refuses to overwrite an existing one and to write invalid data.
    pub async fn create(&self, id: &ProjectId, snapshot: &Snapshot) -> Result<(), StoreError> {
        snapshot.validate().map_err(|error| StoreError::Unusable {
            id: id.clone(),
            reason: error.to_string(),
        })?;
        let _write = self.write.lock().await;
        let file = self.file(id);
        let exists = tokio::fs::try_exists(file.path())
            .await
            .with_context(|| format!("failed to look for {}", file.path().display()))?;
        if exists {
            return Err(StoreError::AlreadyExists(id.clone()));
        }
        file.save(snapshot).await?;
        Ok(())
    }

    pub async fn remove(&self, id: &ProjectId) -> Result<(), StoreError> {
        let _write = self.write.lock().await;
        let path = self.path_of(id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                Err(StoreError::NotFound(id.clone()))
            }
            Err(error) => Err(anyhow::Error::new(error)
                .context(format!("failed to delete {}", path.display()))
                .into()),
        }
    }

    /// The project to open on the next start; `None` if nothing was recorded or the record is garbage.
    pub async fn active(&self) -> Option<ProjectId> {
        let text = tokio::fs::read_to_string(self.active_path()).await.ok()?;
        ProjectId::parse(text.trim()).ok()
    }

    pub async fn set_active(&self, id: &ProjectId) -> anyhow::Result<()> {
        let _write = self.write.lock().await;
        let tmp = self.dir.join(format!("{ACTIVE_FILE}.tmp"));
        tokio::fs::write(&tmp, id.as_str())
            .await
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        tokio::fs::rename(&tmp, self.active_path())
            .await
            .context("failed to record the active project")
    }

    /// What has to happen before the lab starts: a project exists, one is chosen (see `open_active`) and
    /// recorded, and the folder is writable, so a read-only disk shows now and not on the first change.
    /// The project file is only read: rewriting it would drop what this version does not know.
    pub async fn bootstrap(
        &self,
        explicit: Option<ProjectId>,
    ) -> anyhow::Result<(ProjectId, Snapshot)> {
        self.ensure_default().await?;
        let (project, snapshot) = self
            .open_active(explicit)
            .await
            .map_err(|error| self.startup_error(error))?;
        // Files are replaced by renaming, so it is the folder that has to be writable, not the file.
        self.set_active(&project)
            .await
            .context("the projects folder is not writable")?;
        Ok((project, snapshot))
    }

    /// Creates an empty `default` when no project can be used and there is no `default` file. A file that
    /// cannot be used is never replaced (it stays listed as unusable), and a `default` the user deleted
    /// does not come back while other projects exist.
    pub async fn ensure_default(&self) -> anyhow::Result<()> {
        let default = ProjectId::default_project();
        let ids = self.ids().await?;
        for id in &ids {
            if *id == default || self.load(id).await.is_ok() {
                return Ok(());
            }
        }
        if !ids.is_empty() {
            tracing::warn!(
                dir = %self.dir.display(),
                "no project in the folder can be used; starting with an empty `default` (the other files are left as they are)"
            );
        }
        match self.create(&default, &Snapshot::default()).await {
            Ok(()) | Err(StoreError::AlreadyExists(_)) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Opens the project to start with. An explicitly requested one must exist and be usable. Otherwise:
    /// the one used last time, then `default`, then the first usable one, logging every skipped file; if
    /// none works, the first problem is returned.
    async fn open_active(
        &self,
        explicit: Option<ProjectId>,
    ) -> Result<(ProjectId, Snapshot), StoreError> {
        if let Some(id) = explicit {
            let snapshot = self.load(&id).await?;
            return Ok((id, snapshot));
        }

        let default = ProjectId::default_project();
        let mut tried = Vec::new();
        let mut first_problem = None;
        let preferred = self.active().await.into_iter().chain([default.clone()]);
        for id in preferred.chain(self.ids().await?) {
            if tried.contains(&id) {
                continue;
            }
            match self.load(&id).await {
                Ok(snapshot) => {
                    if first_problem.is_some() {
                        tracing::warn!(project = %id, "opening this project instead of the ones that could not be used");
                    }
                    return Ok((id, snapshot));
                }
                Err(StoreError::NotFound(_)) => {
                    tracing::info!(project = %id, "no such project, trying the next one");
                }
                Err(error) => {
                    tracing::warn!(%error, "skipping a project that cannot be used");
                    first_problem.get_or_insert(error);
                }
            }
            tried.push(id);
        }
        Err(first_problem.unwrap_or(StoreError::NotFound(default)))
    }

    /// Start-up message for a project that could not be opened: unlike API answers it may name paths.
    fn startup_error(&self, error: StoreError) -> anyhow::Error {
        match error {
            StoreError::Unusable { id, reason } => anyhow::anyhow!(
                "project {id} cannot be used: {reason}\n  file: {}\n  \
                 Fix or delete the file, or open another project with SHL_STORAGE__PROJECT=<id> (storage.project)",
                self.path_of(&id).display()
            ),
            StoreError::NotFound(id) => anyhow::anyhow!(
                "project {id} does not exist\n  expected: {}\n  \
                 Check SHL_STORAGE__PROJECT / storage.project, or look at the files in {}",
                self.path_of(&id).display(),
                self.dir.display()
            ),
            other => other.into(),
        }
    }
}

#[cfg(test)]
mod tests;
