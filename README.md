# Gazelle

**One light, clear control app for Antelope Audio's Zen Quadro Synergy Core and Zen Studio+ interfaces.** Preamps, 48V, the four internal mixes, routing, outputs, the clock, effects and the reverb, for every attached interface at once, in one window or any browser on your network. And **Gazelle Aggregate**, a single audio driver that lets your DAW record from both interfaces at once, lined up to the sample.

![The Mixer page: named, coloured channels in a Quadro's monitor mix, with the device cards, output meters and Control Room in the sidebar.](docs/images/mixer-quadro.png)

> **Early software, independently written, tested on one person's two interfaces.** Read [what has and has not been tested](#how-much-it-has-been-tested) and [the safety notes](#safety) before you connect speakers or headphones.

## Why

The vendor's software gives each interface a panel application of its own, so two interfaces mean two applications and constant switching. Gazelle manages every attached interface from one place, presents the mixer as named channels rather than numbered inputs, will show you the exact bytes of every command it sends, and stays small: one executable of a few megabytes, per-user, with no administrator rights needed to install or run it.

A DAW opens one audio driver at a time, so two interfaces are ordinarily two separate devices and you cannot record across both in one project. Gazelle Aggregate is one driver with both underneath it, and Gazelle is what sets it up, checks it and lines it up.

## What it does

- **Devices:** clock source and sample rate with lock status, device presets (Studio+), power, front panel brightness, a test oscillator, the Quadro's panning law and DC coupling.
- **Inputs:** preamp type, gain, 48V (with a two-click confirmation) and phase; digital input gains; microphone emulation with polar patterns on the Quadro.
- **Mixer:** all four mixes as named, coloured, grouped channels; choose each channel's input and mixes and Gazelle does the routing; per-mix mono; saved layouts; links across channels and across devices; a **×2** badge when the same signal reaches a mix twice.
- **Routing:** the full matrix, with click, shift-click and drag.
- **Outputs and Control Room:** volumes, mutes, dim, trims, the Quadro's hard mute, the Studio+'s talkback, and your monitoring outputs always in the sidebar.
- **Effects:** add, remove and reorder effects in each chain, bypass, a settings editor for almost every effect, effect meters, and the reverb.
- **Across devices:** surfaces that put strips from both interfaces side by side, declared digital cables with clock and signal warnings, snapshots you can compare with the present, and workspace backup.
- **Recording across both interfaces:** Gazelle Aggregate presents both as one device to your DAW. The **Aggregate** page says whether your PC can run it and why not, registers the driver, names every channel, and measures how far apart the interfaces really record, so it can line every session up to the sample rather than trusting the figures the drivers report.
- **Around it:** a desktop window and tray icon, a mixer dock on every page, dark and light themes, phone-sized layouts, an HTTP and WebSocket API, a signed in-app updater, and a built-in emulator of both devices for trying it all without hardware.

## How much it has been tested

Plainly: **one person's two units**, one Zen Quadro Synergy Core and one Zen Studio+, on one Windows 11 PC. No other unit, firmware version, computer or operating system has been tried. The last release is 1.2.0; Gazelle Aggregate is newer than that and arrives in the next one.

Over a thousand automated tests run against a built-in emulator of both devices, and every command's bytes are checked against reference bytes generated from the vendor software's own command definitions. That shows Gazelle sends what it means to send; it cannot show what your device does with it.

**Driven and checked on the real devices:** finding and opening both interfaces and reading their state; mixer faders, mute and solo on both, read back from every mix; input and output metering (Quadro); microphone emulation with an Edge Duo (Quadro); front panel brightness (Quadro); inserting and removing one effect and changing one effect setting (Quadro); the effects meter report with signal (Quadro); changing the driver's buffer size and Safe Mode, including with a DAW recording (both); and, on 2026-09-20, per-mix fader writes, chains of several effects and reordering them (both models), the Studio+'s effect meter report with effects loaded, hard mute, DC coupling, the test oscillator and its levels, snapshot capture and compare, preset save (both) and recall (Studio+; the Quadro ignores it), and the panning law's effect on a centre-panned channel.

**Gazelle Aggregate, on the real devices:** both interfaces recording together in one Cubase session; the Aggregate page's readiness answer, driver registration and buffer matching on the real PC; the click measurement that finds how far apart the interfaces record; and the per session phase measurement. That last one is the reason the rest can be trusted: the interfaces land a different whole number of 32 sample steps apart each time the driver opens, which is why a fixed correction never held. With the phase measured and a reference set, eight checks in a row on 2026-09-21 put the two interfaces 0.02 samples apart, although the sessions had started in three different states.

**Gazelle Aggregate, not yet tried on the devices:** long sessions; a sample rate change mid session; unplugging an interface while it streams; three or more interfaces; measuring the output side; and the Aggregate page's own controls for the phase setup and Check, which were driven through the API on the night rather than clicked.

**Written and tested against the emulator only, never on a device:** all other effect settings, the Studio+ Equalizer, the reverb controls, Control Room mono and talkback, surfaces and digital cables in use, unplugging a device while Gazelle runs, Start on boot, the installer and the updater.

If your unit behaves differently, you have probably found something nobody has seen yet. Please [report it](docs/manual/17-troubleshooting.md#reporting-a-problem).

## Safety

Software that controls an audio interface can make it very loud, very suddenly. Vendors rarely say so; this project will.

- **Level jumps.** A volume, gain, fader or route reaches your monitors and headphones the moment you change it. In Gazelle, clicking a bar jumps straight to that point, Ctrl+click puts a fader or volume at unity (a double-click goes to a safe level unless you choose unity: -20 dB on a fader, -30 dB on a volume, off on a reverb send), Home on a fader is 0 dB, and the mouse wheel moves whatever is under the pointer. Routing a source straight to an output bypasses every fader. *Gazelle* sends nothing until you act, disables every control the moment its connection drops and never replays a change later, and offers Mute everywhere and Hard mute on the Quadro. *You:* keep monitors and headphones low whenever you try something new, and know where your physical mute is.
- **Feedback loops.** Routing a signal back into itself, including across two cabled interfaces, can build to full level in a fraction of a second. Gazelle does not detect loops. Change routing with monitors low.
- **48V phantom power** can damage ribbon microphones and some gear, and thumps when it switches. Gazelle asks for a second click (or Ctrl+click) to switch it on, offers it only for the Mic input type, and never completes a confirmation you left behind.
- **Clock and sample rate changes** interrupt audio and your recording software; a clock source with no signal leaves the device unlocked. Gazelle keeps those menus off the mouse wheel, sends a change only when you confirm it, and shows lock status.
- **Doubled signals.** The same audio reaching a mix twice (easy to do, since an empty effect chain passes its input through) is about 6 dB louder. Gazelle asks before it happens and marks both strips **×2** after.
- **Effects with gain**, such as a Guitar Amp at full drive, can raise a level sharply. Gazelle reads an effect's settings before it writes any, but most effect changes have never been tried on hardware.
- **Presets, the test oscillator and DC coupling** ask for a second click before a recall or switching on. A preset may change anything, 48V and the clock included; the oscillator at 0 dBFS is as loud as the device goes; DC coupled outputs can damage speakers.
- **The vendor's software at the same time.** Each program's view goes stale and can undo the other's changes. Antelope's service normally prevents it by holding the devices; use one program at a time.
- **Mid-change failures.** Gazelle is not in the audio path: if it quits or loses its connection, the interface keeps its last settings. The devices do not acknowledge settings, so a command lost in transit leaves the screen and the device disagreeing until you reload. Snapshot recall, the one feature that would send many changes at once, is deliberately not built; its plan silences the outputs first, switches 48V on last, asks before raising any output by more than 6 dB, and stops at the first failure.
- **Measuring the aggregate plays a click** out of a real output, at a modest level, into whatever is plugged in, and the page says so and asks for a confirming click first. Turn amplifiers down the first time. The per session phase measurement is quieter and goes down the digital cable between the interfaces on a channel your DAW never sees, but an interface that monitors that digital input would let you hear it.
- **Registering the aggregate driver is the one thing that asks for administrator rights**, because Windows keeps its list of audio drivers for every program on the PC. Windows puts up its own prompt; declining it changes nothing.
- **Try it safely.** `--backend loopback` runs a built-in emulator that never touches hardware; `--dry-run` shows the bytes each command would send and sends none. Gazelle's own test suites refuse to open real devices (`GAZELLE_NO_HARDWARE=1`).

The full version is the manual's [Safety chapter](docs/manual/02-safety.md).

## Install

Gazelle runs on **Windows**. You also need Antelope's own software installed for the audio driver, and its background service stopped while Gazelle runs.

**From a release:** download `Gazelle-Setup.exe` from the [releases page](https://github.com/doesdev/gazelle-audio/releases), double-click it and choose **Install**. That installs Gazelle for your user account (no administrator rights) with a Start Menu entry, and opens it. The program is not code-signed, so Windows SmartScreen may say "Windows protected your PC" the first time: choose **More info**, then **Run anyway**.

Or download `gazelle-audio-x86_64-pc-windows-msvc.zip`, unzip it, and in a terminal there:

```
gazelle-audio-server.exe --install
```

To run without installing, double-click `gazelle-audio-serverw.exe` from the zip and choose **Run without installing**.

**Stop Antelope's service** (it holds the interfaces exclusively), in an administrator PowerShell:

```
Stop-Service -Name Antelope-Manager-Service
```

**From source**, with Rust 1.88 or newer and Node.js 24 or newer, to build and install on this PC exactly as a release is built:

```
cargo run -p xtask -- install-local
```

That builds the web app, then the aggregate driver, then the server carrying the driver and the update key, stops a Gazelle already running from the install folder, and installs and starts the new one. `--skip-web` leaves the web app alone for a change that is Rust only, and `--dry-run` says what it would do. The same by hand, where the driver has to be its own build before the server's, because the server reads it while it compiles:

```
corepack pnpm -C web install --frozen-lockfile
corepack pnpm -C web build
cargo build --release -p gazelle-audio-aggregate
GAZELLE_AGGREGATE_DLL=target/release/gazelle_aggregate.dll cargo build --release -p gazelle-audio-server --features window
```

**Without hardware**, to look around: `gazelle-audio-server --backend loopback`.

**To record from both interfaces at once**, open the **Aggregate** page. It checks what the aggregate needs before it can work at all: each interface on its own USB host controller (two on one controller cannot both stream), one digital cable between them with the receiving interface clocked from it, and the same sample rate and buffer size on both. The driver comes with Gazelle, so there is nothing extra to download: **Register the driver**, which is where Windows asks for administrator rights, and your DAW will list **Gazelle Aggregate**. Uninstalling Gazelle offers to remove that registration again. The page can then measure how far apart the interfaces record and line every session up; the manual's [Aggregate chapter](docs/manual/13-aggregate-page.md) walks through it.

More in [Getting started](docs/manual/03-getting-started.md) and [Install, update and uninstall](docs/manual/16-install-update-uninstall.md).

## Known limitations

- **Windows only**, and only Windows 11 tried. The tray, window and installer are Windows-specific; nothing else has been built or run.
- **USB only.** The Studio+'s Thunderbolt connection is not used.
- **Unsigned programs.** SmartScreen warns on first run. Updates are signed and verified by the app itself, but the programs carry no code signature.
- **Antelope's service must be stopped** while Gazelle runs, and the vendor's panels cannot reach the devices meanwhile.
- **Not built, deliberately:** firmware updates or anything that writes firmware; snapshot recall; changing the reverb's room settings; licence management; the Studio+'s selectable output meters; a few effects' settings (the Quadro's ClearQ, Auto-Tune and Instinct, and Guitar Cabinet).
- **Gazelle Aggregate needs the hardware set up for it**: each interface on its own USB host controller and a digital cable between them. Software cannot arrange either, so the Aggregate page tells you which is missing rather than working around it.
- **Lining sessions up needs a path inside the interfaces**: on the interface that drives the callback, a playback channel routed to the socket the digital cable leaves from, and on the other, that cable's socket routed to a record channel. Gazelle says what has to go where, but does not yet set that routing up for you.
- **Untested on hardware:** everything in the lists marked so [above](#how-much-it-has-been-tested).
- **No authentication.** Gazelle listens only on your own computer unless you start it with `--bind`; on a network address, anyone who can reach it can change your levels.

> [!NOTE]
> Gazelle supports two interfaces on Windows because those are the two it can be tested on. Support for other Antelope models, or for macOS and Linux, is possible, but each needs real hardware to test against and a good deal of time. If you would like your interface or platform supported, open an issue: I am happy to talk about an arrangement that covers the device access and the time involved.

## Documentation

- [The manual](docs/README.md): getting started, concepts, every page, safety, troubleshooting, the command line and API, and a glossary.
- [The cheat sheet](docs/cheat-sheet.md): two pages to keep beside the desk.
- Both print to PDF with `corepack pnpm -C web docs:pdf`, into `docs/dist/`.

## For developers

| Path | What it is |
|---|---|
| `crates/` | The Rust implementation: `gazelle-audio-protocol` (the wire format), `gazelle-audio-transport` (framing, correlation, the emulator), `gazelle-audio-server` (the app) and the capture tooling |
| `crates/gazelle-audio-aggregate` | Gazelle Aggregate, the audio driver itself: the DLL a DAW loads, which opens the vendor drivers underneath it and lines them up. [Its README](crates/gazelle-audio-aggregate/README.md) is the full account |
| `crates/gazelle-audio-stream-abi` | The low latency audio driver interface, declared by hand from its public shape. No vendor SDK is included, copied or needed |
| `crates/gazelle-audio-aggregate-status` | The shared record and event log the driver publishes, which is how Gazelle watches it without the two ever talking directly |
| `crates/gazelle-audio-calibrate` | The measurement: plays a click through the aggregate itself and reads how far apart the interfaces landed |
| `crates/gazelle-audio-aggregate-probe` | The command line tool that answered, at the hardware, whether two vendor drivers could live in one process at all |
| `web/` | The web app and `gazelle-audio-client`, the typed TypeScript client for the API |
| `refs/` | The recovered command model (`refs/schemas`) and the tools that extract it |
| `docs/` | The user manual and its build |
| [`docs/protocol.md`](docs/protocol.md) | The control protocol: framing, headers, the field grammar, payloads, reports, and what the commands mean |
| [`docs/reverse-engineering.md`](docs/reverse-engineering.md) | How the command definitions were recovered, and how to regenerate them from your own copy of the vendor software |
| [`docs/releasing.md`](docs/releasing.md) | How a release is built, signed and published |

```
cargo test --workspace
corepack pnpm -C web test
corepack pnpm -C web e2e
```

Every test starts its servers on the emulator and with `GAZELLE_NO_HARDWARE=1`, so running the suites never touches attached hardware. As a backstop, `.cargo/config.toml` sets that variable for everything cargo starts in this repository, tests and anything they spawn included. To drive real interfaces from source, say so: `GAZELLE_NO_HARDWARE=0 cargo run -p gazelle-audio-server`. The installed app is not started by cargo and is unaffected.

**On the reverse engineering.** This repository ships a [protocol specification](docs/protocol.md), the device command definitions recovered from the vendor software as data (`refs/schemas`), the tools that recover them, generators for the test vectors, and its own implementation. It does not ship Antelope's binaries or their decompiled source; those stay local and ignored by git. If you own the hardware and the software, [Reverse engineering](docs/reverse-engineering.md) documents how to regenerate the inputs from your own copy.

## Independence and trademarks

Gazelle is not affiliated with, endorsed by, sponsored by or supported by Antelope Audio. Antelope Audio, Zen Quadro, Zen Studio, Synergy Core, Edge and the effect and microphone model names shown in the app are trademarks of their respective owners, used only to say which hardware and features Gazelle works with.

## Licence

MIT; see `LICENSE`. No warranty of any kind.
