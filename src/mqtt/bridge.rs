use super::{
    discovery::{self, COMPONENTS, OFFLINE, ONLINE},
    topics::Topics,
};
use crate::{
    domain::{Device, DeviceId, State},
    lab::{Event, EventKind, Lab, MqttState, Source},
    settings::Mqtt,
};
use rumqttc::{
    AsyncClient, ClientError, ConnectReturnCode, Event as MqttEvent, EventLoop, LastWill,
    MqttOptions, Outgoing, Packet, Publish, QoS,
};
use secrecy::ExposeSecret;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const KEEP_ALIVE: Duration = Duration::from_secs(30);
const RECONNECT_DELAY: Duration = Duration::from_secs(3);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_QUEUE: usize = 64;

enum Trigger {
    /// Re-subscribe and republish discovery + state of every device. After a (re)connect it also
    /// asks the broker for the discovery configs it holds, to find leftovers of earlier runs.
    Resync { after_connect: bool },
    /// Republish the state of one device.
    State(DeviceId),
    /// The broker holds a discovery config of ours for this device and platform (the retained
    /// messages that arrive after subscribing, or an echo of what we published ourselves).
    Found {
        component: &'static str,
        id: DeviceId,
    },
}

/// State shared by the poller and the publisher; "connected" is the lab's `MqttState`.
struct Link {
    topics: Topics,
    lab: Arc<Lab>,
    /// `host:port`, for log messages.
    broker: String,
    /// Cancelled when the bridge is shutting down.
    stopping: CancellationToken,
    triggers: mpsc::UnboundedSender<Trigger>,
    /// Take the entities out of Home Assistant on shutdown.
    remove_on_exit: bool,
    /// Devices whose discovery config is on the broker because of this process, with the Home Assistant
    /// platform it was published under. What is no longer a device is withdrawn on the next resync; an id
    /// that comes back as another kind has its old platform's config cleared, or Home Assistant would see
    /// two entities with one `unique_id`.
    announced: Mutex<HashMap<DeviceId, &'static str>>,
}

/// Bridges the lab to an MQTT broker using Home Assistant MQTT Discovery. The poller drives the
/// connection and handles incoming messages (it never publishes, so it cannot stall it); the
/// publisher forwards lab events. Everything is republished after every (re)connect and when Home
/// Assistant announces itself, so an event missed while offline does not matter.
pub struct MqttBridge {
    client: AsyncClient,
    link: Arc<Link>,
    publisher: JoinHandle<()>,
    poller: JoinHandle<()>,
}

pub fn start(settings: &Mqtt, lab: Arc<Lab>) -> anyhow::Result<MqttBridge> {
    let topics = Topics::new(&settings.discovery_prefix, &settings.base_topic)?;

    let mut options = MqttOptions::new(settings.client_id(), &settings.host, settings.port);
    options
        .set_keep_alive(KEEP_ALIVE)
        .set_clean_session(true)
        .set_last_will(LastWill::new(
            topics.status(),
            OFFLINE,
            QoS::AtLeastOnce,
            true,
        ));
    if let Some(username) = &settings.username {
        let password = settings
            .password
            .as_ref()
            .map(|p| p.expose_secret())
            .unwrap_or_default();
        options.set_credentials(username, password);
    }

    let (client, eventloop) = AsyncClient::new(options, REQUEST_QUEUE);
    let (triggers, trigger_rx) = mpsc::unbounded_channel();
    let events = lab.subscribe();
    lab.set_mqtt_state(MqttState::Disconnected);

    let link = Arc::new(Link {
        topics,
        lab,
        broker: format!("{}:{}", settings.host, settings.port),
        stopping: CancellationToken::new(),
        triggers,
        remove_on_exit: settings.remove_on_exit,
        announced: Mutex::default(),
    });
    let publisher = Publisher {
        client: client.clone(),
        link: link.clone(),
    };
    Ok(MqttBridge {
        client,
        publisher: tokio::spawn(publisher.run(events, trigger_rx)),
        poller: tokio::spawn(poll(eventloop, link.clone())),
        link,
    })
}

