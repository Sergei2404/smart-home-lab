//! MQTT bridge tests. A second MQTT client (`Probe`) plays the part of Home Assistant:
//! it listens to the discovery and state topics, sends commands and the birth message.
//!
//! Tests that need a broker are `#[ignore]`d. Start one with `docker compose up -d`
//! and run `cargo test -- --ignored` (set `SHL_TEST_BROKER=host:port` for a non-default address).

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::Value;
use smart_home_lab::{
    domain::{Device, DeviceId, DeviceKind, Snapshot, State},
    lab::{EventKind, Lab, MqttState, Source},
    mqtt::{
        self, MqttBridge,
        discovery::{self, COMPONENTS},
        topics::Topics,
    },
    settings::Mqtt,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};

const WAIT: Duration = Duration::from_secs(10);

fn broker() -> (String, u16) {
    let address = std::env::var("SHL_TEST_BROKER").unwrap_or_else(|_| "127.0.0.1:1883".to_owned());
    let (host, port) = address
        .rsplit_once(':')
        .expect("SHL_TEST_BROKER is host:port");
    (host.to_owned(), port.parse().expect("invalid broker port"))
}

fn unique() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!(
        "t{}x{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Fails with a hint instead of an opaque timeout when no broker is running.
async fn require_broker() {
    let (host, port) = broker();
    let reachable = timeout(
        Duration::from_secs(2),
        TcpStream::connect((host.as_str(), port)),
    )
    .await;
    if !matches!(reachable, Ok(Ok(_))) {
        panic!(
            "no MQTT broker at {host}:{port}: start one with `docker compose up -d --wait` \
             (or point SHL_TEST_BROKER=host:port at another one)"
        );
    }
}

/// A TCP relay between the bridge and the broker that can be cut and restored, to simulate an outage.
struct Proxy {
    port: u16,
    open: Arc<AtomicBool>,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Proxy {
    async fn start() -> Self {
        let (host, port) = broker();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let open = Arc::new(AtomicBool::new(true));
        let connections: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        tokio::spawn({
            let (open, connections) = (open.clone(), connections.clone());
            async move {
                while let Ok((mut client, _)) = listener.accept().await {
                    if !open.load(Ordering::Acquire) {
                        continue; // dropped at once: the bridge sees a closed connection
                    }
                    let Ok(mut upstream) = TcpStream::connect((host.as_str(), port)).await else {
                        continue;
                    };
                    let relay = tokio::spawn(async move {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                    });
                    connections.lock().unwrap().push(relay);
                }
            }
        });
        Self {
            port: proxy_port,
            open,
            connections,
        }
    }

    /// Drops every connection abruptly (no MQTT DISCONNECT) and refuses new ones.
    fn cut(&self) {
        self.open.store(false, Ordering::Release);
        for relay in self.connections.lock().unwrap().drain(..) {
            relay.abort();
        }
    }

    fn restore(&self) {
        self.open.store(true, Ordering::Release);
    }
}

/// Simulated Home Assistant.
struct Probe {
    client: AsyncClient,
    messages: mpsc::UnboundedReceiver<(String, String)>,
    /// Messages received but not asked for yet. Retained messages arrive in no particular order.
    backlog: Vec<(String, String)>,
    /// Everything pulled from the connection so far, in arrival order.
    received: Vec<(String, String)>,
}

impl Probe {
    async fn connect(topics_to_watch: &[String]) -> Self {
        require_broker().await;
        let (host, port) = broker();
        let mut options = MqttOptions::new(format!("probe-{}", unique()), host, port);
        options.set_keep_alive(Duration::from_secs(5));
        let (client, mut eventloop) = AsyncClient::new(options, 64);
        let (tx, messages) = mpsc::unbounded_channel();
        let (acked, mut subscriptions) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::SubAck(_))) => {
                        let _ = acked.send(());
                    }
                    Ok(Event::Incoming(Packet::Publish(p))) => {
                        let payload = String::from_utf8_lossy(&p.payload).into_owned();
                        if tx.send((p.topic, payload)).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
        });
        for topic in topics_to_watch {
            client.subscribe(topic, QoS::AtLeastOnce).await.unwrap();
        }
        // Once the broker has confirmed the subscriptions, everything the bridge publishes reaches
        // the probe live and in publish order (retained replays have no defined order).
        for _ in topics_to_watch {
            timeout(WAIT, subscriptions.recv())
                .await
                .expect("the broker did not confirm the subscription")
                .expect("probe connection closed");
        }
        Self {
            client,
            messages,
            backlog: Vec::new(),
            received: Vec::new(),
        }
    }

    /// Waits for a message on `topic` whose payload satisfies `accept` and consumes it.
    async fn expect(&mut self, topic: &str, accept: impl Fn(&str) -> bool) -> String {
        let found = timeout(WAIT, async {
            loop {
                let matching = self
                    .backlog
                    .iter()
                    .position(|(t, payload)| t == topic && accept(payload));
                if let Some(index) = matching {
                    return self.backlog.remove(index).1;
                }
                let message = self.messages.recv().await.expect("probe connection closed");
                self.received.push(message.clone());
                self.backlog.push(message);
            }
        })
        .await;
        found.unwrap_or_else(|_| panic!("timed out waiting for a message on {topic}"))
    }

    async fn expect_payload(&mut self, topic: &str, expected: &str) {
        self.expect(topic, |p| p == expected).await;
    }

    /// Forgets everything received so far.
    fn drain(&mut self) {
        self.backlog.clear();
        self.received.clear();
        while self.messages.try_recv().is_ok() {}
    }

    /// Position of the last message with this topic and payload among those seen so far.
    fn last_position(&self, topic: &str, payload: &str) -> Option<usize> {
        self.received
            .iter()
            .rposition(|(t, p)| t == topic && p == payload)
    }

    async fn publish(&self, topic: &str, payload: &str) {
        self.client
            .publish(topic, QoS::AtLeastOnce, false, payload)
            .await
            .unwrap();
    }

    /// Leaves a retained message on the broker, as an earlier run of the lab would have.
    async fn publish_retained(&self, topic: &str, payload: &str) {
        self.client
            .publish(topic, QoS::AtLeastOnce, true, payload)
            .await
            .unwrap();
    }

    /// Pulls everything that arrives within `duration` into the record of what was received.
    async fn listen_for(&mut self, duration: Duration) {
        let _ = timeout(duration, async {
            loop {
                let message = self.messages.recv().await.expect("probe connection closed");
                self.received.push(message.clone());
                self.backlog.push(message);
            }
        })
        .await;
    }

    /// Whether this exact message was received at any point.
    fn saw(&self, topic: &str, payload: &str) -> bool {
        self.last_position(topic, payload).is_some()
    }
}

