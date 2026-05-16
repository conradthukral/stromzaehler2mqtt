mod mqtt;
mod parser;

use rumqttc::{AsyncClient, MqttOptions};
use serde::Deserialize;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

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

struct PublishThrottle {
    interval: Duration,
    last: Option<Instant>,
}

impl PublishThrottle {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    fn ready(&mut self, now: Instant) -> bool {
        if self.last.is_none_or(|t| now - t >= self.interval) {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_first_call_is_ready() {
        let mut t = PublishThrottle::new(Duration::from_secs(60));
        assert!(t.ready(Instant::now()));
    }

    #[test]
    fn throttle_blocks_within_interval() {
        let mut t = PublishThrottle::new(Duration::from_secs(60));
        let now = Instant::now();
        assert!(t.ready(now));
        assert!(!t.ready(now + Duration::from_secs(59)));
    }

    #[test]
    fn throttle_passes_at_interval_boundary() {
        let mut t = PublishThrottle::new(Duration::from_secs(60));
        let now = Instant::now();
        assert!(t.ready(now));
        assert!(t.ready(now + Duration::from_secs(60)));
    }

    #[test]
    fn throttle_resets_after_firing() {
        let mut t = PublishThrottle::new(Duration::from_secs(60));
        let now = Instant::now();
        assert!(t.ready(now));
        assert!(t.ready(now + Duration::from_secs(60)));
        assert!(!t.ready(now + Duration::from_secs(119)));
        assert!(t.ready(now + Duration::from_secs(120)));
    }
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stromzaehler2mqtt=info".into()),
        )
        .init();

    let raw = std::fs::read_to_string("config.yaml").expect("config.yaml not found");
    let config: Config = serde_yaml::from_str(&raw).expect("invalid config.yaml");

    let mut mqtt_options =
        MqttOptions::new(&config.mqtt.client_id, &config.mqtt.host, config.mqtt.port);
    mqtt_options.set_keep_alive(Duration::from_secs(30));

    let (mqtt_client, mut eventloop) = AsyncClient::new(mqtt_options, 16);

    // Drive the MQTT event loop; reconnects on error.
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(_) => {}
                Err(e) => {
                    error!("MQTT error: {e}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });

    let publish_interval = config.publish_interval;
    let base_topic = config.mqtt.base_topic;

    let mut tasks = JoinSet::new();
    for sensor in config.sensors {
        let client = mqtt_client.clone();
        let base_topic = base_topic.clone();
        tasks.spawn(async move {
            run_sensor(sensor, client, publish_interval, base_topic).await;
        });
    }

    while let Some(result) = tasks.join_next().await {
        if let Err(e) = result {
            error!("Sensor task panicked: {e}");
        }
    }
}

async fn run_sensor(
    config: SensorConfig,
    mqtt_client: AsyncClient,
    publish_interval: Duration,
    base_topic: String,
) {
    info!(sensor = %config.name, port = %config.serial_port, baud = config.baud_rate, "Opening serial port");

    let std_file = match std::fs::OpenOptions::new()
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

    if let Err(e) = configure_tty(std_file.as_raw_fd(), config.baud_rate) {
        error!(sensor = %config.name, "Failed to configure serial port: {e}");
        return;
    }

    let mut port = tokio::fs::File::from_std(std_file);
    let sensor = mqtt::Sensor::new(&config.name, base_topic);

    info!(sensor = %sensor.name, "Serial port open, reading data");
    let mut accum: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 256];
    let mut discovery_sent = false;
    let mut throttle = PublishThrottle::new(publish_interval);
    loop {
        match port.read(&mut chunk).await {
            Ok(0) => {
                info!(sensor = %sensor.name, "Serial port closed");
                break;
            }
            Ok(n) => {
                accum.extend_from_slice(&chunk[..n]);
                let (telegrams, remaining) = parser::split_telegrams(&accum);
                let drain_len = accum.len() - remaining.len();
                for raw in telegrams {
                    match parser::parse_telegram(raw) {
                        Ok(t) => {
                            if !discovery_sent {
                                info!(sensor = %sensor.name, device_id = %t.device_id, "Publishing discovery");
                                match mqtt::publish_discovery(&mqtt_client, &sensor, &t.device_id)
                                    .await
                                {
                                    Ok(()) => discovery_sent = true,
                                    Err(e) => {
                                        warn!(sensor = %sensor.name, "Discovery publish failed, will retry: {e}")
                                    }
                                }
                            }
                            if throttle.ready(Instant::now()) {
                                log_telegram(&sensor.name, &t);
                                mqtt::publish_readings(&mqtt_client, &sensor, &t).await;
                            }
                        }
                        Err(e) => error!(sensor = %sensor.name, "Parse error: {e}"),
                    }
                }
                accum.drain(..drain_len);
            }
            Err(e) => {
                error!(sensor = %sensor.name, "Read error: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                break;
            }
        }
    }
}

fn log_telegram(label: &str, t: &parser::Telegram) {
    info!("[{label}] device={}", t.device_id);
    for r in &t.readings {
        info!("[{label}]   {r}");
    }
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
