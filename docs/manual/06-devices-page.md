# The Devices page

The Devices page (`#/devices`) is where Gazelle opens. It shows one device, the one selected in the sidebar, in collapsible sections. Which sections are open is remembered while the window stays open.

## Device

- **Name.** Type a name and press Enter (or leave the field). Leave it empty to use the model's name. The name is saved in the workspace and shown everywhere.
- **Model, Family, Id, USB id, Backend** are read-only.
- **Identity** says **Stable** when the device has a serial number, so its names and layouts survive a replug, or **Changes on reconnect** when it does not.

## Status report

Power (**On** or **Standby**), the current **Preset**, and the **Sync source**, live from the device's own status report. A device whose model Gazelle does not know shows a note instead.

## Clock

| Control | What it sets |
|---|---|
| **Source** | Where the clock comes from. Quadro: Internal, ADAT x1, ADAT x2, ADAT x4, S/PDIF, USB. Studio+: Oven, Word clock, ADAT, ADAT x2, ADAT x4, S/PDIF, USB |
| **Sample rate** | 32, 44.1, 48, 88.2, 96, 176.4 or 192 kHz |
| **Measured** | The rate the device reports it is running at, and **LOCKED** or **NO LOCK** (read-only) |
| **Converter** (Studio+) | S/PDIF sample rate conversion: converts the S/PDIF input's rate so that it need not follow the device's clock |

While the device follows an external clock, it takes its rate from that source and ignores the sample rate chosen here.

> **Warning.** A clock or sample rate change is sent the moment you choose it, and interrupts the device's audio and your recording software's stream. These two menus ignore the mouse wheel. See [Clock and sample rate changes](02-safety.md#clock-and-sample-rate-changes).

## Panning law (Quadro)

**Centre attenuation**: how much a signal panned to the centre is lowered in every mix: 0 dB, -3 dB, -4.5 dB or -6 dB. It also affects a mix summed to mono.

## DC coupling (Quadro)

**Inputs** and **Outputs** each have a **DC coupled** switch, for sending and receiving control voltages to and from modular synthesisers.

> **Warning.** DC coupled outputs pass DC to whatever is connected, which can damage speakers and headphones. The switches act on one click. Leave them off for audio.

## Test oscillator

A sine tone sent straight to the outputs, for checking cables and levels. **Left** and **Right** each have a frequency (1 kHz or 440 Hz) and a **Tone** switch; **Level** (0, -6, -12 or -18 dBFS) applies to both.

> **Warning.** At 0 dBFS the oscillator is as loud as the device can go, and **Tone** starts it with one click. Choose -18 dBFS and turn your monitors down first. Never tried on a real device.

## Presets

The device's own five preset slots, stored in the device.

- **1 to 5** recall that preset at once, with no confirmation. The current preset is highlighted.
- **Save into** a slot, then **Save**, then **Confirm save** within three seconds. Saving overwrites what was in the slot.

These are not Gazelle's snapshots: Gazelle cannot read what a preset holds, and nobody has checked yet exactly which settings a preset recall changes. Treat a recall as a change to anything, 48V and the clock included. Preset save and recall have never been tried from Gazelle on a real device.

## Power and brightness

- **Power on** wakes the device from standby with one click. **Standby** needs a second click, on **Confirm standby**.
- **Brightness** sets the front panel's brightness, 0 to 100%. Double-click for 50%.
