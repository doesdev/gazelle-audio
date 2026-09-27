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
| `--bind <ADDR>` | `127.0.0.1:8420` | The address to listen on. Port 0 picks a free port. A network address puts Gazelle on your network whatever **Allow phones on this network** says (the switch is then shown on and greyed), and every other device must still pair and send its key; see [Remote access](#remote-access). A wildcard (`0.0.0.0:8420`) includes this computer; one specific address (`192.168.1.20:8420`) also opens `127.0.0.1` at the same port, for the window and for pairing. The updater stays on either way, for this computer only |
| `--backend <usb\|loopback>` | `usb` | `usb` drives the attached interfaces. `loopback` runs the built-in emulator of a Quadro and a Studio+ and never touches hardware |
| `--dry-run` | off | Never write to a device: every command reports the bytes it would send. Reads are not sent either |
| `--enable-recall` | off | Allows the snapshot recall route to be asked. Applying a recall is not built, so even with this it refuses, or in dry run reports the bytes |
| `--workspace <FILE>` | `%APPDATA%\gazelle\workspace.json` | Where the workspace is stored |
| `--snapshots-dir <DIR>` | `%APPDATA%\gazelle\snapshots` | Where snapshots are stored. It stays in the config folder even when `--workspace` moves the workspace elsewhere |
| `--no-persist` | off | Keep the workspace and snapshots in memory only; nothing is saved. The phones setting and the recording settings are in memory too, so auto-arm is off |
| `--loopback-models <MODELS>` | `quadro,studio` | Which emulated devices `--backend loopback` creates, comma-separated |
| `--themes-dir <DIR>` | `%APPDATA%\gazelle\themes` | A folder of your own theme files |
| `--loopback-cyclic-ms <MS>` | off | Makes the emulated devices send status and meter reports every MS milliseconds, as a moving test pattern |
| `--no-web-ui` | off | Serve only the API, not the app (and so no window) |
| `--no-tray` | off | No tray icon, and so no window and no log file unless `--log-dir` is given. For services and test harnesses |
| `--no-update` | off | No update checks this run |
| `--no-window` | off | No desktop window, and so no recording widget or hub; open the app in a browser. Auto-arm still works |
| `--hidden` | off | Start in the tray with the window hidden; **Open Gazelle** in the tray, or launching Gazelle again, shows it. What Start on boot runs. If Gazelle is already running, a `--hidden` start leaves it alone. The recording widget still opens if it was open, and **Start in the recording hub** still opens the hub: the setting wins over `--hidden` for the hub, not for the app's window |
| `--log-dir <DIR>` | `%LOCALAPPDATA%\gazelle\logs` for tray runs | Also log to a size-capped file in DIR |
| `--install` | | Install this copy for the current user and stop. See [Install](18-install-update-uninstall.md#install) |
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
gazelle-audio-server.exe --bind 0.0.0.0:8420                     on your network; other devices pair
gazelle-audio-server.exe --backend loopback --bind 127.0.0.1:0   the emulator on a free port
```

### Environment variables

| Variable | Effect |
|---|---|
| `GAZELLE_NO_HARDWARE` | Set to anything but empty, `0` or `false`: refuse the `usb` backend and stop, before opening anything. Test harnesses set it |
| `GAZELLE_CONFIG_DIR` | Where settings live, instead of `%APPDATA%\gazelle` |
| `RUST_LOG` | Log detail, for example `gazelle_audio_server=debug` |

## The HTTP API

Everything the app does goes through this API, under `http://127.0.0.1:8420/api/v1`. A program on this computer needs no key; anything else must be a paired phone and send its key (see [Remote access](#remote-access)). Errors come back as `{"error": {"code": "...", "message": "..."}}`.

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
| `GET /workspace`, `PUT /workspace` | Read or replace the workspace document. Its `recording.presets` are the Recording page's presets: a phone's save that changes them is refused with `not_local` (403), and a save that changes the preset the recorder is armed with gets `recording_armed` (409) |
| `GET /themes` | Your theme files |
| `GET /snapshots`, `POST /snapshots` | List snapshots, or take one (`{"name": "..."}`) |
| `GET`, `PATCH`, `DELETE /snapshots/{id}` | Read one; rename it or change its note; delete it |
| `GET /snapshots/{id}/compare` | Read the devices now and list the differences |
| `POST /snapshots/import` | Add snapshots from a list; ones already here are kept |
| `POST /snapshots/{id}/recall/plan` | The recall plan, as the preview shows it. Sends nothing |
| `POST /snapshots/{id}/recall` | Refused: applying a recall is not built |
| `GET /update`, `POST /update/check`, `POST /update/download` | The updater, only from your own computer (a phone gets `not_local`, 403). The status says the running version, the channel, when it last checked (`last_check_ms`), and its state: `unknown`, `checking`, `up_to_date`, `available`, `downloading`, `staged` or `failed` |
| `POST /update/restart` | Restart into the version waiting on disk. Answers `{"restarting": true, "version": "..."}` **before** the server stops, which it then does a fraction of a second later, so expect the connection to drop and come back. With nothing waiting it refuses with `nothing_staged` (409) and stops nothing. Only from your own computer |
| `GET /aggregate` | Whether this computer can run the aggregate audio driver, and what is in the way. In one answer: every audio driver installed here, whether Gazelle Aggregate is registered and what its registration points at, each chosen interface's live clock source, lock, sample rate, buffer size and USB controller, what the driver itself is reporting now, the last few lines of its event log, and `ready` with a list of `reasons`. Each reason has a `code` (`not_configured`, `device_missing`, `not_registered`, `dll_missing`, `device_not_attached`, `device_not_matched`, `driver_unreadable`, `one_usb_controller`, `controller_unknown`, `rates_differ`, `buffers_differ`, `no_cable`, `clock_not_cabled`, `not_locked`, `phase_not_measured`), a `severity` (`blocking` or `warning`, and `ready` is false only for a blocking one) and, where Gazelle can put it right, a `fix` naming the route and body to send. Only from your own computer |
| `POST /aggregate/match-buffers` | Put every chosen interface on one buffer size: `{"buffer_size": 512}`, with `"force": true` to change one while a program is using it. Each interface answers for itself, so one refusing does not stop the others; a refusal carries the same codes as `PUT /devices/{id}/driver`. Refused as a whole with `not_configured` (409) when no interfaces have been chosen. Only from your own computer |
| `POST /aggregate/register`, `POST /aggregate/unregister` | Register or unregister the aggregate driver. This needs administrator rights, so Windows puts up its own prompt; a declined prompt comes back as `run.started` false, not as an error. The answer always includes the `command` you could run yourself instead. With no driver file to register it refuses with `dll_not_found` (409) and says where it looked. Only from your own computer |
| `GET /aggregate/calibrate` | The one measurement at a time; while the Recording page is armed, starting one is refused with `recording_armed` (409). `state` is `idle`, `running`, `done` or `failed`; while it runs, `step` and `progress` (0 to 1); when it failed, a `refusal` in one sentence; when it is done, an `outcome`. The outcome has per interface `readings` (how far behind the reference, in samples, the `spread` across the clicks and the widest it could be and still count, clicks found, blocks lost), the `trims` it implies (`was`, `measured`, `now`, and `phase_reference` with `was` and `now` beside an input trim on an interface whose phase is measured), what each interface's `phases` came to, any `witnesses`, whether it was `clean`, and `warnings`. A `checking` outcome offers no trims. Only from your own computer |
| `POST /aggregate/calibrate` | Start one: `{"direction": "inputs", "outputs": [{"device": 0, "channel": 0}, {"device": 0, "channel": 1}], "inputs": [{"device": 0, "channel": 0}, {"device": 1, "channel": 0}]}` names the cabling one output and one input per interface, in the setup's order. Every channel is an interface, by its place in the setup, and that interface's own channel number, both counted from zero, so `{"device": 1, "channel": 0}` is the second interface's first channel. It is never a number in the aggregate's own list: the run opens the drivers and works that out itself. Optional: `witnesses` (extra inputs, named the same way, to record and report, taking no part in any trim), `clicks`, `level_dbfs`, and `"check": true` to line the session up as a DAW's would be and report how far apart a recording would land, rather than measuring a trim. **It plays a click out of a real output** and takes both audio drivers while it runs. Refused with `not_started` (409) when one is already running or the request does not make sense, `not_configured` (409) with fewer than two interfaces, and `bad_value` (400) when a channel is not written as an interface and a channel. A channel the run cannot place is a failed run whose `refusal` says what would have been right, before anything plays: an interface that is not one of them, a channel the interface has not got (with how many it has, from its own driver), one the setup keeps out of the aggregate, or one the setup keeps for the phase measurement. Only from your own computer |
| `POST /aggregate/calibrate/stop` | Stop the one that is running, between one block and the next, letting both drivers go. Answers `{"stopped": true}` when something was running and `false` when nothing was. Only from your own computer |
| `GET /recording` | The recorder. `state` is `off`, `arming`, `armed`, `recording` or `disarming`. While armed: the `preset` it holds (`id`, `name`, `folder`, `format`), the aggregate's `rate` and `buffer_size`, the recorded `channels` (each with its `name` as the aggregate names it, `device`, `device_index`, `channel`, and `peak_dbfs` since the last reading), the `preroll` (`available_bytes` it was sized from, `percent_asked`, `percent`, `bytes`, `preroll_seconds`, why it was `clamped` if it was, and `held_seconds` now), the `take` while recording (`elapsed_seconds` since Record, `preroll_seconds` it started with, `number`, `folder`, `files`), `overruns` (blocks lost because the disk fell behind), `dropouts` and `dropouts_by_device` (blocks the aggregate lost), `disk_free_bytes`, `disk_seconds_left`, `disk_low`, `reset_asked` (a driver asked to be restarted) and `problem`. Always: `last_preset`, `default_folder`, `loopback`, and `auto_arm`: whether it is `on`, its `preset` and `preset_name`, its `phase` (`off`, `armed`, `arming`, `paused` after a disarm by hand, `waiting_for_interfaces`, `waiting_for_measurement`, `backing_off`), the `reason` and `failures` while backing off, `retry_at_ms` (the next try, in milliseconds since 1970), and `lost` when it disarmed because the interfaces went away. A paired phone may use every recording route but the settings |
| `POST /recording/arm` | `{"preset": "<id>"}`, a preset from the workspace's `recording.presets`. Opens the aggregate, reserves the pre-roll and starts holding it; answers once armed, as `GET /recording` does. Refused with `no_preset` (404), `measuring` (409) while a measurement has the interfaces, or `arm_refused` (409) with the reason: a channel the aggregate has not got, too little memory, a driver that would not open, `GAZELLE_NO_HARDWARE` set, or already armed with another preset. With `--backend loopback` it records the emulator's test tones into `%TEMP%\Gazelle loopback recordings`, whatever the preset says |
| `POST /recording/record`, `POST /recording/stop` | Start a take, reaching back into the pre-roll, or end it and stay armed. Each takes effect at the next block of audio. Record is refused with `not_armed` or `record_refused` (409), for example when the disk has less than a minute of room; Stop always answers, with `stopped` true when a take was running |
| `POST /recording/disarm` | Stop any take, let go of the drivers and free the memory. While recording it needs `{"confirm": true}`, else `confirm_disarm` (409). Answers with `disarmed`. With auto-arm on, this pauses it until Gazelle next starts or `/recording/arm` is asked |
| `GET /recording/settings`, `PUT /recording/settings` | Auto-arm and starting in the hub, kept in `%APPDATA%\gazelle\recording.json`: `{"auto_arm": false, "auto_arm_preset": null, "start_in_hub": false}`. A `PUT` changes what it names and keeps the rest; `auto_arm_preset: null` forgets the preset. Turning auto-arm on needs a preset (`bad_value`, 400) that is in the workspace (`no_preset`, 409), and it arms within a second. `--no-persist` keeps them in memory, off. Only from your own computer |
| `GET /recording/takes` | `{"takes": [...]}`, the takes since Gazelle started, newest first: `number`, `preset`, `folder`, `files` (whole paths), `log`, `date`, `time`, `seconds`, `preroll_seconds`, `overruns`, `dropouts`, and `stopped_by` and `problem` when Gazelle stopped it or something went wrong |
| `POST /window/show` | Brings the window to the front; only from your own computer |
| `GET /window/widget`, `GET /window/hub` | Whether the recording widget and hub are open: `{"available", "widget", "hub", "hub_full_screen"}`, with a `reason` when this Gazelle has no windows (`--no-window`, `--no-tray`). Only from your own computer |
| `POST /window/widget`, `POST /window/hub` | `{"open": true}` or `false` opens or closes the widget or the hub; the hub also takes `{"full_screen": false}` to leave full screen, and `true` to go back. Answers as the `GET` does, or `no_window` (409) without windows. Only from your own computer |
| `GET /remote` | Phones on the network: `allow_phones`; `fixed_by_bind` (the `--bind` address when that decides instead, or null); `listening` (whether a phone can reach Gazelle now); `error` (why it could not listen, or null); the `port`, `addresses` (`ip`, and `primary` for the adapter the internet goes through) and `urls` a phone would use; the paired `phones` (`id`, `name`, `paired_ms`, `last_seen_ms`, `last_address`); the `pairing` running, or null; and the server's `now_ms`. Only from your own computer |
| `PUT /remote` | `{"allow_phones": true}` or `false`. Takes effect at once, with no restart; off closes every phone's connection. Refused with `fixed_by_bind` (409) when `--bind` decides. Only from your own computer |
| `POST /remote/pairing`, `DELETE /remote/pairing` | Start pairing, replacing any code running, or stop it. The answer has the `code` (`XXXX-XXXX`), `expires_ms`, `pair_urls` (`http://<address>:<port>/pair#code=<code>`, the likeliest first) and `qr`, the first of those as a QR code: its `size` and `rows` of `1` (dark) and `0` (light) modules, to be drawn with a quiet zone of four modules. Refused with `phones_off` (409) while no phone could reach Gazelle. Only from your own computer |
| `POST /remote/pair` | From the phone, with no key: `{"code": "...", "name": "..."}`. Answers `{"token": "...", "phone": {...}}` and sets the same token as a cookie, `gazelle_token`, HttpOnly and SameSite=Strict. Refused with `pairing_refused` (403) for a wrong, used or expired code, `too_many_attempts` (429) after five wrong codes from one address, or twenty from all, in a minute, and `too_many_phones` (409) when 32 are paired |
| `DELETE /remote/phones/{id}` | Revoke a phone: its next request is refused and its open WebSocket closes (code 4401). `unknown_phone` (404) when there is no such phone. Only from your own computer |
| `GET /ws` | The WebSocket, below |

For example, to see what setting channel 7 of a Quadro's mix 1 to -10 dB would send, without sending it (`level` is dB of attenuation):

```
curl -X POST "http://127.0.0.1:8420/api/v1/devices/<id>/command/set_mixer?dry_run=true" -H "content-type: application/json" -d "{\"mixer_id\": 0, \"channel\": 7, \"level\": 10}"
```

Field values are integers (booleans become 0 or 1, byte arrays are hex strings), exactly as the device's command defines them; the `commands` route lists every field. The few commands in a device's command set that manage its licence are not offered at all (`unknown_command`): Gazelle reads which emulations and effects a device is licensed for, and changes nothing about it. A field you leave out is sent as its default, not as the device's current value: `set_mixer` above also sets the channel's pan, mute and solo. Commands go straight to the device: nothing checks that a value is sensible for your speakers. Every [safety](02-safety.md) consideration applies, more so.

Error codes: `unknown_device` (404), `unknown_command` (404), `bad_value` (400), `timeout` (504, no answer within 3 seconds), `refused` (502, the device said no), `device_gone` (503), `no_registry` (501, a model Gazelle does not know), `unsupported` (501), `unknown_snapshot` (404), `storage_error` and `protocol_error` (500). Any route can also answer with the remote access codes below: `unauthorized` (401), `not_local`, `remote_off`, `bad_host` and `bad_origin` (all 403).

## Remote access

Every request passes one check before its route, on every address Gazelle listens on.

- **Who is asking** is the connection's own address. Headers such as `X-Forwarded-For` are never believed, so a proxy in front of Gazelle makes every request look like the proxy.
- **The `Host`** must be `localhost`, an IP address, or this computer's own name (with `.local` or not); anything else gets `bad_host` (403). This defeats DNS rebinding, where a web page on another site reaches `127.0.0.1` under a name of its own.
- **An `Origin`**, which browsers send on posts and WebSocket upgrades, must be this server itself, the same host and port as `Host`; otherwise `bad_origin` (403). Programs that are not browsers send none and are not affected.
- **This computer** (`127.0.0.1` or `::1`) needs nothing more.
- **Any other device** is refused with `remote_off` (403) while phones are not allowed and `--bind` is on loopback. Otherwise it may load the app's own files and `POST /remote/pair`; everything else under `/api` needs its key, as `Authorization: Bearer <token>` or the `gazelle_token` cookie, or gets `unauthorized` (401). With a key, the update, window, aggregate and remote routes, and the recording settings, still answer `not_local` (403). The other recording routes do not: a phone may arm, record, stop and disarm.

A script on another machine pairs as a phone does: start pairing on this computer (`POST /remote/pairing`, or the Workspace page), send the code from there, keep the `token`, and send it as a bearer token:

```
curl -X POST http://192.168.1.20:8420/api/v1/remote/pair -H "content-type: application/json" -d "{\"code\": \"YHV8-YRJM\", \"name\": \"Studio script\"}"
curl http://192.168.1.20:8420/api/v1/devices -H "Authorization: Bearer <token>"
```

Keys are 256 random bits, kept by Gazelle only as SHA-256 fingerprints in `%APPDATA%\gazelle\remote.json`, beside the phones setting. The traffic is plain HTTP, so a key crosses the network in the clear; see [what pairing does not protect](05-the-app.md#what-pairing-protects-and-what-it-does-not).

## The WebSocket

`GET /api/v1/ws` upgrades to a WebSocket carrying JSON text frames. From another device the upgrade needs a key like any other request, as a bearer token or the cookie; a revoked phone's socket is closed with code 4401.

- The server's first frame is `{"type": "hello", ...}` with the version, backend, dry run, the devices, any notices, and `phone`: true on a paired phone's connection, which the routes that stay on the computer refuse.
- The hello also carries `recording`, the recorder's state as `GET /recording` answers it.
- Then events: `device_added`, `device_removed`, `cyclic` (a decoded status or meter report: `device_id`, `report_id`, `fields`), `undecoded`, `recording` (`{"type": "recording", "recording": {...}}`, the recorder's whole state whenever it changes: five times a second while armed, with the levels and the pre-roll held), and `lagged` when a slow client missed some.
- To send a command: `{"id": 1, "device_id": "...", "command": "set_mute", "args": {"id": 0, "mute": 1}}`, optionally with `"dry_run": true`. The answer is `{"type": "rpc_response", "id": 1, "result": {...}}` or `{"type": "rpc_error", "id": 1, "error": {...}}`, in the order commands finish.

The web app's own TypeScript client, `gazelle-audio-client` (in `web/packages/client`), wraps all of this with types generated from the command definitions.
