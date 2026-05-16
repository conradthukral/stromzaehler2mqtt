# ADR-001: Replace async runtime with blocking threads

**Status:** Accepted  
**Date:** 2026-05-17

## Context

The original implementation used Tokio (`current_thread` flavour) with `AsyncFd` for serial I/O and `rumqttc` for MQTT. Profiling on the target hardware showed ~3% CPU load, most of which came from the async machinery rather than from actual work:

- `ep_item_poll → tty_poll → n_tty_poll` on every reactor tick
- timer wheel calling `clock_gettime` on every reactor iteration
- `hrtimer` setup/teardown on every `epoll_wait` call with a timeout

Tokio multiplexing overhead is worthwhile when juggling hundreds of file descriptors. Here the program manages two serial ports and one TCP socket — the reactor costs more than it saves.

## Decision

Rewrite the I/O layer using plain blocking threads and `std::sync::mpsc`:

```
sensor thread (one per port)
  │  tcflush → blocking read() loop until complete telegram
  │  parse telegram
  │  thread::sleep(publish_interval)
  └─► mpsc::channel ──► mqtt thread
                          hand-rolled MQTT QoS 0 sync client
                          publish on receive
```

- Each sensor runs in a `std::thread`. The kernel scheduler handles waiting; zero epoll/reactor overhead.
- Parsed telegrams are sent over `std::sync::mpsc::channel`.
- The MQTT thread blocks on `channel.recv()` and publishes.
- `rumqttc` (async-only) is replaced by a minimal hand-rolled MQTT 3.1.1 publisher: QoS 0 only, no subscriptions, keep-alive disabled (avoids PINGREQ/PINGRESP). This removes the only async dependency without introducing a C library (`paho-mqtt` was the alternative but links against the C paho library, complicating cross-compilation).

### Publish-interval sleep

Rather than reading the serial port continuously and throttling publication in userspace, the sensor thread reads exactly one telegram per publish interval:

1. `tcflush(TCIFLUSH)` — discards all data buffered during sleep (the kernel TTY buffer fills and wraps after ~4 s at 9600 baud; we discard it rather than process stale readings).
2. Raw-mode `read()` loop — accumulates chunks until `split_telegrams` finds a complete `/`…`!` frame. Takes at most one telegram period (~300 ms at 9600 baud).
3. Parse and publish.
4. `thread::sleep(publish_interval)` — thread is fully dormant for the remainder of the interval.

A canonical-mode approach (`ICANON` + `VEOL='!'`) was considered to get a single `read()` per telegram, but `\n` is a hardcoded line terminator in the kernel TTY line discipline that cannot be disabled, so it splits every line of the telegram into a separate `read()` return rather than waiting for `!`.

## Consequences

- CPU overhead is limited to the brief read window (~300 ms per publish interval) plus one channel send. The process is fully sleeping for the rest of the interval.
- The binary no longer links against any async runtime or external C library — cross-compilation for the target hardware is simpler.
- Readings published reflect the state at the moment of the read, not a rolling average. Any telegrams that arrived during sleep are discarded. This is acceptable: the meter state changes slowly and the publish interval is configured by the operator.
- Reconnect on MQTT error is handled by a plain retry loop in the MQTT thread. Discovery messages use `retain=true`, so Home Assistant recovers automatically after a broker restart without the sensor thread needing to know.
- The design does not support subscriptions or QoS 1/2. These are not required for this use case.
