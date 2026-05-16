use rumqttc::{AsyncClient, ClientError, QoS};
use serde_json::json;
use tracing::error;

use crate::parser::{Reading, Telegram};

pub struct Sensor {
    pub name: String,
    pub base_topic: String,
}

impl Sensor {
    pub fn new(name: impl Into<String>, base_topic: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_topic: base_topic.into(),
        }
    }

    fn value_topic(&self, subtopic: &str) -> String {
        format!("{}/{}/{subtopic}", self.base_topic, self.name)
    }
}

struct ReadingMeta {
    subtopic: &'static str,
    name: &'static str,
    unit: &'static str,
    device_class: &'static str,
    state_class: &'static str,
}

const READINGS: &[ReadingMeta] = &[
    ReadingMeta {
        subtopic: "energy_import",
        name: "Energy Import",
        unit: "kWh",
        device_class: "energy",
        state_class: "total_increasing",
    },
    ReadingMeta {
        subtopic: "energy_export",
        name: "Energy Export",
        unit: "kWh",
        device_class: "energy",
        state_class: "total_increasing",
    },
    ReadingMeta {
        subtopic: "power_total",
        name: "Power Total",
        unit: "W",
        device_class: "power",
        state_class: "measurement",
    },
];

fn sanitize_device_id(device_id: &str) -> String {
    device_id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Returns the MQTT state topic suffix and payload for a reading, or None if not published.
fn reading_to_state(reading: &Reading) -> Option<(&'static str, String)> {
    match reading {
        Reading::EnergyImport(v) => Some(("energy_import", v.to_string())),
        Reading::EnergyExport(v) => Some(("energy_export", v.to_string())),
        Reading::PowerTotal(v) => Some(("power_total", v.to_string())),
        _ => None,
    }
}

/// Returns (config_topic, payload_json) pairs for all discovery entries.
fn discovery_entries(sensor: &Sensor, device_id: &str) -> Vec<(String, String)> {
    let sanitized_name = sanitize_device_id(&sensor.name);
    let sanitized_id = sanitize_device_id(device_id);
    let device_key = format!("{sanitized_name}_{sanitized_id}");
    READINGS
        .iter()
        .map(|meta| {
            let unique_id = format!("{device_key}_{}", meta.subtopic);
            let config_topic = format!("homeassistant/sensor/{unique_id}/config");
            let payload = json!({
                "name": meta.name,
                "device_class": meta.device_class,
                "state_class": meta.state_class,
                "unit_of_measurement": meta.unit,
                "state_topic": sensor.value_topic(meta.subtopic),
                "unique_id": unique_id,
                "device": {
                    "identifiers": [&device_key],
                    "name": &sensor.name,
                }
            });
            (config_topic, payload.to_string())
        })
        .collect()
}

pub async fn publish_discovery(
    mqtt: &AsyncClient,
    sensor: &Sensor,
    device_id: &str,
) -> Result<(), ClientError> {
    for (topic, payload) in discovery_entries(sensor, device_id) {
        mqtt.publish(topic, QoS::AtMostOnce, true, payload).await?;
    }
    Ok(())
}

pub async fn publish_readings(mqtt: &AsyncClient, sensor: &Sensor, telegram: &Telegram) {
    for reading in &telegram.readings {
        let Some((subtopic, value)) = reading_to_state(reading) else {
            continue;
        };
        let topic = sensor.value_topic(subtopic);
        if let Err(e) = mqtt.publish(&topic, QoS::AtMostOnce, false, value).await {
            error!(sensor = %sensor.name, %topic, "MQTT publish error: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_lowercase_and_replace() {
        assert_eq!(
            sanitize_device_id("EBZ5DD32R06ETA_107"),
            "ebz5dd32r06eta_107"
        );
        assert_eq!(sanitize_device_id("EBZ-DD3.2R"), "ebz_dd3_2r");
        assert_eq!(sanitize_device_id("abc123"), "abc123");
    }

    #[test]
    fn reading_to_state_published_variants() {
        let cases = [
            (Reading::EnergyImport(2714.128), "energy_import", "2714.128"),
            (Reading::EnergyExport(1.206), "energy_export", "1.206"),
            (Reading::PowerTotal(211.26), "power_total", "211.26"),
        ];
        for (reading, expected_subtopic, expected_value) in cases {
            let (subtopic, value) = reading_to_state(&reading).expect("should be Some");
            assert_eq!(subtopic, expected_subtopic);
            assert_eq!(value, expected_value);
        }
    }

    #[test]
    fn reading_to_state_skipped_variants() {
        let skipped = [
            Reading::MeterId("x".into()),
            Reading::SerialNumber("x".into()),
            Reading::StatusFlags(0),
            Reading::OperatingTime(0),
            Reading::PowerL1(0.0),
            Reading::PowerL2(0.0),
            Reading::PowerL3(0.0),
            Reading::Unknown {
                code: "x".into(),
                value: "y".into(),
                unit: None,
            },
        ];
        for r in &skipped {
            assert!(reading_to_state(r).is_none(), "{r} should be skipped");
        }
    }

    #[test]
    fn discovery_entries_count_and_topics() {
        let sensor = Sensor::new("main", "stromzaehler2mqtt");
        let entries = discovery_entries(&sensor, "EBZ5DD32R06ETA_107");
        assert_eq!(entries.len(), 3);

        let topics: Vec<&str> = entries.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            topics.contains(&"homeassistant/sensor/main_ebz5dd32r06eta_107_energy_import/config")
        );
        assert!(
            topics.contains(&"homeassistant/sensor/main_ebz5dd32r06eta_107_energy_export/config")
        );
        assert!(
            topics.contains(&"homeassistant/sensor/main_ebz5dd32r06eta_107_power_total/config")
        );
    }

    #[test]
    fn discovery_payload_fields() {
        let sensor = Sensor::new("main", "stromzaehler2mqtt");
        let entries = discovery_entries(&sensor, "EBZ5DD32R06ETA_107");
        let (_, payload_json) = entries
            .iter()
            .find(|(t, _)| t.contains("energy_import"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(payload_json).unwrap();

        assert_eq!(v["device_class"], "energy");
        assert_eq!(v["state_class"], "total_increasing");
        assert_eq!(v["unit_of_measurement"], "kWh");
        assert_eq!(v["state_topic"], "stromzaehler2mqtt/main/energy_import");
        assert_eq!(v["unique_id"], "main_ebz5dd32r06eta_107_energy_import");
        assert_eq!(v["device"]["name"], "main");
        assert_eq!(v["device"]["identifiers"][0], "main_ebz5dd32r06eta_107");
    }
}
