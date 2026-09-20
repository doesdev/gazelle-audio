# Gazelle Aggregate

One audio driver that a DAW opens, and several Antelope drivers underneath it.

A DAW opens one low latency audio driver at a time, so recording and playing through more than one
interface in a single session needs a driver that presents itself as one device and opens the
vendor drivers itself. That is this. Phase 0 of this work settled the questions it depended on, at
the real hardware: two Antelope drivers do live in one process, they agree on buffer size and
sample format, and a pair locked by a digital cable holds identical sample counts.

**Nothing here is written for two devices, or for these two models.** Which drivers to open, what
to call their channels, which one drives the callback and how the streams line up all come from a
configuration file. Another Antelope interface is a line in that file, not a change to the code.

**Unreleased, and never run against hardware.** Everything below has been tested against devices
made of data. Nothing in this crate has been registered on a PC or opened by a DAW. It ships with
nothing: it is not installed, not registered and not mentioned in the release notes.

Windows and 64 bit only.

## What it does

- Presents every selected channel of every configured device as one device, in configuration
  order, named so a person can tell them apart: "Quadro 1", "Studio+ 1".
- One device's callback drives the DAW. Every other device's audio crosses a lock free ring
  buffer, one block at a time.
- Reports one input and one output latency figure for the whole aggregate, and holds the nearer
  devices back so that every channel lines up.
- Runs every device at the same rate and the same buffer size, and refuses as a whole if any one of
  them will not.
- Keeps going when a device stops calling back: its inputs read as silence, its outputs are muted,
  and the rest of the aggregate carries on.
- Publishes what it is doing into shared memory, and keeps a short log of what happened, so that
  Gazelle can watch it and a person can read it afterwards. Runs perfectly well with Gazelle
  closed.

## What it will not do

- **It will not resample.** Every device must run at the same rate, and if they are not locked to
  one clock they will drift apart. Lock them with a digital cable and pick the clock source on the
  devices themselves, before opening the DAW.
- **It will not set a clock source.** Which device follows which is a matter for the interfaces and
  their cables. It reports one clock source of its own, "Set on the devices".
- **It will not touch a vendor driver from a callback thread.** Nothing is restarted, reset or
  asked a question from the audio path. A device that goes away is muted and left alone.
- **It will not repeat a buffer.** Everything that goes wrong sounds like silence: an empty ring,
  a device that stopped, a DAW that wrote nothing.
- **It will not open a window.** There is no control panel; what it does is in the file below.

## The configuration file

`%APPDATA%\gazelle\aggregate.json`, read once when the DAW opens the driver. Everything in it is
optional. A field this version does not know is ignored, so a newer file still works in an older
driver. A file that is not valid JSON is refused, with the file named, rather than quietly ignored.

**With no file at all** the driver opens every Antelope driver registered on the PC, in registry
order, takes the first one as the device that drives the callback, exposes every channel, and lines
the devices up.

**Trimming a device.** A driver's reported latency is not always the whole truth, and two devices
can then record a few samples apart even though the aggregate lined them up by the figures they
gave. To measure it: record one source into both devices at once, then compare the two recordings
and see how far apart the same moment lands. Swap the sources between the devices and record again:
an offset that follows the source is the source, and one that stays with the device is the device.
Put that number in `input_trim`, **positive** for the device that records late, and record again to
check it is gone. The sign is worth getting right: a device that records late has a longer path than
its driver admits, so its latency figure goes up, and the aggregate holds the others back to meet
it. Trimming the wrong way doubles the error, which is how this was found (2026-09-21): the measured
difference was about 28 samples at 96 kHz, a trim of -28 made it 53, and +28 is what nulls it.

**To give a device only some of its channels**, add `inputs` or `outputs` with the indexes to keep:
`"inputs": [0, 1, 2, 3]` exposes that device's first four inputs and no others. Leave the field out
to take them all, which is almost always what you want; a device with a long list of playback
channels you never record is the case where it is worth trimming.

