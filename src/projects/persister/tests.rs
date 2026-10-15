use super::*;
use crate::{
    domain::{DeviceKind, ProjectId, Snapshot},
    lab::Lab,
};
use std::time::Duration;

async fn store() -> (tempfile::TempDir, Arc<ProjectStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = ProjectStore::open(dir.path().join("projects"))
        .await
        .unwrap();
    (dir, Arc::new(store))
}

#[tokio::test]
async fn persister_writes_changes() {
    let (_dir, store) = store().await;
    let lab = Lab::new(Snapshot::default());
    let stop = CancellationToken::new();
    let persister = spawn_persister(store.clone(), lab.snapshots(), stop.clone());

    lab.add_device("Lamp", DeviceKind::Switch).unwrap();

    let saved = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(snapshot) = store.load(&ProjectId::default_project()).await
                && !snapshot.devices.is_empty()
            {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("snapshot was not persisted");
    assert_eq!(saved.devices[0].id.as_str(), "lamp");
    stop.cancel();
    persister.await.unwrap();
}

#[tokio::test]
async fn stopping_the_persister_flushes_the_latest_state() {
    let (_dir, store) = store().await;
    let lab = Lab::new(Snapshot::default());
    let stop = CancellationToken::new();
    let persister = spawn_persister(store.clone(), lab.snapshots(), stop.clone());

    lab.add_device("Lamp", DeviceKind::Switch).unwrap();
    stop.cancel();
    persister.await.unwrap();

    let saved = store.load(&ProjectId::default_project()).await.unwrap();
    assert_eq!(saved, lab.snapshot());
    assert_eq!(saved.devices.len(), 1);
}

#[tokio::test]
async fn every_payload_lands_in_the_file_of_its_own_project() {
    let (_dir, store) = store().await;
    let home = ProjectId::parse("home").unwrap();
    let dacha = ProjectId::parse("dacha").unwrap();
    let lab = Lab::with_project(home.clone(), Snapshot::default());
    let stop = CancellationToken::new();
    let persister = spawn_persister(store.clone(), lab.snapshots(), stop.clone());

    lab.add_device("Lamp", DeviceKind::Switch).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    lab.replace(dacha.clone(), Snapshot::default());
    lab.add_device("Gate", DeviceKind::Sensor).unwrap();
    stop.cancel();
    persister.await.unwrap();

    let home_devices = store.load(&home).await.unwrap().devices;
    let dacha_devices = store.load(&dacha).await.unwrap().devices;
    assert_eq!(home_devices.len(), 1);
    assert_eq!(
        home_devices[0].id.as_str(),
        "lamp",
        "the old project keeps its own state"
    );
    assert_eq!(dacha_devices.len(), 1);
    assert_eq!(
        dacha_devices[0].id.as_str(),
        "gate",
        "the new project got only its own state"
    );
}
