use crate::helpers::{TestApp, read_until, spawn_app};
use serde_json::{Value, json};

fn night_light(secs: u64) -> Value {
    json!({
        "name": "Night Light",
        "steps": [
            { "type": "set", "device": "lamp", "state": true },
            { "type": "delay", "secs": secs },
            { "type": "set", "device": "lamp", "state": false }
        ]
    })
}

async fn app_with_lamp() -> TestApp {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    app
}

#[tokio::test]
async fn create_scenario_returns_201_and_the_scenario() {
    let app = app_with_lamp().await;

    let response = app.create_scenario(night_light(10)).await;

    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], "night-light");
    assert_eq!(body["name"], "Night Light");
    assert_eq!(body["repeat"], false);
    assert_eq!(body["steps"], night_light(10)["steps"]);
}

#[tokio::test]
async fn duplicate_scenario_returns_409() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(10)).await;

    let response = app.create_scenario(night_light(5)).await;

    assert_eq!(response.status(), 409);
}

#[tokio::test]
async fn invalid_scenarios_return_400() {
    let app = app_with_lamp().await;
    let step_on = json!({ "type": "set", "device": "lamp", "state": true });
    for (case, body) in [
        ("no steps", json!({ "name": "x", "steps": [] })),
        (
            "zero delay",
            json!({ "name": "x", "steps": [{ "type": "delay", "secs": 0 }] }),
        ),
        (
            "huge delay",
            json!({ "name": "x", "steps": [{ "type": "delay", "secs": 86401 }] }),
        ),
        (
            "loop without delay",
            json!({ "name": "x", "repeat": true, "steps": [step_on.clone()] }),
        ),
        (
            "unknown device",
            json!({ "name": "x", "steps": [{ "type": "set", "device": "ghost", "state": true }] }),
        ),
        (
            "blank name",
            json!({ "name": "  ", "steps": [step_on.clone()] }),
        ),
        (
            "non-latin name",
            json!({ "name": "Сцена", "steps": [step_on] }),
        ),
    ] {
        let response = app.create_scenario(body).await;
        assert_eq!(response.status(), 400, "{case}");
        let body: Value = response.json().await.unwrap();
        assert!(body["error"].is_string(), "{case}");
    }
}

#[tokio::test]
async fn malformed_steps_are_rejected() {
    let app = app_with_lamp().await;
    for steps in [
        json!([{ "type": "explode" }]),
        json!([{ "type": "set", "device": "Bad Id", "state": true }]),
        json!([{ "type": "delay", "secs": -1 }]),
    ] {
        let response = app
            .create_scenario(json!({ "name": "x", "steps": steps }))
            .await;
        assert_eq!(response.status(), 400, "{steps}");
        let body: Value = response.json().await.unwrap();
        assert!(body["error"].is_string(), "{steps}");
    }
}

#[tokio::test]
async fn scenarios_are_listed() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(10)).await;

    let list: Vec<Value> = app.get("/api/scenarios").await.json().await.unwrap();

    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], "night-light");
}

#[tokio::test]
async fn update_replaces_the_scenario_but_keeps_its_id() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(10)).await;

    let response = app
        .update_scenario(
            "night-light",
            json!({ "name": "Renamed", "repeat": true, "steps": [{ "type": "delay", "secs": 3 }] }),
        )
        .await;

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], "night-light");
    assert_eq!(body["name"], "Renamed");
    assert_eq!(body["repeat"], true);
}

#[tokio::test]
async fn update_validates_and_requires_an_existing_scenario() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(10)).await;

    let invalid = app
        .update_scenario("night-light", json!({ "name": "x", "steps": [] }))
        .await;
    assert_eq!(invalid.status(), 400);

    let missing = app.update_scenario("ghost", night_light(10)).await;
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn delete_scenario_returns_204_then_404() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(10)).await;

    assert_eq!(app.delete_scenario("night-light").await.status(), 204);
    assert_eq!(app.delete_scenario("night-light").await.status(), 404);
    let list: Vec<Value> = app.get("/api/scenarios").await.json().await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn running_or_stopping_an_unknown_scenario_returns_404() {
    let app = spawn_app().await;
    assert_eq!(app.post("/api/scenarios/ghost/run").await.status(), 404);
    assert_eq!(app.post("/api/scenarios/ghost/stop").await.status(), 404);
}

