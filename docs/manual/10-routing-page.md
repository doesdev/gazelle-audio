# The Routing page

The Routing page (`#/routing`) is the device's routing matrix: which source feeds each channel of each destination.

![The Routing page for a Quadro: sources above, one row per destination below.](../images/routing-quadro.png)

## Reading it

- **Sources** are grouped as the device groups them (PREAMP, USB PLAY, ADAT IN, AFX OUT, the mixes' outputs and so on), each group in its own colour.
- **Destinations** are one row per group: the analogue outputs, the recording channels, the digital outputs, the effect chain inputs (AFX IN) and the mixer inputs. Each cell shows its source's short name, a dash when muted, or **?** before it has been read. Hovering shows the full route, for example "HP1 1 ← PREAMP 3".
- The mixer input rows are read-only here: channels on the [Mixer page](09-mixer-page.md) set them.

## Changing routes

1. Click a source to select it; Shift-click another in the same group to select a run of them.
2. Click a destination cell, or drag the selected sources onto it. A run fills that cell and the ones after it, up to the end of the row.

Or select a cell and press Delete (or Backspace) to mute it. **Mute row** mutes a whole destination. Each change is sent at once, one destination group at a time; Gazelle reads the group from the device again before writing, so routes made elsewhere are kept.

> **Warning.** A source routed straight to an output plays at its full level, with no fader in the way, and routing an output back into an input can make a feedback loop. Change routing with monitors low. See [Safety](02-safety.md#feedback-loops).

**Read from device** reads every destination again, for when something else may have changed the routing.
