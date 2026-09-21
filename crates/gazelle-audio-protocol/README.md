# gazelle-audio-protocol

The control protocol Antelope Audio's Synergy Core interfaces speak over USB, as pure data: the
16 byte header, the field grammar, payload serialisation, reply and cyclic report decoding, the
CRC that guards cyclic reports, and the command registries loaded from `refs/schemas/`.

It knows nothing about USB, HID or any operating system. That is why it is a crate of its own: it
can be tested byte for byte against independently generated vectors without a device anywhere,
and every layer above it (the transport, the server) builds on answers this crate has already
proven.

What the protocol is, and what is confirmed on a device rather than read from the vendor's
software, is in [`docs/protocol.md`](../../docs/protocol.md). How the command model was recovered,
and how to regenerate the schemas from your own copy of the vendor software, is in
[`docs/reverse-engineering.md`](../../docs/reverse-engineering.md). The module docs in
[`src/lib.rs`](src/lib.rs) and each module say which vendor function each piece mirrors.

## Where it sits

| | |
| --- | --- |
| Depends on | `serde` and `serde_json` only. Nothing else in the workspace. |
| Depended on by | [`gazelle-audio-transport`](../gazelle-audio-transport/README.md), and [`gazelle-audio-server`](../gazelle-audio-server/README.md), which loads one registry per device model. |
| Reads | `refs/schemas/quadro_commands.json` and `refs/schemas/studio_commands.json`, located at compile time through `QUADRO_COMMANDS_PATH` and `STUDIO_COMMANDS_PATH`, which every crate's tests use rather than working out a path of their own. |

## Testing

```bash
cargo test -p gazelle-audio-protocol
cargo clippy -p gazelle-audio-protocol --all-targets
```

No test needs hardware. The suites:

- **Ground truth.** `tests/ground_truth.py` builds every request in a schema with default values
  and `tests/gen_cyclic_gt.py` builds a cyclic report, both in Python with `ctypes`, the way the
  vendor software serialises, and independently of the Rust. Their output (`ground_truth.json`,
  `ground_truth_studio.json`, `cyclic_gt.json`) is committed, so the Rust tests never run Python.
  Regenerate them only when a schema changes, with Python 3: `ground_truth.py` takes a schema and
  an output file (`python ground_truth.py SCHEMA OUT`, the Quadro's by default), and
  `gen_cyclic_gt.py` takes nothing.
- **Regressions** (`tests/regressions.rs`): each test names a defect a review found, mostly in
  reply decoding, which the ground truth cannot see because it compares requests.
- **Replies, topology and effects** (`tests/list_replies.rs`, `tests/topology.rs`,
  `tests/effect_parameters.rs`): counted replies, the extracted topology agreeing with the command
  schemas, and each effect type's parameter commands.

## Before changing it

- **A shared misreading passes both sides.** The ground truth and this crate were both written from
  the vendor's software, so agreement between them proves the Rust does what the vendor code does,
  not what the device accepts. Where it matters, check against a capture or a device, and say which
  in `docs/protocol.md`.
- **The schemas are generated.** Change `refs/schemas/*.json` by rerunning the extractors in
  `refs/tools/scripts/`, not by hand, then regenerate the ground truth and run the web client's
  type check (`corepack pnpm -C web test`), which fails when its generated types no longer match.
- **Keep it free of I/O.** Anything that talks to a device belongs in the transport or the server.
