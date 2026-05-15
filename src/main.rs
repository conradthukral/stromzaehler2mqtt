use rumqttc::{AsyncClient, MqttOptions};
use serde::Deserialize;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::task::JoinSet;
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
}

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    sensors: Vec<SensorConfig>,
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

    let mut mqtt_options = MqttOptions::new(
        &config.mqtt.client_id,
        &config.mqtt.host,
        config.mqtt.port,
    );
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

    let mut tasks = JoinSet::new();
    for sensor in config.sensors {
        let client = mqtt_client.clone();
        tasks.spawn(async move {
            run_sensor(sensor, client).await;
        });
    }

    while let Some(result) = tasks.join_next().await {
        if let Err(e) = result {
            error!("Sensor task panicked: {e}");
        }
    }
}

async fn run_sensor(config: SensorConfig, _mqtt: AsyncClient) {
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

    info!(sensor = %config.name, "Serial port open, reading data");
    let mut buf = [0u8; 256];
    loop {
        match port.read(&mut buf).await {
            Ok(0) => {
                info!(sensor = %config.name, "Serial port closed");
                break;
            }
            Ok(n) => dump_hex(&config.name, &buf[..n]),
            Err(e) => {
                error!(sensor = %config.name, "Read error: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                break;
            }
        }
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
            ))
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

fn dump_hex(label: &str, data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let hex: String = chunk.iter().map(|b| format!("{b:02x} ")).collect();
        let ascii: String = chunk
            .iter()
            .map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' })
            .collect();
        info!("[{label}] {:04x}  {hex:48} {ascii}", i * 16);
    }
}