A worked example, which is the setup phase 0 measured. Every device here gives all of its channels,
which is what most people want:

```json
{
  "devices": [
    { "key": "Zen Quadro Synergy Core", "name": "Quadro" },
    { "clsid": "{AE4A4452-A316-11E5-A113-080027F6C1F4}", "name": "Studio+" }
  ],
  "callback_master": "Quadro",
  "alignment": "aligned",
  "rate": 96000,
  "buffer_size": 512,
  "stall_after_buffers": 4,
  "recover_after_buffers": 2,
  "ring_buffers": 4
}
```

| Field | What it means | Default |
| --- | --- | --- |
| `devices` | The sub-devices, in the order their channels appear to the DAW. | every Antelope driver found, in registry order |
| `devices[].key` | The name the vendor driver registers itself under. Matched without case, whole or as a part. | |
| `devices[].clsid` | The vendor driver's class id, which is the sure way to name one. Used in preference to `key`. | |
| `devices[].name` | What to call this device's channels. | its registry key |
| `devices[].input_trim` | Samples to add to what this device's driver says its input latency is. A device that records late takes a positive trim, and the others are held back to match it. | 0 |
| `devices[].output_trim` | The same for its outputs. | 0 |
| `devices[].inputs` | Which of its inputs to expose, by the device's own numbering from zero. | all of them |
| `devices[].outputs` | Which of its outputs to expose. | all of them |
| `callback_master` | Which device drives the DAW's callback, by name, registry key or class id. | the first device in the list |
| `alignment` | `"aligned"` or `"lowest_latency"`. | `"aligned"` |
| `rate` | The rate to put every device at when the driver is opened. | whatever the DAW asks for |
| `buffer_size` | The buffer size to offer the DAW as preferred. | the size the devices agree on |
| `stall_after_buffers` | How many of the master's buffers a device may miss before it is called stalled. | 4 |
| `recover_after_buffers` | How many buffers a stalled device must call back for before it is trusted again. | 2 |
| `ring_buffers` | How many buffers of slack each device's ring holds. | 4 |

Every device needs a `key` or a `clsid`. A device named in the file that is not on the PC is a
refusal that names it, rather than a driver that quietly opens with fewer channels than the person
expected.

### Alignment

`"aligned"` is the default and is what a person recording two interfaces at once wants: every
device's inputs line up with every other's, and so do the outputs, so the tracks land together.
Crossing between two callback threads costs one buffer, and the devices do not report the same
latency as each other, so the nearer devices are held back by the difference, in samples. The
device that drives the callback is the one held back most.

`"lowest_latency"` holds nothing back. The master's path is direct and the other devices sit about
one buffer behind it. The latency figures reported are still the longest path, because that is the
honest answer, but the channels do not line up with each other.

## What it publishes, and what it keeps

The driver has to work with Gazelle closed: **its configuration file is its only requirement**.
When Gazelle is running, the two find each other through the shared format in
`crates/gazelle-audio-aggregate-status`, whose README is the exact record, the names and the rules
for who writes what. Not being able to publish is never a reason to fail: with no section the
driver simply runs, exactly as it does with nothing else on the PC.

### Live state, in shared memory

A fixed size record in a named shared section, `Local\gazelle-aggregate-status-1`. Once it is
mapped, writing it is a handful of atomic stores, with no allocation, no lock and no system call,
which is what makes it safe from the audio path. A file written on a timer from an audio process
would be at the mercy of antivirus and of the filesystem's own metadata churn, and would cap how
often anything could be reported.

It holds, at any moment:

- Whether a DAW has the driver open, and whether audio is running.
- The plan in force: the devices and what they are called, their channel counts, the buffer size,
  the rate, the alignment, the two latency figures, and which device drives the callback.
- Per device: whether it is streaming, whether it has stalled, how many blocks it has dropped or
  been short of, and **the gap**, which is that device's sample count minus the master's. Zero
  while the two are locked, and growing in one direction when they are not.
