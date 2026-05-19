use crate::config::{Config, SensorConfig};
use crate::{mqtt, mqtt_client, parser, serial};
use std::io;
use std::sync::mpsc;
use std::time::Duration;
use tracing::{error, info};

const MQTT_RECONNECT_DELAY: Duration = Duration::from_secs(5);

trait TelegramReader {
    fn read_telegram(&mut self) -> io::Result<Vec<u8>>;
}

impl TelegramReader for serial::SerialPort {
    fn read_telegram(&mut self) -> io::Result<Vec<u8>> {
        serial::read_telegram(self)
    }
}

trait Publisher {
    fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> io::Result<()>;
}

impl Publisher for mqtt_client::MqttClient {
    fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> io::Result<()> {
        mqtt_client::MqttClient::publish(self, topic, payload, retain)
    }
}

trait PublisherFactory {
    type Client: Publisher;

    fn connect(&mut self, host: &str, port: u16, client_id: &str) -> io::Result<Self::Client>;
}

struct MqttPublisherFactory;

impl PublisherFactory for MqttPublisherFactory {
    type Client = mqtt_client::MqttClient;

    fn connect(&mut self, host: &str, port: u16, client_id: &str) -> io::Result<Self::Client> {
        mqtt_client::MqttClient::connect(host, port, client_id)
    }
}

pub fn run(config: Config) {
    let (tx, rx) = mpsc::channel::<mqtt::Publish>();

    let publish_interval = config.publish_interval;
    let base_topic = config.mqtt.base_topic.clone();
    let node_id = config.mqtt.client_id.clone();

    for sensor in config.sensors {
        let tx = tx.clone();
        let base_topic = base_topic.clone();
        let node_id = node_id.clone();
        std::thread::Builder::new()
            .name(sensor.name.clone())
            .spawn(move || run_sensor(sensor, tx, publish_interval, base_topic, node_id))
            .expect("failed to spawn sensor thread");
    }
    drop(tx);

    run_mqtt_loop(rx, &config.mqtt.host, config.mqtt.port, &node_id);
}

fn run_mqtt_loop(rx: mpsc::Receiver<mqtt::Publish>, host: &str, port: u16, client_id: &str) {
    let mut factory = MqttPublisherFactory;
    run_mqtt_loop_with(rx, host, port, client_id, &mut factory, std::thread::sleep);
}

fn run_mqtt_loop_with<F, S>(
    rx: mpsc::Receiver<mqtt::Publish>,
    host: &str,
    port: u16,
    client_id: &str,
    factory: &mut F,
    mut sleep: S,
) where
    F: PublisherFactory,
    S: FnMut(Duration),
{
    loop {
        let mut client = connect_mqtt_with_retry(host, port, client_id, factory, &mut sleep);

        match publish_until_error_or_disconnect(&rx, &mut client) {
            PublishLoopAction::Reconnect => continue,
            PublishLoopAction::Shutdown => return,
        }
    }
}

fn run_sensor(
    config: SensorConfig,
    tx: mpsc::Sender<mqtt::Publish>,
    publish_interval: Duration,
    base_topic: String,
    node_id: String,
) {
    info!(sensor = %config.name, port = %config.serial_port, baud = config.baud_rate, "Opening serial port");

    let mut port = match serial::open_serial_port(&config.serial_port, config.baud_rate) {
        Ok(p) => p,
        Err(e) => {
            error!(sensor = %config.name, "Failed to open serial port: {e}");
            return;
        }
    };

    info!(sensor = %config.name, "Serial port open, reading data");

    run_sensor_loop(
        &config,
        &mut port,
        &tx,
        publish_interval,
        &base_topic,
        &node_id,
        std::thread::sleep,
    );
}

