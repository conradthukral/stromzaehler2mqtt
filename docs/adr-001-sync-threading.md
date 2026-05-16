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
  │  blocking read() + VMIN=32
  │  parse telegram
  └─► mpsc::channel ──► mqtt thread
                          hand-rolled MQTT QoS 0 sync client
                          publish on receive
```

- Each sensor runs in a `std::thread`, blocking on `read()`. The kernel scheduler handles waiting; zero epoll/reactor overhead.
- Parsed telegrams are sent over `std::sync::mpsc::channel`.
- The MQTT thread blocks on `channel.recv()` and publishes.
- `rumqttc` (async-only) is replaced by a minimal hand-rolled MQTT 3.1.1 publisher: QoS 0 only, no subscriptions, keep-alive disabled (avoids PINGREQ/PINGRESP). This removes the only async dependency without introducing a C library (`paho-mqtt` was the alternative but links against the C paho library, complicating cross-compilation).

## Consequences

- Overhead reduces to one `read()` syscall per 32 bytes (VMIN=32 batching) and one channel wakeup per telegram. Estimated CPU: <1% vs ~3%.
- The binary no longer links against any async runtime or external C library — cross-compilation for the target hardware is simpler.
- Reconnect on MQTT error is handled by a plain retry loop in the MQTT thread. Discovery messages use `retain=true`, so Home Assistant recovers automatically after a broker restart without the sensor thread needing to know.
- The design does not support subscriptions or QoS 1/2. These are not required for this use case.