#[tokio::test]
async fn scenario_plays_through_and_is_streamed() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(1)).await;
    let mut stream = app.get("/api/events").await;

    let response = app.post("/api/scenarios/night-light/run").await;
    assert_eq!(response.status(), 202);

    let received = read_until(&mut stream, "scenario_finished").await;
    assert!(received.contains("scenario_started"), "{received}");
    assert!(
        received.contains(r#""source":{"type":"scenario","id":"night-light"}"#),
        "{received}"
    );
    assert_eq!(app.devices().await[0]["state"], false);
    assert!(app.runs().await.is_empty());

    let log: Vec<Value> = app.get("/api/log").await.json().await.unwrap();
    let types: Vec<_> = log.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(
        types,
        [
            "device_added",
            "scenario_added",
            "scenario_started",
            "state_changed",
            "state_changed",
            "scenario_finished"
        ]
    );
}

#[tokio::test]
async fn running_scenario_is_listed_and_can_be_stopped() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(60)).await;
    let mut stream = app.get("/api/events").await;

    app.post("/api/scenarios/night-light/run").await;
    read_until(&mut stream, "state_changed").await;

    assert_eq!(
        app.runs().await,
        [json!({ "id": "night-light", "step": 1 })]
    );
    assert_eq!(
        app.post("/api/scenarios/night-light/run").await.status(),
        409
    );

    assert_eq!(
        app.post("/api/scenarios/night-light/stop").await.status(),
        204
    );
    read_until(&mut stream, "scenario_stopped").await;
    assert!(app.runs().await.is_empty());
    assert_eq!(app.devices().await[0]["state"], true);
}

#[tokio::test]
async fn deleting_a_running_scenario_stops_it() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(60)).await;
    let mut stream = app.get("/api/events").await;
    app.post("/api/scenarios/night-light/run").await;
    read_until(&mut stream, "state_changed").await;

    assert_eq!(app.delete_scenario("night-light").await.status(), 204);

    read_until(&mut stream, "scenario_stopped").await;
    assert!(app.runs().await.is_empty());
}

#[tokio::test]
async fn scenarios_can_set_numeric_values() {
    let app = spawn_app().await;
    app.create_device("Outside", "temperature").await;
    app.create_device("Humidity", "percent").await;

    let response = app
        .create_scenario(json!({
            "name": "Cold snap",
            "steps": [
                { "type": "set", "device": "outside", "state": -15 },
                { "type": "set", "device": "humidity", "state": 90 },
                { "type": "delay", "secs": 1 },
                { "type": "set", "device": "outside", "state": 2 }
            ]
        }))
        .await;
    assert_eq!(response.status(), 201);
    let mut stream = app.get("/api/events").await;

    app.post("/api/scenarios/cold-snap/run").await;

    read_until(&mut stream, "scenario_finished").await;
    let devices = app.devices().await;
    assert_eq!(devices[0]["state"], 2);
    assert_eq!(devices[1]["state"], 90);
}

#[tokio::test]
async fn scenario_values_must_fit_the_device() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    app.create_device("Humidity", "percent").await;

    for (case, step) in [
        (
            "number on a switch",
            json!({ "type": "set", "device": "lamp", "state": 1 }),
        ),
        (
            "on/off on a percent sensor",
            json!({ "type": "set", "device": "humidity", "state": true }),
        ),
        (
            "percent above 100",
            json!({ "type": "set", "device": "humidity", "state": 101 }),
        ),
        (
            "negative percent",
            json!({ "type": "set", "device": "humidity", "state": -1 }),
        ),
    ] {
        let response = app
            .create_scenario(json!({ "name": "x", "steps": [step] }))
            .await;
        assert_eq!(response.status(), 400, "{case}");
    }
}