fn run_sensor_loop<R, S>(
    config: &SensorConfig,
    reader: &mut R,
    tx: &mpsc::Sender<mqtt::Publish>,
    publish_interval: Duration,
    base_topic: &str,
    node_id: &str,
    mut sleep: S,
) where
    R: TelegramReader,
    S: FnMut(Duration),
{
    let Some(discovery_raw) = read_sensor_telegram(reader, &config.name) else {
        return;
    };
    let discovery_telegram = match parser::parse_telegram(&discovery_raw) {
        Ok(telegram) => telegram,
        Err(error) => {
            error!(sensor = %config.name, "Parse error: {error}");
            return;
        }
    };

    let sensor = discovery_sensor(config, base_topic, &discovery_telegram);
    info!(sensor = %sensor.name, device_id = %sensor.device_id, "Publishing discovery");
    if !publish_messages(&tx, discovery_messages(&sensor, &node_id)) {
        return;
    }

    loop {
        let Some(raw) = read_sensor_telegram(reader, &sensor.name) else {
            return;
        };

        let (telegram, messages) = match process_sensor_telegram(&sensor, &raw) {
            Ok(result) => result,
            Err(error) => {
                error!(sensor = %sensor.name, "Parse error: {error}");
                continue;
            }
        };

        log_telegram(&sensor.name, &telegram);
        if !publish_messages(&tx, messages) {
            return;
        }

        // Sleep keeps the process fully dormant between publishes (~0% CPU).
        // Continuous blocking reads cost ~1-2% CPU due to scheduler churn even
        // with VMIN tuning. tcflush at the start of the next read discards any
        // bytes that arrived during the sleep.
        sleep(publish_interval);
    }
}

fn connect_mqtt_with_retry<F, S>(
    host: &str,
    port: u16,
    client_id: &str,
    factory: &mut F,
    sleep: &mut S,
) -> F::Client
where
    F: PublisherFactory,
    S: FnMut(Duration),
{
    loop {
        match factory.connect(host, port, client_id) {
            Ok(client) => {
                info!("MQTT connected to {host}:{port}");
                return client;
            }
            Err(error) => {
                error!("MQTT connect failed: {error}");
                sleep(MQTT_RECONNECT_DELAY);
            }
        }
    }
}

fn recv_publish(rx: &mpsc::Receiver<mqtt::Publish>) -> Option<mqtt::Publish> {
    rx.recv().ok()
}

fn publish_message<P: Publisher>(client: &mut P, message: &mqtt::Publish) -> io::Result<()> {
    client.publish(&message.topic, message.payload.as_bytes(), message.retain)
}

fn publish_until_error_or_disconnect<P: Publisher>(
    rx: &mpsc::Receiver<mqtt::Publish>,
    client: &mut P,
) -> PublishLoopAction {
    while let Some(message) = recv_publish(rx) {
        if let Err(error) = publish_message(client, &message) {
            error!("MQTT publish error: {error}, reconnecting");
            return PublishLoopAction::Reconnect;
        }
    }

    PublishLoopAction::Shutdown
}

fn read_sensor_telegram<R: TelegramReader>(reader: &mut R, sensor_name: &str) -> Option<Vec<u8>> {
    match reader.read_telegram() {
        Ok(raw) => Some(raw),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            info!(sensor = sensor_name, "Serial port closed");
            None
        }
        Err(error) => {
            error!(sensor = sensor_name, "Read error: {error}");
            None
        }
    }
}

enum PublishLoopAction {
    Reconnect,
    Shutdown,
}

fn discovery_sensor(
    config: &SensorConfig,
    base_topic: &str,
    discovery_telegram: &parser::Telegram,
) -> mqtt::Sensor {
    let device_id = discovery_telegram
        .meter_id()
        .unwrap_or(&discovery_telegram.device_id);
    mqtt::Sensor::new(&config.name, base_topic, device_id)
}

fn discovery_messages(sensor: &mqtt::Sensor, node_id: &str) -> Vec<mqtt::Publish> {
    mqtt::discovery_publishes(sensor, node_id)
}

fn reading_messages(sensor: &mqtt::Sensor, telegram: &parser::Telegram) -> Vec<mqtt::Publish> {
    mqtt::reading_publishes(sensor, telegram)
}

fn process_sensor_telegram(
    sensor: &mqtt::Sensor,
    raw: &[u8],
) -> Result<(parser::Telegram, Vec<mqtt::Publish>), parser::ParseError> {
    let telegram = parser::parse_telegram(raw)?;
    let messages = reading_messages(sensor, &telegram);
    Ok((telegram, messages))
}

fn publish_messages(tx: &mpsc::Sender<mqtt::Publish>, messages: Vec<mqtt::Publish>) -> bool {
    for msg in messages {
        if tx.send(msg).is_err() {
            return false;
        }
    }
    true
}

