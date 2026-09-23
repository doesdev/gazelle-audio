# Changelog

What changed in each release of Gazelle, written for the people who use it. The newest release is
first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and version
numbers follow [Semantic Versioning](https://semver.org/).

Each release's notes on GitHub are its section below, copied word for word when the release is
built. A version's heading carries its date once it is released; a heading without a date is a
version that has not been published yet.

## [Unreleased]

## [1.4.0] - 2026-09-23

Gazelle Aggregate now opens at the rate your interfaces are really on, even when an interface's
own driver remembers another one.

### Added

- **The Aggregate page warns when an interface's driver remembers another rate.** An interface's
  own driver can remember 44.1 kHz while the interface runs at 96 kHz, and put it back to 44.1 kHz
  when a DAW opens the aggregate. The page now says so, and where nothing would stop it, a button
  puts the right rate into the aggregate's setup after a confirming click.
- **The Aggregate page says which rate is in force.** Under the setup's rate, a line says what the
  aggregate will run at and whether that is the rate you chose or the one your interfaces are on.
- **Gazelle Aggregate makes sure every interface's driver really took the rate.** It reads the rate
  back after asking for it. The Antelope drivers take it as soon as they are asked; for a driver
  that says yes and does not move, the aggregate closes it and opens it again, up to twice, and
  checks once more before anything plays. The log on the Aggregate page says what it had to do, and
  a driver that still will not move is refused with the rate it held, rather than a session
  running at two rates.

### Fixed

- **"Whatever the interfaces are on" now means the rate the interfaces are running at.** Before,
  opening the aggregate left each interface to its driver, so a driver remembering another rate
  moved the interface, and the others followed it over the cable. The aggregate now puts every
  interface at the rate they are all on, and follows it when that changes.
- **An aggregate that cannot move every interface to a rate puts each back where it was.** When one
  interface refused a new rate, the ones already moved were not put back, or were put back to the
  wrong rate.

## [1.3.0] - 2026-09-22

Gazelle Aggregate: record from both interfaces at once, in one DAW session, lined up to the
sample. Also faster mix building, and a Start on boot that stays in the tray.

### Added

- **Record across both interfaces at once.** Gazelle Aggregate is a single audio driver a DAW
  opens with two or more interfaces underneath it, so a Quadro and a Studio+ appear as one device
  with one long list of channels. It comes with Gazelle: installing or updating puts it in
  Gazelle's folder, an update while your DAW has it open finishes the next time the DAW starts,
  and uninstalling offers to remove its registration.
- **The Aggregate page** sets it up and runs it:
  - **Whether your PC can do it, and why not**, in plain words: a missing driver, two interfaces
    on one USB host controller, rates or buffer sizes that differ, no digital cable between them,
    or a clock that is wrong or not locked. Where Gazelle can put a reason right, a button beside
    it does.
  - **Registering the driver**, behind Windows' own administrator prompt. It is the one thing in
    Gazelle that asks for administrator rights.
  - **The setup**: which interfaces, in what order, which drives the callback, the rate, the buffer
    size, each driver's buffer size and Safe Mode, and a trim for each. It is kept in the
    workspace, so it travels with a backup.
  - **Measure and Check.** Measure plays a click out of one interface, records it on all of them
    and writes the trims that line them up. The interfaces land a different whole number of 32
    samples apart each time the driver opens, so each follower also measures its phase over the
    digital cable, and every later session is put back where the trims were measured. Check says
    how far apart a recording would land now. A measurement taken across a dropout, or whose
    clicks disagree, says so and offers nothing.
  - **Watching it while the DAW plays**: the gap between the interfaces, stalls, the phase each
    session was lined up by, and the driver's own log, which records what each session lost.
- **One set of names, on the page and in your DAW.** Each interface goes by its name in Gazelle.
  Each channel is named for what it carries, then its USB channel: an input for what the routing
  sends it, "Vocal mic, USB A REC 1", and an output for where it ends up, "Monitor L, USB 1
  PLAY 1". Re-routing renames it, and Gazelle keeps the DAW's names in step. A name of your own
  wins until you clear it, and any channel can be left out.
- **Where the DAW can play and record.** Each interface's card lists its output sockets and
  which DAW channels reach each one, and its input sockets and which DAW inputs carry each one,
  directly, through a mix or through an effect. A socket nothing reaches says so, with a button
  that routes the first free DAW channels to it after a confirming click.
- **Build a mix by dragging sources from the Routing page onto the mixer dock.** Each source you
  drop becomes a channel in the mix the dock shows, routed as if you had made it on the Mixer
  page. The dock says what a drop will do, opens if it was folded, and asks first if the mix
  already has one of those inputs.
- A manual chapter, **The Aggregate page**.

### Changed

- **Switch a mixer to another layout at any time.** **Start from** is always on the Mixer page
  now, beside **Save as**, and applying a layout over channels you have set up asks for a
  confirming click, since it replaces them.
- **Start on boot starts Gazelle in the tray**, without opening its window. An entry turned on
  with an earlier version changes to work this way the next time Gazelle starts, and restarting
  into an update brings the window back as it was.
- **Fields and their labels line up.** Boxes, menus and readouts share one height and spacing,
  and labels share a column, so values start in the same place down the page.

### Fixed

- **Picking a device in the sidebar works on every page.** On a surface it used to take you off
  the surface, and on the Aggregate page it did nothing.
- **A mixer strip fed by the oscillator, an emulated mic or a loopback return now shows a level.**
  It is metered at the mixer's own input. The Zen Quadro meters its mixer channels for Mix 1
  only, and a strip that still cannot be metered says so when you hover it.

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
