use crate::config::{Config, SensorConfig};
use crate::{mqtt, mqtt_client, parser, serial};
use std::io;
use std::sync::mpsc;
use std::time::Duration;
use tracing::{error, info};

trait TelegramReader {
    fn read_telegram(&mut self) -> io::Result<Vec<u8>>;
}

impl TelegramReader for serial::SerialPort {
    fn read_telegram(&mut self) -> io::Result<Vec<u8>> {
        serial::read_telegram(self)
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
    loop {
        let mut client = loop {
            match mqtt_client::MqttClient::connect(host, port, client_id) {
                Ok(c) => {
                    info!("MQTT connected to {host}:{port}");
                    break c;
                }
                Err(e) => {
                    error!("MQTT connect failed: {e}");
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        };

        loop {
            match rx.recv() {
                Ok(msg) => {
                    if let Err(e) = client.publish(&msg.topic, msg.payload.as_bytes(), msg.retain) {
                        error!("MQTT publish error: {e}, reconnecting");
                        break;
                    }
                }
                Err(_) => return,
            }
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
    let discovery_telegram = match reader.read_telegram() {
        Ok(raw) => match parse_discovery_telegram(&raw) {
            Ok(telegram) => telegram,
            Err(e) => {
                error!(sensor = %config.name, "Parse error: {e}");
                return;
            }
        },
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            info!(sensor = %config.name, "Serial port closed");
            return;
        }
        Err(e) => {
            error!(sensor = %config.name, "Read error: {e}");
            return;
        }
    };

    let sensor = discovery_sensor(config, base_topic, &discovery_telegram);
    info!(sensor = %sensor.name, device_id = %sensor.device_id, "Publishing discovery");
    if !publish_messages(&tx, discovery_messages(&sensor, &node_id)) {
        return;
    }

    loop {
        let raw = match reader.read_telegram() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                info!(sensor = %sensor.name, "Serial port closed");
                return;
            }
            Err(e) => {
                error!(sensor = %sensor.name, "Read error: {e}");
                return;
            }
        };

        let (telegram, messages) = match process_sensor_telegram(&sensor, &raw) {
            Ok(result) => result,
            Err(e) => {
                error!(sensor = %sensor.name, "Parse error: {e}");
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

fn parse_discovery_telegram(raw: &[u8]) -> Result<parser::Telegram, parser::ParseError> {
    parser::parse_telegram(raw)
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
    use std::collections::VecDeque;

    fn sensor_config(name: &str) -> SensorConfig {
        SensorConfig {
            name: name.to_string(),
            serial_port: "/dev/null".to_string(),
            baud_rate: 9600,
        }
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
                mqtt::Publish {
                    topic: "meters/main/energy_import".to_string(),
                    payload: "2714.12830185".to_string(),
                    retain: false,
                },
                mqtt::Publish {
                    topic: "meters/main/energy_export".to_string(),
                    payload: "1.206".to_string(),
                    retain: false,
                },
                mqtt::Publish {
                    topic: "meters/main/power_total".to_string(),
                    payload: "211.26".to_string(),
                    retain: false,
                },
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
            mqtt::Publish {
                topic: "meters/main/energy_import".to_string(),
                payload: "1.0".to_string(),
                retain: false,
            },
            mqtt::Publish {
                topic: "meters/main/power_total".to_string(),
                payload: "2.0".to_string(),
                retain: false,
            },
        ];

        let sent = publish_messages(&tx, messages);
        let received: Vec<_> = rx.try_iter().collect();

        assert!(sent);
        assert_eq!(
            received,
            vec![
                mqtt::Publish {
                    topic: "meters/main/energy_import".to_string(),
                    payload: "1.0".to_string(),
                    retain: false,
                },
                mqtt::Publish {
                    topic: "meters/main/power_total".to_string(),
                    payload: "2.0".to_string(),
                    retain: false,
                },
            ]
        );
    }

    #[test]
    fn publish_messages_returns_false_when_channel_is_closed() {
        let (tx, rx) = mpsc::channel();
        drop(rx);

        let sent = publish_messages(
            &tx,
            vec![mqtt::Publish {
                topic: "meters/main/energy_import".to_string(),
                payload: "1.0".to_string(),
                retain: false,
            }],
        );

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
            Duration::from_secs(15),
            "meters",
            "stromzaehler2mqtt",
            |duration| sleeps.push(duration),
        );
        drop(tx);

        let published: Vec<_> = rx.try_iter().collect();

        assert_eq!(sleeps, vec![Duration::from_secs(15)]);
        assert_eq!(published.len(), 6);
        assert!(published.iter().take(3).all(|msg| {
            msg.topic
                .starts_with("homeassistant/sensor/stromzaehler2mqtt/")
        }));
        assert_eq!(
            published[3..],
            [
                mqtt::Publish {
                    topic: "meters/main/energy_import".to_string(),
                    payload: "2714.12830185".to_string(),
                    retain: false,
                },
                mqtt::Publish {
                    topic: "meters/main/energy_export".to_string(),
                    payload: "1.206".to_string(),
                    retain: false,
                },
                mqtt::Publish {
                    topic: "meters/main/power_total".to_string(),
                    payload: "211.26".to_string(),
                    retain: false,
                },
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
            Duration::from_secs(15),
            "meters",
            "stromzaehler2mqtt",
            |_| slept = true,
        );
        drop(tx);

        let published: Vec<_> = rx.try_iter().collect();

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
            Duration::from_secs(15),
            "meters",
            "stromzaehler2mqtt",
            |_| slept = true,
        );
        drop(tx);

        let published: Vec<_> = rx.try_iter().collect();

        assert!(!slept);
        assert_eq!(published.len(), 3);
        assert!(published.iter().all(|msg| msg.retain));
    }
}
