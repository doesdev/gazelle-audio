# gazelle-audio-capture

`gazelle-capture`: a capture loop for working out which bytes on the USB bus a control in a
vendor's own software sends. A person moves one control through a planned series of values in the
vendor's app; this records the USB traffic while they do it, marks each step on the timeline, and
then analyses the recording to find the command and the readback that changed with the control,
and how the value is encoded.

It is a development tool. It is not part of the Gazelle app and is not in a release.

## Why it is its own crate, and why it names no vendor

Everything else in the workspace knows the protocol it is talking to. This crate deliberately does
not: it is a tool for finding out what a protocol is, so it has to work on an interface nobody has
mapped yet. Two tests hold it to that (`tests/neutrality.rs`): its manifest depends on no other
`gazelle-audio-*` crate, and no file under `src/` contains the vendor's name. Keep both true.
Anything specific to one device belongs in the probe plan a person writes, not in the code.

## What is in it

`src/lib.rs` is one line; the map is the modules themselves, each of which opens with its own
docs.

| Module | What it is |
| --- | --- |
| [`session`](src/session/) | Sessions on disk, the parameters and probe plans a person declares, and the step protocol: a pure state machine that turns the operator's marks and the packet clock into a timeline. |
| [`capture`](src/capture/) | Where packets come from (live through `USBPcapCMD.exe` on Windows, an imported `.pcap`/`.pcapng` from USBPcap or Linux usbmon, or a demo source), and the pipeline that decodes frames into events and writes them. |
| [`panel`](src/panel/) | The companion panel the operator follows: one page, one WebSocket and one read-only state endpoint, on localhost only with a bearer token made fresh at each start and Host and Origin checks. |
| [`analysis`](src/analysis/) | Pure functions over events and timelines: segments, channel discrimination, a noise model of idle traffic, attribution of commands and readbacks to steps, encoding fits, and the field map with its Markdown report. |
| [`ops`](src/ops/) | The one operation set agents use, with the procedure they are taught. MCP tools and OpenAI function definitions are both generated from it. |
| [`agent`](src/agent/) | The transports over that set: OpenAI compatible tools, MCP over HTTP, a stdio relay for clients that launch MCP servers, and `drive`, a chat completions loop. |
| [`synth`](src/synth/) | A synthetic device and scripted operator, for tests and demos. |

**The operator boundary.** Only the panel can mark a step done, redo it or skip it; no agent
operation can. `tests/operator_boundary.rs` checks that the proof those commands need is granted
only by the panel, the synthetic operator and in-crate unit tests. An agent can plan and read a
probe; the person at the controls says when a step happened.

## Running it

```
cargo run -p gazelle-audio-capture -- --help
```

The subcommands are `serve` (the panel and one probe), `import` (a capture file to JSON lines),
`synth` (a synthetic session), `analyze` (a recorded probe, into `analysis/<parameter>.json` and
`.md`), `agent`, `mcp-stdio` and `drive` (the same session served to agents), and `hubs` (USBPcap's
root hubs and what is on them). Each says what it takes with `--help`.

**It only listens.** Nothing here writes to a USB device: the vendor's own software, driven by the
person, makes every change, and this records the traffic. A live capture needs USBPcap installed
and the target's driver stack to include it (if it does not, a reboot is what fixes it). Opening a
USBPcap hub asks Windows for administrator rights.

`drive` sends the session's tool calls to the model at `--base-url`, and the API key only when the
variable named by `--api-key-env` is set. Point it only at a model you mean to share the session
with.

## Testing

```bash
cargo test -p gazelle-audio-capture
cargo clippy -p gazelle-audio-capture --all-targets
```

No test captures live traffic or needs a device. The suites run on synthetic sessions and on
committed fixtures in `tests/fixtures/`: two synthetic captures (USBPcap and usbmon link types)
and two trimmed live captures, each with the `session.json` that goes with it. The panel and agent
tests start their servers on a free localhost port.

## Before changing it

- **Keep it vendor neutral** (see above); the neutrality test fails on the name in any file under
  `src/`, the panel's page included.
- **Keep the analysis pure.** Only `analysis::run` touches disk; everything else takes events and
  timelines and returns results, which is what lets the fixtures test it.
- **The panel is a local service.** Anything new it serves goes behind the same token, Host and
  Origin checks as the rest (`src/panel/security.rs`).
