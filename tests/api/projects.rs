use crate::helpers::{TestApp, read_until, spawn_app, spawn_app_with_persister};
use serde_json::{Value, json};

fn ids(list: &Value, key: &str) -> Vec<String> {
    list[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

async fn count(app: &TestApp, path: &str) -> usize {
    app.get(path)
        .await
        .json::<Vec<Value>>()
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn a_fresh_lab_has_the_default_project_and_two_presets() {
    let app = spawn_app().await;

    let list = app.projects().await;

    assert_eq!(list["active"], "default");
    assert_eq!(ids(&list, "projects"), ["default"]);
    assert_eq!(ids(&list, "presets"), ["apartment", "dacha"]);
    let apartment = &list["presets"][0];
    assert_eq!(apartment["title"], "Apartment");
    assert!(apartment["devices"].as_u64().unwrap() >= 8);
    assert!(apartment["scenarios"].as_u64().unwrap() >= 3);
}

#[tokio::test]
async fn a_project_made_from_a_preset_can_be_switched_to() {
    let app = spawn_app().await;

    let created = app
        .create_project(json!({ "name": "My Flat", "from": { "preset": "apartment" } }))
        .await;
    assert_eq!(created.status(), 201);
    assert_eq!(
        created.json::<Value>().await.unwrap(),
        json!({ "id": "my-flat" })
    );
    assert_eq!(
        app.lab.project().as_str(),
        "default",
        "creating does not switch"
    );
    assert_eq!(count(&app, "/api/devices").await, 0);

    let activated = app.activate_project("my-flat").await;

    assert_eq!(activated.status(), 200);
    assert_eq!(
        activated.json::<Value>().await.unwrap(),
        json!({ "active": "my-flat" })
    );
    assert!(count(&app, "/api/devices").await >= 8);
    assert!(count(&app, "/api/scenarios").await >= 3);
    let status: Value = app.get("/api/status").await.json().await.unwrap();
    assert_eq!(status["project"], "my-flat");
    let log: Vec<Value> = app.get("/api/log").await.json().await.unwrap();
    assert_eq!(log.len(), 1, "a project starts with a fresh log");
    assert_eq!(log[0]["type"], "project_changed");
    assert_eq!(log[0]["project"], "my-flat");
}

#[tokio::test]
async fn projects_keep_their_own_devices_and_states() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    app.set_state("lamp", true).await;
    app.create_project(json!({ "name": "Dacha", "from": { "preset": "dacha" } }))
        .await;

    app.activate_project("dacha").await;
    assert_eq!(
        app.set_state("lamp", true).await.status(),
        404,
        "the lamp belongs to the other project"
    );
    app.set_state("gate", true).await;
    app.activate_project("default").await;

    let devices = app.devices().await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["id"], "lamp");
    assert_eq!(
        devices[0]["state"], true,
        "the lamp state survived the round trip"
    );

    app.activate_project("dacha").await;
    let gate = app
        .devices()
        .await
        .into_iter()
        .find(|d| d["id"] == "gate")
        .unwrap();
    assert_eq!(
        gate["state"], true,
        "so did the change made in the other project"
    );
}

#[tokio::test]
async fn copying_a_project_includes_unsaved_live_changes() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    app.set_state("lamp", true).await;

    let created = app
        .create_project(json!({ "name": "Copy", "from": { "copy_of": "default" } }))
        .await;
    assert_eq!(created.status(), 201);
    app.activate_project("copy").await;

    assert_eq!(app.devices().await[0]["state"], true);
}