#[tokio::test]
async fn scenario_can_take_a_device_offline_and_bring_it_back() {
    let app = spawn_app().await;
    app.create_device("Door", "sensor").await;
    let response = app
        .create_scenario(json!({
            "name": "Dropout",
            "steps": [
                { "type": "availability", "device": "door", "online": false },
                { "type": "delay", "secs": 1 },
                { "type": "availability", "device": "door", "online": true }
            ]
        }))
        .await;
    assert_eq!(response.status(), 201);
    let mut stream = app.get("/api/events").await;

    app.post("/api/scenarios/dropout/run").await;

    read_until(&mut stream, r#""online":false"#).await;
    assert_eq!(app.devices().await[0]["online"], false);
    read_until(&mut stream, "scenario_finished").await;
    assert_eq!(app.devices().await[0]["online"], true);
}

#[tokio::test]
async fn availability_step_needs_an_existing_device() {
    let app = spawn_app().await;

    let response = app
        .create_scenario(json!({
            "name": "Ghost",
            "steps": [{ "type": "availability", "device": "ghost", "online": false }]
        }))
        .await;

    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn a_device_used_by_a_scenario_cannot_be_deleted() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(10)).await;

    let response = app.delete_device("lamp").await;

    assert_eq!(response.status(), 409);
    let body: Value = response.json().await.unwrap();
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("used by scenarios night-light"), "{error}");
    assert_eq!(app.devices().await.len(), 1);

    assert_eq!(app.delete_scenario("night-light").await.status(), 204);
    assert_eq!(app.delete_device("lamp").await.status(), 204);
}

#[tokio::test]
async fn renaming_a_scenario_to_a_taken_name_returns_409() {
    let app = app_with_lamp().await;
    app.create_scenario(night_light(10)).await;
    let mut other = night_light(10);
    other["name"] = json!("Beta");
    app.create_scenario(other).await;

    let mut rename = night_light(10);
    rename["name"] = json!("beta");
    let response = app.update_scenario("night-light", rename).await;

    assert_eq!(response.status(), 409);
}

#[tokio::test]
async fn a_misspelled_scenario_key_is_an_error_not_a_scenario_that_does_not_repeat() {
    let app = app_with_lamp().await;

    let response = app
        .create_scenario(json!({
            "name": "Blink",
            "repeats": true,
            "steps": [
                { "type": "set", "device": "lamp", "state": true },
                { "type": "delay", "secs": 1 }
            ]
        }))
        .await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("unknown field"),
        "{body}"
    );
    let scenarios: Vec<Value> = app.get("/api/scenarios").await.json().await.unwrap();
    assert!(scenarios.is_empty());
}

#[tokio::test]
async fn a_renamed_scenario_keeps_its_id_and_the_conflict_says_so() {
    let app = app_with_lamp().await;
    let steps = json!([{ "type": "set", "device": "lamp", "state": true }]);
    app.create_scenario(json!({ "name": "Blink", "steps": steps }))
        .await;

    let renamed = app
        .update_scenario("blink", json!({ "name": "Pulse", "steps": steps }))
        .await;
    assert_eq!(renamed.status(), 200);
    let body: Value = renamed.json().await.unwrap();
    assert_eq!(body["id"], "blink", "an id does not follow the name");

    let again = app
        .create_scenario(json!({ "name": "Blink", "steps": steps }))
        .await;

    assert_eq!(again.status(), 409);
    let body: Value = again.json().await.unwrap();
    let message = body["error"].as_str().unwrap();
    assert!(
        message.contains("scenario id blink is already used by \"Pulse\""),
        "{message}"
    );
    assert!(
        message.contains("does not change when it is renamed"),
        "{message}"
    );
    assert_eq!(app.post("/api/scenarios/pulse/run").await.status(), 404);
    assert_eq!(app.post("/api/scenarios/blink/run").await.status(), 202);
}
