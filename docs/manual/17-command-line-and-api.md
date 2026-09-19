# The command line and the API

This chapter is for power users: running Gazelle with options, and driving it from scripts or other programs over HTTP and WebSocket.

## The programs

| Program | Use |
|---|---|
| `gazelle-audio-server.exe` | Gazelle with a console window: logs appear in the terminal. Use it for options, `--install` and `--uninstall` |
| `gazelle-audio-serverw.exe` | The same program with no console. What the Start Menu shortcut and Start on boot run. If it stops with an error, it shows the error in a message box |

Both take the same options. `gazelle-audio-server.exe --help` lists them; this table must agree with it (the docs build checks).

## Options

| Option | Default | What it does |
|---|---|---|
| `--bind <ADDR>` | `127.0.0.1:8420` | The address to listen on. A non-local address (`0.0.0.0:8420`) exposes device control to your network with no password, logs a warning, and turns the updater off. Port 0 picks a free port |
| `--backend <usb\|loopback>` | `usb` | `usb` drives the attached interfaces. `loopback` runs the built-in emulator of a Quadro and a Studio+ and never touches hardware |
| `--dry-run` | off | Never write to a device: every command reports the bytes it would send. Reads are not sent either |
| `--enable-recall` | off | Allows the snapshot recall route to be asked. Applying a recall is not built, so even with this it refuses, or in dry run reports the bytes |
| `--workspace <FILE>` | `%APPDATA%\gazelle\workspace.json` | Where the workspace is stored |
| `--snapshots-dir <DIR>` | `%APPDATA%\gazelle\snapshots` | Where snapshots are stored. It stays in the config folder even when `--workspace` moves the workspace elsewhere |
| `--no-persist` | off | Keep the workspace and snapshots in memory only; nothing is saved |
| `--loopback-models <MODELS>` | `quadro,studio` | Which emulated devices `--backend loopback` creates, comma-separated |
| `--themes-dir <DIR>` | `%APPDATA%\gazelle\themes` | A folder of your own theme files |
| `--loopback-cyclic-ms <MS>` | off | Makes the emulated devices send status and meter reports every MS milliseconds, as a moving test pattern |
| `--no-web-ui` | off | Serve only the API, not the app (and so no window) |
| `--no-tray` | off | No tray icon, and so no window and no log file unless `--log-dir` is given. For services and test harnesses |
| `--no-update` | off | No update checks this run |
| `--no-window` | off | No desktop window; open the app in a browser |
| `--log-dir <DIR>` | `%LOCALAPPDATA%\gazelle\logs` for tray runs | Also log to a size-capped file in DIR |
| `--install` | | Install this copy for the current user and stop. See [Install](15-install-update-uninstall.md#install) |
| `--start` | | With `--install`: start the installed copy |
| `--no-start` | | With `--install`: do not start it, and do not ask |
| `--uninstall` | | Remove the installed copy and stop. Settings and logs are kept unless `--purge` |
| `--purge` | | With `--uninstall`: also remove settings and logs |
| `--keep-config` | | With `--uninstall`: keep them without asking |
| `--yes` | | Take the default answer to every question |
| `--help`, `-h` | | Print the options (`-h` for a summary) |
| `--version`, `-V` | | Print the version |

An uninstall that relocates itself passes a hidden `--uninstall-target` to its copy; it is not for use by hand.

Examples:

```
gazelle-audio-server.exe --backend loopback --no-persist         the emulator, nothing saved
gazelle-audio-server.exe --dry-run                               your devices, nothing written
gazelle-audio-server.exe --bind 0.0.0.0:8420                     reachable from your network
gazelle-audio-server.exe --backend loopback --bind 127.0.0.1:0   the emulator on a free port
```

### Environment variables

| Variable | Effect |
|---|---|
| `GAZELLE_NO_HARDWARE` | Set to anything but empty, `0` or `false`: refuse the `usb` backend and stop, before opening anything. Test harnesses set it |
| `GAZELLE_CONFIG_DIR` | Where settings live, instead of `%APPDATA%\gazelle` |
| `RUST_LOG` | Log detail, for example `gazelle_audio_server=debug` |

## The HTTP API

Everything the app does goes through this API, under `http://127.0.0.1:8420/api/v1`. There is no authentication: anyone who can reach the address can use it, which is why Gazelle listens only on your own computer unless told otherwise. Errors come back as `{"error": {"code": "...", "message": "..."}}`.

| Method and path | What it does |
|---|---|
| `GET /health` | Status, version, backend, device count, dry run, and any notices |
| `GET /devices` | The attached devices: id, model, family (`quadro` or `studio`), USB ids, backend |
| `GET /devices/{id}` | One device |
| `GET /devices/{id}/commands` | Every command the device's model takes, with its fields, and its status reports |
| `GET /commands` | The command names per known model |
| `POST /devices/{id}/command/{name}` | Send a command. The body is a JSON object of its fields; `?dry_run=true` reports the bytes without sending |
| `GET /devices/{id}/driver` | The audio driver's settings for the device: buffer size, latencies, Safe Mode. `?refresh=true` asks the driver again |
| `PUT /devices/{id}/driver` | Change the driver's buffer size and/or Safe Mode: `{"buffer_size": 256}`, `{"safe_mode": false}`, with `"force": true` to change it while a program uses ASIO. Answers the call as sent and what the driver reports afterwards (`outcome` is `applied`, `mismatch`, `unconfirmed`, `failed` or `unchanged`); `?dry_run=true` builds the call without sending it. An error means nothing was sent: `not_offered` or `nothing_to_change` (400), `asio_in_use`, `unavailable` or `unreadable` (409) |
| `GET /workspace`, `PUT /workspace` | Read or replace the workspace document |
| `GET /themes` | Your theme files |
| `GET /snapshots`, `POST /snapshots` | List snapshots, or take one (`{"name": "..."}`) |
| `GET`, `PATCH`, `DELETE /snapshots/{id}` | Read one; rename it or change its note; delete it |
| `GET /snapshots/{id}/compare` | Read the devices now and list the differences |
| `POST /snapshots/import` | Add snapshots from a list; ones already here are kept |
| `POST /snapshots/{id}/recall/plan` | The recall plan, as the preview shows it. Sends nothing |
| `POST /snapshots/{id}/recall` | Refused: applying a recall is not built |
| `GET /update`, `POST /update/check`, `POST /update/download` | The updater, only when listening on your own computer |
| `POST /window/show` | Brings the window to the front; only from your own computer |
| `GET /ws` | The WebSocket, below |

For example, to see what setting channel 7 of a Quadro's mix 1 to -10 dB would send, without sending it (`level` is dB of attenuation):

```
curl -X POST "http://127.0.0.1:8420/api/v1/devices/<id>/command/set_mixer?dry_run=true" -H "content-type: application/json" -d "{\"mixer_id\": 0, \"channel\": 7, \"level\": 10}"
```

Field values are integers (booleans become 0 or 1, byte arrays are hex strings), exactly as the device's command defines them; the `commands` route lists every field. The few commands in a device's command set that manage its licence are not offered at all (`unknown_command`): Gazelle reads which emulations and effects a device is licensed for, and changes nothing about it. A field you leave out is sent as its default, not as the device's current value: `set_mixer` above also sets the channel's pan, mute and solo. Commands go straight to the device: nothing checks that a value is sensible for your speakers. Every [safety](02-safety.md) consideration applies, more so.

Error codes: `unknown_device` (404), `unknown_command` (404), `bad_value` (400), `timeout` (504, no answer within 3 seconds), `refused` (502, the device said no), `device_gone` (503), `no_registry` (501, a model Gazelle does not know), `unsupported` (501), `unknown_snapshot` (404), `storage_error` and `protocol_error` (500).

## The WebSocket

`GET /api/v1/ws` upgrades to a WebSocket carrying JSON text frames.

- The server's first frame is `{"type": "hello", ...}` with the version, backend, dry run, the devices and any notices.
- Then events: `device_added`, `device_removed`, `cyclic` (a decoded status or meter report: `device_id`, `report_id`, `fields`), `undecoded`, and `lagged` when a slow client missed some.
- To send a command: `{"id": 1, "device_id": "...", "command": "set_mute", "args": {"id": 0, "mute": 1}}`, optionally with `"dry_run": true`. The answer is `{"type": "rpc_response", "id": 1, "result": {...}}` or `{"type": "rpc_error", "id": 1, "error": {...}}`, in the order commands finish.

The web app's own TypeScript client, `gazelle-audio-client` (in `web/packages/client`), wraps all of this with types generated from the command definitions.
