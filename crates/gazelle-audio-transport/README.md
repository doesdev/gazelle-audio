# gazelle-audio-transport

Everything between a protocol message and the bytes on the wire, with no wire in it: segmenting a
large message into 8052 segments and reassembling the device's 8053 segments, matching a reply to
the request that asked for it, and a loopback device that stands in for real hardware.

It is its own crate so that the rules about **how** a message travels are tested apart from the
rules about **what** it says ([`gazelle-audio-protocol`](../gazelle-audio-protocol/README.md)) and
apart from any real USB stack. The server's HID backend is a thin adapter over the `Device` trait
here, and the loopback implements the same trait, so every rule is exercised end to end before a
device is attached.

The module docs are the full account and say where each rule was recovered from:
[`src/lib.rs`](src/lib.rs) (the `Device` trait and the loopback),
[`src/framing.rs`](src/framing.rs) (segment headers, send and receive) and
[`src/correlation.rs`](src/correlation.rs) (how a reply is accepted). The wire format itself is in
[`docs/protocol.md`](../../docs/protocol.md), "Segmentation" and "Requests and replies".

## Where it sits

| | |
| --- | --- |
| Depends on | `gazelle-audio-protocol`, and nothing else outside the standard library. |
| Depended on by | [`gazelle-audio-server`](../gazelle-audio-server/README.md), whose USB backend feeds it HID reports and whose loopback backend is built on its `LoopbackDevice`. |

## Testing

```bash
cargo test -p gazelle-audio-transport
cargo clippy -p gazelle-audio-transport --all-targets
```

No test needs hardware. `tests/transport_e2e.rs` drives a request from the protocol crate through
segmentation, the loopback and correlation. `tests/host_receive.rs` covers the host's side of the
receive path, including 8053 reassembly. `tests/replay_capture.rs` replays inbound traffic taken
from live captures, which are committed as hex fixtures in `tests/fixtures/`, each with a header
saying where it came from. `tests/regressions.rs` pins defects a review found in the receive path.

## Before changing it

- **Correlation is recovered, not guessed.** A reply is the **next** report off the queue, accepted
  only on its `cmd` and `ext2`; `seq` plays no part, the queue is drained before sending, and a
  report that does not match fails the request rather than being skipped. An earlier version matched
  on `seq` because notes said so, and it was wrong. Read `src/correlation.rs` before touching it.
- **The receive path is device controlled input.** A malformed or out of order segment must be
  refused, never panic and never wedge the reassembler. Add a regression test for every such case.
- **Keep it free of OS code.** Anything that opens a device belongs in the server's backends.
