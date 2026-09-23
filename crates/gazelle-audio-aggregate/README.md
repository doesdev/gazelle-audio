# Gazelle Aggregate

One audio driver that a DAW opens, and several Antelope drivers underneath it.

A DAW opens one low latency audio driver at a time, so recording and playing through more than one
interface in a single session needs a driver that presents itself as one device and opens the
vendor drivers itself. That is this.
[Phase 0](../gazelle-audio-aggregate-probe/README.md) of this work settled the questions it
depended on, at the real hardware: two Antelope drivers do live in one process, they agree on buffer size and
sample format, and a pair locked by a digital cable holds identical sample counts.

**Nothing here is written for two devices, or for these two models.** Which drivers to open, what
to call their channels, which one drives the callback and how the streams line up all come from a
configuration file. Another Antelope interface is a line in that file, not a change to the code.

**Unreleased, and run at the real hardware a great deal.** It has been registered on a PC and has
recorded and played through a Quadro and a Studio+ in one DAW session, and the measurements quoted
below were taken there, on 2026-09-20 and 2026-09-21. Its tests still run against devices made of
data and never touch hardware. Gazelle's Aggregate page sets it up, watches it and registers it
([the manual's chapter](../../docs/manual/13-aggregate-page.md) is the person's guide), and the
release notes under Unreleased describe that page. From the next release the DLL travels inside
Gazelle's own executables and is written out beside them on install and update; no published
release carries it yet.

Windows and 64 bit only.

## What it does

- Presents every selected channel of every configured device as one device, in configuration
  order, named so a person can tell them apart: "Quadro 1", "Studio+ 1", or "Vocal mic (Quadro 1)"
  when the file gives that channel a name of its own.
- One device's callback drives the DAW. Every other device's audio crosses a lock free ring
  buffer, one block at a time.
- Reports one input and one output latency figure for the whole aggregate, and holds the nearer
  devices back so that every channel lines up.
- Measures, at the start of every session, where each interface's capture actually started, over the
  digital cable that already locks them together, and puts the session back in the state its trims
  were measured in, so the trims hold in every session rather than only in that one. See "The
  phase" below.
- Runs every device at the same rate and the same buffer size, and refuses as a whole if any one of
  them will not. A driver that says yes to a rate is held to it: see "Holding a rate" below.
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
gave. `input_trim` and `output_trim` correct for it, **positive** for the device that records late.
The sign is worth getting right: a device that records late has a longer path than its driver
admits, so its latency figure goes up, and the aggregate holds the others back to meet it. Trimming
the wrong way doubles the error, which is how this was found (2026-09-21): the measured difference
was about 28 samples at 96 kHz, a trim of -28 made it 53, and +28 is what nulls it.

The way to arrive at a trim is to measure it: the Aggregate page's **Measure** plays a click out of
one interface into every interface and reads the difference out of the aggregate's own buffers,
which is [`gazelle-audio-calibrate`](../gazelle-audio-calibrate/README.md). The trims it offers
are written with one button, and for an interface with a `phase` each one carries the reference
that goes with it. A trim can still be found by hand,
by recording one source into both devices and comparing, but a trim written by hand has no
reference beside it, so it holds only in sessions that happen to start in the state it was
measured in. See "The phase" below for why that matters.

**Naming the channels.** A device's channels are its USB audio channels, the ones its vendor driver
publishes: input *k* is its USB record channel *k* and output *k* its USB playback channel *k* (16
each way on the Zen Quadro Synergy Core, `USB A REC` and `USB 1 PLAY`; 24 each way on the Zen Studio+,
`USB REC` and `USB PLAY`). A channel is called "Quadro 1" unless the file says otherwise, which
tells you which interface and which channel but nothing about what it carries. `input_names` and
`output_names` give a channel a label, keyed by the device's own channel number from zero, the
same numbering `inputs` and `outputs` use:

```json
{ "key": "Zen Quadro Synergy Core", "name": "Quadro",
  "input_names": { "0": "Vocal mic", "2": "DI" },
  "output_names": { "0": "Main L", "1": "Main R" } }
```

That channel then appears in the DAW as "Vocal mic (Quadro 1)": the label first, and the interface
and channel still there in brackets, so a patch you have forgotten is one glance away. The interface
carries 31 characters, and when the label and the bracketed automatic name together will not fit,
the label alone is what is kept, because half a bracket reads as a name that was cut off. A name for a channel the
device does not expose is simply unused, and a name that is empty or only spaces is the same as not
giving one.

**Gazelle writes these names, and keeps them in step.** When Gazelle writes this file, every name in
it is Gazelle's: each device's `name` is the person's own name for the device in Gazelle, else its
model's short form ("Quadro", "Studio+", and "Quadro 2" for a second one), `callback_master` is that
same name, and every channel has a label: an input's is what Gazelle's routing sends to its USB record
channel, and an output's is where the routing sends its USB playback channel ("Monitor L", "Click in
Cue", "Not routed"), with a name the person typed on the Aggregate page put over it. When the routing changes
through Gazelle, it writes the file again and the driver takes the new names at its next reset, as
it takes any other change. A long device name leaves no room for the bracketed part, and the DAW
then shows the label alone; a short name for the device in Gazelle keeps it.

**To give a device only some of its channels**, add `inputs` or `outputs` with the indexes to keep:
`"inputs": [0, 1, 2, 3]` exposes that device's first four inputs and no others. Leave the field out
to take them all, which is almost always what you want; a device with a long list of playback
channels you never record is the case where it is worth trimming.

A worked example, which is the setup phase 0 measured. Every device here gives all of its channels,
which is what most people want, and the Studio+ is measured over the S/PDIF cable that already
clocks it:

```json
{
  "devices": [
    { "key": "Zen Quadro Synergy Core", "name": "Quadro" },
    { "clsid": "{AE4A4452-A316-11E5-A113-080027F6C1F4}", "name": "Studio+",
      "phase": { "master_output": 8, "input": 16 } }
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
| `devices[].name` | What to call this device's channels. Gazelle writes its own name for the device here. | its registry key |
| `devices[].input_trim` | Samples to add to what this device's driver says its input latency is. A device that records late takes a positive trim, and the others are held back to match it. | 0 |
| `devices[].output_trim` | The same for its outputs. | 0 |
| `devices[].phase` | How this interface's capture phase is measured at the start of a session, as `{"master_output": n, "input": n, "reference": n}`: the output of the interface that drives the callback the cable leaves from, and this interface's own input it arrives on, both by the devices' own channel numbering from zero, and the phase measured when its input trim was measured, which a calibration run writes. Not for the interface that drives the callback. See "The phase" below. | not measured |
| `devices[].inputs` | Which of its inputs to expose, by the device's own numbering from zero. | all of them |
| `devices[].outputs` | Which of its outputs to expose. | all of them |
| `devices[].input_names` | A label for each of its inputs, keyed by the device's own channel number from zero: `{"0": "Vocal mic"}`. The channel is then "Vocal mic (Quadro 1)". Gazelle writes one for every channel. | the automatic name |
| `devices[].output_names` | The same for its outputs. | the automatic name |
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

### Holding a rate

A driver can answer yes to a rate and not move. It can keep the old one until it is closed and
opened again, move only once its buffers are made, or ask to be reset before it will. A driver
that remembers its rate puts the interface back to it whenever it is opened, and every interface
clocked from that one follows, so a yes that was not meant is a session at the wrong rate.

So after every rate it asks for, at opening (a `rate` in the file) and whenever a DAW or a
calibration run asks for one, the aggregate reads the rate back:

1. **It read back right**: nothing more is done, and nothing is written. This is the usual case,
   and the Antelope drivers' case: on 2026-09-22 the Quadro's driver took a rate the first time it
   was asked, and remembered it for the next opening. None of the steps below were needed for it.
2. **It holds another rate, or asked to be reset**: the driver is closed, opened again in the same
   slot, and asked again, up to two times.
3. **It still holds it**: it is not refused yet, because some drivers only move once their buffers
   are made. After every device's buffers are made and before anything starts, every device is
   read once more. One that moved is asked again; one that still holds, or asked to be reset, is
   closed and opened again with its buffers made again, up to two times.
4. **It still holds it after that**: `createBuffers` is refused, naming the driver, the rate it was
   asked for and the rate it holds, and every buffer is let go of.

If a driver refuses a rate outright, every device that already moved is put back on the rate it
was on itself, held there the same way. A driver that cannot be opened again at all leaves a hole,
so the aggregate lets every device go rather than answer for one it no longer has.

Before a device's buffers belong to a running stream, a reset request or a rate change it sends is
kept for the aggregate, which is still opening and acts on it itself. Once they do, the DAW is the
host: the request goes up to the DAW exactly as it came, and nothing is closed or opened from
inside a callback. A rate asked for while a DAW has the buffers made cannot close anything under
them either, so a driver that did not move then is left to the DAW: the aggregate asks the DAW to
reset, and holds the driver to the rate when the DAW comes back for its buffers.

Each step that was needed is one `rate` line in the event log, in words:

```
2026-09-22 20:41:10 rate USB Box's driver held 44.1 kHz after being asked for 96 kHz; reopened it and it took 96 kHz
2026-09-22 20:41:12 refused USB Box's driver still held 44.1 kHz after being asked for 96 kHz and reopened twice, so the aggregate did not open
```

### Alignment

`"aligned"` is the default and is what a person recording two interfaces at once wants: every
device's inputs line up with every other's, and so do the outputs, so the tracks land together.
Crossing between two callback threads costs one buffer, and the devices do not report the same
latency as each other, so the nearer devices are held back by the difference, in samples. The
device that drives the callback is the one held back most.

`"lowest_latency"` holds nothing back. The master's path is direct and the other devices sit about
one buffer behind it. The latency figures reported are still the longest path, because that is the
honest answer, but the channels do not line up with each other.

## The phase

**Two interfaces do not start their capture in the same place, and where they start changes every
time.** Measured at the devices on 2026-09-21:

- Within one session the two record a fixed number of samples apart. The spread of a measurement is
  0.00 samples, and the same fractional part comes back every time.
- Between sessions that number moves in steps of 32 samples. Seen at 64, 128 and 256 sample
  buffers, with the vendor drivers' Safe Mode on and off.
- The drivers report identical latency figures on every open, so the aggregate's own padding is the
  same every run and is not the cause.
- The aggregate moves a follower's audio only in whole blocks, so an offset smaller than a block was
  already in what the drivers handed over. It is each interface's capture pipeline settling on a
  different phase when its stream starts.
- **The phase belongs to an interface's capture pipeline as a whole.** Witness channels settled it:
  in six runs an interface's analogue input and its S/PDIF input moved together every time, their
  difference constant at 47.29 samples to two decimals while the absolute figures jumped by 96 and
  160 samples.

That last one is what makes this worth doing: a phase measured on the digital input corrects the
analogue inputs too, so one measurement over the cable that already locks the interfaces together
lines up everything they record.

### Three numbers

They are different things, and all three apply. Getting them mixed up is how somebody would correct
the same thing twice, or line a session up to the wrong state.

| | The trim | The reference | The phase |
| --- | --- | --- | --- |
| What it is | The constant difference between what an interface's driver reports and what its converters really do | The phase this driver measured in the session the trim was measured in | Where the interface's capture pipeline happened to start this time |
| How it is found | Measured by a calibration run, with a click | Measured by the same calibration run, in the same session, over the digital cable | Measured by the driver, over the digital cable, at the start of every session |
| How often it changes | When the interfaces are measured again | With the trim, and only with it | Every session, in steps of 32 samples |
| Where it lives | `input_trim` in this file | `phase.reference` in this file, beside the cable | Nowhere: it is measured again each time, and written to the event log afterwards |

**A trim is only true of the state its session was in.** So every session is first put back into
that state, by **the reference minus the phase**, exactly, and then the trim applies as it always
has. The interface is held back by that much when it is positive, and the others are held back to
meet it when it is negative.

### What the hardware said, and the rule it overturned

The first version of this measured the phase and then asked whether **the measured value itself**
was near a whole multiple of 32 samples, refusing it if not and rounding it to one if so. It ran
against the two real interfaces for the first time on 2026-09-21. It heard its signal every time,
and refused every measurement as off the grid. Beside the click lag the calibration measured in the
same runs:

| run | click lag (samples) | phase measured | lag minus phase |
| --- | --- | --- | --- |
| 1 | +60.4 | -84 | 144.4 |
| 2 | -3.6 | -148 | 144.4 |
| 3 | -3.6 | -148 | 144.4 |
| 4 | -2.6 | -147 | 144.4 |
| 5 | -162.6 | -307 | 144.4 |
| 6 | -162.6 | -307 | 144.4 |

Three earlier runs, -99.6 against -244 and -162.6 against -307 twice, came to 144.4 as well.

So **the measurement tracks the real offset between the interfaces exactly**, to the sample, in
every state the hardware settled into, including a one sample wobble (-3.6 and -148 in one run,
-2.6 and -147 in the next).

**The rule was wrong, for two reasons.** The measured value carries a large constant of its own (the
digital cable's own path, the difference between the interfaces' converters, where the detector
takes its reading), so it is never near a multiple of 32 and asking whether it is means nothing;
only the **change** between sessions moves in steps. And those steps are not exactly 32 either:
changes of 63, 64, 159 and 160 were all seen, which is steps of 32 plus the wobble. Rounding to the
grid would have thrown away the sample of wobble the measurement had in fact caught. That rule is
gone, and nothing is rounded.

### How it is measured

Give the interface a `phase` and the driver measures it:

```json
{ "key": "ZenStudioTB ASIO Driver", "name": "Studio+", "input_trim": 60,
  "phase": { "master_output": 8, "input": 16, "reference": -84 } }
```

`master_output` is the output of the interface that drives the callback that the cable leaves from,
and `input` is this interface's own input that it arrives on. Both are the **devices' own** channel
numbers from zero, the same numbering `inputs` and `outputs` use, so the setting survives a change
to which channels are exposed. It is the cable the aggregate already needs for clocking: nothing new
has to be plugged in. `reference` is written by a calibration run beside the trim it measured; it
is not something to type in.

- **The path is configured, never guessed.** An interface with no `phase` is not measured, and its
  session runs exactly as it did before this existed.
- **Those two channels are the driver's, not the DAW's.** They are opened at the interfaces, because
  the measurement needs them, and both are kept out of the channel list a DAW is given. Nothing a
  DAW plays can land on the measurement channel, and the measurement can never be heard on a channel
  anybody is using.
- **It happens once, at the start of the session**, in the first fraction of a second, out of the
  audio path's own blocks. The signal is the driver's own: a burst of four samples about 42 dB below
  full scale, over a digital cable, where nothing is gained by making it louder.
- **Each interface is measured on its own cable, against the one that drives the callback.** Three
  interfaces are three independent measurements; nothing here assumes two.
- **`"lowest_latency"` measures nothing**, because it holds nothing back on purpose and a
  measurement would have nothing to do.

What is measured is not the absolute distance but how far the capture landed from where the drivers'
own figures put it: the master's reported output latency, this interface's reported input latency,
and the one block the aggregate's own ring costs. Those are the figures **as the drivers report
them, without the trims**, so that writing a new trim never moves the phase a session measures and
the reference stays comparable with every session after it.

### Where the reference comes from

**A calibration run supplies it.** A run measures the click lag and the phase in the same session,
so it is exactly the thing that can pair a trim with its reference.

- **During a measuring run the phase is measured and not applied.** The lag a run hears becomes the
  trim, and the phase heard beside it becomes the trim's reference, so both have to be the raw
  figures of one session. A run that lined itself up first would measure a trim on top of a correction made from
  the old reference, and the pair it wrote down would describe a state no session is ever in.
- **The trims a run offers carry the reference beside them**: the new trim, and the phase that was
  measured while it was measured. Writing the trims writes both, and a trim offered with no phase
  heard beside it takes the old reference out rather than leaving it next to a trim it does not
  belong to.
- **Only input trims have one.** The phase is the capture pipeline's; nothing lines the outputs up
  by it.

What a run hands back for this, field by field, is in
[the calibrate crate's README](../gazelle-audio-calibrate/README.md#watching-a-run-and-what-it-leaves-behind).

### Checking it

A **check** is the other kind of run: the same clicks, but the session is lined up exactly as a
DAW's would be, the phase applied from its reference and the trims in force, so the click lag it
hears is what a recording would get. It offers no trim, because a lag heard on top of a correction
is a verdict on the trim, not a new one. It is **Check**, beside Measure, on the Aggregate page.

It is also how this was shown to work. At the devices on 2026-09-21, with a reference set, eight
checks in a row read a click lag of 0.02 samples, although the sessions they ran in had started in
three different states.

### With no reference

An interface with a `phase` and no `reference` is measured at the start of every session and then
**left exactly where the drivers' figures put it**. Without a reference there is nothing to line it
up to, so nothing is applied, and the event log says that the interfaces need measuring once. After
one calibration run every session is lined up.

### What it refuses

A measurement the driver will not use is **a refusal to correct, not a correction of zero**. The
session then runs exactly as it does today, on the figures the drivers reported, and the reason goes
into the event log where somebody will find it in the morning.

- **Nothing arrived** on the measurement channel inside the window. The cable is out, or the
  channels in the file are not the ones it is on.
- **The change from the reference was nowhere near a whole number of 32 sample steps.** That is the
  only way the hardware moves, so a change that is not is a measurement of something else. Up to 4
  samples either side of a whole number of steps is allowed, which covers the one sample of wobble
  the hardware showed; the correction itself is still the exact difference.
- **The correction was further than the room the delays keep for it**, which is 512 samples either
  way.

### What it does with it

What was measured minus the reference is added to that interface's input path (its reported input
latency plus its trim) and the padding is worked out again, which is exactly what this driver
already does with a reported latency and a trim. The effect is to hold the interface back by the
reference minus what was measured: an interface that turns out to be **early** against its
reference is **delayed**; one that is **late** means the others are held back to meet it, and the
aggregate's own input latency grows by as much. The live record's `phase_applied` is that added
figure, so in the example below it reads -64 for an interface held back by 64 samples.

The driver does not interrupt the DAW to tell it that. It answers the figure in force whenever it is
asked for it again, and it publishes it in the live record, but a DAW that read the latency before
the audio started is still holding the figure it was given. Until it asks again the tracks line up
with each other and sit up to the corrected amount later than that DAW thinks they do, which for a
correction of a few steps of 32 samples is a few milliseconds at most.

**Changing the alignment after the DAW has had a block is a discontinuity**, and this is not free:
the delays that were holding audio back drop what they were holding, so there is a click on the
channels that moved. It happens in the first fraction of a second of a session, before anything is
being recorded, and it is the price of lining the interfaces up by what they actually did rather
than by what their drivers said they would do. Only the inputs move: the phase is a capture
pipeline's, and the outputs are left where the reported figures put them.

### What it says afterwards

The live record carries, per interface, what the measurement came to, what was applied and which of
the states above it is in, beside the gap: `applied`, `no_reference`, `measured_only` for a
calibration run that is measuring a trim, or one of the refusals (`not_heard`, `off_the_grid`,
`too_far`). That goes the moment the DAW closes, so a line goes in the
event log as well, and that line is the only thing that survives the session:

```
2026-09-21 21:14:09 phase Studio+ was measured at -148 samples from where its driver's figures put it, against -84 when its trim was measured, so it was held back by 64 samples to put it back where its trim holds
2026-09-21 22:40:31 phase Studio+ was measured at -148 samples from where its driver's figures put it, and was not lined up: there is no phase from the session its trim was measured in to line it up to. Measure the interfaces once on the Aggregate page and every session after that is lined up. The session ran on the figures the drivers reported.
2026-09-22 09:02:41 phase Studio+ was not lined up: nothing arrived on its measurement channel. Check the cable and the channels the file names, or take the phase setting out. The session ran on the figures the drivers reported.
```

## What it publishes, and what it keeps

The driver has to work with Gazelle closed: **its configuration file is its only requirement**.
When Gazelle is running, the two find each other through the shared format in
[`gazelle-audio-aggregate-status`](../gazelle-audio-aggregate-status/README.md), whose README is the
exact record, the names and the rules for who writes what. Not being able to publish is never a reason to fail: with no section the
driver simply runs, exactly as it does with nothing else on the PC.

### Live state, in shared memory

A fixed size record in a named shared section. Its name, its layout, and why it is shared memory
rather than a file are in
[the status crate's README](../gazelle-audio-aggregate-status/README.md#the-names); the short of it is that writing it is a
handful of atomic stores, which is what makes it safe from the audio path.

It holds, at any moment:

- Whether a DAW has the driver open, and whether audio is running.
- The plan in force: the devices and what they are called, their channel counts, the buffer size,
  the rate, the alignment, the two latency figures, and which device drives the callback.
- Per device: whether it is streaming, whether it has stalled, how many blocks it has dropped or
  been short of, and **the gap**, which is that device's sample count minus the master's. Zero
  while the two are locked, and growing in one direction when they are not.
- Per device, beside the gap: what this session's phase measurement came to, what was applied
  because of it, and which of its states it is in. See "The phase" above.
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
2026-09-20 21:14:07 phase Studio+ was measured at -148 samples from where its driver's figures put it, against -84 when its trim was measured, so it was held back by 64 samples to put it back where its trim holds
2026-09-20 21:31:44 stalled Studio+
2026-09-20 21:31:46 recovered Studio+
2026-09-20 21:47:02 glitched Studio+ dropped a block, the first this session has lost: it was handing them over faster than they could be taken
2026-09-20 22:02:11 session-ended ran for 47 minutes 12 seconds. Quadro lost nothing; Studio+ dropped 3 blocks and missed 1 block
2026-09-21 09:14:02 refused Studio+ will not run at 96000 Hz, so neither will the aggregate
2026-09-22 20:41:10 rate USB Box's driver held 44.1 kHz after being asked for 96 kHz; reopened it and it took 96 kHz
```

This is the half that survives the driver exiting, which is exactly when somebody wants to know why
last night's session would not start. The line format, the full list of words and the trimming rule
are in [the status crate's README](../gazelle-audio-aggregate-status/README.md#the-event-log).

### Blocks lost, and where they are written down

The live record's counters go the moment the DAW closes, so **a session's line says what it lost**:
how long it ran, and per interface how many blocks were dropped and how many were not there in
time. That one line is the difference between a clean night and a bad one when somebody reads the
file the next morning, and it is what makes "was anything dropped while I was recording?" a
question the driver can answer after the fact.

Two lines, not a stream of them. The **first** block a session loses gets a `glitched` line of its
own, because the moment it first went wrong is usually what a person is looking for; every one
after it is counted and nothing more, and the totals go in the session's own line. A line per lost
block would bury the file at exactly the moment it has to be readable.

**None of that happens on the audio path.** The rings count a lost block with one relaxed add, and
that is all the callback does. Turning a count into a line of the log is the watcher thread's work,
and the session's totals are written on the thread the DAW stopped the driver from.

**The first few blocks of a stream are not counted, and this is why.** The two interfaces'
callbacks are woken by two converters, and where one lands inside the other's block is not settled
when a session starts. While the interface that drives the callback reaches the ring first there is
nothing in it for it, and the block of silence it takes is what puts the other one a block ahead,
which is where it stays for the rest of the session and which is exactly the one buffer the
aggregate's arithmetic already allows a device that crosses a ring. Nothing is missing from a
recording: it is a few milliseconds into a session, before anybody is recording, and what follows
the silence is every sample the interface captured. Measured against both interfaces on 2026-09-20,
at 256 samples: the interface that drives the callback lost nothing ever, dropped nothing ever; the
follower took one of these in a five second session, none in a thirteen second one and one in a
twenty five second one. It does not grow with the length of a session, because it is not a rate.
Counting it would have put a lost block on the record of every clean session.

**A block of silence after that is a real one** and is counted, reported and left audible as what it
is. By then the ring has a whole buffer of room, so an empty one means the block was not made in
time, and it costs that interface another buffer of path for the rest of the session on top of the
click. The same forgiveness starts again for an interface coming back from a stall, whose ring was
thrown away and whose two ends have to find each other again: that is already reported as a stall,
and counting the refill would be reporting it twice under another name.

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

The Aggregate page in Gazelle offers to do it, behind Windows' own prompt, and shows the same
command for anyone who would rather type it. It looks for the DLL beside Gazelle, in the build
output a developer has just made, and in Gazelle's own folder, and says which copy a registration
points at.

**In a release there is nothing to build.** A release's executables carry the DLL inside them
and write it out beside themselves on install and on the first start after an update, so the
copy the page offers to register is already in Gazelle's folder. Uninstalling offers to remove
the registration. How it is embedded, and why a driver a DAW has open is renamed aside rather
than overwritten, is in [`docs/releasing.md`](../../docs/releasing.md).

For a copy of your own, build it:

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

MIT, like the rest of this workspace. **No Steinberg code, and no SDK.** The interface this driver
implements is declared by hand in
[`gazelle-audio-stream-abi`](../gazelle-audio-stream-abi/README.md) from its public shape. Nothing
from the SDK is included, copied or compiled, it is not needed to build this crate, and it must
never be committed. That is why this driver can be MIT at all; the stream-abi README tells the
whole of it.

The driver's name is "Gazelle Aggregate". Steinberg's trademark rules forbid their technology's
name in a product's name, so neither the driver nor either crate is named after it.
