# Changelog

What changed in each release of Gazelle, written for the people who use it. The newest release is
first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and version
numbers follow [Semantic Versioning](https://semver.org/).

Each release's notes on GitHub are its section below, copied word for word when the release is
built. A version's heading carries its date once it is released; a heading without a date is a
version that has not been published yet.

## [Unreleased]

## [1.2.0] - 2026-09-20

A Mixer that shows one mix at a time, updates that tell you what they are doing, and a set of
fixes from a session at the real devices.

### Added

- **See which version you are running.** The tray menu's first line now names it, and with the
  explain mode (**?**) on, the header shows it in dim text beside the backend badge.
- Gazelle asks before it lets one input into a mix twice. Choosing an input or a main mix that
  would do it waits behind a Confirm button with the reason on it, and **Add to** takes two clicks.
  You can still do it on purpose; dry alongside the same signal through an effect chain is a
  parallel setup and is not questioned.
- **The app tells you about updates, in the app.** Beside the version, the header says when a new
  version is downloading, when one is ready ("1.2.0 ready, restart"), and when a check or a
  download failed, with a press to try again. It says nothing at all while you are up to date. A
  phone shows it too.
- **Restart into an update without leaving the page.** Pressing the ready prompt asks once more,
  then restarts Gazelle into the new version: the devices are let go and picked up again, and the
  page reconnects on the new version by itself. The tray's **Restart to update to X** does the
  same. Scripts can do it with `POST /api/v1/update/restart`.

### Changed

- The Quadro no longer offers device presets. Its firmware accepts a recall and does nothing with
  it, and Antelope's own panel never sends one, so the buttons, the save slot and the preset shown
  on its card and status are gone. The Studio+ keeps them, where recall works. Use snapshots on the
  Quadro.

- Mono lowers the mix by as much as summing it actually gains on your device, instead of a flat
  6 dB. Measured at both: the Studio+ gains the full 6 dB, while the Quadro's centre attenuation
  has already taken some of it, so at -4.5 dB Mono barely changes the level at all.

- **Mono no longer jumps the level.** Summing a mix to mono makes both sides play through each
  output, which was about 6 dB louder; Gazelle now lowers that mix by 6 dB while Mono is on and
  gives that step back when you turn it off. You can still ride the mix's master while Mono is on,
  and turning Mono off never leaves the mix louder than it was before.
- The Mixer shows one mix at a time, and only what is routed to it. Pick a mix from the row of
  buttons at the top; a strip's fader is that channel's level in the mix you picked, whether it is
  the channel's main mix or one it is sent to. **Show all channels** beside the buttons brings back
  every channel you have made, dimmed where it belongs to another mix, so you can move one in. That
  choice is remembered for each device, and a mix with nothing in it now says so and says how to
  put something there.
- The line showing the last command's bytes appears only while the explain mode is on, or in dry
  run. It used to sit on every page all the time, where it read as debug output.


- **Updates fetch themselves.** A new version found on GitHub is now downloaded and checked
  without being asked for, and the only thing left to say yes to is the restart. To go back to
  being told and deciding, put `"auto_download": false` in `%APPDATA%\gazelle\update.json`.
- **A fresh start looks for updates.** Gazelle checks half a minute after it starts, and every six
  hours after that. It used to wait out the whole six hours first, so an app opened in the morning
  and closed at night could go a long time without ever looking.

### Fixed

- The Devices page shows whether the S/PDIF converter is on. The button held the right state, but
  it looked the same either way, so the converter seemed to do nothing.
- A cable into a converted S/PDIF input no longer warns that the rates differ or that the device is
  not locked. With the converter on, the incoming rate is converted and that device need not follow
  the sender's clock.

- Channels and inputs can be linked from a surface. The link badges were there, but the bar that
  finishes the pick was not, so a link started on a surface could not be completed.
- The **×2** badge now shows on the mixer dock's slim strips too, not only on the Mixer page.

## [1.1.0] - 2026-09-19

A simpler way to install, and a fix for PCs without Microsoft's Visual C++ runtime.

### Added

- **Install without a terminal:** download `Gazelle-Setup.exe`, double-click it and choose
  Install. It installs for your account with no administrator rights, adds Gazelle to the Start
  Menu and opens it. Run it again to update an older version; it never replaces a newer one, and
  if Gazelle is running it asks you to quit it first. Double-clicking the app from an unzipped
  download also offers to install it, or to run it from where it is without asking again.

### Fixed

- Gazelle starts on a fresh Windows installation. It no longer needs Microsoft's Visual C++
  runtime to be installed first, which some PCs lack ("VCRUNTIME140.dll was not found").

## [1.0.0] - 2026-09-19

The first release: one app for Antelope Audio's Zen Quadro Synergy Core and Zen Studio+, for
every attached interface at once, in its own window or in any browser on your network. Windows
only, and tested on one person's two interfaces; read what has and has not been tried on real
hardware in the README before you connect speakers or headphones.

### Added

- **Devices:** clock source and sample rate with lock status, device presets, power, front panel
  brightness, a test oscillator, the Quadro's panning law and DC coupling.
- **Inputs:** preamp type, gain, 48V (with a two-click confirmation) and phase; digital input
  gains; microphone emulation with polar patterns on the Quadro.
- **Mixer:** all four mixes as named, coloured, grouped channels; choose each channel's input and
  mixes and Gazelle does the routing; per-mix mono; saved layouts; links across channels and
  across devices; a badge when the same signal reaches a mix twice.
- **Routing:** the full matrix, with click, shift-click and drag.
- **Outputs and Control Room:** volumes, mutes, dim, trims, the Quadro's hard mute, the Studio+'s
  talkback, and your monitoring outputs always in the sidebar.
- **Effects:** add, remove and reorder effects in each chain, bypass, a settings editor for almost
  every effect, effect meters, and the reverb.
- **Across devices:** surfaces that put strips from both interfaces side by side, digital cables
  with clock and signal warnings, snapshots you can compare with the present, and workspace
  backup.
- **Around it:** a desktop window and tray icon, a mixer dock on every page, dark and light
  themes, phone-sized layouts, an HTTP and WebSocket API, a built-in emulator of both devices for
  trying it all without hardware, a per-user installer that needs no administrator rights, and a
  signed in-app updater that checks for new releases and installs one only when you ask.

### Known limitations

- Windows only, and USB only: the Studio+'s Thunderbolt connection is not used.
- The programs carry no code signature, so SmartScreen warns on first run. Updates are signed
  and checked by the app itself.
- Antelope's own service must be stopped while Gazelle runs.
- No authentication: Gazelle listens only on your own computer unless you start it with `--bind`.
