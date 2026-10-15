use crate::helpers::spawn_app;
use serde_json::json;

#[tokio::test]
async fn health_check_works() {
    let app = spawn_app().await;
    assert_eq!(app.get("/health").await.status(), 200);
}

#[tokio::test]
async fn create_device_returns_201_and_the_device() {
    let app = spawn_app().await;

    let response = app.create_device("Living Lamp", "switch").await;

    assert_eq!(response.status(), 201);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body,
        json!({ "id": "living-lamp", "name": "Living Lamp", "kind": "switch", "state": false, "online": true })
    );
}

#[tokio::test]
async fn created_devices_are_listed_in_creation_order() {
    let app = spawn_app().await;
    app.create_device("Zebra", "sensor").await;
    app.create_device("Alpha", "relay").await;

    let ids: Vec<_> = app
        .devices()
        .await
        .into_iter()
        .map(|d| d["id"].as_str().unwrap().to_owned())
        .collect();

    assert_eq!(ids, ["zebra", "alpha"]);
}

#[tokio::test]
async fn duplicate_device_returns_409() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;

    let response = app.create_device("lamp", "sensor").await;

    assert_eq!(response.status(), 409);
    assert_eq!(app.devices().await.len(), 1);
}

#[tokio::test]
async fn invalid_device_name_returns_400() {
    let app = spawn_app().await;
    for name in ["", "   ", "Лампа", &"a".repeat(65)] {
        let response = app.create_device(name, "sensor").await;
        assert_eq!(response.status(), 400, "name: {name:?}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"].is_string());
    }
}

#[tokio::test]
async fn unknown_device_kind_is_rejected_with_a_json_error() {
    let app = spawn_app().await;

    let response = app.create_device("Lamp", "toaster").await;

    assert_eq!(response.status(), 400);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body: serde_json::Value = response.json().await.unwrap();
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("unknown variant `toaster`"), "{error}");
}

#[tokio::test]
async fn set_state_updates_the_device() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;

    let response = app.set_state("lamp", true).await;

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["state"], true);
    assert_eq!(app.devices().await[0]["state"], true);

    app.set_state("lamp", false).await;
    assert_eq!(app.devices().await[0]["state"], false);
}

#[tokio::test]
async fn set_state_of_unknown_device_returns_404() {
    let app = spawn_app().await;
    assert_eq!(app.set_state("ghost", true).await.status(), 404);
}

#[tokio::test]
async fn malformed_device_id_is_a_json_400() {
    let app = spawn_app().await;

    for response in [
        app.set_state("Bad%20Id", true).await,
        app.delete_device("UPPER").await,
        app.set_availability("Bad%20Id", true).await,
    ] {
        assert_eq!(response.status(), 400);
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("invalid id"),
            "{body}"
        );
    }
}

#[tokio::test]
async fn bad_request_bodies_get_a_json_error_too() {
    let app = spawn_app().await;
    let url = app.url("/api/devices");

    let not_json = app
        .client
        .post(&url)
        .header("content-type", "application/json")
        .body("{ nope")
        .send()
        .await
        .unwrap();
    assert_eq!(not_json.status(), 400);
    assert!(not_json.json::<serde_json::Value>().await.unwrap()["error"].is_string());

    let wrong_type = app
        .client
        .post(&url)
        .header("content-type", "text/plain")
        .body(r#"{"name":"x","kind":"sensor"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_type.status(), 415);
    assert!(wrong_type.json::<serde_json::Value>().await.unwrap()["error"].is_string());

    let missing_field = app
        .client
        .post(&url)
        .json(&json!({ "name": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_field.status(), 400);
    let body: serde_json::Value = missing_field.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("kind"), "{body}");
}

#[tokio::test]
async fn unknown_routes_get_a_json_404() {
    let app = spawn_app().await;

    let response = app.get("/api/nothing-here").await;

    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "no such endpoint");
}

#[tokio::test]
async fn a_wrong_method_gets_a_json_405_that_still_says_what_is_allowed() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;

    for (case, request) in [
        (
            "no single-device resource",
            app.client.get(app.url("/api/devices/lamp")),
        ),
        (
            "a list is not replaced",
            app.client.put(app.url("/api/devices")),
        ),
        (
            "state is not created",
            app.client.post(app.url("/api/devices/lamp/state")),
        ),
    ] {
        let response = request.send().await.unwrap();

        assert_eq!(response.status(), 405, "{case}");
        assert!(response.headers().contains_key("allow"), "{case}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("not allowed"),
            "{case}: {body}"
        );
    }
}

#[tokio::test]
async fn unknown_keys_in_a_request_body_are_refused_instead_of_ignored() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;

    let attempts = [
        (
            "a device",
            app.client
                .post(app.url("/api/devices"))
                .json(&json!({ "name": "Door", "kind": "sensor", "colour": "red" })),
        ),
        (
            "a state (the old key)",
            app.client
                .put(app.url("/api/devices/lamp/state"))
                .json(&json!({ "state": true, "on": true })),
        ),
        (
            "an availability",
            app.client
                .put(app.url("/api/devices/lamp/availability"))
                .json(&json!({ "online": false, "offline": true })),
        ),
    ];
    for (case, request) in attempts {
        let response = request.send().await.unwrap();

        assert_eq!(response.status(), 400, "{case}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("unknown field"),
            "{case}: {body}"
        );
    }
    let devices = app.devices().await;
    assert_eq!(devices.len(), 1, "no device was created");
    assert_eq!(devices[0]["state"], false);
    assert_eq!(devices[0]["online"], true);
}

