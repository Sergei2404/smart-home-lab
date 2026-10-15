use crate::domain::{
    Device, DeviceId, DeviceKind, DomainError, MAX_DEVICES, MAX_SCENARIOS, ProjectId, Scenario,
    ScenarioId, Snapshot, State, Step,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard, PoisonError},
};
use tokio::sync::{broadcast, watch};

const EVENT_CHANNEL_CAPACITY: usize = 256;
const LOG_CAPACITY: usize = 200;

/// What the persister receives: a state with the project it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Persisted {
    pub project: ProjectId,
    pub snapshot: Snapshot,
}

/// Who caused a state change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    Ui,
    Mqtt,
    Scenario { id: ScenarioId },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MqttState {
    #[default]
    Disabled,
    Disconnected,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    DeviceAdded {
        device: Device,
    },
    DeviceRemoved {
        id: DeviceId,
    },
    StateChanged {
        id: DeviceId,
        state: State,
        source: Source,
    },
    AvailabilityChanged {
        id: DeviceId,
        online: bool,
        source: Source,
    },
    ScenarioAdded {
        scenario: Scenario,
    },
    ScenarioUpdated {
        scenario: Scenario,
    },
    ScenarioRemoved {
        id: ScenarioId,
    },
    ScenarioStarted {
        id: ScenarioId,
    },
    /// Progress of a running scenario (0-based index of the current step). Not kept in the log.
    ScenarioStep {
        id: ScenarioId,
        step: usize,
    },
    ScenarioFinished {
        id: ScenarioId,
    },
    ScenarioStopped {
        id: ScenarioId,
    },
    ScenarioFailed {
        id: ScenarioId,
        reason: String,
    },
    MqttStatus {
        state: MqttState,
    },
    /// The lab now holds another project: everything a client knows must be reloaded.
    ProjectChanged {
        project: ProjectId,
    },
    /// The list of projects changed (created, deleted, counts). Not kept in the log.
    ProjectsChanged,
}

impl EventKind {
    fn is_transient(&self) -> bool {
        matches!(self, Self::ScenarioStep { .. } | Self::ProjectsChanged)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// What an id refers to, in error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Device,
    Scenario,
}

impl std::fmt::Display for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Device => "device",
            Self::Scenario => "scenario",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LabError {
    #[error("{kind} {id} not found")]
    NotFound { kind: Resource, id: String },
    /// `renamed`: the item's name no longer matches its id, as after a rename.
    #[error("{}", describe_taken(*.kind, .id, .used_by, *.renamed))]
    AlreadyExists {
        kind: Resource,
        id: String,
        used_by: String,
        renamed: bool,
    },
    #[error("a project can have at most {max} {what}")]
    LimitReached { what: &'static str, max: usize },
    #[error("unknown device {0}")]
    UnknownDevice(DeviceId),
    #[error(
        "device {device} is used by {}; remove it from the scenario first",
        list_scenarios(.scenarios)
    )]
    DeviceInUse {
        device: DeviceId,
        scenarios: Vec<ScenarioId>,
    },
    #[error(transparent)]
    Invalid(#[from] DomainError),
}

fn taken_scenario_id(existing: &Scenario) -> LabError {
    LabError::AlreadyExists {
        kind: Resource::Scenario,
        id: existing.id.to_string(),
        used_by: existing.name.clone(),
        renamed: ScenarioId::from_name(&existing.name).is_ok_and(|id| id != existing.id),
    }
}

impl LabError {
    fn no_device(id: &DeviceId) -> Self {
        Self::NotFound {
            kind: Resource::Device,
            id: id.to_string(),
        }
    }

    fn no_scenario(id: &ScenarioId) -> Self {
        Self::NotFound {
            kind: Resource::Scenario,
            id: id.to_string(),
        }
    }
}

fn describe_taken(kind: Resource, id: &str, used_by: &str, renamed: bool) -> String {
    let mut message = format!("{kind} id {id} is already used by {used_by:?}");
    if renamed {
        message.push_str(&format!(
            " (an id is made from the name the {kind} was created with and does not change when it is renamed)"
        ));
    }
    message
}

fn list_scenarios(ids: &[ScenarioId]) -> String {
    let names: Vec<&str> = ids.iter().map(ScenarioId::as_str).collect();
    format!("scenarios {}", names.join(", "))
}

struct Inner {
    project: ProjectId,
    devices: Vec<Device>,
    scenarios: Vec<Scenario>,
    log: VecDeque<Event>,
    mqtt: MqttState,
}

impl Inner {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            devices: self.devices.clone(),
            scenarios: self.scenarios.clone(),
        }
    }

    /// Every step must name an existing device, and `set` steps must carry a state that fits its kind.
    fn check_steps(&self, scenario: &Scenario) -> Result<(), LabError> {
        let find = |id: &DeviceId| {
            self.devices
                .iter()
                .find(|d| &d.id == id)
                .ok_or_else(|| LabError::UnknownDevice(id.clone()))
        };
        for step in &scenario.steps {
            match step {
                Step::Set { device, state } => {
                    find(device)?.kind.check(*state)?;
                }
                Step::Availability { device, .. } => {
                    find(device)?;
                }
                Step::Delay { .. } => {}
            }
        }
        Ok(())
    }
}

