use crate::helpers::spawn_app;
use serde_json::{Value, json};

const HEADER: &str = "x-project";

/// A lab running project `dacha` that also has a project `default` with a lamp in it, which is
/// what a page that missed the switch still believes it is looking at.
async fn app_on_dacha() -> crate::helpers::TestApp {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    app.create_project(
        json!({ "name": "Dacha", "from": { "snapshot": { "devices": [
        { "id": "lamp", "name": "Lamp", "kind": "switch", "state": false }
    ] } } }),
    )
    .await;
    assert_eq!(app.activate_project("dacha").await.status(), 200);
    app
}

#[tokio::test]
async fn a_page_showing_another_project_cannot_change_the_running_one() {
    let app = app_on_dacha().await;

    let attempts = [
        app.client
            .put(app.url("/api/devices/lamp/state"))
            .json(&json!({ "state": true })),
        app.client
            .post(app.url("/api/devices"))
            .json(&json!({ "name": "Gate", "kind": "sensor" })),
        app.client.delete(app.url("/api/devices/lamp")),
        app.client.post(app.url("/api/scenarios/x/run")),
    ];
    for attempt in attempts {
        let response = attempt.header(HEADER, "default").send().await.unwrap();

        assert_eq!(response.status(), 409);
        let body: Value = response.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("dacha"),
            "the answer says which project is running: {body}"
        );
    }

    let devices = app.devices().await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["state"], false, "nothing was changed");
}

#[tokio::test]
async fn a_page_showing_the_running_project_is_served_as_usual() {
    let app = app_on_dacha().await;

    let response = app
        .client
        .put(app.url("/api/devices/lamp/state"))
        .header(HEADER, "dacha")
        .json(&json!({ "state": true }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(app.devices().await[0]["state"], true);
}

#[tokio::test]
async fn plain_clients_and_reading_and_the_project_endpoints_are_not_affected() {
    let app = app_on_dacha().await;

    // No header: curl and scripts.
    assert_eq!(app.set_state("lamp", true).await.status(), 200);

    // Reading is harmless, whatever the page believes.
    let read = app
        .client
        .get(app.url("/api/devices"))
        .header(HEADER, "default")
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);

    // Switching, creating and deleting name their project themselves, so a stale page can recover.
    let create = app
        .client
        .post(app.url("/api/projects"))
        .header(HEADER, "default")
        .json(&json!({ "name": "Spare" }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 201);
    let switch = app
        .client
        .post(app.url("/api/projects/default/activate"))
        .header(HEADER, "somewhere-else")
        .send()
        .await
        .unwrap();
    assert_eq!(switch.status(), 200);
}
