mod mqtt;
mod mqtt_client;
mod parser;

use serde::Deserialize;
use std::io;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
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

    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY)
        .open(&config.serial_port)
    {
        Ok(f) => f,
        Err(e) => {
            error!(sensor = %config.name, "Failed to open serial port: {e}");
            return;
        }
    };

    if let Err(e) = configure_tty(file.as_raw_fd(), config.baud_rate) {
        error!(sensor = %config.name, "Failed to configure serial port: {e}");
        return;
    }

    let sensor = mqtt::Sensor::new(&config.name, base_topic);
    info!(sensor = %sensor.name, "Serial port open, reading data");

    let fd = file.as_raw_fd();
    let mut discovery_sent = false;

    loop {
        let raw = match read_telegram(&mut file, fd) {
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

        if !discovery_sent {
            info!(sensor = %sensor.name, device_id = %telegram.device_id, "Publishing discovery");
            for msg in mqtt::discovery_publishes(&sensor, &telegram.device_id, &node_id) {
                if tx.send(msg).is_err() {
                    return;
                }
            }
            discovery_sent = true;
        }

        log_telegram(&sensor.name, &telegram);
        for msg in mqtt::reading_publishes(&sensor, &telegram) {
            if tx.send(msg).is_err() {
                return;
            }
        }

        std::thread::sleep(publish_interval);
    }
}

fn log_telegram(label: &str, t: &parser::Telegram) {
    info!("[{label}] device={}", t.device_id);
    for r in &t.readings {
        info!("[{label}]   {r}");
    }
}

fn read_telegram(file: &mut std::fs::File, fd: std::os::unix::io::RawFd) -> io::Result<Vec<u8>> {
    unsafe { libc::tcflush(fd, libc::TCIFLUSH) };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        if let Some(i) = chunk[..n].iter().position(|&b| b == b'/') {
            buf.extend_from_slice(&chunk[i..n]);
            break;
        }
    }

    while !buf.contains(&b'!') {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        buf.extend_from_slice(&chunk[..n]);
    }

    Ok(buf)
}

fn configure_tty(fd: std::os::unix::io::RawFd, baud_rate: u32) -> io::Result<()> {
    let speed = match baud_rate {
        300 => libc::B300,
        600 => libc::B600,
        1200 => libc::B1200,
        2400 => libc::B2400,
        4800 => libc::B4800,
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported baud rate: {baud_rate}"),
            ));
        }
    };
    unsafe {
        let mut tios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut tios) != 0 {
            return Err(io::Error::last_os_error());
        }
        libc::cfmakeraw(&mut tios);
        // 7E1: 7 data bits, even parity, 1 stop bit (EN62056-21 mode D)
        tios.c_cflag &= !(libc::CSIZE | libc::PARODD | libc::CSTOPB);
        tios.c_cflag |= libc::CS7 | libc::PARENB | libc::CREAD | libc::CLOCAL;
        libc::cfsetspeed(&mut tios, speed);
        if libc::tcsetattr(fd, libc::TCSANOW, &tios) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
