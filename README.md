# Antelope RE

A clean-room-ish, hardware-compatible control stack for **Antelope Audio** USB interfaces —
built by reverse-engineering the wire protocol of the stock Windows software.

Targets **Zen Quadro Synergy Core** and **Zen Studio+**.

> **Status: pre-alpha.** The protocol and transport layers are implemented and tested; the
> control server is in progress. **No physical device adapter exists yet, and nothing here
> has ever been run against real hardware.**

## Why

The stock panels bind one application to one device, so using two interfaces together means
switching back and forth. This project aims at a single server that manages every connected
device at once, with a unified surface — custom groupings, channel linking, collapsible
sections — plus an HTTP/WebSocket API that a web or mobile client can drive.

## What works

| Crate | What it does | Tests |
|---|---|---|
| `gazelle-audio-protocol` | The byte-level wire protocol: 16-byte header, field grammar, payload serialization, CRC32. Transport-agnostic. | 28 |
| `gazelle-audio-transport` | Framing, segmentation, request/response correlation, and a hardware-free loopback device. | 25 |
| `gazelle-audio-server` | Multi-device HTTP + WebSocket control server. Loopback-backed; no hardware adapter yet. | 48 |

```bash
cargo test
```

The protocol implementation is validated **byte-for-byte** against reference vectors
generated from the recovered command model — including faithfully reproducing two bugs in
the original software, because the device expects those bytes.

Request/response correlation is read directly from the original's bytecode rather than
guessed: see `.agent/reference/decompilation-fidelity.md`.

## Repository layout

```
crates/     the Rust implementation
refs/
  schemas/  the recovered command model (63 in-scope commands, field layouts)
  tools/    extraction and decompilation tooling
  bin/         )
  extracted/   )  local only, git-ignored — see below
  decompiled/  )
.agent/     design decisions, protocol reference, project status
```

## On the reverse engineering

This repository ships **a protocol specification, the tooling, and our own implementation**.
It does **not** ship Antelope's binaries or anything decompiled from them — those stay local
and git-ignored.

If you own the hardware and the software, `.agent/reference/decompilation.md` documents the
recovery pipeline end to end so you can regenerate the inputs from your own copy.

Not affiliated with, endorsed by, or supported by Antelope Audio.

## Safety

Driving audio hardware over a reverse-engineered protocol can fault the device. The server
therefore defaults to a **loopback** device and requires an explicit flag to touch real
hardware, and offers a dry-run mode that shows the bytes a command *would* send. Flash and
firmware commands are out of scope and are never issued.

## Documentation

| Read | For |
|---|---|
| `.agent/STATUS.md` | current state, blockers, how to run things |
| `.agent/reference/protocol.md` | the wire format |
| `.agent/reference/usb-access.md` | how the device is reached per OS — **read before writing transport code** |
| `.agent/decisions/` | why the architecture is the way it is |

## License

MIT — see `LICENSE`.
