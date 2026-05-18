use crate::config::{Config, SensorConfig};
use crate::{mqtt, mqtt_client, parser, serial};
use std::io;
use std::sync::mpsc;
use std::time::Duration;
use tracing::{error, info};

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