#[tokio::test]
async fn the_not_found_answer_says_what_was_not_found() {
    let app = spawn_app().await;

    let response = app.set_state("ghost", true).await;

    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"], "device ghost not found");
}

#[tokio::test]
async fn delete_device_returns_204_then_404() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;

    assert_eq!(app.delete_device("lamp").await.status(), 204);
    assert!(app.devices().await.is_empty());
    assert_eq!(app.delete_device("lamp").await.status(), 404);
}

#[tokio::test]
async fn numeric_sensors_start_at_zero() {
    let app = spawn_app().await;

    let humidity: serde_json::Value = app
        .create_device("Humidity", "percent")
        .await
        .json()
        .await
        .unwrap();
    let outside: serde_json::Value = app
        .create_device("Outside", "temperature")
        .await
        .json()
        .await
        .unwrap();

    assert_eq!(
        humidity,
        json!({ "id": "humidity", "name": "Humidity", "kind": "percent", "state": 0, "online": true })
    );
    assert_eq!(
        outside,
        json!({ "id": "outside", "name": "Outside", "kind": "temperature", "state": 0, "online": true })
    );
}

#[tokio::test]
async fn percent_sensor_accepts_0_to_100() {
    let app = spawn_app().await;
    app.create_device("Humidity", "percent").await;

    for value in [0, 57, 100] {
        let response = app.set_state("humidity", value).await;
        assert_eq!(response.status(), 200, "{value}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["state"], value);
    }
    assert_eq!(app.devices().await[0]["state"], 100);
}

#[tokio::test]
async fn percent_sensor_rejects_values_out_of_range() {
    let app = spawn_app().await;
    app.create_device("Humidity", "percent").await;
    app.set_state("humidity", 40).await;

    for value in [-1, 101] {
        let response = app.set_state("humidity", value).await;
        assert_eq!(response.status(), 400, "{value}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("range"), "{body}");
    }
    assert_eq!(app.devices().await[0]["state"], 40);
}

#[tokio::test]
async fn temperature_sensor_accepts_negative_and_large_values() {
    let app = spawn_app().await;
    app.create_device("Outside", "temperature").await;

    for value in [-273, -5, 0, 36, 1_000_000] {
        let response = app.set_state("outside", value).await;
        assert_eq!(response.status(), 200, "{value}");
        assert_eq!(app.devices().await[0]["state"], value);
    }
}

#[tokio::test]
async fn state_must_match_the_kind_of_device() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    app.create_device("Outside", "temperature").await;

    assert_eq!(app.set_state("lamp", 5).await.status(), 400);
    assert_eq!(app.set_state("outside", true).await.status(), 400);
    assert_eq!(app.devices().await[0]["state"], false);
    assert_eq!(app.devices().await[1]["state"], 0);
}

#[tokio::test]
async fn malformed_states_are_rejected_and_explained() {
    let app = spawn_app().await;
    app.create_device("Outside", "temperature").await;

    for state in [
        json!(1.5),
        json!("on"),
        json!(null),
        json!(3_000_000_000_i64),
    ] {
        let response = app.set_state("outside", state.clone()).await;
        assert_eq!(response.status(), 400, "{state}");
        let body: serde_json::Value = response.json().await.unwrap();
        let error = body["error"].as_str().unwrap();
        assert!(
            error.contains("true, false or a whole number between -2147483648 and 2147483647"),
            "{state}: {error}"
        );
    }
    assert_eq!(app.devices().await[0]["state"], 0);
}

#[tokio::test]
async fn devices_can_go_offline_and_come_back() {
    let app = spawn_app().await;
    app.create_device("Door", "sensor").await;

    let response = app.set_availability("door", false).await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["online"], false);
    assert_eq!(app.devices().await[0]["online"], false);

    assert_eq!(app.set_availability("door", true).await.status(), 200);
    assert_eq!(app.devices().await[0]["online"], true);
}

#[tokio::test]
async fn availability_is_independent_of_the_state() {
    let app = spawn_app().await;
    app.create_device("Outside", "temperature").await;
    app.set_state("outside", -4).await;

    app.set_availability("outside", false).await;
    let still_changeable = app.set_state("outside", 9).await;

    assert_eq!(still_changeable.status(), 200);
    let device = &app.devices().await[0];
    assert_eq!(device["state"], 9);
    assert_eq!(device["online"], false);
}

#[tokio::test]
async fn availability_of_an_unknown_device_returns_404() {
    let app = spawn_app().await;
    assert_eq!(app.set_availability("ghost", false).await.status(), 404);
}

#[tokio::test]
async fn availability_needs_a_boolean() {
    let app = spawn_app().await;
    app.create_device("Door", "sensor").await;
    let response = app
        .client
        .put(app.url("/api/devices/door/availability"))
        .json(&json!({ "online": "no" }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_client_error());
    assert_eq!(app.devices().await[0]["online"], true);
}
