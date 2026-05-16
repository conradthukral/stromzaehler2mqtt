# MQTT Publishing

## State topics

Each telegram is parsed and selected readings are published to individual topics under the
`stromzaehler/{sensor_name}/` prefix, where `{sensor_name}` is the `name` field from
`config.yaml`.

| Reading | Topic suffix | Unit | HA device class | HA state class |
|---|---|---|---|---|
| Energy import | `energy_import` | kWh | `energy` | `total_increasing` |
| Energy export | `energy_export` | kWh | `energy` | `total_increasing` |
| Total active power | `power_total` | W | `power` | `measurement` |

Payloads are plain UTF-8 strings (not JSON). QoS 0, retain false.

## Home Assistant MQTT Discovery

On first telegram receipt the app publishes a retained discovery config payload for each reading
to `homeassistant/sensor/{node_id}/{unique_id}/config`. Home Assistant picks these up
automatically — no manual `configuration.yaml` entries required.

`{node_id}` is the `mqtt.client_id` from `config.yaml`.

`{unique_id}` is `{sensor_name}_{device_id}_{reading}`, where `{sensor_name}` is the configured
sensor name and `{device_id}` is the meter's device ID from the telegram header (both lowercased,
non-alphanumeric characters replaced with `_`). The sensor name prefix ensures each configured
sensor appears as a separate HA device even if two meters report identical device IDs.

### Example discovery payload — energy import

```
Topic:   homeassistant/sensor/stromzaehler2mqtt/main_ebz5dd32r06eta_107_energy_import/config
Retain:  true
Payload:
{
  "name": "Energy Import",
  "device_class": "energy",
  "state_class": "total_increasing",
  "unit_of_measurement": "kWh",
  "state_topic": "stromzaehler/main/energy_import",
  "unique_id": "main_ebz5dd32r06eta_107_energy_import",
  "device": {
    "identifiers": ["main_ebz5dd32r06eta_107"],
    "name": "main"
  }
}
```

Discovery configs are published once per sensor startup, not on every telegram.
