use rumqttc::{AsyncClient, MqttOptions};
use serde::Deserialize;
use std::time::Duration;
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
    info!(sensor = %config.name, port = %config.serial_port, "Starting sensor reader");
    // TODO: open serial port, read telegrams, parse OBIS values, publish to MQTT
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
