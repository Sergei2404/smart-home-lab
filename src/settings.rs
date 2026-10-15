use secrecy::SecretString;
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf};

const CONFIG_FILE: &str = "config.toml";
const LOCAL_CONFIG_FILE: &str = "config.local.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub server: Server,
    pub storage: Storage,
    pub mqtt: Mqtt,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    pub bind: SocketAddr,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 8765)),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Storage {
    /// The folder with one `<project>.json` file per project.
    pub dir: PathBuf,
    /// Open this project instead of the one used last time.
    pub project: Option<String>,
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("projects"),
            project: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Mqtt {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// Generated on every start when not set, so two instances never kick each other off the broker.
    pub client_id: Option<String>,
    pub username: Option<String>,
    pub password: Option<SecretString>,
    pub discovery_prefix: String,
    pub base_topic: String,
    /// Take the lab's entities out of Home Assistant when the lab stops, so test runs leave nothing
    /// behind. Turn it off to keep them (shown as unavailable) between runs.
    pub remove_on_exit: bool,
}

impl Mqtt {
    pub fn client_id(&self) -> String {
        self.client_id.clone().unwrap_or_else(generated_client_id)
    }
}

fn generated_client_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("smart-home-lab-{:x}{:x}", std::process::id(), nanos)
}

impl Default for Mqtt {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "127.0.0.1".to_owned(),
            port: 1883,
            client_id: None,
            username: None,
            password: None,
            discovery_prefix: "homeassistant".to_owned(),
            base_topic: "smarthomelab".to_owned(),
            remove_on_exit: true,
        }
    }
}

/// `SHL_MQTT__ENABLED=true` overrides `[mqtt] enabled`. Values stay strings until serde converts
/// them to the type of the field, so `SHL_MQTT__PASSWORD=0042` really is `0042`.
fn environment() -> config::Environment {
    config::Environment::with_prefix("SHL")
        .prefix_separator("_")
        .separator("__")
}

impl Settings {
    pub fn load() -> Result<Self, config::ConfigError> {
        config::Config::builder()
            .add_source(config::File::new(CONFIG_FILE, config::FileFormat::Toml).required(false))
            .add_source(
                config::File::new(LOCAL_CONFIG_FILE, config::FileFormat::Toml).required(false),
            )
            .add_source(environment())
            .build()?
            .try_deserialize()
    }
}

#[cfg(test)]
mod tests;
