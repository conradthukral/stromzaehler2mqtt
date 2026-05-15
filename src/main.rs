use rumqttc::{AsyncClient, MqttOptions};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{error, info};

#[derive(Clone, Debug)]
struct SensorConfig {
    name: String,
    serial_port: String,
    baud_rate: u32,
}

struct MqttConfig {
    host: String,
    port: u16,
    client_id: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stromzaehler2mqtt=info".into()),
        )
        .init();

    let mqtt_config = MqttConfig {
        host: "localhost".into(),
        port: 1883,
        client_id: "stromzaehler2mqtt".into(),
    };

    let sensors = vec![
        SensorConfig {
            name: "sensor_1".into(),
            serial_port: "/dev/ttyUSB0".into(),
            baud_rate: 9600,
        },
        SensorConfig {
            name: "sensor_2".into(),
            serial_port: "/dev/ttyUSB1".into(),
            baud_rate: 9600,
        },
    ];

    let mut mqtt_options = MqttOptions::new(
        &mqtt_config.client_id,
        &mqtt_config.host,
        mqtt_config.port,
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
    for sensor in sensors {
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
