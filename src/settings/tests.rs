use super::*;
use secrecy::ExposeSecret;

fn parse(toml: &str) -> Settings {
    config::Config::builder()
        .add_source(config::File::from_str(toml, config::FileFormat::Toml))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap()
}

#[test]
fn environment_overrides_the_config_file() {
    let env = std::collections::HashMap::from([
        ("SHL_MQTT__ENABLED".to_owned(), "true".to_owned()),
        ("SHL_MQTT__PORT".to_owned(), "1884".to_owned()),
        ("SHL_SERVER__BIND".to_owned(), "0.0.0.0:9000".to_owned()),
    ]);
    let settings: Settings = config::Config::builder()
        .add_source(config::File::from_str(
            "[mqtt]\nport = 1883",
            config::FileFormat::Toml,
        ))
        .add_source(environment().source(Some(env)))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap();
    assert!(settings.mqtt.enabled);
    assert_eq!(settings.mqtt.port, 1884);
    assert_eq!(settings.server.bind.port(), 9000);
}

#[test]
fn empty_config_uses_defaults() {
    let settings = parse("");
    assert_eq!(settings.server.bind.port(), 8765);
    assert!(!settings.mqtt.enabled);
    assert_eq!(settings.mqtt.port, 1883);
    assert_eq!(settings.mqtt.discovery_prefix, "homeassistant");
}

#[test]
fn entities_are_removed_on_exit_unless_the_config_says_otherwise() {
    assert!(parse("").mqtt.remove_on_exit);
    assert!(!parse("[mqtt]\nremove_on_exit = false").mqtt.remove_on_exit);
    let from_environment = from_env(&[("SHL_MQTT__REMOVE_ON_EXIT", "false")]).unwrap();
    assert!(!from_environment.mqtt.remove_on_exit);
}

#[test]
fn mqtt_section_is_parsed() {
    let settings = parse(
        r#"
        [mqtt]
        enabled = true
        host = "192.168.1.10"
        username = "lab"
        password = "s3cret"
        base_topic = "lab"
        "#,
    );
    assert!(settings.mqtt.enabled);
    assert_eq!(settings.mqtt.host, "192.168.1.10");
    assert_eq!(settings.mqtt.username.as_deref(), Some("lab"));
    assert_eq!(settings.mqtt.password.unwrap().expose_secret(), "s3cret");
    assert_eq!(settings.mqtt.base_topic, "lab");
    assert_eq!(settings.mqtt.client_id, None);
}

#[test]
fn client_id_is_generated_per_start_unless_configured() {
    let default = Mqtt::default();
    let (first, second) = (default.client_id(), default.client_id());
    assert!(first.starts_with("smart-home-lab-"), "{first}");
    assert!(first.len() > "smart-home-lab-".len());
    // Two calls in the same process differ only by the clock, which may tick slowly: just
    // check the shape here and that an explicit id wins.
    assert!(second.starts_with("smart-home-lab-"));

    let named = Mqtt {
        client_id: Some("my-lab".to_owned()),
        ..Mqtt::default()
    };
    assert_eq!(named.client_id(), "my-lab");
}

fn from_env(vars: &[(&str, &str)]) -> Result<Settings, config::ConfigError> {
    let env = std::collections::HashMap::from_iter(
        vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    config::Config::builder()
        .add_source(environment().source(Some(env)))
        .build()?
        .try_deserialize()
}

#[test]
fn values_that_look_like_numbers_stay_strings() {
    let settings = from_env(&[
        ("SHL_MQTT__PASSWORD", "0042"),
        ("SHL_MQTT__USERNAME", "1e3"),
        ("SHL_MQTT__CLIENT_ID", "007"),
    ])
    .unwrap();
    assert_eq!(settings.mqtt.password.unwrap().expose_secret(), "0042");
    assert_eq!(settings.mqtt.username.as_deref(), Some("1e3"));
    assert_eq!(settings.mqtt.client_id.as_deref(), Some("007"));
}

#[test]
fn typed_values_are_still_parsed_from_the_environment() {
    let settings = from_env(&[
        ("SHL_MQTT__ENABLED", "true"),
        ("SHL_MQTT__PORT", "1884"),
        ("SHL_SERVER__BIND", "0.0.0.0:9000"),
    ])
    .unwrap();
    assert!(settings.mqtt.enabled);
    assert_eq!(settings.mqtt.port, 1884);
    assert_eq!(settings.server.bind.port(), 9000);
}

#[test]
fn misspelled_keys_are_an_error_instead_of_being_ignored() {
    let error = from_env(&[("SHL_MQTT__ENABLE", "true")])
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown field `enable`"), "{error}");

    let error = config::Config::builder()
        .add_source(config::File::from_str(
            "[mqt]\nenabled = true",
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
        .try_deserialize::<Settings>()
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown field `mqt`"), "{error}");
}

#[test]
fn storage_defaults_point_at_the_projects_folder() {
    let settings = parse("");
    assert_eq!(settings.storage.dir, std::path::PathBuf::from("projects"));
    assert_eq!(settings.storage.project, None);
}

#[test]
fn the_project_to_open_can_come_from_the_environment() {
    let settings = from_env(&[
        ("SHL_STORAGE__PROJECT", "dacha"),
        ("SHL_STORAGE__DIR", "/tmp/shl-projects"),
    ])
    .unwrap();
    assert_eq!(settings.storage.project.as_deref(), Some("dacha"));
    assert_eq!(
        settings.storage.dir,
        std::path::PathBuf::from("/tmp/shl-projects")
    );
}