impl MqttBridge {
    /// Takes the entities out of Home Assistant (unless `remove_on_exit` is off), marks the bridge
    /// offline, disconnects cleanly and waits briefly for the connection to close.
    pub async fn shutdown(self) {
        let Self {
            client,
            link,
            publisher,
            poller,
        } = self;
        publisher.abort();
        link.stopping.cancel();
        let closing = async {
            if link.remove_on_exit && link.connected() {
                // The publisher is gone, so nothing announces anything while this runs.
                if let Err(error) = link.withdraw_all(&client).await {
                    tracing::warn!(%error, "could not remove all entities from the broker");
                }
            }
            let _ = client
                .publish(link.topics.status(), QoS::AtLeastOnce, true, OFFLINE)
                .await;
            let _ = client.disconnect().await;
            let _ = poller.await;
        };
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, closing)
            .await
            .is_err()
        {
            tracing::warn!("MQTT bridge did not shut down cleanly");
        }
    }
}

async fn poll(mut eventloop: EventLoop, link: Arc<Link>) {
    let mut reported = false;
    loop {
        match eventloop.poll().await {
            Ok(MqttEvent::Incoming(Packet::ConnAck(ack)))
                if ack.code == ConnectReturnCode::Success =>
            {
                reported = false;
                link.lab.set_mqtt_state(MqttState::Connected);
                tracing::info!(broker = %link.broker, "connected to MQTT broker");
                let _ = link.triggers.send(Trigger::Resync {
                    after_connect: true,
                });
            }
            Ok(MqttEvent::Incoming(Packet::Publish(publish))) => link.on_publish(&publish),
            Ok(MqttEvent::Outgoing(Outgoing::Disconnect)) => break,
            Ok(_) => {}
            Err(error) => {
                if link.stopping.is_cancelled() {
                    break;
                }
                link.lab.set_mqtt_state(MqttState::Disconnected);
                if reported {
                    tracing::debug!(%error, "MQTT broker still unavailable");
                } else {
                    tracing::warn!(
                        %error,
                        broker = %link.broker,
                        "MQTT broker unavailable, retrying every {}s",
                        RECONNECT_DELAY.as_secs()
                    );
                    reported = true;
                }
                // Shutting down must not wait for the next retry.
                tokio::select! {
                    () = tokio::time::sleep(RECONNECT_DELAY) => {}
                    () = link.stopping.cancelled() => break,
                }
            }
        }
    }
}

impl Link {
    fn connected(&self) -> bool {
        self.lab.mqtt_state() == MqttState::Connected
    }

