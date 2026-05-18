# Testability Refactoring Plan

This project currently has useful unit coverage around parsing, serial framing,
MQTT topic generation, and MQTT packet encoding. The main coverage gap is that
`main.rs` owns configuration loading, thread orchestration, sensor loops, MQTT
reconnect behavior, and real serial/MQTT dependencies. That makes the most
important runtime behavior hard to test without hardware or network services.

The goal of this refactoring plan is to improve testability while keeping the
runtime architecture simple.

## Current Coverage Gaps

As of the latest `cargo llvm-cov` run:

- Overall line coverage is about 56%.
- `main.rs` has 0% line coverage because it is mostly binary entrypoint and
  orchestration code.
- `serial.rs` has good happy-path framing tests, but misses EOF/error paths.
- `parser.rs` covers the sample telegram, but misses several reading variants,
  formatting paths, unknown OBIS values, and malformed value cases.
- `mqtt.rs` tests internal helpers, but should also test public publish
  functions directly.
- `mqtt_client.rs` covers packet encoding, but not CONNACK parsing or socket
  behavior.

## Recommended Refactoring Sequence

### 1. Move Configuration Out of `main.rs`

Create a dedicated `config` module:

```text
src/config.rs
```

Move these items there:

- `SensorConfig`
- `MqttConfig`
- `Config`
- `deserialize_duration_secs`
- a new `load_config(path: impl AsRef<Path>) -> Result<Config, ConfigError>`

This makes config parsing directly testable and keeps `main()` focused on
startup wiring.

After this change, `main()` should be close to:

```rust
fn main() {
    init_logging();
    let config = config::load_config("config.yaml").expect("invalid config.yaml");
    app::run(config);
}
```

### 2. Extract Testable MQTT Packet Parsing

`mqtt_client.rs` currently validates CONNACK inline while reading from a
`TcpStream`. Extract the validation into a pure function:

```rust
fn parse_connack(buf: [u8; 4]) -> std::io::Result<()>
```

This allows tests for:

- valid CONNACK
- invalid packet type or remaining length
- broker refusal return codes

This is a small refactor with a good coverage payoff.

### 3. Extract Sensor Processing Units

`run_sensor` combines serial reading, parsing, discovery publishing, reading
publishing, logging, error handling, and sleeping. Keep the outer loop, but move
the meaningful behavior into smaller functions, for example:

```rust
fn parse_discovery_telegram(raw: &[u8]) -> Result<parser::Telegram, SensorError>
fn discovery_messages(...)
fn reading_messages(...)
fn process_sensor_telegram(...)
```

The exact function names can follow the final code shape, but the important
goal is that one telegram can be processed without needing a real serial port,
thread, or sleep.

Useful tests after this refactor:

- discovery uses `meter_id()` when present
- discovery falls back to `device_id` when no meter ID exists
- malformed readings after discovery are skipped rather than terminating the
  sensor
- published reading messages match the expected topics and payloads
- closed publish channel exits cleanly

### 4. Introduce Traits Only Where Needed

If loop-level tests become valuable, add small interfaces at the hardware and
network boundaries:

```rust
trait TelegramReader {
    fn read_telegram(&mut self) -> std::io::Result<Vec<u8>>;
}

trait Publisher {
    fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> std::io::Result<()>;
}
```

Use fake implementations in tests. This would make it possible to cover EOF
handling, parse-error recovery, MQTT publish failures, and reconnect behavior.

This step should be done after the smaller extraction work above, not before.
Traits add indirection, so they should solve specific tests that are otherwise
awkward.

### 5. Keep `main.rs` Thin

After the refactor, `main.rs` should only:

- initialize logging
- load configuration
- call the application runner

It is acceptable if `main.rs` remains mostly uncovered once it is this small.
Coverage should focus on the modules that contain behavior and decisions.

## Immediate Test Additions

These tests can be added before or alongside the refactor:

- Parser tests for `SerialNumber`, `EnergyExport`, `PowerTotal`, `PowerL1`,
  `PowerL2`, `PowerL3`, unknown OBIS values with and without units, invalid
  UTF-8, invalid float values, and invalid hex values.
- `Display` tests for `Reading` and `ParseError`.
- Serial framing tests for EOF before any header and EOF after a header but
  before `!`.
- MQTT tests for `discovery_publishes` and `reading_publishes` directly,
  including retained flags, non-retained reading publishes, expected topics,
  expected payloads, and skipped readings.
- MQTT client tests for extracted CONNACK parsing.

## Target Shape

A modest final module layout would be:

```text
src/main.rs         # binary entrypoint only
src/config.rs       # config structs and loading
src/app.rs          # orchestration, sensor loop, MQTT loop
src/parser.rs       # telegram parsing
src/serial.rs       # serial port setup and telegram framing
src/mqtt.rs         # Home Assistant discovery and reading publish mapping
src/mqtt_client.rs  # TCP MQTT client
```

This keeps the codebase small while making most behavior testable with ordinary
unit tests.