- The callback count, the sample position, and the time of the last block on the machine's own
  clock, which is how a watcher can tell a driver that has stopped from one that is quiet.
- The last refusal, in the same words the DAW was given, and where the configuration came from.

The audio path writes the counters, the gap and the time of the block **every block**. Everything
else is written when it changes. The record is published under a sequence counter, so a reader that
catches a write half way through simply reads it again; and the audio path never waits for anything
here, so a block whose update did not fit is skipped and the next one is written instead.

### Durable events, in a file

`%APPDATA%\gazelle\aggregate-events.log`. One line each, written when it happens and **never on a
timer**, so every line in the file means something:

```
2026-09-20 21:14:07 session-started 40 in, 40 out at 96000 Hz
2026-09-20 21:31:44 stalled Studio+
2026-09-20 21:31:46 recovered Studio+
2026-09-20 22:02:11 session-ended
2026-09-21 09:14:02 refused Studio+ will not run at 96000 Hz, so neither will the aggregate
```

The words are `refused`, `stalled`, `recovered`, `session-started`, `session-ended`, `adopted` and
`reset-asked`. The time is local, because the person reading it is the person it happened to. This
is the half that survives the driver exiting, which is exactly when somebody wants to know why last
night's session would not start. The file is trimmed to its last 400 lines when the driver opens
it, and that is the only time it is ever rewritten.

## Changing its mind while it is loaded

Gazelle writes the configuration file, bumps a counter in the shared record and signals a named
event. A watcher thread inside the driver, **never the audio thread**, wakes up and re-reads the
file.

- **A configuration that does not parse, or that names a device this PC does not have, is refused.**
  What is in force stays in force, the reason goes into the record and the event log, and the DAW is
  left alone holding a plan that works.
- **If nothing is streaming it is taken up there and then**, quietly.
- **If a DAW is streaming, the host is asked to reset**, which is the message a vendor driver sends
  when its own settings change. The DAW puts the driver down and picks it up again, and the new plan
  is in force when it does. Audio drops for a moment, exactly as it does when the buffer size is
  changed.
- **A change that cannot be opened puts back the one that worked**, so a refusal leaves a working
  driver rather than none.

Editing the file by hand still works the way it always did: it is read when a DAW opens the driver.
The watcher is only there so that a change can also land while the driver is already loaded.

## Registering it

Registration writes to `HKEY_LOCAL_MACHINE`, so it needs administrator rights. It is a separate,
optional step, and it touches nothing that Gazelle itself installs: Gazelle's own install is per
user and goes nowhere near these keys.

Build it:

```
cargo build -p gazelle-audio-aggregate --release
```

That leaves `target\release\gazelle_aggregate.dll`. Put it somewhere it will stay, because the
registry records where it is, and a DAW will load it from there every time. Then, from an
**elevated** command prompt:

```
regsvr32 "C:\path\to\gazelle_aggregate.dll"
```

To undo it, from an elevated prompt again:

```
regsvr32 /u "C:\path\to\gazelle_aggregate.dll"
```

Registering writes exactly two trees, and unregistering removes both:

- `HKLM\SOFTWARE\ASIO\Gazelle Aggregate`, with `CLSID` and `Description`.
- `HKLM\SOFTWARE\Classes\CLSID\{F18C80B4-2DE1-43B8-AC84-19DDC22EBFC8}`, with the DLL's path under
  `InprocServer32` and `ThreadingModel` set to `Apartment`.

That class id never changes: it is how a DAW, and every saved project that has chosen this driver,
finds it again.

## Licence

MIT, like the rest of this workspace. The interface this driver implements is declared by hand in
`gazelle-audio-stream-abi` from its public shape; no SDK is used, included or needed, and this
crate builds with none present.

The driver's name is "Gazelle Aggregate". Steinberg's trademark rules forbid their technology's
name in a product's name, so neither the driver nor either crate is named after it.