    fn announced(&self) -> std::sync::MutexGuard<'_, HashMap<DeviceId, &'static str>> {
        self.announced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Removes every entity this process announced: the config under the platform it was published
    /// under, and the state and availability topics.
    async fn withdraw_all(&self, client: &AsyncClient) -> Result<(), ClientError> {
        let announced: Vec<_> = self.announced().drain().collect();
        for (id, component) in announced {
            clear_device(client, &self.topics, &id, &[component]).await?;
        }
        Ok(())
    }

    fn on_publish(&self, publish: &Publish) {
        if publish.topic == self.topics.ha_status() {
            if publish.payload.as_ref() == ONLINE.as_bytes() {
                tracing::info!("Home Assistant is online, republishing discovery");
                let _ = self.triggers.send(Trigger::Resync {
                    after_connect: false,
                });
            }
        } else if let Some(id) = self.topics.device_from_command(&publish.topic) {
            self.on_command(id, &publish.payload);
        } else if let Some((component, id)) = self.topics.device_from_config(&publish.topic)
            && let Some(component) = COMPONENTS.iter().copied().find(|c| *c == component)
            && discovery::is_ours(&self.topics, &id, &publish.payload)
        {
            // An empty payload (a config being cleared) is not valid JSON and is not ours either.
            let _ = self.triggers.send(Trigger::Found { component, id });
        }
    }

    fn on_command(&self, id: DeviceId, payload: &[u8]) {
        let Some(on) = discovery::parse_command(payload) else {
            tracing::warn!(
                device = %id,
                payload = %String::from_utf8_lossy(payload),
                "ignoring MQTT command with an unknown payload"
            );
            return;
        };
        let Ok(device) = self.lab.device(&id) else {
            tracing::debug!(device = %id, "ignoring MQTT command for an unknown device");
            return;
        };
        if !device.kind.accepts_commands() {
            tracing::warn!(device = %id, "ignoring MQTT command: this device is read-only");
        } else if !device.online {
            tracing::warn!(device = %id, "ignoring MQTT command: this device is offline");
        } else if device.state == State::Bool(on) {
            // Nothing changes, but Home Assistant is waiting for the state to be confirmed.
            let _ = self.triggers.send(Trigger::State(id));
        } else if let Err(error) = self.lab.set_state(&id, on, Source::Mqtt) {
            tracing::debug!(device = %id, %error, "MQTT command ignored");
        }
    }
}

struct Publisher {
    client: AsyncClient,
    link: Arc<Link>,
}

impl Publisher {
    async fn run(
        self,
        mut events: broadcast::Receiver<Event>,
        mut triggers: mpsc::UnboundedReceiver<Trigger>,
    ) {
        loop {
            let result = tokio::select! {
                trigger = triggers.recv() => match trigger {
                    Some(Trigger::Resync { after_connect }) => self.resync(after_connect).await,
                    Some(Trigger::State(id)) => self.republish_state(&id).await,
                    Some(Trigger::Found { component, id }) => self.tidy(component, &id).await,
                    None => break,
                },
                event = events.recv() => match event {
                    Ok(event) => self.on_event(event.kind).await,
                    Err(broadcast::error::RecvError::Lagged(_)) => self.resync(false).await,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            if let Err(error) = result {
                tracing::warn!(%error, "MQTT publish failed");
            }
        }
    }

    async fn on_event(&self, kind: EventKind) -> Result<(), ClientError> {
        if !self.link.connected() {
            return Ok(());
        }
        match kind {
            EventKind::DeviceAdded { device } => self.announce(&device).await,
            EventKind::DeviceRemoved { id } => self.withdraw(&id).await,
            EventKind::StateChanged { id, state, .. } => {
                // An offline device reports nothing; it sends its current state when it returns.
                if self.is_online(&id) {
                    self.publish_state(&id, state).await
                } else {
                    Ok(())
                }
            }
            EventKind::AvailabilityChanged { id, online, .. } => {
                self.change_availability(&id, online).await
            }
            // A new project means new devices: announce them and withdraw the old project's entities.
            EventKind::ProjectChanged { .. } => self.resync(false).await,
            _ => Ok(()),
        }
    }

    async fn resync(&self, after_connect: bool) -> Result<(), ClientError> {
        let topics = &self.link.topics;
        self.client
            .subscribe(topics.command_filter(), QoS::AtLeastOnce)
            .await?;
        self.client
            .subscribe(topics.ha_status(), QoS::AtLeastOnce)
            .await?;
        if after_connect {
            // The broker replays its retained discovery configs on subscribing; those of ours that
            // nothing in the lab explains (earlier runs, other projects) are cleaned up by `tidy`.
            self.client
                .subscribe(topics.config_filter(), QoS::AtLeastOnce)
                .await?;
        }
        self.client
            .publish(topics.status(), QoS::AtLeastOnce, true, ONLINE)
            .await?;
        let devices = self.link.lab.devices();
        for device in &devices {
            self.announce(device).await?;
        }
        let current: HashSet<&DeviceId> = devices.iter().map(|d| &d.id).collect();
        let stale: Vec<DeviceId> = self
            .link
            .announced()
            .keys()
            .filter(|id| !current.contains(id))
            .cloned()
            .collect();
        for id in stale {
            self.withdraw(&id).await?;
        }
        Ok(())
    }

    /// Deals with a discovery config of ours that is on the broker: one for a device the lab does not
    /// have (left by an earlier run or another project) is removed with its topics, one under a
    /// platform the device no longer has is cleared, one that matches is left to `announce`.
    async fn tidy(&self, component: &'static str, id: &DeviceId) -> Result<(), ClientError> {
        let topics = &self.link.topics;
        match self.link.lab.device(id) {
            Err(_) => {
                tracing::info!(device = %id, "removing a device from the broker that is not in the lab (left by an earlier run)");
                self.withdraw(id).await
            }
            Ok(device) if discovery::component(device.kind) != component => {
                self.client
                    .publish(topics.config(component, id), QoS::AtLeastOnce, true, "")
                    .await
            }
            Ok(_) => Ok(()),
        }
    }

    async fn republish_state(&self, id: &DeviceId) -> Result<(), ClientError> {
        match self.link.lab.device(id) {
            Ok(device) if device.online && self.link.connected() => {
                self.publish_state(id, device.state).await
            }
            _ => Ok(()),
        }
    }

    fn is_online(&self, id: &DeviceId) -> bool {
        self.link.lab.device(id).is_ok_and(|device| device.online)
    }

    /// Discovery config, then what the device reports, all retained. An online device reports its
    /// state first and only then says it is online; an offline one reports nothing.
    async fn announce(&self, device: &Device) -> Result<(), ClientError> {
        let topics = &self.link.topics;
        let component = discovery::component(device.kind);
        let before = self.link.announced().get(&device.id).copied();
        if let Some(old) = before.filter(|old| *old != component) {
            // Only once this is published is the old platform forgotten, so a failure is retried.
            self.client
                .publish(topics.config(old, &device.id), QoS::AtLeastOnce, true, "")
                .await?;
        }
        self.link.announced().insert(device.id.clone(), component);
        let config = discovery::config_payload(topics, device).to_string();
        self.client
            .publish(
                topics.config(component, &device.id),
                QoS::AtLeastOnce,
                true,
                config,
            )
            .await?;
        if device.online {
            self.publish_state(&device.id, device.state).await?;
        }
        self.publish_availability(&device.id, device.online).await
    }

    /// A device that comes back reports its current state first and only then says it is online.
    async fn change_availability(&self, id: &DeviceId, online: bool) -> Result<(), ClientError> {
        if online && let Ok(device) = self.link.lab.device(id) {
            self.publish_state(id, device.state).await?;
        }
        self.publish_availability(id, online).await
    }

    /// An empty retained payload removes the entity from Home Assistant and clears its topics.
    async fn withdraw(&self, id: &DeviceId) -> Result<(), ClientError> {
        self.link.announced().remove(id);
        clear_device(&self.client, &self.link.topics, id, &COMPONENTS).await
    }

    async fn publish_availability(&self, id: &DeviceId, online: bool) -> Result<(), ClientError> {
        self.client
            .publish(
                self.link.topics.availability(id),
                QoS::AtLeastOnce,
                true,
                if online { ONLINE } else { OFFLINE },
            )
            .await
    }

    async fn publish_state(&self, id: &DeviceId, state: State) -> Result<(), ClientError> {
        self.client
            .publish(
                self.link.topics.state(id),
                QoS::AtLeastOnce,
                true,
                discovery::state_payload(state),
            )
            .await
    }
}

/// Publishes empty retained payloads to the config of `id` under each of `components`, and to its
/// state and availability topics.
async fn clear_device(
    client: &AsyncClient,
    topics: &Topics,
    id: &DeviceId,
    components: &[&str],
) -> Result<(), ClientError> {
    for component in components {
        client
            .publish(topics.config(component, id), QoS::AtLeastOnce, true, "")
            .await?;
    }
    client
        .publish(topics.state(id), QoS::AtLeastOnce, true, "")
        .await?;
    client
        .publish(topics.availability(id), QoS::AtLeastOnce, true, "")
        .await
}
