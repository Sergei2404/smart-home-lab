use super::*;
use crate::{
    domain::{Device, DeviceKind, State, Step},
    lab::Source,
    runner::RunError,
};
use serde_json::json;
use std::time::Duration;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<ProjectStore>,
    lab: Arc<Lab>,
    runner: Arc<ScenarioRunner>,
    projects: Arc<ProjectService>,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ProjectStore::open(dir.path().join("projects"))
            .await
            .unwrap(),
    );
    store.ensure_default().await.unwrap();
    let lab = Arc::new(Lab::new(Snapshot::default()));
    let runner = ScenarioRunner::new(lab.clone());
    let projects = Arc::new(ProjectService::new(
        store.clone(),
        lab.clone(),
        runner.clone(),
    ));
    Fixture {
        _dir: dir,
        store,
        lab,
        runner,
        projects,
    }
}

fn id(name: &str) -> ProjectId {
    ProjectId::parse(name).unwrap()
}

fn with_device(name: &str, kind: DeviceKind) -> Snapshot {
    Snapshot {
        devices: vec![Device::new(name, kind).unwrap()],
        ..Snapshot::default()
    }
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<crate::lab::Event>) -> Vec<EventKind> {
    let mut kinds = Vec::new();
    while let Ok(event) = rx.try_recv() {
        kinds.push(event.kind);
    }
    kinds
}

#[tokio::test]
async fn the_listing_has_projects_presets_and_the_live_counts_of_the_active_one() {
    let f = fixture().await;
    f.store
        .save(&id("dacha"), &with_device("Gate", DeviceKind::Sensor))
        .await
        .unwrap();
    f.lab.add_device("Lamp", DeviceKind::Switch).unwrap();
    f.lab.add_device("Door", DeviceKind::Sensor).unwrap();

    let listing = f.projects.list().await.unwrap();

    assert_eq!(listing.active, ProjectId::default_project());
    let counts: Vec<_> = listing
        .projects
        .iter()
        .map(|p| (p.id.as_str().to_owned(), p.devices))
        .collect();
    assert_eq!(counts, [("dacha".to_owned(), 1), ("default".to_owned(), 2)]);
    let presets: Vec<_> = listing.presets.iter().map(|p| p.id).collect();
    assert_eq!(presets, ["apartment", "dacha"]);
    assert!(
        listing
            .presets
            .iter()
            .all(|p| p.devices > 0 && p.scenarios > 0)
    );
}

#[tokio::test]
async fn a_project_can_be_created_from_a_preset_without_switching_to_it() {
    let f = fixture().await;
    let mut rx = f.lab.subscribe();

    let created = f
        .projects
        .create("My Flat", Template::Preset("apartment".into()))
        .await
        .unwrap();

    assert_eq!(created, id("my-flat"));
    let saved = f.store.load(&created).await.unwrap();
    assert_eq!(saved, presets::find("apartment").unwrap().snapshot());
    assert_eq!(f.lab.project(), ProjectId::default_project());
    assert!(f.lab.devices().is_empty());
    assert_eq!(drain(&mut rx), vec![EventKind::ProjectsChanged]);
}

#[tokio::test]
async fn creating_can_start_empty_and_refuses_bad_input() {
    let f = fixture().await;

    f.projects.create("Blank", Template::Empty).await.unwrap();
    assert_eq!(
        f.store.load(&id("blank")).await.unwrap(),
        Snapshot::default()
    );

    assert!(matches!(
        f.projects.create("blank", Template::Empty).await,
        Err(ProjectError::Store(StoreError::AlreadyExists(_)))
    ));
    assert!(matches!(
        f.projects
            .create("Other", Template::Preset("nope".into()))
            .await,
        Err(ProjectError::UnknownPreset(_))
    ));
    assert!(matches!(
        f.projects.create("Дача", Template::Empty).await,
        Err(ProjectError::Name(DomainError::EmptySlug))
    ));
    assert!(matches!(
        f.projects
            .create("Ghost Copy", Template::CopyOf(id("ghost")))
            .await,
        Err(ProjectError::Store(StoreError::NotFound(_)))
    ));
}

#[tokio::test]
async fn copying_the_active_project_uses_its_live_state_not_a_possibly_stale_file() {
    let f = fixture().await;
    let lamp = f.lab.add_device("Lamp", DeviceKind::Switch).unwrap().id;
    f.lab.set_state(&lamp, true, Source::Ui).unwrap();
    // Nothing has been written to disk (there is no persister in this test).
    assert!(
        f.store
            .load(&ProjectId::default_project())
            .await
            .unwrap()
            .devices
            .is_empty()
    );

    f.projects
        .create("Copy", Template::CopyOf(ProjectId::default_project()))
        .await
        .unwrap();

    let copy = f.store.load(&id("copy")).await.unwrap();
    assert_eq!(copy.devices[0].state, State::Bool(true));
}

