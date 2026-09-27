# The app

This chapter covers what surrounds the pages: the window and the tray, the header, the sidebar, the mixer dock, notices, themes, using Gazelle from a phone, and the gestures every control shares.

## The window and the tray

Gazelle is one program with three faces: a desktop window, a tray icon, and a small web server that both of them, and any browser you allow, talk to.

- **The window** shows the app. Closing it hides it; Gazelle keeps running in the tray, and your devices keep their settings. Its size and position are remembered.
- **The tray icon** (notification area) brings the window back with a click. Right-click it for the menu:

| Tray item | What it does |
|---|---|
| **Gazelle X.Y.Z** | The first line: which version is running. The icon's own tooltip stays **Gazelle** |
| **Open Gazelle** | Shows the window (or opens the browser, where there is no window) |
| **Open in browser** | Opens the same app in your web browser |
| **Recording widget** | Opens or closes the small recording window that stays on top; ticked while it is open. See [The recording widget](14-recording-page.md#the-recording-widget) |
| **Recording hub** | Opens the full-screen recording hub on the monitor it was last on. See [The recording hub](14-recording-page.md#the-recording-hub) |
| Listening on, Backend, Dry run | Information: the address, `usb` or `loopback`, and whether dry run is on |
| Device lines | Each attached device, or **No devices attached** |
| **Warning: Antelope's service is holding the devices** | Shown while that service runs; see [Getting started](03-getting-started.md#stop-antelopes-service) |
| **Rescan devices** | Looks for interfaces again at once (it also does so every two seconds) |
| Update lines | The update state, **Check for updates**, and, when one is ready, **Restart to update to X**. **Download update X** appears only if you set `auto_download` to `false`; see [Updates](18-install-update-uninstall.md#updates) |
| **Start on boot** | Starts Gazelle in the tray when you log in, with its window hidden until you open it |
| **Auto-arm with** *preset* | Arms whenever Gazelle starts and when the interfaces come back; see [Auto-arm](14-recording-page.md#auto-arm). Greyed until a preset is chosen on the Recording page |
| **Start in the recording hub** | Opens the recording hub full screen whenever Gazelle starts, at login too |
| **Allow phones on this network** | The same switch as on the Workspace page; see [Phones on your network](#phones-on-your-network). Greyed, and shown on, when `--bind` decides instead |
| **Open log folder** | Opens the folder with Gazelle's log files |
| **Quit** | Stops Gazelle. The devices keep their settings. A take being recorded is finished first |

Starting Gazelle while it is already running brings the running window to the front instead of starting a second copy.

## The header

Along the top: the page tabs, then three things worth a glance. On a phone, or in a window of phone width, **Remote** is the first tab, in the accent colour; see [The Remote page](15-remote-page.md).

- **The backend badge.** **USB** (in the warning colour) when Gazelle drives real interfaces; **LOOPBACK** when it runs the emulator.
- **Dry run**, shown only when Gazelle was started with `--dry-run`: commands report the bytes they would send, and nothing is written to a device.
- **The connection**: Connected, Reconnecting or Disconnected. While it is not connected, a banner says so and every control is disabled.

**Explain mode.** The **?** button turns it on and off, and it is remembered in this browser. While it is on, pointing at or tabbing to anything shows a small panel saying what it is, what changing it does and what to watch for; on a touch screen, the **i** button beside it makes the next taps explain instead of act. Nothing is sent to a device either way.

**The version.** While the explain mode is on, the version Gazelle is running reads out in dim text just left of the backend badge, so the number is to hand when you report something or ask about an update. It is out of the way the rest of the time, and a phone, whose header line is already full, leaves it out; the tray menu's first line says the same version.

**Updates.** Beside the version, and whether or not the explain mode is on, the header says what the updater wants, and nothing at all while there is nothing to want:

| What it shows | What it means | What pressing it does |
|---|---|---|
| nothing | Up to date, or checking, or this server has no updater | |
| **X downloading** | A new version is being fetched and checked | Nothing to press; wait |
| **X ready, restart** | It is downloaded, checked, and in place | Asks once more (**Confirm**), then restarts Gazelle into it. The devices are let go and picked up again, and the page reconnects by itself |
| **Get X** | A new version was found and this install does not fetch by itself (`auto_download` is `false`) | Downloads and checks it |
| **Update failed, try again** | A check or a download failed; its tooltip says why | Looks again |

A phone drops the version readout but keeps this, since it is something to do rather than something to read. The whole story is in [Updates](18-install-update-uninstall.md#updates).

At the right, two menus. **Double-click** chooses what a double-click does to a level: its **safe level** (the default: -20 dB on a fader, -30 dB on a volume, off on a reverb send), or **unity**. It is kept in this browser, and a phone, which has no double-click, does not show it. The **theme** menu switches between Gazelle Dark, Gazelle Light and any community or user themes.

![The header's right end: the LOOPBACK badge, Connected, the Double-click menu on safe level, and the theme menu.](../images/header-menus.png)

## The sidebar

One sidebar holds three sections, each folded or opened by clicking its title. It sits on the right; the arrow button at its top moves it to the other side, and the double arrow folds it to a narrow rail.

- **Devices.** A card per interface: its name, the measured clock rate and **LOCKED** or **NO LOCK**, **On** or **Standby**, the current preset (Studio+ only), and a light that shows input signal (and turns to the clip colour briefly on a clip). Click a card to show that device on the current page.
- **Meter.** The shown device's outputs as level bars with a held peak and a clip light. The Quadro reports Monitor, HP1, HP2 and Line out; the Studio+ reports its output levels in a way Gazelle does not read yet, so it shows a note instead. **Clear** clears every clip light on every device; the menu beside it sets how long a clip light stays lit after the clip ends (2, 5, 10 or 30 seconds, or **Hold** until cleared). Clicking a single clip light clears it.
- **Control Room.** The outputs you monitor on, for the shown device: by default Monitor, HP1 and HP2; the **CR** button on the Outputs page adds or removes others. Each has a volume bar, **Mute**, **Dim** (Quadro) and **Mono**, and a caption saying what feeds it. On a Studio+, the talkback controls sit below: a **Talk** button you hold, the talkback level, and which of HP1, HP2 and Monitor it goes to.

![The Control Room panel for a Quadro with four outputs chosen, each with its volume, Mute, Dim and Mono.](../images/control-room.png)

**Mono** in the Control Room sums the mix that feeds the output to mono, by centring every channel's pan in that mix and lowering that mix by as much as summing gains (6 dB on the Studio+, less on the Quadro, where centre attenuation already takes some) so the level stays about the same; both are put back afterwards. Every output playing that mix goes mono too, and the button's tooltip names them. If more than one mix, or no mix, feeds the output, Mono is disabled and its tooltip says why. The first press may read the device's routing first, to find the mix.

## The mixer dock

The **Mixer** band along the bottom of every page (except the Mixer page itself) keeps one mix's faders in reach. It shows the selected device's channels in the selected mix as slim strips, with the mix master at the right.

- **Show** chooses **This device** or any [surface](16-surfaces-and-cables.md) you have built.
- The mix menu beside the device name is the same choice as the Mixer page's Mix buttons.
- Click the band's title to fold it away; Gazelle remembers.
- Drag sources from the [Routing page](10-routing-page.md#adding-sources-to-a-mix) onto it to add a channel for each to the mix it shows.

## Notices and the "last sent" line

Failures and warnings appear as notices in the bottom-right corner, each closed with its ×. Examples: a command that failed, a workspace change the server refused (and that has been undone on screen), or Antelope's service holding the devices.

Every page that sends commands can show, at the right of its top bar, the last command it sent and its bytes, for example "Sent set_mixer: 70000000...". It appears while the explain mode is on, and in dry run, where it reads "Dry run, would send ..."; the rest of the time it stays out of the way.

## Phones on your network

Gazelle is a web app served by the program on your computer, so a phone's browser, or a second computer's, can have the same controls. By default Gazelle listens only on the computer itself (`127.0.0.1:8420`) and nothing else can reach it. Letting phones in takes two steps, both on the computer: allow phones on the network, then pair each phone.

**Allow phones.** On the [Workspace page](12-workspace-page.md), the **Phones** section has **Allow phones on this network** (also in the tray menu). Turning it on takes a second click. Gazelle then also listens on every network connection your computer has, on the same port, straight away and without a restart; the section shows the addresses a phone would use, such as `http://192.168.1.20:8420/`. Turning it off stops listening at once and closes every phone's connection. The setting is kept in `%APPDATA%\gazelle\remote.json`.

**Windows may ask.** The first time Gazelle listens on the network, Windows Defender Firewall may ask whether to allow it. Allow it on **private networks only** (your home or studio network), not public ones such as a café's. Gazelle never changes firewall settings itself; that is yours to decide. If you declined by mistake, the rule can be changed in Windows Security, Firewall and network protection, Allow an app through firewall.

**Pair a phone.** Choose **Pair a phone**. Gazelle shows a code, such as `YHV8-YRJM`, and a QR code of the pairing address. Scan the QR code with the phone's camera, or open the address shown on the phone and type the code; letters and digits only, and case, spaces and dashes do not matter. The phone asks for a name for itself, then opens Gazelle on its [Remote page](15-remote-page.md), the page laid out for a phone; every other page is a tab along from it. A code lasts 5 minutes, works once, and stops working after ten wrong tries; **Stop pairing** ends it sooner, and **New code** replaces it. From then on the phone stays paired, with nothing to type, until you revoke it.

**On Android**, the Gazelle Remote app is an alternative to the browser: Gazelle full screen, pairing by scanning the QR code itself. See [The Android app](15-remote-page.md#the-android-app).

**Paired phones** are listed under the pairing, each with when it was paired and when and from which address it was last seen. **Revoke** (two clicks) unpairs a phone at once: its next request is refused and its open connection closes. The phone then says **This phone is no longer paired with Gazelle** in place of the app and stops trying to reconnect; its **Pair again** link opens the pairing page. Pair it again to let it back.

Everything in the Phones section can be changed only from the computer Gazelle runs on. On a phone the section says so, and a phone never sees the update prompt: updating and restarting are the computer's business too. A phone does not even ask for them, so nothing it does is refused.

![The Mixer page at phone width. The sidebar becomes a drawer, opened with the menu button.](../images/phone-mixer.png)

At phone width the sidebar becomes a drawer, opened with the ☰ button and closed with Escape, a tap outside it, or by choosing a page. The mixer dock starts folded.

### What pairing protects, and what it does not

- **Only paired phones can control Gazelle.** Every request from another device must carry its phone's key, which pairing hands over once (as a cookie the phone's browser keeps, and in the answer for an app). Without one, the answer is a refusal. The key is kept on the computer only as a fingerprint that cannot be turned back into it.
- **Pairing needs your screen.** A code comes only from the computer, lasts minutes, and is used once, so being on your network is not enough to get in: someone has to see the code.
- **Some things stay on the computer**, whatever a phone holds: the Phones section itself, updates and restarting, the window, and the aggregate driver.
- **What it does not protect: the connection is plain HTTP.** Anyone on the same network who can watch its traffic (on a shared or open Wi-Fi, or through a compromised router) can see everything the phone and Gazelle send each other, including the phone's key, and could use that key until you revoke the phone. Pairing decides who can control Gazelle, not who can watch. Allow phones only on a network you trust, and revoke a phone you have lost.
- **This computer is trusted without pairing**, as it always was, so the window and your own scripts work as before. Gazelle refuses requests from a web page on another site, and requests that arrive under a name that is not this computer's, so a web page you visit cannot use your browser to reach it.

### Started with `--bind`

Starting Gazelle with `--bind` on a network address (see [the command line](20-command-line-and-api.md#options)) puts it on the network whatever the switch says, and the switch is shown on and greyed. Every other device still has to pair. With one specific address, such as `--bind 192.168.1.20:8420`, Gazelle also listens on `127.0.0.1` at that port, so the window and the pairing still work from the computer itself.

## Gestures

Every bar, fader and knob-like control in Gazelle responds to the same gestures.

| Gesture | What it does |
|---|---|
| **Drag** | Follows the pointer. A pan dragged near the centre snaps to centre |
| **Click** | Jumps straight to the value under the pointer |
| **Double-click** | Resets to the control's default (below) |
| **Ctrl+click** (Cmd+click) | On a level (faders, volumes, sends, returns): unity, wherever you click |
| **Mouse wheel** | One step per notch, up for more; pan steps through centre without snapping |
| **Arrow keys** | One step; Up and Right for more |
| **Page Up / Page Down** | A larger step (6 dB on faders, volumes and preamp gains) |
| **Home / End** | The control's two ends (see the warning below) |

What double-click resets to:

| Control | Reset |
|---|---|
| Mixer fader, mix master | -20 dB (Ctrl+click: 0 dB) |
| Pan | Centre |
| Studio+ reverb Send (Mixer page, mix 1) | off (Ctrl+click: 0 dB) |
| Output volume, Control Room volume, talkback level | -30 dB (Ctrl+click: 0 dB) |
| Preamp gain, digital input gain | 0 dB |
| Brightness | 50% |
| Reverb level | -18 dB, the nearest it has to -20 (Ctrl+click: 0 dB) |
| Quadro reverb returns | 20 steps below full (Ctrl+click: full) |
| Quadro reverb sends | off (Ctrl+click: 0 dB) |
| Effect settings | The vendor panel's starting value |

The rows with a Ctrl+click are **levels**. Their double-click goes to a safe level, and Ctrl+click to unity; with the header's **Double-click** menu on **unity**, double-click goes to unity as well. A level's tooltip says what its double-click and Ctrl+click do.

> **Warning.** Home and End go to the raw ends of a control's scale, which point different ways on different controls: on a mixer fader **Home is 0 dB**, while on an output volume **End is 0 dB**. Ctrl+click, and double-click when you have chosen unity, put a level at full level. With monitors up, prefer the arrow keys.

**Drop-down menus take the wheel too**, one option per notch, and most send their change at once. The clock source, sample rate, the driver's buffer size, **Add effect**, a channel's **Input** and **Main mix**, the mix master's **+ Output**, a surface's **Route** and mix pin menus, and the header's **Double-click** menu ignore the wheel, because one accidental notch there would interrupt audio, change routing or change a safety setting. Ctrl with the wheel still zooms the page.

**Text fields** (names) save on Enter or when you leave the field; Escape puts back what was there.

**Buttons that ask twice** read **Confirm** (or **Confirm N**, **Confirm save**, **Confirm standby**) after the first click, and forget it after three seconds: 48V on, a test tone on, DC coupling on, recalling or saving a device preset, the driver's Safe Mode either way, Standby, and removing a channel, a surface, a strip, a cable or a snapshot. Turning a thing off never asks, except Safe Mode, which restarts a DAW's audio either way. The clock source, sample rate and driver buffer size menus ask with a **Confirm** button beside them. Enter and Space press these buttons as a click does.

Apart from those, the only keyboard shortcuts are Escape (to close the phone drawer or a colour picker) and the usual Tab to move between controls.

## Where preferences are kept

| Kept in the workspace, for everyone using this Gazelle | Kept in this browser only |
|---|---|
| Device names and colours | Theme |
| Mixer channels, groups, mix names, links, saved layouts | Sidebar side, folded state and sections |
| Mono state of each mix | Mixer dock folded, and what it shows |
| Control Room output choices | Selected device, and the selected mix per device |
| Surfaces and digital cables | Mixer strip width |
| | Clip light auto-clear time |
| | What a double-click does to a level |

The window's own size and position are kept in `%APPDATA%\gazelle\window.json`.
