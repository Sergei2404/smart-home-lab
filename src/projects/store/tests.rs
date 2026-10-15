use super::*;
use crate::domain::{Device, DeviceKind, State};
use std::sync::Arc;

fn id(name: &str) -> ProjectId {
    ProjectId::parse(name).unwrap()
}

fn snapshot(device: &str) -> Snapshot {
    Snapshot {
        devices: vec![Device::new(device, DeviceKind::Switch).unwrap()],
        ..Snapshot::default()
    }
}

async fn store() -> (tempfile::TempDir, ProjectStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ProjectStore::open(dir.path().join("projects"))
        .await
        .unwrap();
    (dir, store)
}

async fn write_broken(store: &ProjectStore, name: &str) {
    tokio::fs::write(store.dir().join(format!("{name}.json")), "{ nope")
        .await
        .unwrap();
}

async fn names(store: &ProjectStore) -> Vec<String> {
    store
        .ids()
        .await
        .unwrap()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

#[tokio::test]
async fn open_creates_the_folder_and_it_starts_empty() {
    let (_dir, store) = store().await;
    assert!(store.dir().is_dir());
    assert!(store.list().await.unwrap().is_empty());
    assert_eq!(store.active().await, None);
}

#[tokio::test]
async fn projects_round_trip() {
    let (_dir, store) = store().await;
    store.save(&id("home"), &snapshot("Lamp")).await.unwrap();
    assert_eq!(store.load(&id("home")).await.unwrap(), snapshot("Lamp"));
}

#[tokio::test]
async fn a_missing_project_is_not_found_rather_than_empty() {
    let (_dir, store) = store().await;
    assert!(matches!(
        store.load(&id("ghost")).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.remove(&id("ghost")).await,
        Err(StoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_an_io_error_not_a_missing_project() {
    let (_dir, store) = store().await;
    tokio::fs::create_dir(store.dir().join("odd.json"))
        .await
        .unwrap();

    let error = store.load(&id("odd")).await.unwrap_err();

    assert!(matches!(error, StoreError::Io(_)), "{error}");
    assert!(
        names(&store).await.is_empty(),
        "a folder is not a project file"
    );
}

#[tokio::test]
async fn create_refuses_to_overwrite_or_to_write_invalid_data() {
    let (_dir, store) = store().await;
    store.create(&id("home"), &snapshot("Lamp")).await.unwrap();

    assert!(matches!(
        store.create(&id("home"), &snapshot("Other")).await,
        Err(StoreError::AlreadyExists(_))
    ));
    assert_eq!(store.load(&id("home")).await.unwrap(), snapshot("Lamp"));

    let mut broken = snapshot("Lamp");
    broken.devices[0].state = State::Int(5);
    let error = store.create(&id("bad"), &broken).await.unwrap_err();
    assert!(matches!(error, StoreError::Unusable { .. }), "{error}");
    assert!(matches!(
        store.load(&id("bad")).await,
        Err(StoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn list_is_sorted_and_skips_files_that_are_not_projects() {
    let (_dir, store) = store().await;
    store.save(&id("zeta"), &snapshot("Lamp")).await.unwrap();
    store
        .save(&id("alpha"), &Snapshot::default())
        .await
        .unwrap();
    for junk in [
        "notes.txt",
        "Bad Name.json",
        "alpha.json.tmp",
        ".active",
        "x.JSON",
    ] {
        tokio::fs::write(store.dir().join(junk), "junk")
            .await
            .unwrap();
    }

    let entries = store.list().await.unwrap();

    let names: Vec<_> = entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(names, ["alpha", "zeta"]);
    assert_eq!((entries[1].devices, entries[1].scenarios), (1, 0));
    assert!(entries.iter().all(|e| e.error.is_none()));
}

#[tokio::test]
async fn a_corrupt_project_is_listed_with_its_error_and_cannot_be_loaded() {
    let (dir, store) = store().await;
    write_broken(&store, "broken").await;

    let entries = store.list().await.unwrap();

    assert_eq!(entries.len(), 1);
    let reason = entries[0].error.as_deref().unwrap();
    assert!(reason.contains("failed to parse"), "{reason}");
    assert!(
        !reason.contains(dir.path().to_str().unwrap()),
        "the list is sent to the browser and must not contain paths: {reason}"
    );
    assert!(matches!(
        store.load(&id("broken")).await,
        Err(StoreError::Unusable { .. })
    ));
}

#[tokio::test]
async fn remove_deletes_the_file() {
    let (_dir, store) = store().await;
    store.save(&id("home"), &snapshot("Lamp")).await.unwrap();
    store.remove(&id("home")).await.unwrap();
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn the_active_project_is_remembered_and_garbage_is_ignored() {
    let (_dir, store) = store().await;
    store.set_active(&id("dacha")).await.unwrap();
    assert_eq!(store.active().await, Some(id("dacha")));

    tokio::fs::write(store.dir().join(".active"), "../../etc/passwd")
        .await
        .unwrap();
    assert_eq!(store.active().await, None);
}

#[tokio::test]
async fn open_active_prefers_the_explicit_then_the_remembered_then_default() {
    let (_dir, store) = store().await;
    store.save(&id("home"), &snapshot("Home")).await.unwrap();
    store.save(&id("dacha"), &snapshot("Dacha")).await.unwrap();
    store
        .save(&ProjectId::default_project(), &snapshot("Main"))
        .await
        .unwrap();

    let (opened, loaded) = store.open_active(None).await.unwrap();
    assert_eq!(opened, ProjectId::default_project());
    assert_eq!(
        loaded,
        snapshot("Main"),
        "the project comes with its content"
    );

    store.set_active(&id("dacha")).await.unwrap();
    assert_eq!(store.open_active(None).await.unwrap().0, id("dacha"));
    assert_eq!(
        store.open_active(Some(id("home"))).await.unwrap().0,
        id("home")
    );
    assert!(matches!(
        store.open_active(Some(id("ghost"))).await,
        Err(StoreError::NotFound(_))
    ));

    store.remove(&id("dacha")).await.unwrap();
    assert_eq!(
        store.open_active(None).await.unwrap().0,
        ProjectId::default_project(),
        "a remembered project that no longer exists falls back to default"
    );
}

#[tokio::test]
async fn a_folder_without_default_and_without_a_marker_opens_what_is_there() {
    let (_dir, store) = store().await;
    store.save(&id("home"), &snapshot("Lamp")).await.unwrap();

    store.ensure_default().await.unwrap();
    let (opened, _) = store.open_active(None).await.unwrap();

    assert_eq!(opened, id("home"));
    assert_eq!(
        names(&store).await,
        ["home"],
        "a deleted default does not come back while other projects exist"
    );
}

#[tokio::test]
async fn an_explicit_project_that_is_broken_is_an_error_not_a_fallback() {
    let (_dir, store) = store().await;
    store.ensure_default().await.unwrap();
    write_broken(&store, "broken").await;

    assert!(matches!(
        store.open_active(Some(id("broken"))).await,
        Err(StoreError::Unusable { .. })
    ));
}

#[tokio::test]
async fn a_remembered_project_that_is_broken_falls_back_to_one_that_works() {
    let (_dir, store) = store().await;
    store
        .save(&ProjectId::default_project(), &snapshot("Main"))
        .await
        .unwrap();
    write_broken(&store, "broken").await;
    store.set_active(&id("broken")).await.unwrap();

    let (opened, _) = store.open_active(None).await.unwrap();

    assert_eq!(opened, ProjectId::default_project());
}

#[tokio::test]
async fn when_default_is_missing_the_first_usable_project_is_opened() {
    let (_dir, store) = store().await;
    write_broken(&store, "aaa").await;
    store.save(&id("zeta"), &snapshot("Lamp")).await.unwrap();
    store.save(&id("beta"), &snapshot("Lamp")).await.unwrap();
    store.set_active(&id("ghost")).await.unwrap();

    let (opened, _) = store.open_active(None).await.unwrap();

    assert_eq!(opened, id("beta"));
}

#[tokio::test]
async fn when_no_file_can_be_used_an_empty_default_is_created_and_the_broken_files_stay() {
    let (_dir, store) = store().await;
    write_broken(&store, "home").await;

    store.ensure_default().await.unwrap();
    let (opened, loaded) = store.open_active(None).await.unwrap();

    assert_eq!(opened, ProjectId::default_project());
    assert_eq!(loaded, Snapshot::default());
    let broken = tokio::fs::read_to_string(store.dir().join("home.json"))
        .await
        .unwrap();
    assert_eq!(broken, "{ nope", "the broken file is left for the user");
}

#[tokio::test]
async fn a_broken_default_is_never_replaced_and_the_start_up_message_says_what_to_do() {
    let (_dir, store) = store().await;
    write_broken(&store, "default").await;

    store.ensure_default().await.unwrap();
    let error = store.open_active(None).await.unwrap_err();

    assert!(matches!(error, StoreError::Unusable { .. }), "{error}");
    let message = format!("{:#}", store.startup_error(error));
    assert!(message.contains("default.json"), "{message}");
    assert!(message.contains("SHL_STORAGE__PROJECT"), "{message}");
    assert!(message.contains("failed to parse"), "{message}");
    let kept = tokio::fs::read_to_string(store.dir().join("default.json"))
        .await
        .unwrap();
    assert_eq!(kept, "{ nope");
}

#[tokio::test]
async fn the_start_up_message_for_a_missing_explicit_project_names_the_file() {
    let (_dir, store) = store().await;
    let error = store.open_active(Some(id("ghost"))).await.unwrap_err();

    let message = store.startup_error(error).to_string();

    assert!(message.contains("ghost.json"), "{message}");
    assert!(message.contains("storage.project"), "{message}");
}

#[tokio::test]
async fn default_is_created_on_demand_and_only_once() {
    let (_dir, store) = store().await;
    store.ensure_default().await.unwrap();
    store.ensure_default().await.unwrap();

    let entries = store.list().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, ProjectId::default_project());
}

#[tokio::test]
async fn a_queued_save_writes_what_is_newest_when_its_turn_comes_not_what_it_saw_when_it_queued() {
    let (_dir, store) = store().await;
    let store = Arc::new(store);
    let lab = crate::lab::Lab::with_project(id("home"), snapshot("First"));
    let mut latest = lab.snapshots();
    let other_write = store.write.lock().await;
    let queued = {
        let store = store.clone();
        tokio::spawn(async move {
            store
                .save_latest(|| {
                    let published = latest.borrow_and_update().clone();
                    (published.project, published.snapshot)
                })
                .await
        })
    };
    tokio::task::yield_now().await;

    // While it waits, the lab moves on to another project.
    lab.replace(id("dacha"), snapshot("Second"));
    drop(other_write);
    queued.await.unwrap().unwrap();

    assert!(
        matches!(store.load(&id("home")).await, Err(StoreError::NotFound(_))),
        "the state of the project that was left must not be written over the final save"
    );
    assert_eq!(store.load(&id("dacha")).await.unwrap(), snapshot("Second"));
}

#[tokio::test]
async fn concurrent_saves_never_corrupt_a_project_file() {
    let (_dir, store) = store().await;
    let store = Arc::new(store);
    let home = id("home");
    let writers: Vec<_> = (0..24)
        .map(|i| {
            let (store, home) = (store.clone(), home.clone());
            tokio::spawn(async move {
                store
                    .save(&home, &snapshot(&format!("Lamp {i}")))
                    .await
                    .unwrap();
            })
        })
        .collect();
    for writer in writers {
        writer.await.unwrap();
    }

    let loaded = store.load(&home).await.unwrap();
    assert_eq!(loaded.devices.len(), 1);
    assert!(loaded.devices[0].name.starts_with("Lamp "));
    assert!(!store.dir().join("home.json.tmp").exists());
}

#[tokio::test]
async fn bootstrap_opens_a_project_records_it_and_leaves_its_file_alone() {
    let (_dir, store) = store().await;
    let path = store.path_of(&id("home"));
    // A field this version does not know, as a newer one might write it.
    let written = r#"{"devices":[],"future_field":1}"#;
    tokio::fs::write(&path, written).await.unwrap();

    let (opened, snapshot) = store.bootstrap(None).await.unwrap();

    assert_eq!(opened, id("home"));
    assert_eq!(snapshot, Snapshot::default());
    assert_eq!(store.active().await, Some(id("home")));
    assert_eq!(
        tokio::fs::read_to_string(&path).await.unwrap(),
        written,
        "starting must not rewrite the project"
    );
}

#[tokio::test]
async fn bootstrap_on_an_empty_folder_creates_default() {
    let (_dir, store) = store().await;

    let (opened, _) = store.bootstrap(None).await.unwrap();

    assert_eq!(opened, ProjectId::default_project());
    assert_eq!(names(&store).await, ["default"]);
}

#[tokio::test]
async fn bootstrap_reports_a_folder_it_cannot_write_to() {
    let (_dir, store) = store().await;
    store.save(&id("home"), &snapshot("Lamp")).await.unwrap();
    // Something that is not a file where the marker has to go.
    tokio::fs::create_dir(store.dir().join(".active"))
        .await
        .unwrap();

    let error = store.bootstrap(None).await.unwrap_err();

    assert!(format!("{error:#}").contains("not writable"), "{error:#}");
}

#[tokio::test]
async fn bootstrap_explains_why_the_asked_for_project_cannot_be_opened() {
    let (_dir, store) = store().await;

    let error = store.bootstrap(Some(id("ghost"))).await.unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("ghost.json"), "{message}");
    assert!(message.contains("storage.project"), "{message}");
}