struct Setup {
    lab: Arc<Lab>,
    topics: Topics,
    bridge: MqttBridge,
    probe: Probe,
}

/// Builds the retained messages to leave on the broker, given the test's private topics.
type Retained = fn(&Topics) -> Vec<(String, String)>;

/// What a test may arrange before the bridge starts.
#[derive(Default)]
struct Options {
    /// The bridge talks to the broker through this local port (a `Proxy`).
    bridge_port: Option<u16>,
    /// Retained messages already on the broker when the bridge connects, as an earlier run leaves them.
    retained: Option<Retained>,
    /// Keep the entities on the broker when the bridge stops (`remove_on_exit = false`).
    keep_on_exit: bool,
}

/// Starts a bridge with private topic prefixes so tests (and a real Home Assistant) never interfere.
async fn setup(snapshot: Snapshot) -> Setup {
    setup_with(snapshot, Options::default()).await
}

/// Like `setup`, but the bridge may talk to the broker through another local port (a `Proxy`).
async fn setup_via(snapshot: Snapshot, bridge_port: Option<u16>) -> Setup {
    setup_with(
        snapshot,
        Options {
            bridge_port,
            ..Options::default()
        },
    )
    .await
}

async fn setup_with(snapshot: Snapshot, options: Options) -> Setup {
    let id = unique();
    let (host, broker_port) = broker();
    let (host, port) = match options.bridge_port {
        Some(port) => ("127.0.0.1".to_owned(), port),
        None => (host, broker_port),
    };
    let settings = Mqtt {
        enabled: true,
        host,
        port,
        client_id: Some(format!("bridge-{id}")),
        discovery_prefix: format!("ha-{id}"),
        base_topic: format!("shl-{id}"),
        remove_on_exit: !options.keep_on_exit,
        ..Mqtt::default()
    };
    let topics = Topics::new(&settings.discovery_prefix, &settings.base_topic).unwrap();
    let mut probe = Probe::connect(&[
        format!("{}/#", settings.discovery_prefix),
        format!("{}/#", settings.base_topic),
    ])
    .await;
    if let Some(retained) = options.retained {
        for (topic, payload) in retained(&topics) {
            probe.publish_retained(&topic, &payload).await;
            // Once it has come back the broker holds it, so the bridge will be sent it on connecting.
            probe.expect_payload(&topic, &payload).await;
        }
    }

    let lab = Arc::new(Lab::new(snapshot));
    let bridge = mqtt::start(&settings, lab.clone()).unwrap();
    probe
        .expect_payload(&topics.status(), discovery::ONLINE)
        .await;
    Setup {
        lab,
        topics,
        bridge,
        probe,
    }
}

