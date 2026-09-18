# Gazelle

**One light, clear control app for Antelope Audio's Zen Quadro Synergy Core and Zen Studio+ interfaces.** Preamps, 48V, the four internal mixes, routing, outputs, the clock, effects and the reverb, for every attached interface at once, in one window or any browser on your network.

![The Mixer page: named, coloured channels in a Quadro's monitor mix, with the device cards, output meters and Control Room in the sidebar.](docs/images/mixer-quadro.png)

> **Early software, independently written, tested on one person's two interfaces.** Read [what has and has not been tested](#how-much-it-has-been-tested) and [the safety notes](#safety) before you connect speakers or headphones.

## Why

The vendor's software gives each interface a panel application of its own, so two interfaces mean two applications and constant switching. Gazelle manages every attached interface from one place, presents the mixer as named channels rather than numbered inputs, shows the exact bytes of every command it sends, and stays small: one executable of a few megabytes, per-user, no administrator rights.

## What it does

- **Devices:** clock source and sample rate with lock status, device presets, power, front panel brightness, a test oscillator, the Quadro's panning law and DC coupling.
- **Inputs:** preamp type, gain, 48V (with a two-click confirmation) and phase; digital input gains; microphone emulation with polar patterns on the Quadro.
- **Mixer:** all four mixes as named, coloured, grouped channels; choose each channel's input and mixes and Gazelle does the routing; per-mix mono; saved layouts; links across channels and across devices; a **×2** badge when the same signal reaches a mix twice.
- **Routing:** the full matrix, with click, shift-click and drag.
- **Outputs and Control Room:** volumes, mutes, dim, trims, the Quadro's hard mute, the Studio+'s talkback, and your monitoring outputs always in the sidebar.
- **Effects:** add, remove and reorder effects in each chain, bypass, a settings editor for almost every effect, effect meters, and the reverb.
- **Across devices:** surfaces that put strips from both interfaces side by side, declared digital cables with clock and signal warnings, snapshots you can compare with the present, and workspace backup.
- **Around it:** a desktop window and tray icon, a mixer dock on every page, dark and light themes, phone-sized layouts, an HTTP and WebSocket API, a signed in-app updater, and a built-in emulator of both devices for trying it all without hardware.

## How much it has been tested

Plainly: **one person's two units**, one Zen Quadro Synergy Core and one Zen Studio+, on one Windows 11 PC. No other unit, firmware version, computer or operating system has been tried. Version 0.1.0; no release has been published yet.

Over a thousand automated tests run against a built-in emulator of both devices, and every command's bytes are checked against reference bytes generated from the vendor software's own command definitions. That shows Gazelle sends what it means to send; it cannot show what your device does with it.

**Driven and checked on the real devices:** finding and opening both interfaces and reading their state; mixer faders, mute and solo on both, read back from every mix; input and output metering (Quadro); microphone emulation with an Edge Duo (Quadro); front panel brightness (Quadro); inserting and removing one effect and changing one effect setting (Quadro); the effects meter report with signal (Quadro).

**Written and tested against the emulator only, never on a device:** device preset save and recall, hard mute, DC coupling, the test oscillator, S/PDIF sample rate conversion, reordering effects and chains of several effects, all other effect settings, the Studio+ Equalizer, the reverb controls, Control Room mono and talkback, surfaces and digital cables in use, unplugging a device while Gazelle runs, Start on boot, the installer and the updater.

If your unit behaves differently, you have probably found something nobody has seen yet. Please [report it](docs/manual/16-troubleshooting.md#reporting-a-problem).

## Safety

Software that controls an audio interface can make it very loud, very suddenly. Vendors rarely say so; this project will.

- **Level jumps.** A volume, gain, fader or route reaches your monitors and headphones the moment you change it. In Gazelle, clicking a bar jumps straight to that point, some double-click resets go to full level (a mixer fader resets to 0 dB, unity), Home on a fader is 0 dB, and the mouse wheel moves whatever is under the pointer. Routing a source straight to an output bypasses every fader. *Gazelle* sends nothing until you act, disables every control the moment its connection drops and never replays a change later, and offers Mute everywhere and Hard mute on the Quadro. *You:* keep monitors and headphones low whenever you try something new, and know where your physical mute is.
- **Feedback loops.** Routing a signal back into itself, including across two cabled interfaces, can build to full level in a fraction of a second. Gazelle does not detect loops. Change routing with monitors low.
- **48V phantom power** can damage ribbon microphones and some gear, and thumps when it switches. Gazelle asks for a second click (or Ctrl+click) to switch it on, offers it only for the Mic input type, and never completes a confirmation you left behind.
- **Clock and sample rate changes** interrupt audio and your recording software; a clock source with no signal leaves the device unlocked. Gazelle keeps those menus off the mouse wheel and shows lock status, but sends a change as soon as you choose it.
- **Doubled signals.** The same audio reaching a mix twice (easy to do, since an empty effect chain passes its input through) is about 6 dB louder. Gazelle marks both strips **×2**.
- **Effects with gain**, such as a Guitar Amp at full drive, can raise a level sharply. Gazelle reads an effect's settings before it writes any, but most effect changes have never been tried on hardware.
- **Presets, the test oscillator and DC coupling** act on one click. A preset may change anything, 48V and the clock included; the oscillator at 0 dBFS is as loud as the device goes; DC coupled outputs can damage speakers.
- **The vendor's software at the same time.** Each program's view goes stale and can undo the other's changes. Antelope's service normally prevents it by holding the devices; use one program at a time.
- **Mid-change failures.** Gazelle is not in the audio path: if it quits or loses its connection, the interface keeps its last settings. The devices do not acknowledge settings, so a command lost in transit leaves the screen and the device disagreeing until you reload. Snapshot recall, the one feature that would send many changes at once, is deliberately not built; its plan silences the outputs first, switches 48V on last, asks before raising any output by more than 6 dB, and stops at the first failure.
- **Try it safely.** `--backend loopback` runs a built-in emulator that never touches hardware; `--dry-run` shows the bytes each command would send and sends none. Gazelle's own test suites refuse to open real devices (`GAZELLE_NO_HARDWARE=1`).

The full version is the manual's [Safety chapter](docs/manual/02-safety.md).

## Install

Gazelle runs on **Windows**. You also need Antelope's own software installed for the audio driver, and its background service stopped while Gazelle runs.

**From a release** (none published yet): download `gazelle-audio-x86_64-pc-windows-msvc.zip` from the [releases page](https://github.com/doesdev/gazelle-audio/releases), unzip it, and in a terminal there:

```
gazelle-audio-server.exe --install
```

That installs Gazelle for your user account (no administrator rights) with a Start Menu entry. The programs are not code-signed, so Windows SmartScreen may warn the first time. To run without installing, double-click `gazelle-audio-serverw.exe`.

**Stop Antelope's service** (it holds the interfaces exclusively), in an administrator PowerShell:

```
Stop-Service -Name Antelope-Manager-Service
```

**From source**, with Rust 1.88 or newer and Node.js 24 or newer:

```
corepack pnpm -C web install --frozen-lockfile
corepack pnpm -C web build
cargo build --release -p gazelle-audio-server --features window
```

**Without hardware**, to look around: `gazelle-audio-server --backend loopback`.

More in [Getting started](docs/manual/03-getting-started.md) and [Install, update and uninstall](docs/manual/15-install-update-uninstall.md).

## Known limitations

- **Windows only**, and only Windows 11 tried. The tray, window and installer are Windows-specific; nothing else has been built or run.
- **USB only.** The Studio+'s Thunderbolt connection is not used.
- **Unsigned programs.** SmartScreen warns on first run. Updates are signed and verified by the app itself, but the programs carry no code signature.
- **Antelope's service must be stopped** while Gazelle runs, and the vendor's panels cannot reach the devices meanwhile.
- **Not built, deliberately:** firmware updates or anything that writes firmware; snapshot recall; changing the reverb's room settings; licence management; the Studio+'s selectable output meters; a few effects' settings (the Quadro's ClearQ, Auto-Tune and Instinct, and Guitar Cabinet).
- **Untested on hardware:** everything in the second list [above](#how-much-it-has-been-tested).
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
| `web/` | The web app and `gazelle-audio-client`, the typed TypeScript client for the API |
| `refs/` | The recovered command model (`refs/schemas`) and the tools that extract it |
| `docs/` | This manual and its build |
| `.agent/` | Design decisions, specifications, protocol reference and project status |

```
cargo test --workspace
corepack pnpm -C web test
corepack pnpm -C web e2e
```

Every test starts its servers on the emulator and with `GAZELLE_NO_HARDWARE=1`, so running the suites never touches attached hardware.

**On the reverse engineering.** This repository ships a protocol specification, the tooling, and its own implementation. It does not ship Antelope's binaries or anything decompiled from them; those stay local and ignored by git. If you own the hardware and the software, `.agent/reference/decompilation.md` documents how to regenerate the inputs from your own copy.

## Independence and trademarks

Gazelle is not affiliated with, endorsed by, sponsored by or supported by Antelope Audio. Antelope Audio, Zen Quadro, Zen Studio, Synergy Core, Edge and the effect and microphone model names shown in the app are trademarks of their respective owners, used only to say which hardware and features Gazelle works with.

## Licence

MIT; see `LICENSE`. No warranty of any kind.
