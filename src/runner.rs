use crate::{
    domain::{DeviceId, Scenario, ScenarioId, Step},
    lab::{EventKind, Lab, LabError, Source},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

struct Run {
    token: CancellationToken,
    step: usize,
}

/// A scenario that is playing right now and the (0-based) step it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunInfo {
    pub id: ScenarioId,
    pub step: usize,
}

enum Outcome {
    Finished,
    Stopped,
}

/// Why a scenario could not be started.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("scenario is already running")]
    AlreadyRunning,
    #[error("the project is being switched, try again in a moment")]
    ProjectSwitching,
    #[error(transparent)]
    Scenario(#[from] LabError),
}

/// The runs in progress, and whether new ones are refused. One lock guards both, so a run is either
/// registered before a switch starts (and then cancelled by it) or refused.
#[derive(Default)]
struct Runs {
    active: HashMap<ScenarioId, Run>,
    /// While set, no run may start (a project switch is in progress).
    switching: bool,
}

/// Plays scenarios as background tasks; every device change goes through `Lab::set_state`.
pub struct ScenarioRunner {
    lab: Arc<Lab>,
    runs: Mutex<Runs>,
    /// How many runs are active, so a project switch can wait until there are none.
    running: watch::Sender<usize>,
}

/// Returned by `ScenarioRunner::begin_switch`; new runs are refused until it is dropped.
pub struct SwitchGuard {
    runner: Arc<ScenarioRunner>,
}

impl Drop for SwitchGuard {
    fn drop(&mut self) {
        self.runner.lock().switching = false;
    }
}

/// Ends a run when dropped: removes it and reports how it ended. Doing it in `Drop` means a run that
/// panics, or whose task is dropped, still leaves the list instead of blocking every later project
/// switch. Removing and reporting are one step, so a `run()` slipping in between cannot announce its
/// start before the old run's end.
struct EndOfRun<'a> {
    runner: &'a ScenarioRunner,
    id: ScenarioId,
    /// How the run ended; `None` when it did not get to say (a panic, or the task being dropped).
    event: Option<EventKind>,
}

impl Drop for EndOfRun<'_> {
    fn drop(&mut self) {
        let event = self
            .event
            .take()
            .unwrap_or_else(|| EventKind::ScenarioFailed {
                id: self.id.clone(),
                reason: "the scenario ended unexpectedly".to_owned(),
            });
        let mut runs = self.runner.lock();
        runs.active.remove(&self.id);
        self.runner.lab.report(event);
        self.runner.running.send_replace(runs.active.len());
    }
}

/// Why a step failed, for the event log (an error that names the device is not given the name twice).
fn step_failure(index: usize, device: &DeviceId, error: &LabError) -> String {
    match error {
        LabError::NotFound { .. } => format!("step {}: {error}", index + 1),
        _ => format!("step {}: {device}: {error}", index + 1),
    }
}

impl ScenarioRunner {
    pub fn new(lab: Arc<Lab>) -> Arc<Self> {
        Arc::new(Self {
            lab,
            runs: Mutex::default(),
            running: watch::channel(0).0,
        })
    }

    /// Prepares a project switch: refuses new runs, cancels every run in progress and waits until
    /// they have all ended (their final events are reported before this returns). While the guard
    /// is alive nothing of the old project can touch the devices of the new one.
    pub async fn begin_switch(self: &Arc<Self>) -> SwitchGuard {
        {
            // Flag and cancellation happen under the lock `run()` registers with: a run is cancelled or refused.
            let mut runs = self.lock();
            runs.switching = true;
            for run in runs.active.values() {
                run.token.cancel();
            }
        }
        let guard = SwitchGuard {
            runner: Arc::clone(self),
        };
        let mut count = self.running.subscribe();
        // The sender lives as long as the runner, so waiting cannot fail.
        let _ = count.wait_for(|running| *running == 0).await;
        guard
    }

