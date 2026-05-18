use serde::Deserialize;
use std::fmt;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SensorConfig {
    pub name: String,
    pub serial_port: String,
    pub baud_rate: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub base_topic: String,
}

fn deserialize_duration_secs<'de, D>(d: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    u64::deserialize(d).map(Duration::from_secs)
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Config {
    pub mqtt: MqttConfig,
    pub sensors: Vec<SensorConfig>,
    #[serde(
        rename = "publish_interval_secs",
        deserialize_with = "deserialize_duration_secs"
    )]
    pub publish_interval: Duration,
}

#[derive(Debug)]
pub enum ConfigError {
    Read(std::io::Error),
    Parse(serde_yaml::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read(err) => write!(f, "failed to read config: {err}"),
            ConfigError::Parse(err) => write!(f, "failed to parse config: {err}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Read(err) => Some(err),
            ConfigError::Parse(err) => Some(err),
        }
    }
}

pub fn load_config(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(ConfigError::Read)?;
    serde_yaml::from_str(&raw).map_err(ConfigError::Parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_config_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "stromzaehler2mqtt-config-{}-{nanos}.yaml",
            std::process::id()
        ))
    }

    #[test]
    fn load_config_parses_yaml_file() {
        let path = temp_config_path();
        std::fs::write(
            &path,
            "mqtt:\n  host: localhost\n  port: 1883\n  client_id: app\n  base_topic: meters\npublish_interval_secs: 15\nsensors:\n  - name: sensor_a\n    serial_port: /dev/ttyUSB0\n    baud_rate: 9600\n",
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(config.mqtt.host, "localhost");
        assert_eq!(config.mqtt.port, 1883);
        assert_eq!(config.mqtt.client_id, "app");
        assert_eq!(config.mqtt.base_topic, "meters");
        assert_eq!(config.publish_interval, Duration::from_secs(15));
        assert_eq!(
            config.sensors,
            vec![SensorConfig {
                name: "sensor_a".into(),
                serial_port: "/dev/ttyUSB0".into(),
                baud_rate: 9600,
            }]
        );
    }

    #[test]
    fn load_config_returns_read_error_for_missing_file() {
        let path = temp_config_path();

        let err = load_config(&path).unwrap_err();

        assert!(matches!(err, ConfigError::Read(_)));
    }

    #[test]
    fn load_config_returns_parse_error_for_invalid_yaml() {
        let path = temp_config_path();
        std::fs::write(&path, "mqtt: [broken").unwrap();

        let err = load_config(&path).unwrap_err();
        std::fs::remove_file(&path).unwrap();

        assert!(matches!(err, ConfigError::Parse(_)));
    }
}
