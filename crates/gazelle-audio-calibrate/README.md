# Gazelle Calibrate

Measures how far apart two interfaces in Gazelle Aggregate actually record, and works out the trim
that cancels it.

Gazelle Aggregate lines its interfaces up from the latency figures their drivers report. Those
figures are close, but they are not exact, so two interfaces can still land a few tens of samples
apart on the same sound. The cure is `input_trim` and `output_trim`, a number per interface in
`%APPDATA%\gazelle\aggregate.json`. Until now, arriving at one meant recording something, staring
at two waveforms, and guessing. This measures it instead.

**It measures through the aggregate, not around it.** It opens the aggregate itself, with the
configuration the person is actually using, at their rate and buffer size, drives it exactly as a
DAW does, plays a click and reads the aggregate's own input buffers. Everything the aggregate does
to line the interfaces up has already happened by then, so whatever offset is left in that buffer
**is** the error, in the same coordinates a trim is written in. Adding it to the trim cancels it.
Nothing new has to touch the interfaces: the aggregate already opens them. The one thing a
measuring run leaves out on purpose is the per session phase correction, because the phase it hears
becomes the new trim's reference; see "Watching a run" below.

**This one really does drive the hardware.** It plays audio out of a real interface into whatever
is plugged into it. The click is quiet by default (about -20 dBFS) and can never be louder than
-6 dBFS whatever it is asked for, but turn the monitors down before the first run all the same.

**Unreleased.** Gazelle's server is built with it: it is what the Aggregate page's **Measure** and
**Check** run, and the release notes under Unreleased describe them. No published release carries
it yet. Its tests never touch hardware: the arithmetic is tested against signals made of data, and
the whole session is tested by driving the real aggregate against sub-devices made of data with a
delay built into one of them. It has been run at the real interfaces a great deal, and the findings
below that carry a date were measured there.

Windows only.

## The cabling

One output channel per interface being measured, **all of them on one interface**, each cabled into
one chosen input channel of its own interface. Two interfaces is two cables. A third interface is a
third cable, from a third output of the same interface. Nothing here assumes two.

Both copies of the click leave the same interface on the same sample, so the difference between
where they land is the difference between the interfaces and nothing else.