fn snapshot_with(devices: Vec<Device>) -> Snapshot {
    Snapshot {
        devices,
        ..Snapshot::default()
    }
}

fn lamp(on: bool) -> Device {
    Device {
        state: State::Bool(on),
        ..Device::new("Lamp", DeviceKind::Switch).unwrap()
    }
}

fn door() -> Device {
    Device::new("Door", DeviceKind::Sensor).unwrap()
}

fn id(s: &str) -> DeviceId {
    DeviceId::parse(s).unwrap()
}

async fn eventually(what: &str, condition: impl Fn() -> bool) {
    timeout(WAIT, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting until {what}"));
}

#[tokio::test]
async fn unreachable_broker_is_reported_and_does_not_break_the_lab() {
    let unused_port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let settings = Mqtt {
        enabled: true,
        port: unused_port,
        ..Mqtt::default()
    };
    let lab = Arc::new(Lab::new(snapshot_with(vec![lamp(false)])));

    let bridge = mqtt::start(&settings, lab.clone()).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(lab.mqtt_state(), MqttState::Disconnected);
    assert_eq!(
        lab.set_state(&id("lamp"), true, Source::Ui).unwrap().state,
        State::Bool(true)
    );
    timeout(Duration::from_secs(5), bridge.shutdown())
        .await
        .expect("shutdown must not hang without a broker");
}

#[test]
fn invalid_topic_prefix_is_rejected_at_startup() {
    let settings = Mqtt {
        enabled: true,
        base_topic: "bad/+".to_owned(),
        ..Mqtt::default()
    };
    let lab = Arc::new(Lab::new(Snapshot::default()));
    assert!(mqtt::start(&settings, lab).is_err());
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn existing_devices_are_announced_when_connected() {
    let (lamp, door) = (lamp(true), door());
    let mut s = setup(snapshot_with(vec![lamp.clone(), door.clone()])).await;

    let config = s
        .probe
        .expect(&s.topics.config("switch", &lamp.id), |_| true)
        .await;
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config, discovery::config_payload(&s.topics, &lamp));
    s.probe
        .expect_payload(&s.topics.state(&lamp.id), "ON")
        .await;

    let config = s
        .probe
        .expect(&s.topics.config("binary_sensor", &door.id), |_| true)
        .await;
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config, discovery::config_payload(&s.topics, &door));
    assert!(config.get("command_topic").is_none());
    s.probe
        .expect_payload(&s.topics.state(&door.id), "OFF")
        .await;

    assert_eq!(s.lab.mqtt_state(), MqttState::Connected);
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn state_changes_are_published() {
    let mut s = setup(snapshot_with(vec![lamp(false)])).await;
    let topic = s.topics.state(&id("lamp"));
    s.probe.expect_payload(&topic, "OFF").await;

    s.lab.set_state(&id("lamp"), true, Source::Ui).unwrap();
    s.probe.expect_payload(&topic, "ON").await;

    s.lab.set_state(&id("lamp"), false, Source::Ui).unwrap();
    s.probe.expect_payload(&topic, "OFF").await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn commands_change_the_device_and_are_confirmed() {
    let mut s = setup(snapshot_with(vec![lamp(false)])).await;
    s.probe
        .expect_payload(&s.topics.state(&id("lamp")), "OFF")
        .await;
    s.probe.drain();

    s.probe.publish(&s.topics.command(&id("lamp")), "ON").await;

    eventually("the lamp is on", || {
        s.lab.device(&id("lamp")).unwrap().state == State::Bool(true)
    })
    .await;
    s.probe
        .expect_payload(&s.topics.state(&id("lamp")), "ON")
        .await;
    let last = s.lab.log().pop().unwrap();
    assert_eq!(
        last.kind,
        EventKind::StateChanged {
            id: id("lamp"),
            state: State::Bool(true),
            source: Source::Mqtt
        }
    );
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn sensors_and_bad_payloads_ignore_commands() {
    let s = setup(snapshot_with(vec![lamp(false), door()])).await;
    let mut events = s.lab.subscribe();

    s.probe.publish(&s.topics.command(&id("door")), "ON").await;
    s.probe
        .publish(&s.topics.command(&id("lamp")), "TOGGLE")
        .await;
    s.probe.publish(&s.topics.command(&id("ghost")), "ON").await;
    // Commands are handled in order, so once this one is applied the earlier ones were seen.
    s.probe.publish(&s.topics.command(&id("lamp")), "ON").await;

    eventually("the lamp is on", || {
        s.lab.device(&id("lamp")).unwrap().state == State::Bool(true)
    })
    .await;
    assert_eq!(s.lab.device(&id("door")).unwrap().state, State::Bool(false));
    let mut changes = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event.kind, EventKind::StateChanged { .. }) {
            changes += 1;
        }
    }
    assert_eq!(changes, 1);
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn command_that_changes_nothing_still_confirms_the_state() {
    let mut s = setup(snapshot_with(vec![lamp(true)])).await;
    let topic = s.topics.state(&id("lamp"));
    s.probe.expect_payload(&topic, "ON").await;
    s.probe.drain();

    s.probe.publish(&s.topics.command(&id("lamp")), "ON").await;

    s.probe.expect_payload(&topic, "ON").await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn devices_added_and_removed_at_runtime_are_announced_and_withdrawn() {
    let mut s = setup(Snapshot::default()).await;

    let heater = s.lab.add_device("Heater", DeviceKind::Relay).unwrap();
    let config = s
        .probe
        .expect(&s.topics.config("switch", &heater.id), |p| !p.is_empty())
        .await;
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config["device_class"], "outlet");
    s.probe
        .expect_payload(&s.topics.state(&heater.id), "OFF")
        .await;

    s.lab.remove_device(&heater.id).unwrap();

    for component in COMPONENTS {
        s.probe
            .expect_payload(&s.topics.config(component, &heater.id), "")
            .await;
    }
    s.probe
        .expect_payload(&s.topics.state(&heater.id), "")
        .await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn home_assistant_birth_message_triggers_a_republish() {
    let lamp = lamp(true);
    let mut s = setup(snapshot_with(vec![lamp.clone()])).await;
    s.probe
        .expect_payload(&s.topics.state(&lamp.id), "ON")
        .await;
    s.probe.drain();

    s.probe.publish(&s.topics.ha_status(), "online").await;

    s.probe
        .expect(&s.topics.config("switch", &lamp.id), |p| !p.is_empty())
        .await;
    s.probe
        .expect_payload(&s.topics.state(&lamp.id), "ON")
        .await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn shutdown_marks_the_bridge_offline() {
    let mut s = setup(snapshot_with(vec![lamp(false)])).await;

    s.bridge.shutdown().await;

    s.probe
        .expect_payload(&s.topics.status(), discovery::OFFLINE)
        .await;
}

fn numeric(name: &str, kind: DeviceKind, value: i32) -> Device {
    Device {
        state: State::Int(value),
        ..Device::new(name, kind).unwrap()
    }
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn numeric_sensors_are_announced_with_their_unit_and_value() {
    let (outside, humidity) = (
        numeric("Outside", DeviceKind::Temperature, -12),
        numeric("Humidity", DeviceKind::Percent, 57),
    );
    let mut s = setup(snapshot_with(vec![outside.clone(), humidity.clone()])).await;

    let config = s
        .probe
        .expect(&s.topics.config("sensor", &outside.id), |_| true)
        .await;
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config, discovery::config_payload(&s.topics, &outside));
    assert_eq!(config["device_class"], "temperature");
    assert_eq!(config["unit_of_measurement"], "°C");
    s.probe
        .expect_payload(&s.topics.state(&outside.id), "-12")
        .await;

    let config = s
        .probe
        .expect(&s.topics.config("sensor", &humidity.id), |_| true)
        .await;
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config["unit_of_measurement"], "%");
    s.probe
        .expect_payload(&s.topics.state(&humidity.id), "57")
        .await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn numeric_changes_are_published_as_plain_numbers() {
    let outside = numeric("Outside", DeviceKind::Temperature, 0);
    let mut s = setup(snapshot_with(vec![outside.clone()])).await;
    let topic = s.topics.state(&outside.id);
    s.probe.expect_payload(&topic, "0").await;

    for (value, payload) in [(21, "21"), (-3, "-3"), (0, "0"), (-40, "-40")] {
        s.lab.set_state(&outside.id, value, Source::Ui).unwrap();
        s.probe.expect_payload(&topic, payload).await;
    }
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn numeric_sensors_ignore_commands() {
    let s = setup(snapshot_with(vec![
        lamp(false),
        numeric("Outside", DeviceKind::Temperature, 5),
        numeric("Humidity", DeviceKind::Percent, 50),
    ]))
    .await;

    s.probe
        .publish(&s.topics.command(&id("outside")), "ON")
        .await;
    s.probe
        .publish(&s.topics.command(&id("humidity")), "80")
        .await;
    s.probe.publish(&s.topics.command(&id("lamp")), "ON").await;

    eventually("the lamp is on", || {
        s.lab.device(&id("lamp")).unwrap().state == State::Bool(true)
    })
    .await;
    assert_eq!(s.lab.device(&id("outside")).unwrap().state, State::Int(5));
    assert_eq!(s.lab.device(&id("humidity")).unwrap().state, State::Int(50));
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn removing_a_numeric_sensor_clears_its_entity() {
    let mut s = setup(Snapshot::default()).await;
    let outside = s
        .lab
        .add_device("Outside", DeviceKind::Temperature)
        .unwrap();
    s.probe
        .expect(&s.topics.config("sensor", &outside.id), |p| !p.is_empty())
        .await;

    s.lab.remove_device(&outside.id).unwrap();

    s.probe
        .expect_payload(&s.topics.config("sensor", &outside.id), "")
        .await;
    s.probe
        .expect_payload(&s.topics.state(&outside.id), "")
        .await;
    s.bridge.shutdown().await;
}

fn offline(mut device: Device) -> Device {
    device.online = false;
    device
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn each_device_announces_its_own_availability() {
    let (lamp, door) = (lamp(false), offline(door()));
    let mut s = setup(snapshot_with(vec![door.clone(), lamp.clone()])).await;

    s.probe
        .expect_payload(&s.topics.availability(&door.id), "offline")
        .await;
    s.probe
        .expect_payload(&s.topics.availability(&lamp.id), "online")
        .await;
    // The lamp is announced after the door, so everything the door published has arrived by now.
    assert_eq!(
        s.probe.last_position(&s.topics.state(&door.id), "OFF"),
        None,
        "an offline device must not report a state"
    );
    s.probe
        .expect_payload(&s.topics.state(&lamp.id), "OFF")
        .await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn a_device_that_drops_off_goes_silent_and_reports_when_it_returns() {
    let outside = numeric("Outside", DeviceKind::Temperature, 5);
    let mut s = setup(snapshot_with(vec![outside.clone(), lamp(false)])).await;
    let state = s.topics.state(&outside.id);
    let availability = s.topics.availability(&outside.id);
    s.probe.expect_payload(&availability, "online").await;
    s.probe.expect_payload(&state, "5").await;
    s.probe.drain();

    s.lab.set_online(&outside.id, false, Source::Ui).unwrap();
    s.probe.expect_payload(&availability, "offline").await;

    // The temperature changes while the sensor is unreachable. The lamp event is a marker:
    // events are published in order, so if the temperature had been published it would be seen by now.
    s.lab.set_state(&outside.id, 21, Source::Ui).unwrap();
    s.lab.set_state(&id("lamp"), true, Source::Ui).unwrap();
    s.probe
        .expect_payload(&s.topics.state(&id("lamp")), "ON")
        .await;
    assert_eq!(s.probe.last_position(&state, "21"), None);

    s.lab.set_online(&outside.id, true, Source::Ui).unwrap();
    s.probe.expect_payload(&availability, "online").await;
    let reported = s
        .probe
        .last_position(&state, "21")
        .expect("state is reported on return");
    let announced = s.probe.last_position(&availability, "online").unwrap();
    assert!(
        reported < announced,
        "the state must arrive before the device says it is online"
    );
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn commands_to_an_offline_device_are_ignored() {
    let relay = Device::new("Heater", DeviceKind::Relay).unwrap();
    let s = setup(snapshot_with(vec![lamp(false), relay.clone()])).await;
    s.lab.set_online(&id("lamp"), false, Source::Ui).unwrap();
    s.lab.set_online(&relay.id, false, Source::Ui).unwrap();
    s.lab.set_online(&relay.id, true, Source::Ui).unwrap();

    s.probe.publish(&s.topics.command(&id("lamp")), "ON").await;
    // Commands are handled in order, so once the heater reacts the lamp command was seen too.
    s.probe.publish(&s.topics.command(&relay.id), "ON").await;

    eventually("the heater is on", || {
        s.lab.device(&relay.id).unwrap().state == State::Bool(true)
    })
    .await;
    assert_eq!(s.lab.device(&id("lamp")).unwrap().state, State::Bool(false));

    s.lab.set_online(&id("lamp"), true, Source::Ui).unwrap();
    s.probe.publish(&s.topics.command(&id("lamp")), "ON").await;
    eventually("the lamp is on once it is back", || {
        s.lab.device(&id("lamp")).unwrap().state == State::Bool(true)
    })
    .await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn removing_a_device_clears_its_availability_and_birth_republishes_it() {
    let lamp = lamp(false);
    let mut s = setup(snapshot_with(vec![lamp.clone()])).await;
    let availability = s.topics.availability(&lamp.id);
    s.probe.expect_payload(&availability, "online").await;
    s.probe.drain();

    s.probe.publish(&s.topics.ha_status(), "online").await;
    s.probe.expect_payload(&availability, "online").await;
    let state = s.probe.last_position(&s.topics.state(&lamp.id), "OFF");
    let online = s.probe.last_position(&availability, "online");
    assert!(
        state.is_some() && state < online,
        "on a resync an online device reports its state before saying it is online"
    );

    s.lab.remove_device(&lamp.id).unwrap();
    s.probe.expect_payload(&availability, "").await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn after_an_outage_the_bridge_reconnects_and_withdraws_what_was_deleted() {
    let proxy = Proxy::start().await;
    let (lamp, door) = (lamp(false), door());
    let mut s = setup_via(
        snapshot_with(vec![lamp.clone(), door.clone()]),
        Some(proxy.port),
    )
    .await;
    let status = s.topics.status();
    s.probe
        .expect_payload(&s.topics.state(&door.id), "OFF")
        .await;
    s.probe.drain();

    proxy.cut();

    // The broker announces the bridge's death with its last will.
    s.probe.expect_payload(&status, "offline").await;
    eventually("the bridge notices the outage", || {
        s.lab.mqtt_state() == MqttState::Disconnected
    })
    .await;

    // While the bridge is cut off one device is deleted and the other changes.
    s.lab.remove_device(&door.id).unwrap();
    s.lab.set_state(&lamp.id, true, Source::Ui).unwrap();
    proxy.restore();

    s.probe.expect_payload(&status, "online").await;
    s.probe
        .expect_payload(&s.topics.config("binary_sensor", &door.id), "")
        .await;
    s.probe.expect_payload(&s.topics.state(&door.id), "").await;
    s.probe
        .expect_payload(&s.topics.state(&lamp.id), "ON")
        .await;
    assert_eq!(s.lab.mqtt_state(), MqttState::Connected);
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn two_instances_with_default_settings_do_not_kick_each_other_off() {
    let (host, port) = broker();
    let start = |lab: Arc<Lab>| {
        let id = unique();
        let settings = Mqtt {
            enabled: true,
            host: host.clone(),
            port,
            client_id: None,
            discovery_prefix: format!("ha-{id}"),
            base_topic: format!("shl-{id}"),
            ..Mqtt::default()
        };
        mqtt::start(&settings, lab).unwrap()
    };
    let (first, second) = (
        Arc::new(Lab::new(Snapshot::default())),
        Arc::new(Lab::new(Snapshot::default())),
    );
    let bridges = [start(first.clone()), start(second.clone())];

    for lab in [&first, &second] {
        eventually("a bridge is connected", || {
            lab.mqtt_state() == MqttState::Connected
        })
        .await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;

    for lab in [&first, &second] {
        assert_eq!(lab.mqtt_state(), MqttState::Connected);
    }
    for bridge in bridges {
        bridge.shutdown().await;
    }
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn switching_project_withdraws_the_old_entities_and_announces_the_new_ones() {
    let (lamp, door) = (lamp(false), door());
    let mut s = setup(snapshot_with(vec![lamp.clone(), door.clone()])).await;
    s.probe
        .expect_payload(&s.topics.availability(&door.id), "online")
        .await;
    s.probe.drain();
    let gate = Device::new("Gate", DeviceKind::Sensor).unwrap();
    let dacha = smart_home_lab::domain::ProjectId::parse("dacha").unwrap();

    s.lab.replace(dacha, snapshot_with(vec![gate.clone()]));

    for (component, id) in [("switch", &lamp.id), ("binary_sensor", &door.id)] {
        s.probe
            .expect_payload(&s.topics.config(component, id), "")
            .await;
        s.probe.expect_payload(&s.topics.state(id), "").await;
        s.probe.expect_payload(&s.topics.availability(id), "").await;
    }
    let config = s
        .probe
        .expect(&s.topics.config("binary_sensor", &gate.id), |p| {
            !p.is_empty()
        })
        .await;
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config, discovery::config_payload(&s.topics, &gate));
    s.probe
        .expect_payload(&s.topics.availability(&gate.id), "online")
        .await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn an_id_that_returns_as_another_kind_does_not_leave_its_old_entity_behind() {
    let lamp = lamp(false);
    let mut s = setup(snapshot_with(vec![lamp.clone()])).await;
    s.probe
        .expect(&s.topics.config("switch", &lamp.id), |p| !p.is_empty())
        .await;
    s.probe.drain();
    // The other project has a device with the very same id, but it is a thermometer.
    let thermometer = Device::new("Lamp", DeviceKind::Temperature).unwrap();
    assert_eq!(thermometer.id, lamp.id);
    let dacha = smart_home_lab::domain::ProjectId::parse("dacha").unwrap();

    s.lab
        .replace(dacha, snapshot_with(vec![thermometer.clone()]));

    // Two configs with one unique_id would make two entities in Home Assistant.
    let switch_config = s.topics.config("switch", &lamp.id);
    let sensor_config = s.topics.config("sensor", &lamp.id);
    s.probe.expect_payload(&switch_config, "").await;
    let created = s.probe.expect(&sensor_config, |p| !p.is_empty()).await;
    let cleared_at = s.probe.last_position(&switch_config, "").unwrap();
    let created_at = s.probe.last_position(&sensor_config, &created).unwrap();
    assert!(
        cleared_at < created_at,
        "the old entity must go before the new one appears"
    );
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn commands_after_a_switch_reach_the_new_project_only() {
    let lamp = lamp(false);
    let mut s = setup(snapshot_with(vec![lamp.clone()])).await;
    let relay = Device::new("Pump", DeviceKind::Relay).unwrap();
    let dacha = smart_home_lab::domain::ProjectId::parse("dacha").unwrap();
    s.lab.replace(dacha, snapshot_with(vec![relay.clone()]));
    // Wait for what the bridge does about it (not for the switch itself, which has already happened).
    s.probe
        .expect_payload(&s.topics.config("switch", &lamp.id), "")
        .await;
    s.probe
        .expect(&s.topics.config("switch", &relay.id), |p| !p.is_empty())
        .await;

    // The old device no longer exists, so its command topic is ignored; the new one works.
    s.probe.publish(&s.topics.command(&lamp.id), "ON").await;
    s.probe.publish(&s.topics.command(&relay.id), "ON").await;

    eventually("the pump is on", || {
        s.lab.device(&relay.id).unwrap().state == State::Bool(true)
    })
    .await;
    assert!(s.lab.device(&lamp.id).is_err());
    s.bridge.shutdown().await;
}

/// What an earlier run leaves behind for `device`: its retained config under `component`, state and
/// availability.
fn leftover(topics: &Topics, device: &Device, component: &str) -> Vec<(String, String)> {
    vec![
        (
            topics.config(component, &device.id),
            discovery::config_payload(topics, device).to_string(),
        ),
        (topics.state(&device.id), "ON".to_owned()),
        (topics.availability(&device.id), "online".to_owned()),
    ]
}

fn ghost() -> Device {
    Device::new("Ghost", DeviceKind::Switch).unwrap()
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn a_device_an_earlier_run_left_behind_is_removed_when_the_bridge_connects() {
    let lamp = lamp(false);
    let mut s = setup_with(
        snapshot_with(vec![lamp.clone()]),
        Options {
            retained: Some(|topics| leftover(topics, &ghost(), "switch")),
            ..Options::default()
        },
    )
    .await;
    let ghost = ghost();

    s.probe
        .expect_payload(&s.topics.config("switch", &ghost.id), "")
        .await;
    s.probe.expect_payload(&s.topics.state(&ghost.id), "").await;
    s.probe
        .expect_payload(&s.topics.availability(&ghost.id), "")
        .await;
    s.probe
        .expect(&s.topics.config("switch", &lamp.id), |p| !p.is_empty())
        .await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn a_config_left_under_a_platform_the_device_no_longer_has_is_cleared() {
    let lamp = lamp(false);
    let mut s = setup_with(
        snapshot_with(vec![lamp.clone()]),
        Options {
            // An earlier run had a thermometer with this id.
            retained: Some(|topics| {
                let thermometer = Device::new("Lamp", DeviceKind::Temperature).unwrap();
                leftover(topics, &thermometer, "sensor")
            }),
            ..Options::default()
        },
    )
    .await;

    s.probe
        .expect_payload(&s.topics.config("sensor", &lamp.id), "")
        .await;
    s.probe
        .expect(&s.topics.config("switch", &lamp.id), |p| !p.is_empty())
        .await;
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn the_cleanup_never_touches_a_config_of_the_current_project() {
    let current = lamp(false);
    let mut s = setup_with(
        snapshot_with(vec![current.clone()]),
        Options {
            retained: Some(|topics| leftover(topics, &lamp(false), "switch")),
            ..Options::default()
        },
    )
    .await;
    let lamp = current;
    s.probe
        .expect(&s.topics.config("switch", &lamp.id), |p| !p.is_empty())
        .await;

    s.probe.listen_for(Duration::from_millis(700)).await;

    assert!(
        !s.probe.saw(&s.topics.config("switch", &lamp.id), ""),
        "the config of a device that exists was cleared"
    );
    assert!(!s.probe.saw(&s.topics.state(&lamp.id), ""));
    s.bridge.shutdown().await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn entities_of_other_integrations_and_other_labs_are_left_alone() {
    let mut s = setup_with(
        Snapshot::default(),
        Options {
            retained: Some(|topics| {
                let other = Device::new("Other", DeviceKind::Switch).unwrap();
                let another_lab = Topics::new("ha-elsewhere", "another-lab").unwrap();
                vec![
                    // Another integration, on the same discovery prefix.
                    (
                        topics.config("sensor", &id("thing")),
                        r#"{"unique_id":"zigbee_thing","state_topic":"zigbee2mqtt/thing"}"#
                            .to_owned(),
                    ),
                    // One of our own kind of entity, but another lab's (another base topic).
                    (
                        topics.config("switch", &other.id),
                        discovery::config_payload(&another_lab, &other).to_string(),
                    ),
                ]
            }),
            ..Options::default()
        },
    )
    .await;

    s.probe.listen_for(Duration::from_millis(700)).await;

    let foreign = s.topics.config("sensor", &id("thing"));
    let other_lab = s.topics.config("switch", &id("other"));
    for topic in [&foreign, &other_lab] {
        assert!(!s.probe.saw(topic, ""), "{topic} was cleared");
    }
    s.bridge.shutdown().await;
    // They are still on the broker for whoever connects next.
    let mut later = Probe::connect(&[s.topics.config_filter()]).await;
    later.expect(&foreign, |p| !p.is_empty()).await;
    later.expect(&other_lab, |p| !p.is_empty()).await;
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn stopping_the_bridge_takes_its_entities_out_of_home_assistant() {
    let (lamp, door) = (lamp(false), door());
    let mut s = setup(snapshot_with(vec![lamp.clone(), door.clone()])).await;
    s.probe
        .expect(&s.topics.config("binary_sensor", &door.id), |p| {
            !p.is_empty()
        })
        .await;
    s.probe.drain();

    s.bridge.shutdown().await;

    for (component, device) in [("switch", &lamp), ("binary_sensor", &door)] {
        s.probe
            .expect_payload(&s.topics.config(component, &device.id), "")
            .await;
        s.probe
            .expect_payload(&s.topics.state(&device.id), "")
            .await;
        s.probe
            .expect_payload(&s.topics.availability(&device.id), "")
            .await;
    }
    s.probe
        .expect_payload(&s.topics.status(), discovery::OFFLINE)
        .await;
    // Nothing is left for the next subscriber.
    let mut later = Probe::connect(&[s.topics.config_filter()]).await;
    later.listen_for(Duration::from_millis(500)).await;
    assert!(
        later.received.iter().all(|(_, payload)| payload.is_empty()),
        "something is still retained: {:?}",
        later.received
    );
}

#[tokio::test]
#[ignore = "needs an MQTT broker: docker compose up -d"]
async fn the_entities_stay_when_remove_on_exit_is_off() {
    let lamp = lamp(false);
    let mut s = setup_with(
        snapshot_with(vec![lamp.clone()]),
        Options {
            keep_on_exit: true,
            ..Options::default()
        },
    )
    .await;
    s.probe
        .expect(&s.topics.config("switch", &lamp.id), |p| !p.is_empty())
        .await;

    s.bridge.shutdown().await;
    s.probe.listen_for(Duration::from_millis(500)).await;

    assert!(!s.probe.saw(&s.topics.config("switch", &lamp.id), ""));
    let mut later = Probe::connect(&[s.topics.config_filter()]).await;
    later
        .expect(&s.topics.config("switch", &lamp.id), |p| !p.is_empty())
        .await;
}
