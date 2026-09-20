# Troubleshooting

When something sounds wrong, silence it first (speakers down, then Mute or Hard mute), and find out why second.

## Antelope's service is holding the devices

**Signs:** no devices in the sidebar; a notice saying "Antelope's Manager Service is running and holds the interfaces, so Gazelle cannot open them"; the tray says **Warning: Antelope's service is holding the devices**.

**Fix:** stop the service `Antelope-Manager-Service`, in Services or with `Stop-Service -Name Antelope-Manager-Service` in an administrator PowerShell (see [Getting started](03-getting-started.md#stop-antelopes-service)). The devices appear within a few seconds. Set the service's startup type to Manual if it comes back after every restart.

## No devices appear

With the service stopped and still no devices, a notice says "No Antelope interface is attached, so there is nothing to control yet."

- Check the interface is on, and connected by **USB**. Gazelle does not use Thunderbolt.
- Choose **Rescan devices** in the tray menu. Gazelle also looks every two seconds.
- Another program may have the device open. Close Antelope's panels and launcher.
- Look at the header: if it says **LOOPBACK**, this Gazelle was started with `--backend loopback` and shows the emulator, never hardware.
- A device of a model Gazelle does not know shows as "Unknown model" with only its Devices page.

## Windows SmartScreen warns about the download

Gazelle's programs are not code-signed yet (signing is planned), so Windows may show "Windows protected your PC" the first time. If you downloaded the zip from the project's releases page, choose **More info**, then **Run anyway**. If you got it anywhere else, do not.

## The port is in use

Gazelle listens on `127.0.0.1:8420`. Starting it a second time normally brings the running copy's window to the front and exits. If something else holds the port, Gazelle stops with a message saying the address "is in use" and that what is listening "is not Gazelle". Start Gazelle on another port:

```
gazelle-audio-server.exe --bind 127.0.0.1:8421
```

A Start on boot entry remembers the address it was created with.

## There is no window

The window needs Microsoft's WebView2 runtime, which Windows 11 includes. Without it Gazelle carries on without a window: the tray's **Open Gazelle** opens it in your browser instead, and the log says why. A build made without `--features window` never has a window.

## The page says the interface was not built

A Gazelle built from source without first running `corepack pnpm -C web build` serves a notice instead of the app. Build the web interface, then build Gazelle again.

## "Gazelle cannot reach its server", or "The server is not connected"

The page lost its connection to the Gazelle program: it was quit, it crashed, or (from another computer) the network dropped. Controls are disabled until it reconnects, and nothing you did meanwhile is sent later. Start Gazelle again, then **Try again** or reload.

## What Gazelle shows does not match the device

Gazelle reads each mix, routing group and effect chain once and then assumes it is the only program changing them. If the vendor's software, or another Gazelle page, changed them since, reload the page, or use **Read from device** on the Routing and Effects pages. Volumes, gains, mutes and meters come from the device's own status reports and are always current.

## A command failed

A notice names the command and why:

| Notice says | Meaning |
|---|---|
| timed out | The device did not answer within 3 seconds. It may be busy, in standby, or unplugged |
| refused | The device answered no. Your model or firmware may not support that command |
| is no longer reachable | The device went away: unplugged, or powered off |

## Logs

Gazelle writes a log while it runs from the Start Menu or the tray: `%LOCALAPPDATA%\gazelle\logs\gazelle.log`. The tray's **Open log folder** opens it. The file is capped at 2 MB; older logs are kept as `gazelle.1.log` to `gazelle.4.log`, about 10 MB in all.

- Runs with `--no-tray` write no log file unless given `--log-dir <folder>`.
- For more detail, set the environment variable `RUST_LOG` before starting Gazelle from a terminal. In PowerShell: `$env:RUST_LOG = "gazelle_audio_server=debug"`. The default is `gazelle_audio_server=info,tower_http=info`.

## Reporting a problem

Report problems at https://github.com/doesdev/gazelle-audio/issues. Say which interface and which Windows version, Gazelle's version (`gazelle-audio-server.exe --version`), what you did and what happened. The log helps, as does the "last sent" line from the page, which shows the exact bytes of the last command; turn the explain mode on in the header to see it. Look through the log before you attach it: it names your devices' serial numbers and your folders.
