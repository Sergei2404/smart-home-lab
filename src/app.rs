use crate::{
    lab::Lab,
    projects::{service::ProjectService, store::ProjectStore},
    runner::ScenarioRunner,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct App {
    pub lab: Arc<Lab>,
    pub runner: Arc<ScenarioRunner>,
    pub projects: Arc<ProjectService>,
    /// Cancelled on shutdown, so that connections that never end (the event stream) close.
    pub shutdown: CancellationToken,
}

impl App {
    pub fn new(lab: Arc<Lab>, store: Arc<ProjectStore>) -> Self {
        let runner = ScenarioRunner::new(lab.clone());
        let projects = Arc::new(ProjectService::new(store, lab.clone(), runner.clone()));
        Self {
            lab,
            runner,
            projects,
            shutdown: CancellationToken::new(),
        }
    }
}