/// The single write path for devices and scenarios. Every change is pushed to the
/// event bus, the in-memory log and the snapshot channel used for persistence.
pub struct Lab {
    inner: Mutex<Inner>,
    events: broadcast::Sender<Event>,
    snapshots: watch::Sender<Persisted>,
}

impl Lab {
    /// A lab holding the `default` project.
    pub fn new(snapshot: Snapshot) -> Self {
        Self::with_project(ProjectId::default_project(), snapshot)
    }

    pub fn with_project(project: ProjectId, snapshot: Snapshot) -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let (snapshots, _) = watch::channel(Persisted {
            project: project.clone(),
            snapshot: snapshot.clone(),
        });
        Self {
            inner: Mutex::new(Inner {
                project,
                devices: snapshot.devices,
                scenarios: snapshot.scenarios,
                log: VecDeque::new(),
                mqtt: MqttState::default(),
            }),
            events,
            snapshots,
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        self.lock().devices.clone()
    }

    pub fn device(&self, id: &DeviceId) -> Result<Device, LabError> {
        self.lock()
            .devices
            .iter()
            .find(|d| &d.id == id)
            .cloned()
            .ok_or_else(|| LabError::no_device(id))
    }

    pub fn add_device(&self, name: &str, kind: DeviceKind) -> Result<Device, LabError> {
        let device = Device::new(name, kind)?;
        let mut inner = self.lock();
        if let Some(existing) = inner.devices.iter().find(|d| d.id == device.id) {
            return Err(LabError::AlreadyExists {
                kind: Resource::Device,
                id: device.id.to_string(),
                used_by: existing.name.clone(),
                renamed: false,
            });
        }
        if inner.devices.len() >= MAX_DEVICES {
            return Err(LabError::LimitReached {
                what: "devices",
                max: MAX_DEVICES,
            });
        }
        inner.devices.push(device.clone());
        self.commit(
            &mut inner,
            EventKind::DeviceAdded {
                device: device.clone(),
            },
        );
        Ok(device)
    }

    /// A device that scenarios still refer to cannot be removed: they would fail halfway through.
    pub fn remove_device(&self, id: &DeviceId) -> Result<(), LabError> {
        let mut inner = self.lock();
        let index = inner
            .devices
            .iter()
            .position(|d| &d.id == id)
            .ok_or_else(|| LabError::no_device(id))?;
        let users: Vec<ScenarioId> = inner
            .scenarios
            .iter()
            .filter(|s| s.devices().any(|d| d == id))
            .map(|s| s.id.clone())
            .collect();
        if !users.is_empty() {
            return Err(LabError::DeviceInUse {
                device: id.clone(),
                scenarios: users,
            });
        }
        inner.devices.remove(index);
        self.commit(&mut inner, EventKind::DeviceRemoved { id: id.clone() });
        Ok(())
    }

    /// Sets the state; emits an event only when the value changes.
    pub fn set_state(
        &self,
        id: &DeviceId,
        state: impl Into<State>,
        source: Source,
    ) -> Result<Device, LabError> {
        let mut inner = self.lock();
        let device = inner
            .devices
            .iter_mut()
            .find(|d| &d.id == id)
            .ok_or_else(|| LabError::no_device(id))?;
        let state = device.kind.check(state.into())?;
        if device.state == state {
            return Ok(device.clone());
        }
        device.state = state;
        let device = device.clone();
        self.commit(
            &mut inner,
            EventKind::StateChanged {
                id: id.clone(),
                state,
                source,
            },
        );
        Ok(device)
    }

    /// Takes a device off the network or back; an offline device keeps its state but stops reporting.
    pub fn set_online(
        &self,
        id: &DeviceId,
        online: bool,
        source: Source,
    ) -> Result<Device, LabError> {
        let mut inner = self.lock();
        let device = inner
            .devices
            .iter_mut()
            .find(|d| &d.id == id)
            .ok_or_else(|| LabError::no_device(id))?;
        if device.online == online {
            return Ok(device.clone());
        }
        device.online = online;
        let device = device.clone();
        self.commit(
            &mut inner,
            EventKind::AvailabilityChanged {
                id: id.clone(),
                online,
                source,
            },
        );
        Ok(device)
    }

    pub fn scenarios(&self) -> Vec<Scenario> {
        self.lock().scenarios.clone()
    }

    pub fn scenario(&self, id: &ScenarioId) -> Result<Scenario, LabError> {
        self.lock()
            .scenarios
            .iter()
            .find(|s| &s.id == id)
            .cloned()
            .ok_or_else(|| LabError::no_scenario(id))
    }

