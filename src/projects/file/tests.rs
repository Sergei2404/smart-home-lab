use super::*;
use crate::domain::{Device, DeviceKind, State};

fn snapshot() -> Snapshot {
    let mut lamp = Device::new("Lamp", DeviceKind::Switch).unwrap();
    lamp.state = State::Bool(true);
    Snapshot {
        devices: vec![lamp, Device::new("Door", DeviceKind::Sensor).unwrap()],
        ..Snapshot::default()
    }
}

#[tokio::test]
async fn snapshot_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let storage = ProjectFile::new(dir.path().join("project.json"));

    storage.save(&snapshot()).await.unwrap();

    assert_eq!(storage.load().await.unwrap(), snapshot());
    assert!(!dir.path().join("project.json.tmp").exists());
}

#[tokio::test]
async fn a_missing_file_is_reported_and_not_taken_for_an_empty_lab() {
    let dir = tempfile::tempdir().unwrap();
    let storage = ProjectFile::new(dir.path().join("project.json"));
    assert!(matches!(storage.load().await, Err(LoadError::Missing)));
}

#[tokio::test]
async fn corrupt_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    tokio::fs::write(&path, b"{ not json").await.unwrap();
    assert!(matches!(
        ProjectFile::new(path).load().await,
        Err(LoadError::Invalid(_))
    ));
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_not_a_missing_one_and_does_not_name_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    tokio::fs::create_dir(&path).await.unwrap();

    let error = ProjectFile::new(&path).load().await.unwrap_err();

    assert!(matches!(error, LoadError::Read(_)), "{error}");
    assert!(
        !error.to_string().contains(dir.path().to_str().unwrap()),
        "{error}"
    );
}

#[tokio::test]
async fn invalid_content_is_reported_without_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    tokio::fs::write(&path, b"{ not json").await.unwrap();

    let error = ProjectFile::new(&path).load().await.unwrap_err();

    assert!(error.to_string().contains("failed to parse"), "{error}");
    assert!(
        !error.to_string().contains(dir.path().to_str().unwrap()),
        "{error}"
    );
}

#[tokio::test]
async fn unknown_and_missing_fields_are_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    tokio::fs::write(&path, br#"{"future_field": 1}"#)
        .await
        .unwrap();
    assert_eq!(
        ProjectFile::new(path).load().await.unwrap(),
        Snapshot::default()
    );
}

#[tokio::test]
async fn missing_parent_directories_are_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join("lab.json");
    let storage = ProjectFile::new(&path);

    storage.save(&snapshot()).await.unwrap();

    assert_eq!(storage.load().await.unwrap(), snapshot());
}

#[tokio::test]
async fn the_temporary_file_keeps_the_whole_file_name() {
    for (name, tmp) in [
        ("project.json", "project.json.tmp"),
        ("state.db", "state.db.tmp"),
        ("lab", "lab.tmp"),
    ] {
        let storage = ProjectFile::new(PathBuf::from("dir").join(name));
        assert_eq!(storage.temporary_path(), PathBuf::from("dir").join(tmp));
    }
}

async fn load_error(json: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    tokio::fs::write(&path, json).await.unwrap();
    let error = ProjectFile::new(path).load().await.unwrap_err();
    format!("{error:#}")
}

#[tokio::test]
async fn a_file_that_breaks_the_rules_is_rejected_with_the_reason() {
    let repeat_without_delay = r#"{"scenarios": [{"id": "spin", "name": "Spin", "repeat": true,
        "steps": [{"type": "set", "device": "lamp", "state": true}]}]}"#;
    let message = load_error(repeat_without_delay).await;
    assert!(message.contains("contains invalid data"), "{message}");
    assert!(message.contains("scenario spin"), "{message}");
    assert!(message.contains("needs at least one delay"), "{message}");

    let wrong_state = r#"{"devices": [{"id": "humidity", "name": "Humidity", "kind": "percent", "state": true}]}"#;
    let message = load_error(wrong_state).await;
    assert!(message.contains("device humidity"), "{message}");

    let twice = r#"{"devices": [
        {"id": "lamp", "name": "Lamp", "kind": "switch", "state": false},
        {"id": "lamp", "name": "Other", "kind": "sensor", "state": false}]}"#;
    let message = load_error(twice).await;
    assert!(message.contains("lamp appears more than once"), "{message}");

    let mismatch = r#"{"devices": [{"id": "lamp", "name": "Lamp", "kind": "switch", "state": false}],
        "scenarios": [{"id": "bad", "name": "Bad", "repeat": false,
        "steps": [{"type": "set", "device": "lamp", "state": 5}]}]}"#;
    let message = load_error(mismatch).await;
    assert!(message.contains("scenario bad"), "{message}");
}

#[tokio::test]
async fn a_step_that_names_a_removed_device_is_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.json");
    tokio::fs::write(
        &path,
        r#"{"scenarios": [{"id": "old", "name": "Old", "repeat": false,
            "steps": [{"type": "set", "device": "gone", "state": true}]}]}"#,
    )
    .await
    .unwrap();

    let snapshot = ProjectFile::new(path).load().await.unwrap();

    assert_eq!(snapshot.scenarios.len(), 1);
}

#[tokio::test]
async fn numeric_devices_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let storage = ProjectFile::new(dir.path().join("project.json"));
    let mut temp = Device::new("Outside", DeviceKind::Temperature).unwrap();
    temp.state = State::Int(-12);
    let snapshot = Snapshot {
        devices: vec![temp],
        ..Snapshot::default()
    };

    storage.save(&snapshot).await.unwrap();

    assert_eq!(storage.load().await.unwrap(), snapshot);
}
