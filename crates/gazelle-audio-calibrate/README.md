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
Nothing new has to touch the interfaces: the aggregate already opens them.

**This one really does drive the hardware.** It plays audio out of a real interface into whatever
is plugged into it. The click is quiet by default (about -20 dBFS) and can never be louder than
-6 dBFS whatever it is asked for, but turn the monitors down before the first run all the same.

**Unreleased.** It is not registered, not installed and not in the release. Its tests never touch
hardware: the arithmetic is tested against signals made of data, and the whole session is tested by
driving the real aggregate against sub-devices made of data with a delay built into one of them.

Windows only.

## The cabling

One output channel per interface being measured, **all of them on one interface**, each cabled into
one chosen input channel of its own interface. Two interfaces is two cables. A third interface is a
third cable, from a third output of the same interface. Nothing here assumes two.

Both copies of the click leave the same interface on the same sample, so the difference between
where they land is the difference between the interfaces and nothing else.

### Measuring the inputs

One interface plays, every interface records:

| From | To |
|---|---|
| Quadro out L | Quadro in 1 |
| Quadro out R | Studio+ in 1 |

The Quadro is the reference here, because it is first in the file. What comes out is the residual
error between the two interfaces' **inputs**, and it goes in `input_trim`.

### Measuring the outputs

The same rig with the cabling mirrored: every interface plays, one interface records.

| From | To |
|---|---|
| Quadro out L | Quadro in 1 |
| Studio+ out L | Quadro in 2 |

Now the inputs are what the two copies have in common, so what comes out is the residual error
between the interfaces' **outputs**, and it goes in `output_trim`. It is one code path and one set
of arithmetic: only the direction changes.

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
  narrowest of them, and how many clicks were found at all.

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

Every refusal names what is wrong and what would put it right, and **nothing is opened and no
sample is played until all of them have passed**.

- **`GAZELLE_NO_HARDWARE` is set.** This crate drives real converters, so it refuses outright
  rather than looking for a safe subset.
- **The aggregate is already open.** Something else has these drivers, and they open once. Close
  the DAW and start again.
- **The cabling could not measure what it claims to:** outputs spread across two interfaces when
  the inputs are what is being measured (or the other way round), an input and output count that do
  not match, one channel named for two interfaces, a channel listed for the wrong interface, or a
  channel the aggregate has not got.
- **Fewer than two interfaces.** A lag is the difference between two of them.
- **Nothing arrived on a channel.** That is a cable, not a measurement, and the trim in the file is
  left exactly where it was.
- **Anything the aggregate itself refuses**, passed through in the aggregate's own words.

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

`Settings` carries how many clicks, how far apart, how loud, the rate and buffer size to ask for,
how long to let the interfaces settle first, and how far either side of each click to search.
Everything a server would send on is `serde::Serialize`.

## Running its tests

```bash
cargo test -p gazelle-audio-calibrate
cargo clippy -p gazelle-audio-calibrate --all-targets
```

No test opens a driver, starts a converter or plays a sample.

## Licence

MIT, like the rest of this workspace.