#[tokio::test]
async fn creating_a_project_validates_the_request() {
    let app = spawn_app().await;
    app.create_project(json!({ "name": "Home" })).await;

    for (case, body, status) in [
        ("name taken", json!({ "name": "home" }), 409),
        ("name without latin letters", json!({ "name": "Дача" }), 400),
        ("blank name", json!({ "name": "  " }), 400),
        (
            "unknown preset",
            json!({ "name": "x", "from": { "preset": "castle" } }),
            400,
        ),
        (
            "two sources",
            json!({ "name": "x", "from": { "preset": "dacha", "copy_of": "home" } }),
            400,
        ),
        (
            "unknown project to copy",
            json!({ "name": "x", "from": { "copy_of": "ghost" } }),
            404,
        ),
        (
            "malformed id to copy",
            json!({ "name": "x", "from": { "copy_of": "../etc" } }),
            400,
        ),
        (
            "missing name",
            json!({ "from": { "preset": "dacha" } }),
            400,
        ),
        (
            "an explicit null snapshot is an import, not an empty project",
            json!({ "name": "x", "from": { "snapshot": null } }),
            400,
        ),
        (
            "a misspelled key is not ignored",
            json!({ "name": "x", "presets": "apartment" }),
            400,
        ),
        (
            "the old flat shape is no longer understood",
            json!({ "name": "x", "preset": "apartment" }),
            400,
        ),
        (
            "an unknown way to start",
            json!({ "name": "x", "from": { "template": "apartment" } }),
            400,
        ),
        ("an empty choice", json!({ "name": "x", "from": {} }), 400),
    ] {
        let response = app.create_project(body).await;
        assert_eq!(response.status(), status, "{case}");
        let body: Value = response.json().await.unwrap();
        assert!(body["error"].is_string(), "{case}");
    }
    assert_eq!(ids(&app.projects().await, "projects"), ["default", "home"]);
}

#[tokio::test]
async fn a_project_can_say_explicitly_that_it_starts_empty() {
    let app = spawn_app().await;

    let created = app
        .create_project(json!({ "name": "Blank", "from": "empty" }))
        .await;

    assert_eq!(created.status(), 201);
    assert_eq!(ids(&app.projects().await, "projects"), ["blank", "default"]);
}

#[tokio::test]
async fn switching_to_a_missing_or_broken_project_changes_nothing() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    tokio::fs::write(app.store.dir().join("broken.json"), "{ nope")
        .await
        .unwrap();

    assert_eq!(app.activate_project("ghost").await.status(), 404);
    let broken = app.activate_project("broken").await;
    assert_eq!(broken.status(), 400);
    let body: Value = broken.json().await.unwrap();
    let message = body["error"].as_str().unwrap();
    assert!(message.contains("cannot be used"), "{body}");
    assert!(
        !message.contains(app.store.dir().to_str().unwrap()),
        "an API answer must not reveal where the files are: {body}"
    );
    assert_eq!(app.activate_project("..%2Fetc").await.status(), 400);

    assert_eq!(app.lab.project().as_str(), "default");
    assert_eq!(app.devices().await.len(), 1);
    let list = app.projects().await;
    let broken = list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "broken")
        .unwrap();
    assert!(
        broken["error"].is_string(),
        "a broken file is listed with its problem so it can be deleted"
    );
}

#[tokio::test]
async fn projects_can_be_deleted_except_the_active_one() {
    let app = spawn_app().await;
    app.create_project(json!({ "name": "Old" })).await;

    assert_eq!(app.delete_project("default").await.status(), 409);
    assert_eq!(app.delete_project("old").await.status(), 204);
    assert_eq!(app.delete_project("old").await.status(), 404);
    assert_eq!(app.delete_project("..%2Fx").await.status(), 400);

    assert_eq!(ids(&app.projects().await, "projects"), ["default"]);
}

#[tokio::test]
async fn export_and_import_round_trip() {
    let app = spawn_app().await;
    app.create_device("Outside", "temperature").await;
    app.set_state("outside", -7).await;

    let exported = app.get("/api/projects/default/export").await;
    assert_eq!(exported.status(), 200);
    assert_eq!(
        exported.headers()["content-disposition"],
        "attachment; filename=\"default.json\""
    );
    assert_eq!(exported.headers()["content-type"], "application/json");
    let text = exported.text().await.unwrap();
    assert!(
        text.lines().count() > 5,
        "an exported project is a file people open and edit, so it is pretty-printed"
    );
    let file: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(file["devices"][0]["state"], -7);

    let imported = app
        .create_project(json!({ "name": "Imported", "from": { "snapshot": file } }))
        .await;
    assert_eq!(imported.status(), 201);
    app.activate_project("imported").await;
    assert_eq!(app.devices().await[0]["state"], -7);

    assert_eq!(app.get("/api/projects/ghost/export").await.status(), 404);
}

