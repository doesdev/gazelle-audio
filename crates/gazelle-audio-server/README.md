# gazelle-audio-server

The Gazelle app. One server that attaches every connected Antelope interface at once and exposes
their controls over HTTP and WebSocket, serves the web UI that drives them, and, in the shipped
build, puts that UI in a window of its own with a tray icon beside it. The vendor's software runs
one control server per device, which is why its panels make you switch between them; this one
presents them together.

Everything a person installs is this crate: `gazelle-audio-server` (with a console) and
`gazelle-audio-serverw` (the same server with no console window, which Start on boot runs and
`Gazelle-Setup.exe` is a copy of). What a release carries is in
[`docs/releasing.md`](../../docs/releasing.md); what a person does with it is in
[the manual](../../docs/README.md).

## Read this before running it

> **The default backend is `usb`, and it opens the real interfaces.** A plain `cargo run -p
> gazelle-audio-server` attaches to every Antelope interface plugged into the machine and follows
> them as they come and go.
>
> **Every test and every script must pass `--backend loopback` and set `GAZELLE_NO_HARDWARE=1`.**
> The flag picks the emulator; the variable makes the server refuse the USB backend outright, with a
> message saying so, so a harness that forgets the flag stops before a device is opened instead of
> driving the one on your desk. Pass `--no-tray` as well, so a test run neither shows an icon nor
> writes to your log folder. Stop a server you started by its own process id, never by name: other
> servers on the machine may be somebody else's test run, or the app itself.

```bash
# the emulator: two loopback devices, nothing saved, on http://127.0.0.1:8420
GAZELLE_NO_HARDWARE=1 cargo run -p gazelle-audio-server -- --backend loopback --no-persist --no-tray
curl localhost:8420/api/v1/devices
```

Attaching to real devices is read only; a write is always a deliberate request, and `--dry-run`
makes every command report the bytes it would send instead of sending them. With the USB backend,
the vendor's Manager Service must be stopped first, because it holds the interfaces exclusively.
The server binds to `127.0.0.1` unless `--bind` says otherwise. `--help` lists every option.

## Where it sits

| | |
| --- | --- |
| Depends on | [`gazelle-audio-protocol`](../gazelle-audio-protocol/README.md) and [`gazelle-audio-transport`](../gazelle-audio-transport/README.md) for talking to the devices; [`gazelle-audio-aggregate-status`](../gazelle-audio-aggregate-status/README.md) for the aggregate driver's live record and log; [`gazelle-audio-calibrate`](../gazelle-audio-calibrate/README.md), and through it the aggregate itself, for the Aggregate page's Measure and Check. |
| Depended on by | `xtask`, which uses the updater's own code to name, sign and verify a release. |
| Serves | The web app built from `web/`, embedded at build time (see "Building" below). |

## A map of the modules

Each module opens with docs of its own that say what it decides and why; `src/lib.rs` and
`src/main.rs` carry the safety posture. This is where to start looking.

**Talking to the devices**

| Module | What it does |
| --- | --- |
| [`device`](src/device/) | Identity, one synchronous worker thread per device (so the protocol and transport crates stay free of any async runtime), its async handle, the manager that routes work to the right worker, the HID backend (`usb.rs`), hot plug, and the loopback variants the emulator is built from. |
| [`registry_set`](src/registry_set.rs) | One command registry per device model, chosen by the device's application id. |
| [`value`](src/value.rs) | Protocol values to JSON and back. |
| [`no_hardware`](src/no_hardware.rs) | The `GAZELLE_NO_HARDWARE` refusal. |

**The surface a client sees**

| Module | What it does |
| --- | --- |
| [`http`](src/http/) | Every route, all under `/api/v1`: health, devices, commands, the workspace, snapshots and recall's plan, themes, the window handover, each device's audio driver settings, the aggregate, and the updater. |
| [`ws`](src/ws/) | `/api/v1/ws`: decoded device events out, JSON-RPC commands in. |
| [`web`](src/web.rs) | The embedded web UI, behind the `web-ui` feature, mounted as the fallback so API routes keep priority. |
| [`error`](src/error.rs), [`notice`](src/notice.rs) | The error type and its HTTP form; what the server says about itself when the device list is empty, which is the commonest way the app looks broken when it is not. |

**What it keeps**