    pub fn add_scenario(
        &self,
        name: &str,
        repeat: bool,
        steps: Vec<Step>,
    ) -> Result<Scenario, LabError> {
        let scenario = Scenario::new(name, repeat, steps)?;
        let mut inner = self.lock();
        inner.check_steps(&scenario)?;
        if let Some(existing) = inner.scenarios.iter().find(|s| s.id == scenario.id) {
            return Err(taken_scenario_id(existing));
        }
        if inner.scenarios.len() >= MAX_SCENARIOS {
            return Err(LabError::LimitReached {
                what: "scenarios",
                max: MAX_SCENARIOS,
            });
        }
        inner.scenarios.push(scenario.clone());
        self.commit(
            &mut inner,
            EventKind::ScenarioAdded {
                scenario: scenario.clone(),
            },
        );
        Ok(scenario)
    }

    /// Replaces name, repeat flag and steps; the id stays the same. A run in progress keeps its old steps.
    pub fn update_scenario(
        &self,
        id: &ScenarioId,
        name: &str,
        repeat: bool,
        steps: Vec<Step>,
    ) -> Result<Scenario, LabError> {
        let updated = Scenario::with_id(id.clone(), name, repeat, steps)?;
        let mut inner = self.lock();
        inner.check_steps(&updated)?;
        let index = inner
            .scenarios
            .iter()
            .position(|s| &s.id == id)
            .ok_or_else(|| LabError::no_scenario(id))?;
        // Ids come from names, so a name that maps to another scenario's id would be a duplicate.
        if let Ok(taken) = ScenarioId::from_name(&updated.name)
            && &taken != id
            && let Some(existing) = inner.scenarios.iter().find(|s| s.id == taken)
        {
            return Err(taken_scenario_id(existing));
        }
        inner.scenarios[index] = updated.clone();
        self.commit(
            &mut inner,
            EventKind::ScenarioUpdated {
                scenario: updated.clone(),
            },
        );
        Ok(updated)
    }

    pub fn remove_scenario(&self, id: &ScenarioId) -> Result<(), LabError> {
        let mut inner = self.lock();
        let index = inner
            .scenarios
            .iter()
            .position(|s| &s.id == id)
            .ok_or_else(|| LabError::no_scenario(id))?;
        inner.scenarios.remove(index);
        self.commit(&mut inner, EventKind::ScenarioRemoved { id: id.clone() });
        Ok(())
    }

    pub fn mqtt_state(&self) -> MqttState {
        self.lock().mqtt
    }

    /// Records the MQTT connection state and announces it when it changes.
    pub fn set_mqtt_state(&self, state: MqttState) {
        let mut inner = self.lock();
        if inner.mqtt != state {
            inner.mqtt = state;
            self.emit(&mut inner, EventKind::MqttStatus { state });
        }
    }

    /// Publishes an event that does not change persisted state (scenario run progress).
    pub(crate) fn report(&self, kind: EventKind) {
        let mut inner = self.lock();
        self.emit(&mut inner, kind);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn log(&self) -> Vec<Event> {
        self.lock().log.iter().cloned().collect()
    }

    /// The persisted state of the current project.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshots.borrow().snapshot.clone()
    }

    pub fn project(&self) -> ProjectId {
        self.lock().project.clone()
    }

    /// The current project together with its state, read as one.
    pub fn persisted(&self) -> Persisted {
        self.snapshots.borrow().clone()
    }

    /// The channel the persister listens to; every payload names the project it belongs to.
    pub fn snapshots(&self) -> watch::Receiver<Persisted> {
        self.snapshots.subscribe()
    }

    /// Makes `project` current in one step and returns the replaced project with its last state, for the
    /// caller to save. The log starts afresh. Nothing may be running against the old project (see
    /// `ScenarioRunner::begin_switch`).
    pub fn replace(&self, project: ProjectId, snapshot: Snapshot) -> (ProjectId, Snapshot) {
        let mut inner = self.lock();
        let previous = (inner.project.clone(), inner.snapshot());
        inner.project = project.clone();
        inner.devices = snapshot.devices;
        inner.scenarios = snapshot.scenarios;
        inner.log.clear();
        self.commit(&mut inner, EventKind::ProjectChanged { project });
        previous
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // `Inner` is consistent before any operation can fail, so a poisoned lock is safe to reuse.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Emits the event and publishes the new persisted state.
    fn commit(&self, inner: &mut Inner, kind: EventKind) {
        self.emit(inner, kind);
        self.snapshots.send_replace(Persisted {
            project: inner.project.clone(),
            snapshot: inner.snapshot(),
        });
    }

    fn emit(&self, inner: &mut Inner, kind: EventKind) {
        let event = Event {
            at: Utc::now(),
            kind,
        };
        if !event.kind.is_transient() {
            if inner.log.len() == LOG_CAPACITY {
                inner.log.pop_front();
            }
            inner.log.push_back(event.clone());
        }
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests;
