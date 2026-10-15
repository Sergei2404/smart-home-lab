use crate::helpers::spawn_app;
use serde_json::{Value, json};

#[tokio::test]
async fn state_changes_from_another_origin_are_refused() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;

    let attempts = [
        app.client
            .put(app.url("/api/devices/lamp/state"))
            .json(&json!({ "state": true })),
        app.client.post(app.url("/api/scenarios/x/run")),
        app.client.delete(app.url("/api/devices/lamp")),
    ];
    for attempt in attempts {
        let response = attempt
            .header("origin", "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"], "cross-origin requests are not allowed");
    }
    assert_eq!(app.devices().await[0]["state"], false);
}

#[tokio::test]
async fn the_page_itself_and_plain_clients_are_allowed() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    let host = app.address.trim_start_matches("http://").to_owned();

    // What the UI sends: an Origin that matches the Host.
    let same_origin = app
        .client
        .put(app.url("/api/devices/lamp/state"))
        .header("origin", format!("http://{host}"))
        .json(&json!({ "state": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(same_origin.status(), 200);

    // curl and friends send no Origin at all.
    assert_eq!(app.set_state("lamp", false).await.status(), 200);
}

#[tokio::test]
async fn reading_is_allowed_from_anywhere() {
    let app = spawn_app().await;
    let response = app
        .client
        .get(app.url("/api/devices"))
        .header("origin", "http://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn an_opaque_origin_is_refused() {
    let app = spawn_app().await;
    let response = app
        .client
        .post(app.url("/api/scenarios/x/stop"))
        .header("origin", "null")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
}
