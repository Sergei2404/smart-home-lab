use crate::domain::Snapshot;
use anyhow::Context;
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tokio::io::AsyncWriteExt;

/// Why a state file could not be loaded. The messages never contain the path: they may end up in an
/// API answer, and whoever needs the path (the start-up log) adds it as context.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("the file does not exist")]
    Missing,
    /// The file exists but cannot be read (permissions, a folder in its place, a failing disk).
    #[error("failed to read the file: {0}")]
    Read(std::io::Error),
    /// The content is not a state file or breaks the rules the API enforces.
    #[error("{0}")]
    Invalid(String),
}

/// One project file on disk: read and checked on load, replaced atomically on save.
pub struct ProjectFile {
    path: PathBuf,
}

impl ProjectFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A missing file is reported as missing, not as an empty lab; a file that cannot be parsed or breaks
    /// the domain rules is an error, so it is never overwritten or half-trusted.
    pub async fn load(&self) -> Result<Snapshot, LoadError> {
        let bytes = tokio::fs::read(&self.path)
            .await
            .map_err(|error| match error.kind() {
                ErrorKind::NotFound => LoadError::Missing,
                _ => LoadError::Read(error),
            })?;
        let snapshot: Snapshot = serde_json::from_slice(&bytes)
            .map_err(|error| LoadError::Invalid(format!("failed to parse the file: {error}")))?;
        snapshot
            .validate()
            .map_err(|error| LoadError::Invalid(format!("contains invalid data: {error}")))?;
        Ok(snapshot)
    }

    /// Writes a temporary file next to the target, fsyncs it and renames it over the target, so a crash
    /// leaves the old or the new content, never half of one. Creates missing parent directories.
    pub async fn save(&self, snapshot: &Snapshot) -> anyhow::Result<()> {
        let json = serde_json::to_vec_pretty(snapshot).context("failed to serialize snapshot")?;
        if let Some(dir) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let tmp = self.temporary_path();
        let write = async {
            let mut file = tokio::fs::File::create(&tmp).await?;
            file.write_all(&json).await?;
            file.sync_all().await
        };
        write
            .await
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .with_context(|| format!("failed to replace {}", self.path.display()))
    }

    /// `home.json` -> `home.json.tmp` (the extension is kept, so `state.db` becomes `state.db.tmp`).
    fn temporary_path(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".tmp");
        self.path.with_file_name(name)
    }
}

#[cfg(test)]
mod tests;