#[tokio::test]
async fn an_uploaded_file_is_checked_like_everything_else() {
    let f = fixture().await;
    let valid =
        json!({ "devices": [{ "id": "gate", "name": "Gate", "kind": "sensor", "state": false }] });
    f.projects
        .create("Uploaded", Template::Import(valid))
        .await
        .unwrap();
    assert_eq!(
        f.store.load(&id("uploaded")).await.unwrap().devices.len(),
        1
    );

    let not_a_project = json!({ "hello": "world" });
    assert!(matches!(
        f.projects
            .create("Nope", Template::Import(not_a_project))
            .await,
        Err(ProjectError::BadImport(_))
    ));
    assert!(matches!(
        f.projects
            .create("Nope", Template::Import(json!([1, 2])))
            .await,
        Err(ProjectError::BadImport(_))
    ));

    let breaks_the_rules =
        json!({ "devices": [{ "id": "h", "name": "H", "kind": "percent", "state": 500 }] });
    assert!(matches!(
        f.projects
            .create("Nope", Template::Import(breaks_the_rules))
            .await,
        Err(ProjectError::Store(StoreError::Unusable { .. }))
    ));
    assert!(matches!(
        f.store.load(&id("nope")).await,
        Err(StoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn activating_swaps_the_project_and_saves_the_one_that_was_left() {
    let f = fixture().await;
    f.store
        .save(&id("dacha"), &with_device("Gate", DeviceKind::Sensor))
        .await
        .unwrap();
    let lamp = f.lab.add_device("Lamp", DeviceKind::Switch).unwrap().id;
    f.lab.set_state(&lamp, true, Source::Ui).unwrap();
    let mut rx = f.lab.subscribe();

    let active = f.projects.activate(&id("dacha")).await.unwrap();

    assert_eq!(active, id("dacha"));
    assert_eq!(f.lab.project(), id("dacha"));
    assert_eq!(f.lab.devices()[0].id.as_str(), "gate");
    let left = f.store.load(&ProjectId::default_project()).await.unwrap();
    assert_eq!(
        left.devices[0].state,
        State::Bool(true),
        "the last change of the old project is on disk"
    );
    assert_eq!(f.store.active().await, Some(id("dacha")));
    let kinds = drain(&mut rx);
    assert!(kinds.contains(&EventKind::ProjectChanged {
        project: id("dacha")
    }));
    assert!(kinds.contains(&EventKind::ProjectsChanged));
}

#[tokio::test]
async fn activating_the_current_project_does_nothing() {
    let f = fixture().await;
    let mut rx = f.lab.subscribe();

    f.projects
        .activate(&ProjectId::default_project())
        .await
        .unwrap();

    assert!(drain(&mut rx).is_empty());
}

#[tokio::test]
async fn a_switch_whose_request_is_dropped_still_completes() {
    let f = fixture().await;
    let lamp = f.lab.add_device("Lamp", DeviceKind::Switch).unwrap().id;
    f.lab.set_state(&lamp, true, Source::Ui).unwrap();
    f.store
        .save(&id("dacha"), &with_device("Gate", DeviceKind::Sensor))
        .await
        .unwrap();

    // The client goes away right after the request started: its future is dropped unfinished.
    let dacha = id("dacha");
    let request = f.projects.activate(&dacha);
    tokio::select! {
        biased;
        _ = request => panic!("a switch needs more than one poll"),
        () = tokio::task::yield_now() => {}
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.store.active().await != Some(id("dacha")) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the switch was abandoned halfway");

    assert_eq!(f.lab.project(), id("dacha"));
    let left = f.store.load(&ProjectId::default_project()).await.unwrap();
    assert_eq!(left.devices[0].state, State::Bool(true));
}

#[tokio::test]
async fn a_failing_save_of_the_current_project_stops_nothing_and_changes_nothing() {
    let f = fixture().await;
    let lamp = f.lab.add_device("Lamp", DeviceKind::Switch).unwrap().id;
    let demo = f
        .lab
        .add_scenario(
            "Demo",
            false,
            vec![
                Step::Set {
                    device: lamp.clone(),
                    state: State::Bool(true),
                },
                Step::Delay { secs: 60 },
            ],
        )
        .unwrap()
        .id;
    f.store
        .save(&id("dacha"), &Snapshot::default())
        .await
        .unwrap();
    f.runner.run(&demo).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    // A folder where the project's file should be: the file cannot be replaced.
    let file = f.store.path_of(&ProjectId::default_project());
    tokio::fs::remove_file(&file).await.unwrap();
    tokio::fs::create_dir(&file).await.unwrap();
    let mut rx = f.lab.subscribe();

    let result = f.projects.activate(&id("dacha")).await;

    assert!(
        matches!(result, Err(ProjectError::Store(StoreError::Io(_)))),
        "{result:?}"
    );
    assert_eq!(f.lab.project(), ProjectId::default_project());
    assert_eq!(f.runner.runs().len(), 1, "the scenario is still running");
    assert!(drain(&mut rx).is_empty());
    assert_eq!(f.store.active().await, None);
}

#[tokio::test]
async fn a_missing_or_broken_project_leaves_everything_as_it_was() {
    let f = fixture().await;
    let lamp = f.lab.add_device("Lamp", DeviceKind::Switch).unwrap().id;
    tokio::fs::write(f.store.dir().join("broken.json"), "{ nope")
        .await
        .unwrap();
    let mut rx = f.lab.subscribe();

    assert!(matches!(
        f.projects.activate(&id("ghost")).await,
        Err(ProjectError::Store(StoreError::NotFound(_)))
    ));
    assert!(matches!(
        f.projects.activate(&id("broken")).await,
        Err(ProjectError::Store(StoreError::Unusable { .. }))
    ));

    assert_eq!(f.lab.project(), ProjectId::default_project());
    assert!(f.lab.device(&lamp).is_ok());
    assert!(drain(&mut rx).is_empty());
    assert_eq!(f.store.active().await, None);
}

#[tokio::test]
async fn running_scenarios_end_with_the_switch_and_never_touch_the_new_project() {
    let f = fixture().await;
    let lamp = f.lab.add_device("Lamp", DeviceKind::Switch).unwrap().id;
    let demo = f
        .lab
        .add_scenario(
            "Demo",
            false,
            vec![
                Step::Set {
                    device: lamp.clone(),
                    state: State::Bool(true),
                },
                Step::Delay { secs: 60 },
                Step::Set {
                    device: lamp.clone(),
                    state: State::Bool(false),
                },
            ],
        )
        .unwrap()
        .id;
    // The other project has a device with the very same id.
    f.store
        .save(&id("dacha"), &with_device("Lamp", DeviceKind::Switch))
        .await
        .unwrap();
    f.runner.run(&demo).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(f.lab.device(&lamp).unwrap().state == State::Bool(true));

    f.projects.activate(&id("dacha")).await.unwrap();

    assert!(f.runner.runs().is_empty());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        f.lab.device(&lamp).unwrap().state,
        State::Bool(false),
        "the new project is untouched"
    );
    let old = f.store.load(&ProjectId::default_project()).await.unwrap();
    assert_eq!(
        old.devices[0].state,
        State::Bool(true),
        "the old project kept what the run had done"
    );
    assert!(
        f.runner.run(&demo).is_err(),
        "the scenario belongs to the old project and does not exist in the new one"
    );
}

#[tokio::test]
async fn runs_can_start_again_after_a_switch() {
    let f = fixture().await;
    let dacha = Snapshot {
        devices: vec![Device::new("Lamp", DeviceKind::Switch).unwrap()],
        scenarios: vec![
            crate::domain::Scenario::new(
                "On",
                false,
                vec![Step::Set {
                    device: crate::domain::DeviceId::parse("lamp").unwrap(),
                    state: State::Bool(true),
                }],
            )
            .unwrap(),
        ],
    };
    f.store.save(&id("dacha"), &dacha).await.unwrap();

    f.projects.activate(&id("dacha")).await.unwrap();
    let scenario = crate::domain::ScenarioId::parse("on").unwrap();
    f.runner.run(&scenario).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        f.lab
            .device(&crate::domain::DeviceId::parse("lamp").unwrap())
            .unwrap()
            .state,
        State::Bool(true)
    );
}

#[tokio::test]
async fn the_active_project_cannot_be_deleted_but_others_can() {
    let f = fixture().await;
    f.store
        .save(&id("old"), &Snapshot::default())
        .await
        .unwrap();
    let mut rx = f.lab.subscribe();

    assert!(matches!(
        f.projects.remove(&ProjectId::default_project()).await,
        Err(ProjectError::Active(_))
    ));
    f.projects.remove(&id("old")).await.unwrap();
    assert!(matches!(
        f.projects.remove(&id("old")).await,
        Err(ProjectError::Store(StoreError::NotFound(_)))
    ));

    assert_eq!(drain(&mut rx), vec![EventKind::ProjectsChanged]);
    let remaining: Vec<_> = f
        .store
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(remaining, [ProjectId::default_project()]);
}

#[tokio::test]
async fn export_gives_the_live_state_for_the_active_project_and_the_file_for_others() {
    let f = fixture().await;
    f.lab.add_device("Lamp", DeviceKind::Switch).unwrap();
    f.store
        .save(&id("dacha"), &with_device("Gate", DeviceKind::Sensor))
        .await
        .unwrap();

    assert_eq!(
        f.projects
            .export(&ProjectId::default_project())
            .await
            .unwrap()
            .devices[0]
            .id
            .as_str(),
        "lamp"
    );
    assert_eq!(
        f.projects.export(&id("dacha")).await.unwrap().devices[0]
            .id
            .as_str(),
        "gate"
    );
    assert!(matches!(
        f.projects.export(&id("ghost")).await,
        Err(ProjectError::Store(StoreError::NotFound(_)))
    ));
}

/// A thread changes a device as fast as it can while the project it belongs to is being left.
/// Whatever the last accepted change was, it must be what ends up in that project's file: a
/// change made between "save before the switch" and "swap" is the case a unit test cannot aim at.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_changed_while_a_project_is_being_left_is_lost() {
    use crate::projects::persister::spawn_persister;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use tokio_util::sync::CancellationToken;

    let f = fixture().await;
    let counter = f
        .lab
        .add_device("Counter", DeviceKind::Temperature)
        .unwrap()
        .id;
    f.store
        .save(&id("other"), &with_device("Gate", DeviceKind::Sensor))
        .await
        .unwrap();
    let stop_persister = CancellationToken::new();
    let persister = spawn_persister(f.store.clone(), f.lab.snapshots(), stop_persister.clone());
    let mut next = 0;
    let mut accepted_changes = 0;

    for round in 0..40 {
        let (stop, last) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicI32::new(next)),
        );
        let hammer = {
            let (lab, counter, stop, last) =
                (f.lab.clone(), counter.clone(), stop.clone(), last.clone());
            std::thread::spawn(move || {
                let mut n = next;
                while !stop.load(Ordering::Relaxed) {
                    n = n.wrapping_add(1);
                    // Fails once the project has been swapped: `counter` is not in the other one.
                    if lab.set_state(&counter, State::Int(n), Source::Ui).is_ok() {
                        last.store(n, Ordering::Relaxed);
                    }
                }
            })
        };
        while last.load(Ordering::Relaxed) == next {
            tokio::task::yield_now().await;
        }

        f.projects.activate(&id("other")).await.unwrap();
        stop.store(true, Ordering::Relaxed);
        hammer.join().unwrap();

        let last = last.load(Ordering::Relaxed);
        let saved = f.store.load(&ProjectId::default_project()).await.unwrap();
        assert_eq!(
            saved.devices[0].state,
            State::Int(last),
            "round {round}: the last accepted change is not what was saved"
        );
        accepted_changes += i64::from(last.wrapping_sub(next));
        next = last;
        f.projects
            .activate(&ProjectId::default_project())
            .await
            .unwrap();
        assert_eq!(f.lab.device(&counter).unwrap().state, State::Int(last));
    }

    stop_persister.cancel();
    persister.await.unwrap();
    assert_eq!(
        f.store.load(&ProjectId::default_project()).await.unwrap(),
        f.lab.snapshot()
    );
    assert!(accepted_changes > 40, "the hammer never ran");
}

#[tokio::test]
async fn new_runs_are_refused_only_while_the_switch_is_in_progress() {
    let f = fixture().await;
    let lamp = f.lab.add_device("Lamp", DeviceKind::Switch).unwrap().id;
    let on = f
        .lab
        .add_scenario(
            "On",
            false,
            vec![Step::Set {
                device: lamp,
                state: State::Bool(true),
            }],
        )
        .unwrap()
        .id;
    f.store
        .save(&id("dacha"), &Snapshot::default())
        .await
        .unwrap();
    // A finished switch leaves no restriction behind.
    f.projects.activate(&id("dacha")).await.unwrap();
    f.projects
        .activate(&ProjectId::default_project())
        .await
        .unwrap();

    f.runner.run(&on).unwrap_or_else(|error| {
        assert!(
            !matches!(error, RunError::ProjectSwitching),
            "a finished switch must not leave runs refused"
        );
        panic!("{error}")
    });
}
