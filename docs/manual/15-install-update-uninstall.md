# Install, update and uninstall

Gazelle installs for your Windows user account only: no administrator rights, no system-wide changes. The program you download is its own installer.

## Install

### With the setup file

Download `Gazelle-Setup.exe` from a release and double-click it. (Windows SmartScreen may say "Windows protected your PC" the first time, because the program is not code-signed: choose **More info**, then **Run anyway**.) A small window titled **Gazelle** asks one question, depending on what is already installed:

| What is installed | What it asks | What you can choose |
|---|---|---|
| Nothing | "Install Gazelle for your account? It goes in your user folder, needs no administrator rights, and adds Gazelle to the Start Menu." | **Install**, **Cancel** |
| An older version | "Gazelle 1.0.0 is installed. Replace it with Gazelle 1.1.0? Your settings and layouts are kept." | **Replace**, **Cancel** |
| The same version | "Gazelle 1.1.0 is already installed." | **Open Gazelle**, **Install again**, **Cancel** |
| A newer version | "A newer Gazelle is already installed: version 1.2.0. This file is version 1.1.0, so it will not replace it." | **Open Gazelle**, **Cancel** |

(The version numbers are examples.) After **Install**, **Replace** or **Install again**, Gazelle opens from its installed copy, and the setup file can be deleted. A setup file never replaces a newer version with an older one.

If the installed copy is running, it cannot be replaced while it runs, and the setup file says so: "Gazelle is running, so it cannot be replaced yet." Right-click the Gazelle icon in the notification area, next to the clock, choose **Quit**, then choose **Try again**.

The setup file is the windowless program from the zip under another name; the name is what makes it offer to install. It installs only that program, so an install from it has no console program in the install folder. Everything in this chapter works the same way; the uninstall in Settings, Apps uses the windowless program instead, and keeps your settings.

### From the zip

Double-clicking `gazelle-audio-serverw.exe` in an unzipped release, while nothing is installed, asks the same first question with a third choice, **Run without installing**. That runs Gazelle from where it is, and it never asks again (the choice is kept in `%APPDATA%\gazelle\setup.json`; delete that file to be asked again). Once Gazelle is installed it no longer asks.

Or install from a terminal in the folder where you unzipped a release:

```
gazelle-audio-server.exe --install              asks whether to start it
gazelle-audio-server.exe --install --start      installs and starts it
gazelle-audio-server.exe --install --no-start   installs and starts nothing
```

It creates exactly three things, and prints where:

| What | Where |
|---|---|
| Both programs (the setup file: the windowless one) | `%LOCALAPPDATA%\Programs\Gazelle` |
| A Start Menu shortcut, **Gazelle** | `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Gazelle.lnk` |
| An entry in Settings, Apps | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Gazelle` |

The shortcut runs the windowless program with no arguments, so it talks to your interfaces with the default settings.

- Run from a terminal with neither `--start` nor `--no-start`, it asks "Start Gazelle now?". Run where nobody can answer (from Explorer or a script), it **starts** the installed copy. With `--yes`, it does not.
- **Upgrading** is the same command from a newer download, or the newer setup file. It refuses, and changes nothing, while the installed copy is running: quit it from the tray first.
- If **Start on boot** was on, the login entry is pointed at the installed copy. An install never turns Start on boot on by itself.
- After installing, the folder you unzipped can be deleted.

The installer has been tested against a temporary folder and a test registry key, not yet by hand on a real user account. The setup file's decisions are tested the same way; its dialogs have not yet been clicked through by hand, nor has SmartScreen been seen on a downloaded copy.

## Start on boot

The tray menu's **Start on boot** adds a login entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, named **Gazelle**) that starts the windowless program when you log in, with the same address and options the running copy has. Untick it to remove the entry. Not yet tried by hand on a real login.

## Updates

Gazelle checks for a newer release when it starts and every six hours, with one request to GitHub, and says what it found in the tray menu. **It never downloads anything unless you ask.**

1. The tray shows **Update available: X**. Choose **Download update X**.
2. Gazelle downloads the new programs and checks them before using them: each file against the release's list of checksums, and that list against a signature made with the project's release key, which is built into Gazelle. If either check fails, the download is deleted and the tray says so.
3. The checked programs replace the installed ones straight away, while the old one keeps running (it is renamed to `.exe.old`, which the next start deletes). The tray offers **Restart to update to X**; choosing it stops Gazelle cleanly, releasing the devices, and starts the new version.

Some things to know:

- There is no published release yet, so there is nothing to update to. A build made from source without the release key cannot download updates at all, and says so.
- The update controls exist only while Gazelle listens on your own computer. Started with `--bind` on a network address, it has no updater.
- `--no-update` turns update checks off for one run.
- To change the defaults, write `%APPDATA%\gazelle\update.json`; without it, Gazelle uses these:

```json
{ "check": true, "channel": "stable", "interval_hours": 6, "auto_download": false,
  "repo": "doesdev/gazelle-audio", "api_base": "https://api.github.com" }
```

`"channel": "prerelease"` also offers release candidates. `"check": false` removes the updater entirely, including the tray's **Check for updates**. `"auto_download": true` downloads (never installs) without asking. `"interval_hours": 0` checks only at start.

The update path has been rehearsed end to end against a local test release, but the tray's download and restart items have not been used on a real release.

## Uninstall

From **Settings, Apps, Gazelle, Uninstall**, or from a terminal:

```
"%LOCALAPPDATA%\Programs\Gazelle\gazelle-audio-server.exe" --uninstall
```

An install from the setup file has only the windowless program, so from a terminal that is `gazelle-audio-serverw.exe --uninstall` in the same folder; it prints nothing and keeps your settings.

It removes the programs, the shortcut, the Settings entry and any leftover `.exe.old` files, and nothing else. A file you put in the install folder yourself is left there, with its folder. Quit Gazelle first.

Your settings (`%APPDATA%\gazelle`: workspace, layouts, themes, snapshots, window and update settings) and logs (`%LOCALAPPDATA%\gazelle`) are **kept** unless you say otherwise:

| Option | Settings and logs |
|---|---|
| none, in a terminal | Asks, and keeps them unless you answer yes |
| `--purge` | Removed |
| `--keep-config` | Kept, without asking |
| `--yes` | Kept, without asking anything (what Settings, Apps runs for a quiet uninstall) |

An uninstall started from the install folder itself copies itself to your temporary folder and finishes from there, since Windows cannot delete a running program. That copy is the one file left behind, for Windows to clean up.

## Files Gazelle keeps

| Path | What |
|---|---|
| `%APPDATA%\gazelle\workspace.json` | The workspace |
| `%APPDATA%\gazelle\snapshots\` | One file per snapshot |
| `%APPDATA%\gazelle\themes\` | Your own themes, if any |
| `%APPDATA%\gazelle\window.json` | The window's size and position |
| `%APPDATA%\gazelle\update.json` | Update settings |
| `%APPDATA%\gazelle\setup.json` | Present only if you chose **Run without installing** |
| `%LOCALAPPDATA%\gazelle\logs\gazelle.log` | The log; see [Logs](16-troubleshooting.md#logs) |

Setting the `GAZELLE_CONFIG_DIR` environment variable moves the settings (not the logs) elsewhere.
