use super::naming::slug_id;

slug_id!(
    /// Name of a project, which is also the name of its file (`projects/<id>.json`).
    /// The id alphabet has no dots or slashes, so it can never point outside the projects folder.
    ProjectId
);

impl ProjectId {
    /// The project that exists on a fresh install.
    pub fn default_project() -> Self {
        Self("default".to_owned())
    }
}

#[cfg(test)]
mod tests;
