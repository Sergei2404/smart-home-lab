use crate::domain::DeviceId;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid MQTT topic prefix {0:?}: it must not be empty, contain '+' or '#', or start/end with '/'"
)]
pub struct InvalidPrefix(String);

/// All topic names used by the bridge.
///
/// ```text
/// <base>/status              bridge availability ("online" / "offline", last will)
/// <base>/<id>/state          device state ("ON" / "OFF" or a number, retained)
/// <base>/<id>/availability   device availability ("online" / "offline", retained)
/// <base>/<id>/set            commands from Home Assistant ("ON" / "OFF")
/// <prefix>/<component>/<id>/config   MQTT Discovery payload (retained)
/// <prefix>/status            Home Assistant birth/will message
/// ```
#[derive(Debug, Clone)]
pub struct Topics {
    discovery_prefix: String,
    base: String,
}

impl Topics {
    pub fn new(discovery_prefix: &str, base: &str) -> Result<Self, InvalidPrefix> {
        for prefix in [discovery_prefix, base] {
            let invalid = prefix.is_empty()
                || prefix.contains(['+', '#'])
                || prefix.starts_with('/')
                || prefix.ends_with('/');
            if invalid {
                return Err(InvalidPrefix(prefix.to_owned()));
            }
        }
        Ok(Self {
            discovery_prefix: discovery_prefix.to_owned(),
            base: base.to_owned(),
        })
    }

    pub fn status(&self) -> String {
        format!("{}/status", self.base)
    }

    pub fn state(&self, id: &DeviceId) -> String {
        format!("{}/{id}/state", self.base)
    }

    pub fn availability(&self, id: &DeviceId) -> String {
        format!("{}/{id}/availability", self.base)
    }

    pub fn command(&self, id: &DeviceId) -> String {
        format!("{}/{id}/set", self.base)
    }

    pub fn command_filter(&self) -> String {
        format!("{}/+/set", self.base)
    }

    pub fn config(&self, component: &str, id: &DeviceId) -> String {
        format!("{}/{component}/{id}/config", self.discovery_prefix)
    }

    /// Matches every discovery config on the broker, whoever published it.
    pub fn config_filter(&self) -> String {
        format!("{}/+/+/config", self.discovery_prefix)
    }

    pub fn ha_status(&self) -> String {
        format!("{}/status", self.discovery_prefix)
    }

    /// Extracts the platform and the device id from `<prefix>/<component>/<id>/config`.
    pub fn device_from_config<'a>(&self, topic: &'a str) -> Option<(&'a str, DeviceId)> {
        let rest = topic
            .strip_prefix(self.discovery_prefix.as_str())?
            .strip_prefix('/')?
            .strip_suffix("/config")?;
        let (component, id) = rest.split_once('/')?;
        Some((component, DeviceId::parse(id).ok()?))
    }

    /// Extracts the device id from `<base>/<id>/set`.
    pub fn device_from_command(&self, topic: &str) -> Option<DeviceId> {
        let id = topic
            .strip_prefix(self.base.as_str())?
            .strip_prefix('/')?
            .strip_suffix("/set")?;
        DeviceId::parse(id).ok()
    }
}

#[cfg(test)]
mod tests;
