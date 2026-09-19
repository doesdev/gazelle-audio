# Changelog

What changed in each release of Gazelle, written for the people who use it. The newest release is
first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and version
numbers follow [Semantic Versioning](https://semver.org/).

Each release's notes on GitHub are its section below, copied word for word when the release is
built. A version's heading carries its date once it is released; a heading without a date is a
version that has not been published yet.

## [Unreleased]

### Added

- **See which version you are running.** The tray menu's first line now names it, and with the
  explain mode (**?**) on, the header shows it in dim text beside the backend badge.
- **The app tells you about updates, in the app.** Beside the version, the header says when a new
  version is downloading, when one is ready ("1.2.0 ready, restart"), and when a check or a
  download failed, with a press to try again. It says nothing at all while you are up to date. A
  phone shows it too.
- **Restart into an update without leaving the page.** Pressing the ready prompt asks once more,
  then restarts Gazelle into the new version: the devices are let go and picked up again, and the
  page reconnects on the new version by itself. The tray's **Restart to update to X** does the
  same. Scripts can do it with `POST /api/v1/update/restart`.

### Changed

- **Updates fetch themselves.** A new version found on GitHub is now downloaded and checked
  without being asked for, and the only thing left to say yes to is the restart. To go back to
  being told and deciding, put `"auto_download": false` in `%APPDATA%\gazelle\update.json`.
- **A fresh start looks for updates.** Gazelle checks half a minute after it starts, and every six
  hours after that. It used to wait out the whole six hours first, so an app opened in the morning
  and closed at night could go a long time without ever looking.

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
