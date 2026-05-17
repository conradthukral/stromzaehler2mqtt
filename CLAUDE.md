# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`stromzaehler2mqtt` reads data from an electricity meter (Stromzähler) via its optical interface (IEC 62056 D0 port) and publishes the readings to an MQTT broker, making them available to Home Assistant.

**Initially supported hardware:** eBZ DD3 2R06 ETA-ODZ1  
**Data formats:** EN62056-21 (mode D) and EN62056-61 (OBIS-based)

## Documentation

The `docs/` directory contains design and protocol documentation:
- `docs/telegram-format.md` — EN62056-21 telegram format and OBIS code reference
- `docs/mqtt.md` — MQTT topic structure, payload format, and Home Assistant discovery design

## Domain Context

- The optical interface on German smart meters uses infrared (IR) read/write head, typically connected as a serial port (e.g. `/dev/ttyUSB0`)
- EN62056-21 defines the data exchange protocol; EN62056-61 defines the OBIS code structure for identifying measurement values
- OBIS codes identify meter readings (e.g. `1-0:1.8.0` = total active energy import in Wh)
- Home Assistant auto-discovery works by publishing device config to `homeassistant/<component>/<device_id>/config` before publishing state values

## Code Structure

| Module | Responsibility |
|---|---|
| `serial.rs` | Serial port open/configure (`7E1`, raw mode) and raw telegram read |
| `parser.rs` | Parses raw bytes into `Telegram` / `Reading` types; owns OBIS code mapping |
| `mqtt.rs` | Builds HA discovery payloads and maps `Reading` variants to MQTT publish messages |
| `mqtt_client.rs` | Minimal MQTT 3.1.1 over TCP — no external MQTT library |
| `main.rs` | Config loading, spawns one thread per sensor, runs MQTT publish loop |

## Architecture

Data flow: `serial::read_telegram` → `parser::parse_telegram` → `mqtt::reading_publishes` → `mpsc` channel → `mqtt_client::publish`

Threading: one sensor thread per entry in `config.yaml`, all feeding a single MQTT thread via an `mpsc::channel<mqtt::Publish>`. The MQTT thread owns the TCP connection and reconnects on error.

## Development

```
cargo build
cargo test
```