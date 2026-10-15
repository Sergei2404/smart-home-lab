use super::*;

fn topics() -> Topics {
    Topics::new("homeassistant", "smarthomelab").unwrap()
}

fn id(s: &str) -> DeviceId {
    DeviceId::parse(s).unwrap()
}

#[test]
fn topic_names() {
    let t = topics();
    assert_eq!(t.status(), "smarthomelab/status");
    assert_eq!(t.state(&id("lamp")), "smarthomelab/lamp/state");
    assert_eq!(
        t.availability(&id("lamp")),
        "smarthomelab/lamp/availability"
    );
    assert_eq!(t.command(&id("lamp")), "smarthomelab/lamp/set");
    assert_eq!(t.command_filter(), "smarthomelab/+/set");
    assert_eq!(
        t.config("switch", &id("lamp")),
        "homeassistant/switch/lamp/config"
    );
    assert_eq!(t.ha_status(), "homeassistant/status");
}

#[test]
fn command_topic_round_trips() {
    let t = topics();
    assert_eq!(
        t.device_from_command(&t.command(&id("living-lamp"))),
        Some(id("living-lamp"))
    );
}

#[test]
fn foreign_topics_are_not_commands() {
    let t = topics();
    for topic in [
        "smarthomelab/lamp/state",
        "smarthomelab/lamp/availability",
        "smarthomelab/set",
        "smarthomelab//set",
        "smarthomelab/a/b/set",
        "smarthomelab/Bad Id/set",
        "other/lamp/set",
        "smarthomelabx/lamp/set",
        "homeassistant/status",
    ] {
        assert_eq!(t.device_from_command(topic), None, "{topic}");
    }
}

#[test]
fn invalid_prefixes_are_rejected() {
    for bad in ["", "a/+", "#", "/lead", "trail/"] {
        assert!(Topics::new(bad, "ok").is_err(), "{bad:?}");
        assert!(Topics::new("ok", bad).is_err(), "{bad:?}");
    }
    assert!(Topics::new("home/assistant", "my/lab").is_ok());
}

#[test]
fn config_topics_round_trip() {
    let t = topics();
    assert_eq!(t.config_filter(), "homeassistant/+/+/config");
    assert_eq!(
        t.device_from_config(&t.config("switch", &id("lamp"))),
        Some(("switch", id("lamp")))
    );
    for other in [
        "homeassistant/switch/lamp",
        "homeassistant/switch/lamp/state",
        "homeassistant/switch/Bad Id/config",
        "homeassistant/lamp/config",
        "elsewhere/switch/lamp/config",
        "homeassistant/switch/a/b/config",
    ] {
        assert_eq!(t.device_from_config(other), None, "{other}");
    }
}
