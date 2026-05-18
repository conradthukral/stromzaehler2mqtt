use serde_json::json;

use crate::parser::{Reading, Telegram};

pub struct Sensor {
    pub name: String,
    sanitized_name: String,
    pub base_topic: String,
    pub device_id: String,
    sanitized_device_id: String,
}

impl Sensor {
    pub fn new(
        name: impl Into<String>,
        base_topic: impl Into<String>,
        device_id: impl Into<String>,
    ) -> Self {
        let name = name.into();
        let sanitized_name = sanitize_topic_segment(&name);
        let device_id = device_id.into();
        let sanitized_device_id = sanitize_topic_segment(&device_id);
        Self {
            name,
            sanitized_name,
            base_topic: base_topic.into(),
            device_id,
            sanitized_device_id,
        }
    }

    fn device_key(&self) -> String {
        format!("{}_{}", self.sanitized_name, self.sanitized_device_id)
    }

    fn value_topic(&self, subtopic: &str) -> String {
        format!("{}/{}/{subtopic}", self.base_topic, self.sanitized_name)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Publish {
    pub topic: String,
    pub payload: String,
    pub retain: bool,
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

fn sanitize_topic_segment(s: &str) -> String {
    s.to_lowercase()
        .replace('ä', "ae")
        .replace('ö', "oe")
        .replace('ü', "ue")
        .replace('ß', "ss")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
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
fn discovery_entries(sensor: &Sensor, node_id: &str) -> Vec<(String, String)> {
    let device_key = sensor.device_key();
    READINGS
        .iter()
        .map(|meta| {
            let unique_id = format!("{device_key}_{}", meta.subtopic);
            let config_topic = format!("homeassistant/sensor/{node_id}/{unique_id}/config");
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

pub fn discovery_publishes(sensor: &Sensor, node_id: &str) -> Vec<Publish> {
    discovery_entries(sensor, node_id)
        .into_iter()
        .map(|(topic, payload)| Publish {
            topic,
            payload,
            retain: true,
        })
        .collect()
}

pub fn reading_publishes(sensor: &Sensor, telegram: &Telegram) -> Vec<Publish> {
    telegram
        .readings
        .iter()
        .filter_map(|reading| {
            let (subtopic, value) = reading_to_state(reading)?;
            Some(Publish {
                topic: sensor.value_topic(subtopic),
                payload: value,
                retain: false,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_lowercase_and_replace() {
        assert_eq!(
            sanitize_topic_segment("EBZ5DD32R06ETA_107"),
            "ebz5dd32r06eta_107"
        );
        assert_eq!(sanitize_topic_segment("EBZ-DD3.2R"), "ebz_dd3_2r");
        assert_eq!(sanitize_topic_segment("abc123"), "abc123");
        assert_eq!(sanitize_topic_segment("Haushalt"), "haushalt");
        assert_eq!(sanitize_topic_segment("Küche"), "kueche");
        assert_eq!(sanitize_topic_segment("ÜBER"), "ueber");
        assert_eq!(sanitize_topic_segment("Straße"), "strasse");
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
        let sensor = Sensor::new("main", "stromzaehler2mqtt", "EBZ5DD32R06ETA_107");
        let entries = discovery_entries(&sensor, "stromzaehler2mqtt");
        assert_eq!(entries.len(), 3);

        let topics: Vec<&str> = entries.iter().map(|(t, _)| t.as_str()).collect();
        assert!(topics.contains(
            &"homeassistant/sensor/stromzaehler2mqtt/main_ebz5dd32r06eta_107_energy_import/config"
        ));
        assert!(topics.contains(
            &"homeassistant/sensor/stromzaehler2mqtt/main_ebz5dd32r06eta_107_energy_export/config"
        ));
        assert!(topics.contains(
            &"homeassistant/sensor/stromzaehler2mqtt/main_ebz5dd32r06eta_107_power_total/config"
        ));
    }

    #[test]
    fn discovery_payload_fields() {
        let sensor = Sensor::new("main", "stromzaehler2mqtt", "EBZ5DD32R06ETA_107");
        let entries = discovery_entries(&sensor, "stromzaehler2mqtt");
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

    #[test]
    fn discovery_publishes_are_retained_and_match_entries() {
        let sensor = Sensor::new("main", "stromzaehler2mqtt", "EBZ5DD32R06ETA_107");

        let publishes = discovery_publishes(&sensor, "stromzaehler2mqtt");

        assert_eq!(publishes.len(), 3);
        assert!(publishes.iter().all(|publish| publish.retain));
        assert_eq!(
            publishes[0].topic,
            "homeassistant/sensor/stromzaehler2mqtt/main_ebz5dd32r06eta_107_energy_import/config"
        );
    }

    #[test]
    fn reading_publishes_only_emit_supported_readings_without_retain() {
        let sensor = Sensor::new("main", "stromzaehler2mqtt", "EBZ5DD32R06ETA_107");
        let telegram = Telegram {
            device_id: "EBZ5DD32R06ETA_107".into(),
            readings: vec![
                Reading::EnergyImport(2714.128),
                Reading::EnergyExport(1.206),
                Reading::PowerTotal(211.26),
                Reading::PowerL1(157.64),
                Reading::Unknown {
                    code: "1-0:99.9.9*255".into(),
                    value: "ignored".into(),
                    unit: Some("X".into()),
                },
            ],
        };

        let publishes = reading_publishes(&sensor, &telegram);

        assert_eq!(
            publishes,
            vec![
                Publish {
                    topic: "stromzaehler2mqtt/main/energy_import".into(),
                    payload: "2714.128".into(),
                    retain: false,
                },
                Publish {
                    topic: "stromzaehler2mqtt/main/energy_export".into(),
                    payload: "1.206".into(),
                    retain: false,
                },
                Publish {
                    topic: "stromzaehler2mqtt/main/power_total".into(),
                    payload: "211.26".into(),
                    retain: false,
                },
            ]
        );
    }
}