#[tokio::test]
async fn an_uploaded_file_must_be_a_valid_project() {
    let app = spawn_app().await;
    for (case, snapshot) in [
        ("not a project", json!({ "hello": "world" })),
        ("an array", json!([1, 2, 3])),
        (
            "breaks the rules",
            json!({ "devices": [{ "id": "h", "name": "H", "kind": "percent", "state": 500 }] }),
        ),
        (
            "repeats without a delay",
            json!({
                "devices": [{ "id": "l", "name": "L", "kind": "switch", "state": false }],
                "scenarios": [{ "id": "spin", "name": "Spin", "repeat": true,
                    "steps": [{ "type": "set", "device": "l", "state": true }] }]
            }),
        ),
    ] {
        let response = app
            .create_project(json!({ "name": "Upload", "from": { "snapshot": snapshot } }))
            .await;
        assert_eq!(response.status(), 400, "{case}");
    }
    assert_eq!(ids(&app.projects().await, "projects"), ["default"]);
}

#[tokio::test]
async fn a_switch_is_announced_over_sse() {
    let app = spawn_app().await;
    app.create_project(json!({ "name": "Dacha", "from": { "preset": "dacha" } }))
        .await;
    let mut stream = app.get("/api/events").await;

    app.activate_project("dacha").await;

    let received = read_until(&mut stream, "projects_changed").await;
    assert!(
        received.contains(r#""type":"project_changed""#),
        "{received}"
    );
    assert!(received.contains(r#""project":"dacha""#), "{received}");
}

#[tokio::test]
async fn a_scenario_in_progress_is_stopped_by_the_switch() {
    let app = spawn_app().await;
    app.create_device("Lamp", "switch").await;
    app.create_scenario(json!({
        "name": "Long",
        "steps": [
            { "type": "set", "device": "lamp", "state": true },
            { "type": "delay", "secs": 60 }
        ]
    }))
    .await;
    app.create_project(json!({ "name": "Other" })).await;
    let mut stream = app.get("/api/events").await;
    app.post("/api/scenarios/long/run").await;
    read_until(&mut stream, "state_changed").await;

    let response = app.activate_project("other").await;

    assert_eq!(response.status(), 200);
    assert!(app.runs().await.is_empty());
    read_until(&mut stream, "scenario_stopped").await;
    assert!(
        app.devices().await.is_empty(),
        "the new project has no devices"
    );
    assert_eq!(app.post("/api/scenarios/long/run").await.status(), 404);
}

#[tokio::test]
async fn another_origin_cannot_switch_or_delete_projects() {
    let app = spawn_app().await;
    app.create_project(json!({ "name": "Other" })).await;

    for attempt in [
        app.client.post(app.url("/api/projects/other/activate")),
        app.client.delete(app.url("/api/projects/other")),
        app.client
            .post(app.url("/api/projects"))
            .json(&json!({ "name": "x" })),
    ] {
        let response = attempt
            .header("origin", "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    assert_eq!(app.lab.project().as_str(), "default");
    assert_eq!(ids(&app.projects().await, "projects"), ["default", "other"]);
}

fn device_entries(count: usize) -> Vec<Value> {
    (0..count)
        .map(|i| json!({ "id": format!("d{i}"), "name": format!("D{i}"), "kind": "sensor", "state": false }))
        .collect()
}

#[tokio::test]
async fn a_full_project_refuses_more_devices_with_a_conflict() {
    let app = spawn_app().await;
    for i in 0..smart_home_lab::domain::MAX_DEVICES {
        app.lab
            .add_device(
                &format!("Device {i}"),
                smart_home_lab::domain::DeviceKind::Sensor,
            )
            .unwrap();
    }

    let refused = app.create_device("One more", "switch").await;

    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("at most 500 devices"),
        "{body}"
    );
}

#[tokio::test]
async fn an_uploaded_project_over_the_limits_is_refused() {
    let app = spawn_app().await;
    let max = smart_home_lab::domain::MAX_DEVICES;

    let too_many = app
        .create_project(
            json!({ "name": "Big", "from": { "snapshot": { "devices": device_entries(max + 1) } } }),
        )
        .await;
    assert_eq!(too_many.status(), 400);
    let body: Value = too_many.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("at most 500 devices"),
        "{body}"
    );

    let at_the_limit = app
        .create_project(
            json!({ "name": "Big", "from": { "snapshot": { "devices": device_entries(max) } } }),
        )
        .await;
    assert_eq!(at_the_limit.status(), 201);
}

#[tokio::test]
async fn a_request_body_over_two_megabytes_is_refused_as_json() {
    const TWO_MIB: usize = 2 * 1024 * 1024; // the limit the README promises
    let app = spawn_app().await;

    let response = app
        .create_project(
            json!({ "name": "Huge", "from": { "snapshot": { "devices": [], "note": "x".repeat(TWO_MIB) } } }),
        )
        .await;

    assert_eq!(response.status(), 413);
    let body: Value = response.json().await.unwrap();
    assert!(body["error"].is_string(), "{body}");
    assert_eq!(ids(&app.projects().await, "projects"), ["default"]);

    let large_but_fine = app
        .create_project(json!({ "name": "Large", "from": { "snapshot": { "devices": [], "note": "x".repeat(TWO_MIB / 2) } } }))
        .await;
    assert_eq!(large_but_fine.status(), 201);
}

#[test]
fn a_project_at_every_limit_still_fits_in_one_request() {
    use smart_home_lab::{
        domain::{
            Device, DeviceId, DeviceKind, MAX_DEVICES, MAX_SCENARIOS, MAX_STEPS, Scenario,
            ScenarioId, Snapshot, State, Step,
        },
        web::MAX_BODY_BYTES,
    };
    let devices: Vec<_> = (0..MAX_DEVICES)
        .map(|i| Device::new(&format!("Device number {i}"), DeviceKind::Switch).unwrap())
        .collect();
    let steps: Vec<_> = (0..MAX_STEPS)
        .map(|i| Step::Set {
            device: DeviceId::parse(&format!("device-number-{i}")).unwrap(),
            state: State::Bool(i % 2 == 0),
        })
        .collect();
    let scenarios: Vec<_> = (0..MAX_SCENARIOS)
        .map(|i| {
            Scenario::with_id(
                ScenarioId::parse(&format!("scenario-{i}")).unwrap(),
                &format!("Scenario number {i}"),
                false,
                steps.clone(),
            )
            .unwrap()
        })
        .collect();
    let snapshot = Snapshot { devices, scenarios };
    snapshot.validate().unwrap();

    // The export is pretty-printed, and an exported project must be importable again.
    let size = serde_json::to_vec_pretty(&snapshot).unwrap().len();

    assert!(
        size < MAX_BODY_BYTES,
        "a project at its limits is {size} bytes, the body limit is {MAX_BODY_BYTES}"
    );
}

#[tokio::test]
async fn changes_made_through_the_api_are_saved_in_the_file_of_their_own_project() {
    let app = spawn_app_with_persister().await;
    app.create_device("Lamp", "switch").await;
    app.create_project(json!({ "name": "Dacha", "from": { "preset": "dacha" } }))
        .await;
    let in_default = app
        .saved("default", |snapshot| snapshot.devices.len() == 1)
        .await;
    assert_eq!(in_default.devices[0].id.as_str(), "lamp");

    app.activate_project("dacha").await;
    app.create_device("Gate House", "sensor").await;
    let in_dacha = app
        .saved("dacha", |snapshot| {
            snapshot
                .devices
                .iter()
                .any(|d| d.id.as_str() == "gate-house")
        })
        .await;

    assert!(
        in_dacha.devices.iter().all(|d| d.id.as_str() != "lamp"),
        "the first project's device leaked into the second"
    );
    let still_default = app.saved("default", |_| true).await;
    assert_eq!(
        still_default.devices.len(),
        1,
        "the second project's device leaked into the first"
    );

    app.activate_project("default").await;
    app.set_state("lamp", true).await;
    app.saved("default", |snapshot| {
        snapshot.devices[0].state == smart_home_lab::domain::State::Bool(true)
    })
    .await;
    let dacha_again = app.saved("dacha", |_| true).await;
    assert_eq!(
        dacha_again, in_dacha,
        "switching back and changing the first project left the second one alone"
    );
}