fn log_telegram(label: &str, t: &parser::Telegram) {
    info!("[{label}] device={}", t.device_id);
    for r in &t.readings {
        info!("[{label}]   {r}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Reading;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    const BASE_TOPIC: &str = "meters";
    const NODE_ID: &str = "stromzaehler2mqtt";
    const PUBLISH_INTERVAL: Duration = Duration::from_secs(15);

    fn sensor_config(name: &str) -> SensorConfig {
        SensorConfig {
            name: name.to_string(),
            serial_port: "/dev/null".to_string(),
            baud_rate: 9600,
        }
    }

    fn publish(topic: &str, payload: &str, retain: bool) -> mqtt::Publish {
        mqtt::Publish {
            topic: topic.to_string(),
            payload: payload.to_string(),
            retain,
        }
    }

    fn state_publish(subtopic: &str, payload: &str) -> mqtt::Publish {
        publish(&format!("{BASE_TOPIC}/main/{subtopic}"), payload, false)
    }

    fn queued_publish(subtopic: &str, payload: &str) -> mqtt::Publish {
        state_publish(subtopic, payload)
    }

    fn drain_published(rx: &mpsc::Receiver<mqtt::Publish>) -> Vec<mqtt::Publish> {
        rx.try_iter().collect()
    }

    enum ReadStep {
        Telegram(&'static [u8]),
        Eof,
        Error(io::ErrorKind),
    }

    struct FakeReader {
        steps: VecDeque<ReadStep>,
    }

    impl FakeReader {
        fn new(steps: impl IntoIterator<Item = ReadStep>) -> Self {
            Self {
                steps: steps.into_iter().collect(),
            }
        }
    }

    impl TelegramReader for FakeReader {
        fn read_telegram(&mut self) -> io::Result<Vec<u8>> {
            match self.steps.pop_front().expect("missing fake reader step") {
                ReadStep::Telegram(raw) => Ok(raw.to_vec()),
                ReadStep::Eof => Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                ReadStep::Error(kind) => Err(io::Error::from(kind)),
            }
        }
    }

    fn discovery_raw() -> &'static [u8] {
        b"/EBZ5DD32R06ETA_107\r\n\
1-0:0.0.0*255(1EBZ0102861889)\r\n\
!\r\n"
    }

    fn good_reading_raw() -> &'static [u8] {
        b"/EBZ5DD32R06ETA_107\r\n\
1-0:1.8.0*255(002714.12830185*kWh)\r\n\
1-0:2.8.0*255(000001.20600000*kWh)\r\n\
1-0:16.7.0*255(000211.26*W)\r\n\
!\r\n"
    }

    enum ConnectStep {
        Ok {
            publish_results: VecDeque<io::Result<()>>,
        },
        Err(io::ErrorKind),
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PublishAttempt {
        connection_id: usize,
        publish: mqtt::Publish,
    }

    #[derive(Default)]
    struct FakeFactoryState {
        connect_attempts: usize,
        next_connection_id: usize,
        publish_attempts: Vec<PublishAttempt>,
    }

    struct FakePublisherFactory {
        steps: VecDeque<ConnectStep>,
        state: Rc<RefCell<FakeFactoryState>>,
    }

    impl FakePublisherFactory {
        fn new(
            steps: impl IntoIterator<Item = ConnectStep>,
        ) -> (Self, Rc<RefCell<FakeFactoryState>>) {
            let state = Rc::new(RefCell::new(FakeFactoryState::default()));
            (
                Self {
                    steps: steps.into_iter().collect(),
                    state: Rc::clone(&state),
                },
                state,
            )
        }
    }

    struct FakePublisher {
        connection_id: usize,
        publish_results: VecDeque<io::Result<()>>,
        state: Rc<RefCell<FakeFactoryState>>,
    }

    impl Publisher for FakePublisher {
        fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> io::Result<()> {
            self.state
                .borrow_mut()
                .publish_attempts
                .push(PublishAttempt {
                    connection_id: self.connection_id,
                    publish: mqtt::Publish {
                        topic: topic.to_string(),
                        payload: String::from_utf8(payload.to_vec())
                            .expect("payload should be UTF-8"),
                        retain,
                    },
                });

            self.publish_results.pop_front().unwrap_or(Ok(()))
        }
    }

    impl PublisherFactory for FakePublisherFactory {
        type Client = FakePublisher;

        fn connect(
            &mut self,
            _host: &str,
            _port: u16,
            _client_id: &str,
        ) -> io::Result<Self::Client> {
            let mut state = self.state.borrow_mut();
            state.connect_attempts += 1;

            match self.steps.pop_front().expect("missing fake connect step") {
                ConnectStep::Ok { publish_results } => {
                    let connection_id = state.next_connection_id;
                    state.next_connection_id += 1;
                    Ok(FakePublisher {
                        connection_id,
                        publish_results,
                        state: Rc::clone(&self.state),
                    })
                }
                ConnectStep::Err(kind) => Err(io::Error::from(kind)),
            }
        }
    }

    #[test]
    fn discovery_uses_meter_id_when_present() {
        let telegram = parser::Telegram {
            device_id: "EBZ5DD32R06ETA_107".to_string(),
            readings: vec![Reading::MeterId("1EBZ0102861889".to_string())],
        };

        let sensor = discovery_sensor(&sensor_config("main"), "meters", &telegram);

        assert_eq!(sensor.name, "main");
        assert_eq!(sensor.base_topic, "meters");
        assert_eq!(sensor.device_id, "1EBZ0102861889");
    }

    #[test]
    fn discovery_falls_back_to_device_id_without_meter_id() {
        let telegram = parser::Telegram {
            device_id: "EBZ5DD32R06ETA_107".to_string(),
            readings: vec![Reading::EnergyImport(2714.12830185)],
        };

        let sensor = discovery_sensor(&sensor_config("main"), "meters", &telegram);

        assert_eq!(sensor.device_id, "EBZ5DD32R06ETA_107");
    }

    #[test]
    fn process_sensor_telegram_returns_expected_reading_messages() {
        let sensor = mqtt::Sensor::new("main", "meters", "1EBZ0102861889");
        let raw = b"/EBZ5DD32R06ETA_107\r\n\
1-0:1.8.0*255(002714.12830185*kWh)\r\n\
1-0:2.8.0*255(000001.20600000*kWh)\r\n\
1-0:16.7.0*255(000211.26*W)\r\n\
1-0:36.7.0*255(000157.64*W)\r\n\
!\r\n";

        let (telegram, messages) = process_sensor_telegram(&sensor, raw).unwrap();

        assert_eq!(
            telegram,
            parser::Telegram {
                device_id: "EBZ5DD32R06ETA_107".to_string(),
                readings: vec![
                    Reading::EnergyImport(2714.12830185),
                    Reading::EnergyExport(1.206),
                    Reading::PowerTotal(211.26),
                    Reading::PowerL1(157.64),
                ],
            }
        );

        assert_eq!(
            messages,
            vec![
                state_publish("energy_import", "2714.12830185"),
                state_publish("energy_export", "1.206"),
                state_publish("power_total", "211.26"),
            ]
        );
    }

    #[test]
    fn malformed_sensor_telegram_is_returned_as_error() {
        let sensor = mqtt::Sensor::new("main", "meters", "1EBZ0102861889");
        let raw = b"/EBZ5DD32R06ETA_107\r\n1-0:1.8.0*255(bad*kWh)\r\n!\r\n";

        let err = process_sensor_telegram(&sensor, raw).unwrap_err();

        assert!(matches!(err, parser::ParseError::MalformedLine(_)));
    }

    #[test]
    fn publish_messages_returns_true_and_sends_all_messages_in_order() {
        let (tx, rx) = mpsc::channel();
        let messages = vec![
            state_publish("energy_import", "1.0"),
            state_publish("power_total", "2.0"),
        ];

        let sent = publish_messages(&tx, messages);
        let received = drain_published(&rx);

        assert!(sent);
        assert_eq!(
            received,
            vec![
                state_publish("energy_import", "1.0"),
                state_publish("power_total", "2.0"),
            ]
        );
    }

    #[test]
    fn publish_messages_returns_false_when_channel_is_closed() {
        let (tx, rx) = mpsc::channel();
        drop(rx);

        let sent = publish_messages(&tx, vec![state_publish("energy_import", "1.0")]);

        assert!(!sent);
    }

    #[test]
    fn run_sensor_loop_skips_malformed_telegram_after_discovery() {
        let config = sensor_config("main");
        let mut reader = FakeReader::new([
            ReadStep::Telegram(discovery_raw()),
            ReadStep::Telegram(b"/EBZ5DD32R06ETA_107\r\n1-0:1.8.0*255(bad*kWh)\r\n!\r\n"),
            ReadStep::Telegram(good_reading_raw()),
            ReadStep::Eof,
        ]);
        let (tx, rx) = mpsc::channel();
        let mut sleeps = Vec::new();

        run_sensor_loop(
            &config,
            &mut reader,
            &tx,
            PUBLISH_INTERVAL,
            BASE_TOPIC,
            NODE_ID,
            |duration| sleeps.push(duration),
        );
        drop(tx);

        let published = drain_published(&rx);

        assert_eq!(sleeps, vec![PUBLISH_INTERVAL]);
        assert_eq!(published.len(), 6);
        assert!(published.iter().take(3).all(|msg| {
            msg.topic
                .starts_with("homeassistant/sensor/stromzaehler2mqtt/")
        }));
        assert_eq!(
            published[3..],
            [
                state_publish("energy_import", "2714.12830185"),
                state_publish("energy_export", "1.206"),
                state_publish("power_total", "211.26"),
            ]
        );
    }

    #[test]
    fn run_sensor_loop_exits_cleanly_on_eof_after_discovery() {
        let config = sensor_config("main");
        let mut reader = FakeReader::new([ReadStep::Telegram(discovery_raw()), ReadStep::Eof]);
        let (tx, rx) = mpsc::channel();
        let mut slept = false;

        run_sensor_loop(
            &config,
            &mut reader,
            &tx,
            PUBLISH_INTERVAL,
            BASE_TOPIC,
            NODE_ID,
            |_| slept = true,
        );
        drop(tx);

        let published = drain_published(&rx);

        assert!(!slept);
        assert_eq!(published.len(), 3);
        assert!(published.iter().all(|msg| msg.retain));
    }

    #[test]
    fn run_sensor_loop_exits_on_read_error_after_discovery() {
        let config = sensor_config("main");
        let mut reader = FakeReader::new([
            ReadStep::Telegram(discovery_raw()),
            ReadStep::Error(io::ErrorKind::BrokenPipe),
        ]);
        let (tx, rx) = mpsc::channel();
        let mut slept = false;

        run_sensor_loop(
            &config,
            &mut reader,
            &tx,
            PUBLISH_INTERVAL,
            BASE_TOPIC,
            NODE_ID,
            |_| slept = true,
        );
        drop(tx);

        let published = drain_published(&rx);

        assert!(!slept);
        assert_eq!(published.len(), 3);
        assert!(published.iter().all(|msg| msg.retain));
    }

    #[test]
    fn run_mqtt_loop_retries_connect_until_success() {
        let (tx, rx) = mpsc::channel();
        tx.send(queued_publish("energy_import", "1.0")).unwrap();
        drop(tx);

        let (mut factory, state) = FakePublisherFactory::new([
            ConnectStep::Err(io::ErrorKind::ConnectionRefused),
            ConnectStep::Ok {
                publish_results: VecDeque::from([Ok(())]),
            },
        ]);
        let mut sleeps = Vec::new();

        run_mqtt_loop_with(rx, "localhost", 1883, "client", &mut factory, |duration| {
            sleeps.push(duration)
        });

        let state = state.borrow();
        assert_eq!(sleeps, vec![MQTT_RECONNECT_DELAY]);
        assert_eq!(state.connect_attempts, 2);
        assert_eq!(
            state.publish_attempts,
            vec![PublishAttempt {
                connection_id: 0,
                publish: queued_publish("energy_import", "1.0"),
            }]
        );
    }

    #[test]
    fn run_mqtt_loop_reconnects_after_publish_failure() {
        let (tx, rx) = mpsc::channel();
        tx.send(queued_publish("energy_import", "1.0")).unwrap();
        tx.send(queued_publish("power_total", "2.0")).unwrap();
        drop(tx);

        let (mut factory, state) = FakePublisherFactory::new([
            ConnectStep::Ok {
                publish_results: VecDeque::from([Err(io::Error::from(io::ErrorKind::BrokenPipe))]),
            },
            ConnectStep::Ok {
                publish_results: VecDeque::from([Ok(())]),
            },
        ]);

        run_mqtt_loop_with(rx, "localhost", 1883, "client", &mut factory, |_| {});

        let state = state.borrow();
        assert_eq!(state.connect_attempts, 2);
        assert_eq!(
            state.publish_attempts,
            vec![
                PublishAttempt {
                    connection_id: 0,
                    publish: queued_publish("energy_import", "1.0"),
                },
                PublishAttempt {
                    connection_id: 1,
                    publish: queued_publish("power_total", "2.0"),
                },
            ]
        );
    }

    #[test]
    fn run_mqtt_loop_exits_cleanly_when_channel_is_closed() {
        let (tx, rx) = mpsc::channel();
        drop(tx);

        let (mut factory, state) = FakePublisherFactory::new([ConnectStep::Ok {
            publish_results: VecDeque::new(),
        }]);
        let mut slept = false;

        run_mqtt_loop_with(rx, "localhost", 1883, "client", &mut factory, |_| {
            slept = true;
        });

        let state = state.borrow();
        assert!(!slept);
        assert_eq!(state.connect_attempts, 1);
        assert!(state.publish_attempts.is_empty());
    }
}
