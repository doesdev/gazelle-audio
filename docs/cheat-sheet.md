# Gazelle cheat sheet

Control for the Antelope Zen Quadro Synergy Core and Zen Studio+. Independent software, not affiliated with Antelope Audio. Version 1.2.0, Windows. Tested on one person's two units: go carefully.

## Safety first

> **Warning.** Monitors and headphones **down** before you try anything new. Know your **physical mute**: the speakers' or amplifier's knob or power switch, or the interface's own volume knob.

- Silence first, investigate second: speakers down, then **Mute** (or **Hard mute** on a Quadro, Outputs page).
- **48V** takes two clicks (or Ctrl+click). Check for ribbon mics; mind the thump.
- **Clock and sample rate** changes interrupt audio. Each asks for **Confirm**. Stop playback first.
- **Driver buffer size and Safe Mode** (Devices, Driver) restart a DAW's audio. Each asks for **Confirm**; refused while a DAW uses ASIO unless **Change anyway**.
- **Match buffer sizes** (Aggregate) restarts the audio of every program using those drivers. Asks for **Confirm**. Stop the DAW first.
- **×2** on a strip: the same audio reaches that mix twice, about +6 dB. Putting one input into a mix that already has it asks for **Confirm** first.
- **Test oscillator**: 0 dBFS is as loud as the device goes. Start at -18. Tone on takes two clicks.
- **DC coupling** (Quadro) can damage speakers. Leave it off. On takes two clicks.
- **Presets** recall on a second click and may change anything, 48V and clock included.
- **Clicking** a fader jumps there; **double-click** is -20 dB; **Ctrl+click** is 0 dB.
- Never run Antelope's panels on the same device at the same time.
- Try new things on the emulator (`--backend loopback`) or in `--dry-run`.

## Getting out of trouble fast

| Problem | Do |
|---|---|
| Too loud, feedback, noise | Speakers down. Then Mute / Hard mute |
| No devices | Stop `Antelope-Manager-Service`; USB cable (not Thunderbolt); tray **Rescan devices** |
| Header says LOOPBACK | You are on the emulator, not your hardware |
| Controls greyed, "not connected" | Gazelle quit or crashed: start it, reload |
| Display disagrees with device | Reload; **Read from device** (Routing, Effects) |
| Port 8420 in use | `--bind 127.0.0.1:8421` |
| No window | WebView2 missing: tray **Open Gazelle** opens the browser |
| DAW does not list Gazelle Aggregate | Not registered: Aggregate page, **Register the driver** (the one admin prompt) |
| Aggregate gap grows, or clicks | Two clocks: check the cable and every clock source while the DAW plays |
| Interfaces land a different distance apart each session | Set up the phase on the follower's card, measure once, then **Check** |
| Measure says nothing arrived | The interfaces' own routing does not carry the click: playback channel to the socket, socket to a record channel |
| SmartScreen warning | Unsigned: **More info**, **Run anyway**, only for the official download |

## Install and start

Double-click `Gazelle-Setup.exe`, choose **Install**. Or, from the zip:

```
gazelle-audio-server.exe --install
```

Installs to `%LOCALAPPDATA%\Programs\Gazelle` with a Start Menu entry; no admin. Start **Gazelle** from the Start Menu. Closing the window keeps it in the tray; **Quit** is in the tray menu.

Stop Antelope's service first, or no device appears. Admin PowerShell:

```
Stop-Service -Name Antelope-Manager-Service
```

Back to Antelope's software: quit Gazelle, then `Start-Service -Name Antelope-Manager-Service`.

## Files

| What | Where |
|---|---|
| Log | `%LOCALAPPDATA%\gazelle\logs\gazelle.log` (tray: **Open log folder**) |
| Workspace | `%APPDATA%\gazelle\workspace.json` |
| Snapshots | `%APPDATA%\gazelle\snapshots\` |
| Program | `%LOCALAPPDATA%\Programs\Gazelle` |
| Version | The tray menu's first line, or the header with the explain mode on |

<div class="page-break"></div>

## The pages

| Page | For |
|---|---|
| Devices | Name, clock, rate, driver buffer and Safe Mode, presets, power, brightness, oscillator |
| Workspace | Names, colours, surfaces, cables, snapshots, backup |
| Inputs | Preamp type, gain, 48V, Ø; digital gains; mic emulation |
| Outputs | Volume, Mute, Dim, CR, trims, Hard mute, talkback |
| Mixer | One mix at a time as named channels: the fader is the level in that mix. **Show all channels** for the rest; Mono, outputs per mix |
| Routing | Source for every destination; Read from device |
| Effects | Chains, effect settings, reverb |
| Aggregate | Both interfaces as one driver a DAW opens: ready or not and why, registering, order, channel names, measuring the trims and the phase, **Check**, the live gap |

**Sidebar:** Devices (click a card to switch device), Meter (**Clear** for clip lights), Control Room (volume, Mute, Dim, Mono per output; Studio+ talkback). **Dock:** a mix's faders at the bottom of every page. **Header:** USB or LOOPBACK, Dry run, connection, **?** explain mode (with it on, the version reads out left of the backend badge; not on a phone).

## Gestures

| Do | Result |
|---|---|
| Drag | Follows the pointer (pan snaps to centre) |
| Click a bar | **Jumps** to that value |
| Wheel | One step per notch (1 dB); also steps menus |
| Arrows | One step |
| Page Up/Down | 6 dB (faders, volumes, gains) |
| Home / End | The ends: fader **Home = 0 dB**; volume **End = 0 dB** |
| Double-click | Reset (below) |
| Ctrl+click | A level to unity (0 dB; full for a return) |
| Enter / Escape | Save / undo a name |
| Shift-click (Routing) | Select a run of sources |
| Delete (Routing) | Mute a cell |

**Double-click resets:** fader -20 dB; output, CR and talkback volume -30 dB; Studio+ Send and reverb sends off; reverb level -18 dB; Quadro reverb return 20 steps down; gain 0 dB; pan centre; brightness 50%; effect settings to the panel default. Header **Double-click: unity** makes a level's double-click unity (kept in this browser).

**Two clicks (Confirm):** 48V on, tone on, DC coupling on, preset recall and save, clock source and rate, driver buffer size and Safe Mode, Standby, matching the aggregate's buffer sizes, unregistering the aggregate driver, **Measure** and **Check** (both play a click), clearing a phase setup, putting one input into a mix twice, remove a channel, strip, surface, cable, snapshot or aggregate interface. Off is one click.

**Wheel ignored:** the Mixer's Mix buttons, clock source, sample rate, driver buffer size, Add effect, a channel's Input and Main mix, + Output, Route, mix pin, every menu on the Aggregate page.

**Hold:** Talk (mouse, Space or Enter).

## Useful options

| Option | Does |
|---|---|
| `--backend loopback` | The emulator; no hardware |
| `--dry-run` | Real devices, nothing written: shows bytes |
| `--no-persist` | Save nothing |
| `--bind 0.0.0.0:8420` | Reachable from your network (no password) |
| `--log-dir <DIR>` | Log to a folder, for `--no-tray` runs |
| `--install` | Install for this user |
| `--uninstall` | Remove; settings kept unless `--purge` |
| `--version` | Which Gazelle this is |
