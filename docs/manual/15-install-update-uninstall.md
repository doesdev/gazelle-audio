# Install, update and uninstall

Gazelle installs for your Windows user account only: no administrator rights, no installer program, no system-wide changes. The program you download is its own installer.

## Install

From a terminal in the folder where you unzipped a release:

```
gazelle-audio-server.exe --install              asks whether to start it
gazelle-audio-server.exe --install --start      installs and starts it
gazelle-audio-server.exe --install --no-start   installs and starts nothing
```

It creates exactly three things, and prints where:

| What | Where |
|---|---|
| Both programs | `%LOCALAPPDATA%\Programs\Gazelle` |
| A Start Menu shortcut, **Gazelle** | `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Gazelle.lnk` |
| An entry in Settings, Apps | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Gazelle` |

The shortcut runs the windowless program with no arguments, so it talks to your interfaces with the default settings.

- Run from a terminal with neither `--start` nor `--no-start`, it asks "Start Gazelle now?". Run where nobody can answer (from Explorer or a script), it **starts** the installed copy. With `--yes`, it does not.
- **Upgrading** is the same command from a newer download. It refuses, and changes nothing, while the installed copy is running: quit it from the tray first.
- If **Start on boot** was on, the login entry is pointed at the installed copy. An install never turns Start on boot on by itself.
- After installing, the folder you unzipped can be deleted.

The installer has been tested against a temporary folder and a test registry key, not yet by hand on a real user account.

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
| `%LOCALAPPDATA%\gazelle\logs\gazelle.log` | The log; see [Logs](16-troubleshooting.md#logs) |

Setting the `GAZELLE_CONFIG_DIR` environment variable moves the settings (not the logs) elsewhere.
