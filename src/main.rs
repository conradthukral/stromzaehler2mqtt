mod mqtt;
mod mqtt_client;
mod parser;
mod serial;

use serde::Deserialize;
use std::io;
use std::sync::mpsc;
use std::time::Duration;
use tracing::{error, info};

#[derive(Clone, Debug, Deserialize)]
struct SensorConfig {
    name: String,
    serial_port: String,
    baud_rate: u32,
}

#[derive(Deserialize)]
struct MqttConfig {
    host: String,
    port: u16,
    client_id: String,
    base_topic: String,
}

fn deserialize_duration_secs<'de, D>(d: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    u64::deserialize(d).map(Duration::from_secs)
}

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    sensors: Vec<SensorConfig>,
    #[serde(
        rename = "publish_interval_secs",
        deserialize_with = "deserialize_duration_secs"
    )]
    publish_interval: Duration,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stromzaehler2mqtt=info".into()),
        )
        .init();

    let raw = std::fs::read_to_string("config.yaml").expect("config.yaml not found");
    let config: Config = serde_yaml::from_str(&raw).expect("invalid config.yaml");

    let (tx, rx) = mpsc::channel::<mqtt::Publish>();

    let publish_interval = config.publish_interval;
    let base_topic = config.mqtt.base_topic;
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
                Err(_) => return, // all sensor threads exited
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

    let discovery_telegram = match serial::read_telegram(&mut port) {
        Ok(b) => match parser::parse_telegram(&b) {
            Ok(t) => t,
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

    let device_id = discovery_telegram
        .meter_id()
        .unwrap_or(&discovery_telegram.device_id);
    let sensor = mqtt::Sensor::new(&config.name, base_topic, device_id);
    info!(sensor = %sensor.name, device_id = %sensor.device_id, "Publishing discovery");
    for msg in mqtt::discovery_publishes(&sensor, &node_id) {
        if tx.send(msg).is_err() {
            return;
        }
    }

    loop {
        let raw = match serial::read_telegram(&mut port) {
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

        let telegram = match parser::parse_telegram(&raw) {
            Ok(t) => t,
            Err(e) => {
                error!(sensor = %sensor.name, "Parse error: {e}");
                continue;
            }
        };

        log_telegram(&sensor.name, &telegram);
        for msg in mqtt::reading_publishes(&sensor, &telegram) {
            if tx.send(msg).is_err() {
                return;
            }
        }

        // Sleep keeps the process fully dormant between publishes (~0% CPU).
        // Continuous blocking reads cost ~1-2% CPU due to scheduler churn even
        // with VMIN tuning. tcflush at the start of the next read discards any
        // bytes that arrived during the sleep.
        std::thread::sleep(publish_interval);
    }
}

fn log_telegram(label: &str, t: &parser::Telegram) {
    info!("[{label}] device={}", t.device_id);
    for r in &t.readings {
        info!("[{label}]   {r}");
    }
}
