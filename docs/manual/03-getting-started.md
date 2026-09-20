# Getting started

This chapter takes you from nothing to a first look at your interfaces. If you only want to see the app without hardware, skip to [Trying Gazelle without an interface](#trying-gazelle-without-an-interface).

## What you need

- **Windows 11, 64-bit.** Windows is the only platform Gazelle has been built and used on, and only Windows 11 has been tried; Windows 10 should work but is untested. The desktop window uses Microsoft's WebView2 runtime, which Windows 11 includes; without it Gazelle opens in your web browser instead.
- **A Zen Quadro Synergy Core or a Zen Studio+, or both**, connected over USB. Thunderbolt is not supported: Gazelle speaks to the devices over USB only.
- **Antelope's own software installed**, for the device's audio driver. Gazelle does not replace the driver; it replaces the control panels. Its background service has to be stopped while Gazelle runs (below).
- **No administrator rights.** Gazelle installs for your user account only.

## Get Gazelle

### From a release

Each release on the project's GitHub page (https://github.com/doesdev/gazelle-audio/releases) carries a setup file for Windows, `Gazelle-Setup.exe`. To install Gazelle:

1. Download `Gazelle-Setup.exe` from the release's **Assets**.
2. Double-click it. The program is **not code-signed**, so Windows SmartScreen may say "Windows protected your PC". Choose **More info**, then **Run anyway**, but only if you downloaded the file from the project's own releases page.
3. Gazelle asks "Install Gazelle for your account?". Choose **Install**.

That is all: Gazelle copies itself to `%LOCALAPPDATA%\Programs\Gazelle`, adds **Gazelle** to the Start Menu and to Settings, Apps, and opens. No administrator rights are needed, and Windows does not ask for them. You can delete the downloaded file afterwards. Chapter 15, [Install, update and uninstall](16-install-update-uninstall.md), has the details, including what happens when you run it over an installed copy.

#### Or from the zip

The same release also carries a zip, `gazelle-audio-x86_64-pc-windows-msvc.zip`, holding two programs:

| File | What it is |
|---|---|
| `gazelle-audio-server.exe` | Gazelle with a console window, for the command line |
| `gazelle-audio-serverw.exe` | The same program with no console window: what the Start Menu shortcut runs |

Unzip it anywhere (SmartScreen may warn as above). Double-click `gazelle-audio-serverw.exe` and it offers the same choice as the setup file, plus a third: **Run without installing**, which runs Gazelle from the unzipped folder and never asks again. Or install from a terminal in the unzipped folder:

```
gazelle-audio-server.exe --install
```

It installs both programs where the setup file would, and asks whether to start Gazelle.

### From source

You need Rust 1.88 or newer (from https://rustup.rs), Node.js 24 or newer, and Git. From a clone of the repository:

```
corepack pnpm -C web install --frozen-lockfile
corepack pnpm -C web build
cargo build --release -p gazelle-audio-server --features window
```

The first two lines build the web interface, which the program embeds; skip them and the program serves a page saying the interface was not built. The third builds both programs into `target\release`. Without `--features window` you get the same program with no desktop window: it runs in the tray and opens in your browser.

## Stop Antelope's service

While Antelope's background service runs, it holds both interfaces for itself, and no other program can open them. Gazelle then finds no devices, shows a warning notice ("Antelope's Manager Service is running and holds the interfaces, so Gazelle cannot open them"), and the tray menu says **Warning: Antelope's service is holding the devices**.

Gazelle looks for the Windows service named `Antelope-Manager-Service`. To stop it:

- **With Services:** press Win+R, type `services.msc`, find the Antelope Manager service, and choose **Stop**. Set its startup type to **Manual** if you want it to stay stopped after a restart.
- **With PowerShell, as administrator:** `Stop-Service -Name Antelope-Manager-Service`

Gazelle never starts or stops the service itself; it only checks whether it is running. Once the service stops, Gazelle finds the interfaces within a few seconds, with no restart needed.

While the service is stopped, Antelope's own control panels cannot reach the devices. Audio keeps working: the driver is separate.

### Going back to the vendor's software

Quit Gazelle (right-click its tray icon, **Quit**), then start the service again (`Start-Service -Name Antelope-Manager-Service`, or **Start** in Services). Antelope's software works as before. Gazelle leaves the devices set as they were.

## The first run

Start Gazelle from the Start Menu, or by double-clicking `gazelle-audio-serverw.exe`. Three things appear:

- **The window**, showing the Devices page. Closing it only hides it; Gazelle keeps running.
- **A tray icon** in the notification area. Click it to show the window again; right-click it for the menu, including **Quit**.
- **Your interfaces** as cards in the sidebar on the right, each with its clock rate, lock state, power and preset.

Starting Gazelle a second time just brings the running window to the front.

> **Warning.** Opening a page reads your devices and changes nothing. Changing any control changes the device at once. Turn your monitors down before you start exploring; see [Safety](02-safety.md).

If no interface appears, see [No devices](17-troubleshooting.md#no-devices-appear).

![The Devices page for a Quadro, with the sidebar's device cards, meters and Control Room on the right, and the mixer dock along the bottom.](../images/devices-quadro.png)

## Trying Gazelle without an interface

Gazelle has a built-in emulator of one Quadro and one Studio+. It never touches hardware, which makes it the safe way to explore the app, and the way every screenshot in this manual was taken. From a terminal in the folder with the programs:

```
gazelle-audio-server.exe --backend loopback
```

Add `--loopback-cyclic-ms 100` to make the emulated devices report moving meters (every status value becomes a moving test pattern, so readouts such as the clock rate show nonsense), and `--no-persist` to keep your experiments out of your saved workspace. While another copy of Gazelle is running on the default address, use a different port as well, for example `--bind 127.0.0.1:8421`.

The header's badge says **LOOPBACK** while you are on the emulator, and **USB**, in the warning colour, when Gazelle is driving real interfaces.

## A first tour

The pages, left to right along the header:

| Page | For |
|---|---|
| [Devices](06-devices-page.md) | Name, clock and sample rate, presets, power, brightness, the test oscillator |
| [Workspace](12-workspace-page.md) | Everything that spans devices: names and colours, surfaces, cables, snapshots, backup |
| [Inputs](07-inputs-page.md) | Preamp type, gain, 48V and phase; digital input gains; microphone emulation |
| [Outputs](08-outputs-page.md) | Output volumes, mutes, dim, trims, hard mute and talkback |
| [Mixer](09-mixer-page.md) | The four internal mixes, as named channels with faders |
| [Routing](10-routing-page.md) | Which source feeds each output, recording channel and effect chain |
| [Effects](11-effects-page.md) | Effect chains, their settings, and the reverb |
| [Aggregate](13-aggregate-page.md) | One driver a DAW opens with both interfaces under it, and whether this PC can run it |

Each page shows the device selected in the sidebar; click another device's card to switch. The sidebar also holds the output **Meter** and the **Control Room**, and the **Mixer** dock along the bottom keeps a mix's faders in reach on every page. Chapter 5, [The app](05-the-app.md), covers these and the gestures every control shares.