    /// Starts the scenario in the background, with its steps copied at start (edits affect the next run).
    pub fn run(self: &Arc<Self>, id: &ScenarioId) -> Result<(), RunError> {
        let token = CancellationToken::new();
        let scenario = {
            let mut runs = self.lock();
            if runs.switching {
                return Err(RunError::ProjectSwitching);
            }
            // Read under the runs lock, so a whole project switch cannot fit between reading the scenario and
            // registering the run.
            let scenario = self.lab.scenario(id)?;
            if runs.active.contains_key(id) {
                return Err(RunError::AlreadyRunning);
            }
            runs.active.insert(
                id.clone(),
                Run {
                    token: token.clone(),
                    step: 0,
                },
            );
            self.running.send_replace(runs.active.len());
            scenario
        };
        self.lab
            .report(EventKind::ScenarioStarted { id: id.clone() });

        let runner = Arc::clone(self);
        tokio::spawn(async move { runner.execute(scenario, token).await });
        Ok(())
    }

    /// Asks a running scenario to stop; does nothing if it is not running.
    pub fn stop(&self, id: &ScenarioId) -> Result<(), LabError> {
        let runs = self.lock();
        if let Some(run) = runs.active.get(id) {
            // A run is stopped whether or not its scenario still exists.
            run.token.cancel();
            return Ok(());
        }
        self.lab.scenario(id).map(drop)
    }

    /// Deletes a scenario and stops its run, if any, under the lock `run()` registers runs with, so a run
    /// cannot start in between and outlive its scenario. The run reports its end when it notices the
    /// cancellation, so `ScenarioStopped` follows `ScenarioRemoved` by a moment.
    pub fn remove_scenario(&self, id: &ScenarioId) -> Result<(), LabError> {
        let runs = self.lock();
        self.lab.remove_scenario(id)?;
        if let Some(run) = runs.active.get(id) {
            run.token.cancel();
        }
        Ok(())
    }

    pub fn runs(&self) -> Vec<RunInfo> {
        let mut runs: Vec<_> = self
            .lock()
            .active
            .iter()
            .map(|(id, run)| RunInfo {
                id: id.clone(),
                step: run.step,
            })
            .collect();
        runs.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        runs
    }

    async fn execute(&self, scenario: Scenario, token: CancellationToken) {
        let mut end = EndOfRun {
            runner: self,
            id: scenario.id.clone(),
            event: None,
        };
        let result = self.play(&scenario, &token).await;
        end.event = Some(match result {
            Ok(Outcome::Finished) => EventKind::ScenarioFinished {
                id: scenario.id.clone(),
            },
            Ok(Outcome::Stopped) => EventKind::ScenarioStopped {
                id: scenario.id.clone(),
            },
            Err(reason) => EventKind::ScenarioFailed {
                id: scenario.id.clone(),
                reason,
            },
        });
    }

    async fn play(
        &self,
        scenario: &Scenario,
        token: &CancellationToken,
    ) -> Result<Outcome, String> {
        let source = Source::Scenario {
            id: scenario.id.clone(),
        };
        loop {
            for (index, step) in scenario.steps.iter().enumerate() {
                if token.is_cancelled() {
                    return Ok(Outcome::Stopped);
                }
                self.progress(&scenario.id, index);
                match step {
                    Step::Set { device, state } => {
                        self.lab
                            .set_state(device, *state, source.clone())
                            .map_err(|e| step_failure(index, device, &e))?;
                    }
                    Step::Availability { device, online } => {
                        self.lab
                            .set_online(device, *online, source.clone())
                            .map_err(|e| step_failure(index, device, &e))?;
                    }
                    Step::Delay { secs } => {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(*secs)) => {}
                            _ = token.cancelled() => return Ok(Outcome::Stopped),
                        }
                    }
                }
            }
            if !scenario.repeat {
                return Ok(Outcome::Finished);
            }
            // Scenarios are validated to contain a delay, but never let a loop starve the runtime.
            tokio::task::yield_now().await;
        }
    }

    fn progress(&self, id: &ScenarioId, step: usize) {
        if let Some(run) = self.lock().active.get_mut(id) {
            run.step = step;
        }
        self.lab.report(EventKind::ScenarioStep {
            id: id.clone(),
            step,
        });
    }

    fn lock(&self) -> MutexGuard<'_, Runs> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
