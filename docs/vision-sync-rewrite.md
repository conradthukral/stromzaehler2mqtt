# Vision: sync rewrite with blocking threads

## Motivation

The async runtime (tokio) has non-trivial overhead for this workload:
- epoll reactor loop with `ep_item_poll → tty_poll → n_tty_poll` on every tick
- timer wheel calling `clock_gettime` on every reactor iteration
- hrtimer setup/teardown on every `epoll_wait` call with a timeout

Async pays off when multiplexing hundreds of fds. Here we have 2 serial
ports and 1 MQTT socket — the machinery costs more than it saves.

## Proposed architecture

```
sensor thread (one per port)
  │  blocking read() + VMIN=32
  │  parse telegram
  └─► mpsc::channel ──► mqtt thread
                          rumqttc or paho-mqtt sync client
                          publish on receive
```

- Each sensor runs in a plain `std::thread`, blocking on `read()`.
- The kernel scheduler handles waiting; zero epoll/reactor overhead.
- Parsed telegrams are sent over `std::sync::mpsc::channel`.
- The MQTT thread blocks on `channel.recv()` and publishes.

## Expected gain

Overhead reduces to one `read()` syscall per 32 bytes and one channel
wakeup per telegram. Estimated CPU: <1% vs current ~3%.

## Blockers / open questions

- `rumqttc` is async-only; would need to switch to `paho-mqtt` (sync API)
  or wrap a minimal tokio runtime around just the MQTT thread.
- `paho-mqtt` links against the C paho library — adds a C dependency and
  complicates cross-compilation for the target hardware.
- A minimal hand-rolled MQTT publisher (QoS 0 only, no subscriptions)
  would avoid the C dependency and keep the binary small.