**Every cable end is named by interface and that interface's own channel number**, both counted
from zero: `Pick::new(1, 0)` is the second interface's first input. It is never the aggregate's own
channel number, because that depends on how many channels each interface's driver really has and on
which of them the setup keeps out or keeps for the phase measurement, and only the drivers know the
first of those. The run opens the drivers anyway, so it is the run that translates, once they are
open and before a buffer is made, and a count made anywhere else can no longer put a cable on the
wrong interface. (The Zen Quadro Synergy Core's driver has 16 inputs and 16 outputs, and the Zen
Studio+'s 24 and 24, measured at the hardware on 2026-09-20.)

### Measuring the inputs

One interface plays, every interface records:

| From | To |
|---|---|
| Quadro out L | Quadro in 1 |
| Quadro out R | Studio+ in 1 |

The Quadro is what the others are measured against here, because it is first in the file, and its
own trim never moves. (That is not the phase reference, which is another thing entirely; see
"Watching a run" below.) What comes out is the residual error between the two interfaces'
**inputs**, and it goes in `input_trim`.

### Measuring the outputs

The same rig with the cabling mirrored: every interface plays, one interface records.

| From | To |
|---|---|
| Quadro out L | Quadro in 1 |
| Studio+ out L | Quadro in 2 |

Now the inputs are what the two copies have in common, so what comes out is the residual error
between the interfaces' **outputs**, and it goes in `output_trim`. It is one code path and one set
of arithmetic: only the direction changes.

## Witnesses: extra channels to listen in on

A run may carry **witnesses**: extra input channels, named the same way as the cables, that are
recorded and reported alongside everything else, and that **take no part in any trim**.

A witness may be any input channel the aggregate has, *including a second input on an interface
that is already being measured*, which is the whole reason it exists. One input per interface is
what the trim arithmetic means: the difference between two interfaces' channels is the difference
between the interfaces. A second input on the same interface is not a difference between
interfaces, so it can never be a trim. It can be an observation, and that is what a witness is.

Each witness is measured exactly as a reading is: its lag against the channel everything is measured against, the
spread across the clicks, how many clicks were found, and what its interface's audio lost while it
was going. It carries the name of the interface it is on and that interface's own number for it,
exactly as it was asked for, because an interface can have several. It is reported separately from the
readings, so nothing downstream can mistake one for an interface.

**A witness never changes anything.** It produces no trim, it never stops a trim being offered, and
a witness with nothing on it is reported as an empty channel rather than refused. The only things
refused about one are a channel the aggregate has not got, and a channel this run is already
measuring, and both are refused before anything is played.

### Comparing one interface's analogue and digital inputs in a single run

The question this was built for: when an interface's stream starts, does its **whole** capture
pipeline settle on one phase, so that its S/PDIF input and its analogue input shift together by the
same amount? If they move independently, a phase measured over S/PDIF says nothing about what the
analogue inputs did.

Answering it needs one run that records two inputs of the same interface at once. Measure the
inputs as usual, and carry the interface's other input along as a witness:

```rust
use gazelle_calibrate::{Direction, Pick, Rig, Settings};

// The usual two cables: Quadro out 1 into Quadro in 1, Quadro out 2 into the Studio+'s analogue
// in 1. A split of that second output also goes into the Studio+'s S/PDIF input, its own input 9,
// and that channel comes along as a witness.
let rig = Rig::new(Direction::Inputs, vec![Pick::new(0, 0), Pick::new(0, 1)], vec![Pick::new(0, 0), Pick::new(1, 0)])
    .watching(vec![Pick::new(1, 8)]);
let outcome = gazelle_calibrate::session::measure(&rig, &Settings::default());

for witness in &outcome.witnesses {
    println!("{} input {}: {}", witness.device, witness.channel + 1, witness.reading.note);
}
```

The trims that come out are the ones the two measured channels imply, exactly as they would be with
no witness at all. What the witness adds is a second number for the same interface in the same run:
run it several times, and if the analogue lag and the S/PDIF lag move together, run to run, by the
same amount, the interface's capture pipeline has one phase and measuring it over S/PDIF is enough.
If they move apart from each other, it has not, and they have to be measured separately.

**It has been answered.** At the devices on 2026-09-21, in six runs, an interface's analogue and
S/PDIF inputs moved together every time, their difference constant to two decimals while the
absolute figures jumped by whole steps of 32 samples. That is why the aggregate measures each
interface's phase once, over the digital cable, and corrects every input by it; the figures are in
[the aggregate's README](../gazelle-audio-aggregate/README.md#the-phase).

## The sign rule

**The interface whose copy of the click lands later is recording late, and it takes a positive
trim.**

A device that records late has a longer path than its driver admits to. A positive trim is what
tells the aggregate the path is longer, and the aggregate then holds every other device back to
meet it. A negative trim tells it the opposite and moves the two recordings further apart.

This is worth labouring because it has already been got wrong once, by hand, at these devices on
2026-09-21: the offset was about 28 samples, a trim of -28 made it 53, and +28 nulled it. The rule
has a test of its own, and the end to end test proves it by cabling one interface 28 samples late
on purpose, measuring it, putting the measured trim into the configuration, and measuring again.

**A trim that was already there is added to, never replaced.** The run happened with the old trim
in force, so what came out is what is *left over* after it. Every result therefore reports the old
trim, the measured lag and the new trim separately, so that nobody has to work that out or wonder
which of the three they are looking at.

## What it plays, and how it reads it back

- **A short shaped sweep, not a single sample.** A converter's filters turn one sample into a
  symmetrical smear with no start worth measuring. A few dozen samples of a frequency sweep under a
  raised cosine window survive them, and a sweep correlates with itself at exactly one place, so a
  peak cannot land on the wrong lobe the way it can with a tone burst.
- **Several clicks, half a second apart.** Eight by default, each one a measurement of its own.
  One number has no spread and no slope, and no way to tell a measurement from a coincidence.
- **Cross correlation between the two captured channels**, not a threshold. A threshold answers
  "where did this channel first cross a level", which depends on the level, the filters and the
  noise floor, and answers in whole samples. Correlating the two recordings of the same event with
  each other answers the actual question, and fitting a parabola through the correlation peak and
  its two neighbours answers it to a fraction of a sample.
- **Reported per interface:** the median lag across the clicks, the spread between the widest and
  narrowest of them, how many clicks were found at all, and how many blocks the audio under the run
  lost while they were playing. Any witness the run was carrying is reported the same way, and
  separately.

## When the clicks do not agree with each other

A spread is not a decoration on the answer, it is what says whether there is an answer. **A run at
the hardware measured a lag of 60.85 samples with a spread of 64.00**, a whole buffer, and offered
a trim of 61 anyway. Those clicks were not measuring the same thing as each other: one of them
landed a whole block away from the rest, so their middle was a number with nothing behind it.

So a reading is only turned into a trim when the clicks agreed to within:

- about a sample, which is the scatter a converter puts on a click by itself and is always allowed;
- a quarter of the lag itself, because a spread that is a large fraction of the answer means the
  clicks did not agree on the answer;
- and a quarter of a buffer, because a spread near the buffer size is a block gone missing rather
  than a measurement.

Past that the reading says how far apart the clicks were, what it would have had to be, that a
spread near the buffer size is usually one click a whole block out, and that nothing is offered as
a trim until a run comes back with them agreeing.

## Whether the audio underneath was clean

The aggregate counts the blocks it loses, per interface, and this reads those counters twice: once
when the settling time is over and the first click is about to go out, and once when the run stops.
**Every reading carries what its interface lost while it was being measured**, and a reading with
anything but zero there is not turned into a trim: a click measured across a lost block is a whole
buffer out, and nothing in a correlation can tell that apart from a real offset. The sentence says
how many blocks went, which way, and what to do about it, which is to raise the buffer size or take
the PC off whatever else it is doing and measure again.

**It counts from the first click, not from the moment the interfaces were started.** A stream
starting is not a quiet stretch of audio: the interfaces' callbacks find where they sit inside each
other's block in the first few milliseconds, and an interface can take a block of silence doing it.
At the hardware on 2026-09-20 that happened in four runs out of six, and it took four perfectly
good trims with it: every one of those runs was marked unclean and had its trim refused over a
block of silence that happened before the first click was played. Nothing is being measured during
the settling time, so nothing that happens there can make a measurement untrue. A block lost
between the first click and the last still spoils the run it was in, exactly as it did before.
`settle_seconds` is what decides where that line falls.

## When the lag grows: two clocks, not an offset

A line is fitted through the per click lags. **A lag that grows steadily across the run means the
interfaces are not sharing a clock**, and that is a different and more serious finding than an
offset: two converters each running off their own crystal drift apart for as long as they run, and
no fixed trim can cancel something that keeps growing. When the slope is real, the result says so,
in samples per second and in parts per million, and it refuses to turn that reading into a trim.
The cure is a word clock or a digital cable between the interfaces, and the clock source picked on
the interfaces themselves, before the DAW is opened.

A slope is only called real when it is far larger than the scatter of the clicks around it: at
least a whole sample across the run, and at least three times that scatter. Calling ordinary jitter
a drift would send somebody looking for a fault that is not there.

## What it refuses

Every refusal names what is wrong and what would put it right, and **no buffer is made and no
sample is played until all of them have passed**.

- **`GAZELLE_NO_HARDWARE` is set.** This crate drives real converters, so it refuses outright
  rather than looking for a safe subset.
- **The aggregate is already open.** Something else has these drivers, and they open once. Close
  the DAW and start again.
- **The cabling could not measure what it claims to:** outputs spread across two interfaces when
  the inputs are what is being measured (or the other way round), an input and output count that do
  not match, one channel named for two interfaces, or a channel listed for the wrong interface.
- **A cable end the aggregate has nowhere for**, each said with what would have been right: an
  interface that is not one of them (and which ones there are), a channel the interface has not got
  (and how many it has, from its own driver), a channel the setup keeps out of the aggregate (and
  which it does expose), and a channel the setup keeps for the phase measurement (and that this is
  why).
- **Fewer than two interfaces.** A lag is the difference between two of them.
- **Nothing arrived on a channel.** That is a cable, not a measurement, and the trim in the file is
  left exactly where it was. A witness with nothing on it is not a refusal: it is an observation
  that there was nothing to observe, and the rest of the run stands.
- **A witness the aggregate has no channel for**, or one this run is already measuring. A witness
  is an extra channel to listen in on, and a channel that is already a reading is not an extra one.
- **The audio lost a block while the clicks were playing**, or **the clicks did not agree with each
  other.** Both are reported in full and neither becomes a trim.
- **Anything the aggregate itself refuses**, passed through in the aggregate's own words.

## Watching a run, and what it leaves behind

**A run is a session, and it publishes what a session publishes.** The aggregate it opens is given
the driver's own reporter, so everything the driver says about a DAW's session it says about a run,
through the same machinery and into the same places.

**While it runs**, the driver's shared status record is written exactly as it is for a DAW, and
`GET /api/v1/aggregate` reads it: `status.state` is `read` rather than `silent`, with the plan in
force, the rate and buffer size, both interfaces, `open` and `streaming`, the callbacks going by, the
blocks each interface drops or misses, and in each of `status.devices` the phase measurement's
`phase` (`not_configured`, `measuring`, `measured_only` or `not_heard` in a measuring run; a
check can also show `applied`, `no_reference`, `off_the_grid` or `too_far`, exactly as a DAW's
session can), `phase_measured` and `phase_applied`. The Aggregate page shows a run the way it shows
a DAW's session. When the run ends, the record says nothing is open, and once the run has let go of it there
is no record at all, exactly as when a DAW closes the driver.

**Afterwards**, the driver's event log (`%APPDATA%\gazelle\aggregate-events.log`, which
`GET /api/v1/aggregate` returns as `events`) keeps the run's lines: the session starting and
ending with what each interface lost, the first block lost if one was, an interface stalling or
coming back, anything refused, and one `phase` line per interface that was set up to be measured,
saying what it was measured at, or why nothing was. **Every line
a run writes starts its detail with `Gazelle's own measurement:`**, so a person reading the log the
next morning can tell Gazelle measuring from a DAW recording:

```text
2026-09-21 23:40:12 session-started Gazelle's own measurement: 2 in, 2 out at 48000 Hz
2026-09-21 23:40:12 phase Gazelle's own measurement: Studio+ was measured at -148 samples from where its driver's figures put it, and was left there on purpose: this session was measuring the trim, and a trim is measured on the drivers' own figures
2026-09-21 23:40:17 session-ended Gazelle's own measurement: ran for 5 seconds, and no interface lost a block
```

The marking is done at the one place every line passes on its way to the file, so there is no line
a run writes that could forget it.

**The phase comes back in the answer too.** `Outcome::phases` carries one entry per interface,
read out of the record at the end of the run, in the driver's own terms: the state, what was
measured and what was applied, in samples, and the driver's own sentence about it.
`Outcome::phases_refused` picks out the interfaces whose measurement was asked for and turned down,
which is what a person reading a result has to be told. In a measuring run that means nothing came
back on their measurement cable, so the trim offered for them has no phase to be paired with; in a
check it can also mean the driver would not line the session up, which is exactly what a check is
there to find.

**A measuring run measures the phase and applies none of it**, and that is what makes it the place
a trim's phase reference comes from. The driver lines every session up to the phase that was
measured when the trim was, so the trim and that phase have to be the raw figures of one session:
the lag a run hears is on the drivers' own figures with nothing moved underneath it, and the phase
it hears beside that lag is exactly the reference the new trim needs. (Why the driver needs it, and
what it does with it, is in
[the aggregate's README](../gazelle-audio-aggregate/README.md#where-the-reference-comes-from).)
Each input trim for an interface whose phase is measured comes back with a `phase_reference`
beside it, in the same voice as the trim: what the file had (`old`) and what to write (`new`). Writing the trim writes both. A trim offered with no phase
heard beside it carries a `new` of nothing, which takes the old reference out rather than leaving it
next to a trim it does not belong to; a trim that is not offered keeps its old reference. Output
trims, the interface everything was measured against, and interfaces with no phase setting have none.
The pairing is read from the audio path itself, so it is there whether or not the run could publish.

**Not being able to publish never fails a run.** If the shared section cannot be made the run
measures exactly what it would have, and writes its lines to the log if it has one. `phases` is then
empty, because the phase is only ever reported through the record: empty means there was nothing to
read, which is not the same as every interface going unmeasured. The same happens when the section
already exists, which means the driver is loaded in a DAW somewhere: that record belongs to that
driver, and a run never writes into it.

## Checking rather than measuring

A **check** is a run with `Settings::checking` set. It plays the same clicks through the same rig,
but the session is lined up exactly as a DAW's session would be: each interface's phase is applied
from its reference, and the trims in the file are in force. The click lag it hears is therefore what
a recording made now would get, which makes it the verdict on the trims and references already
written: a lag near zero on every interface means they hold.

It offers nothing to write. `Outcome::trims` comes back empty and `Outcome::checking` is true,
because a lag heard on top of a correction is a verdict on the trim, not a new one. Everything else,
the readings, the spread, the lost blocks, the drift and the phases, is reported as for any run.
It is **Check**, beside Measure, on the Aggregate page.

At the devices on 2026-09-21, with a reference set, eight checks in a row read a click lag of 0.02
samples, although the sessions they ran in had started in three different states.

## Using it

```rust
use gazelle_calibrate::{Direction, Rig, Settings};

// Quadro outputs 1 and 2, into Quadro input 1 and Studio+ input 1. Aggregate channel numbers,
// counted from zero, in the order the interfaces appear in aggregate.json.
let rig = Rig::new(Direction::Inputs, vec![0, 1], vec![0, 16]);
let outcome = gazelle_calibrate::session::measure(&rig, &Settings::default());

match outcome.refusal {
    Some(why) => println!("{why}"),
    None => {
        for reading in &outcome.readings {
            println!("{}", reading.note);
        }
        for trim in &outcome.trims {
            println!("{}: {} was {}, measured {}, write {}", trim.device, trim.field, trim.old, trim.measured, trim.new);
        }
    }
}
```

`Settings` carries how many clicks, how far apart, how long each one is, how loud, the rate and
buffer size to ask for, how long to let the interfaces settle first, how far either side of each
click to search, and whether this is a check rather than a measurement.
Everything a server would send on is `serde::Serialize`.

## Running its tests

```bash
cargo test -p gazelle-audio-calibrate
cargo clippy -p gazelle-audio-calibrate --all-targets
```

No test opens a driver, starts a converter or plays a sample.

## Licence

MIT, like the rest of this workspace.