| Module | What it does |
| --- | --- |
| [`workspace`](src/workspace/) | Cross-device layout (groups, links, visibility, the aggregate's setup), persisted so it survives a reload and is shared between clients. |
| [`snapshot`](src/snapshot/) | Named records of a whole setup, compared with the present. A snapshot is a record of reads. Recall builds a plan and sends nothing: applying one is not built, and `--enable-recall` alone does not switch it on. |
| [`config`](src/config.rs) | Where things live on disk: on Windows `%APPDATA%\gazelle` for the workspace, snapshots, themes, update settings and the aggregate driver's file, and `%LOCALAPPDATA%\gazelle\logs` for the log. |

**The PC around the devices**

| Module | What it does |
| --- | --- |
| [`driver`](src/driver/) | The USB audio driver's own settings (buffer size, latency, Safe Mode), read through the driver's user mode API DLL, with exactly one setter. They never cross the HID protocol. |
| [`aggregate`](src/aggregate/) | Everything around the aggregate audio driver: its setup, exported from the workspace to the file the driver reads; whether this PC is ready to run it and why not; its live state and log; registering it behind Windows' own prompt (`elevate.rs`, the one thing in Gazelle that asks for administrator rights); and running a measurement or a check. `bundled.rs` carries the driver a release embeds and writes it beside the executable, renaming aside a copy a DAW has open. The driver itself is [`gazelle-audio-aggregate`](../gazelle-audio-aggregate/README.md) and never talks to this server. |

**The app on the desktop**

| Module | What it does |
| --- | --- |
| [`window`](src/window/) | The desktop window, behind the `window` feature: a system webview pointed at this server, so a phone sees exactly what the desktop does. |
| [`tray`](src/tray/) | The tray icon and its menu, including Start on boot. |
| [`handover`](src/handover.rs) | One instance: a second launch hands its window to the one already running and exits. |
| [`install`](src/install/) | The per user self install (`--install`, `--uninstall`) and setup mode, which is what `Gazelle-Setup.exe` does when double-clicked. Nothing asks for elevation. |
| [`update`](src/update/) | The in-app updater: asks GitHub Releases, downloads, checks the hash and the signature, and stages the new binary for the next start. It never restarts on its own. |
| [`logging`](src/logging.rs), [`icon`](src/icon.rs) | The log file (tray runs and `--log-dir` only) and the drawn app icon. |

## Building

```bash
cargo build -p gazelle-audio-server                      # headless, with the embedded UI
cargo build -p gazelle-audio-server --features window    # the shipped app, with its window
```

The Rust build never runs pnpm. If `web/apps/web/dist` exists it is embedded; otherwise a one page
notice saying the UI was not built is, so `cargo build` and `cargo test` work without Node.
Build the UI first (`corepack pnpm -C web build`) for a binary that serves the real app.

`build.rs` records the target triple the updater asks a release for, and whether
`GAZELLE_UPDATE_PUBKEY` was set; a binary built without that key will not download an update,
because it could not check one. On Windows it also links the icon and the manifest that says both
binaries run as whoever started them.

It also embeds the Gazelle Aggregate driver when `GAZELLE_AGGREGATE_DLL` names one (a relative
path is taken from the workspace root), which only a release sets; `build/driver.rs` checks it
and fails the build over a path that is missing, not a DLL, or built for another machine.
Unset, nothing is embedded and the Aggregate page says there is no driver to register, which
is right for every ordinary build and test. The release procedure is in
[`docs/releasing.md`](../../docs/releasing.md).

The whole workspace builds on Rust 1.88, which the `window` build's dependencies need; CI builds
with a real 1.88 to catch an update that raises it.

## Testing

```bash
cargo test -p gazelle-audio-server
cargo clippy -p gazelle-audio-server --all-targets
cargo check -p gazelle-audio-server --features window
```

The in-process suites build the server on loopback devices. A test that starts the binary
(`tests/cli.rs` is the model to copy) passes `--backend loopback`, `--no-tray` and `--no-persist`
and sets `GAZELLE_NO_HARDWARE` in the child's environment; one test there deliberately leaves the
flag out, with the variable set, to prove the variable refuses the USB backend. CI sets the
variable for every job as well. Everything that would touch the PC (the HID bus, the
audio driver's DLL, the registry, the Run key, elevation, the aggregate's shared memory, a release
source) sits behind a trait with a fake.

`examples/hid_probe.rs` lists the HID interfaces on the machine. It opens nothing and writes
nothing, but it does look at real hardware, so it is not a test and nothing runs it for you.

The web client's tests and the browser tests (`corepack pnpm -C web test`, `corepack pnpm -C web
e2e`) start this server too, on the loopback backend.

## Before changing it

- **Any new harness, script or test that starts this server** passes `--backend loopback`,
  `--no-tray` and `GAZELLE_NO_HARDWARE=1`. The variable is the guard; the flag is still required.
- **A write to a device is its own deliberate act.** Nothing reads and then writes on its own, and
  anything that would send a command honours `--dry-run`.
- **User-visible changes go in `CHANGELOG.md`** under `[Unreleased]`, in the same commit, as the
  root `CLAUDE.md` describes. Bumping this crate's `version` is part of cutting a release
  ([`docs/releasing.md`](../../docs/releasing.md)), and an `xtask` test fails when the version has
  no section in the changelog.
- **No em or en dashes** in anything the app shows or logs, or in comments; `cargo test -p xtask
  --test no_dashes` checks the source.
