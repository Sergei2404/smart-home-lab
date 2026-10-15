use crate::helpers::{read_until, spawn_app};

#[tokio::test]
async fn state_changes_are_streamed_over_sse() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    let mut stream = app.get("/api/events").await;
    assert_eq!(stream.headers()["content-type"], "text/event-stream");

    app.set_state("lamp", true).await;

    let received = read_until(&mut stream, "state_changed").await;
    assert!(received.contains(r#""id":"lamp""#), "{received}");
    assert!(received.contains(r#""state":true"#), "{received}");
    assert!(received.contains(r#""source":{"type":"ui"}"#), "{received}");
}

#[tokio::test]
async fn device_lifecycle_is_streamed_over_sse() {
    let app = spawn_app().await;
    let mut stream = app.get("/api/events").await;

    app.create_device("Door", "sensor").await;
    app.delete_device("door").await;

    let received = read_until(&mut stream, "device_removed").await;
    assert!(received.contains("device_added"), "{received}");
}

#[tokio::test]
async fn log_returns_recent_events() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    app.set_state("lamp", true).await;
    app.set_state("lamp", true).await;

    let log: Vec<serde_json::Value> = app.get("/api/log").await.json().await.unwrap();

    let types: Vec<_> = log.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["device_added", "state_changed"]);
}

#[tokio::test]
async fn status_reports_that_mqtt_is_disabled_by_default() {
    let app = spawn_app().await;

    let status: serde_json::Value = app.get("/api/status").await.json().await.unwrap();

    assert_eq!(
        status,
        serde_json::json!({ "mqtt": "disabled", "project": "default" })
    );
}

#[tokio::test]
async fn numeric_values_are_streamed_as_numbers() {
    let app = spawn_app().await;
    app.create_device("Outside", "temperature").await;
    let mut stream = app.get("/api/events").await;

    app.set_state("outside", -7).await;

    let received = read_until(&mut stream, "state_changed").await;
    assert!(received.contains(r#""state":-7"#), "{received}");
}

#[tokio::test]
async fn availability_changes_are_streamed() {
    let app = spawn_app().await;
    app.create_device("Door", "sensor").await;
    let mut stream = app.get("/api/events").await;

    app.set_availability("door", false).await;

    let received = read_until(&mut stream, "availability_changed").await;
    assert!(received.contains(r#""id":"door""#), "{received}");
    assert!(received.contains(r#""online":false"#), "{received}");
    assert!(received.contains(r#""source":{"type":"ui"}"#), "{received}");
}

#[tokio::test]
async fn shutting_down_ends_the_event_streams_and_lets_the_server_finish() {
    let app = spawn_app().await;
    let mut stream = app.get("/api/events").await;
    app.create_device("Lamp", "switch").await;
    read_until(&mut stream, "device_added").await;

    app.shutdown.cancel();

    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Ok(Some(_)) = stream.chunk().await {}
    })
    .await;
    assert!(ended.is_ok(), "the event stream did not end");
    let finished = tokio::time::timeout(std::time::Duration::from_secs(5), app.server).await;
    assert!(
        matches!(finished, Ok(Ok(Ok(())))),
        "the server did not finish after its streams ended: {finished:?}"
    );
}
