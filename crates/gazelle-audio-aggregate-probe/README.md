# gazelle-audio-aggregate-probe

Phase 0 of the aggregate driver: a coexistence probe. It answered one question before any driver
was written, and it is still the quickest way to ask it again of another PC or another interface.

> Can both Antelope USB audio drivers be created, initialised and started in **one process**, and
> do they stay in step while the S/PDIF cable locks them together?

A DAW opens one ASIO driver at a time, so recording through both interfaces in one Cubase session
means a driver that presents itself as one device and opens both vendor drivers underneath. That
only works if the two vendor drivers tolerate living in the same process. This probe finds out.

**What it found**, at the devices on 2026-09-20: they do. Both vendor drivers were created,
started and run in one process, they agreed on buffer size and sample format, and with the digital
cable locking them their sample counts stayed identical. It also found that two of these interfaces
on one USB host controller cannot both stream, so each needs a controller of its own. The driver
this made possible is [`gazelle-audio-aggregate`](../gazelle-audio-aggregate/README.md).

It is a **host**, not a driver. It contains no Steinberg SDK code and compiles against none: the
driver object's vtable is declared in
[`gazelle-audio-stream-abi`](../gazelle-audio-stream-abi/README.md) from the interface's public
shape, and the crate builds with the SDK absent. The SDK is not needed and must never be committed.
It is MIT like the rest of the workspace, and so is the aggregate driver that followed it:
`gazelle-audio-aggregate` implements the same interface from the same hand written declaration, and
needs no SDK either.

Windows and 64 bit only.

## What it does to the hardware

It reads. It never sets a clock source or a buffer size, never opens a vendor control panel, and
**writes nothing but silence**: the buffer callback zeroes every output buffer it was handed,
counts, and returns. Input is never read and never copied anywhere. It sets a sample rate only when
`--set-rate` asks it to, which is its one write, made before streaming as a DAW makes it. Still, run
it with your monitors and amplifiers down: it does start the converters.

It refuses to run at all when `GAZELLE_NO_HARDWARE=1` is set.

## Running it

Build it first:

```
cargo build -p gazelle-audio-aggregate-probe --release
```

**Start here.** A bare run lists every ASIO driver registered on the PC, with its class id and its
DLL, says which two it would open, and stops. Nothing is opened:

```
target\release\gazelle-audio-aggregate-probe.exe
```

Then one driver alone, to see it work by itself:

```
target\release\gazelle-audio-aggregate-probe.exe --only quadro --yes --seconds 10
target\release\gazelle-audio-aggregate-probe.exe --only studio --yes --seconds 10
```

Then both together, which is the real question:

```
target\release\gazelle-audio-aggregate-probe.exe --yes --seconds 20
```

Options:

| Flag | What it does |
|---|---|
| `--yes` | Open the drivers. Without it the probe lists and stops. |
| `--seconds N` | How long to let them run. Default 20. |
| `--only NAME` | One driver: `quadro`, `studio`, or any part of a driver's name. |
| `--rate HZ` | Ask each driver whether it can run at this rate. Asked, never set. |
| `--set-rate HZ` | Move every driver to this rate before it streams, as a DAW does. The probe's only write. |

Exit code 0 when the run finished, 1 when a step failed or the PC could not be read, 2 when
`GAZELLE_NO_HARDWARE=1` refused it.

## What it prints

Per driver: name and version, input and output channel counts, buffer sizes (min, max, preferred,
granularity), current sample rate, latencies, the sample type of channel 0 in and out, and the
clock sources. If the two drivers report different preferred buffer sizes it says so and carries
on, because an aggregate driver will need them matched.

After the run: whether both started together, the callback count for each, callbacks per second,
and the difference between the two in callbacks and in samples, per second. With the cable locked
the two counts should stay equal. Without it they drift, and the number says by how much.

With two drivers it also reads them together all through the run, and prints what that shows:

- **The gap between the two sample counts**, where it started, where it ended, its widest and how
  fast it grew. This is the plainest measurement and the one to believe over a long run, but a count
  moves a buffer at a time, so a short run can show no gap even between two clocks.
- **The two clocks fitted through every reading**, in samples per second and parts per million, with
  a verdict: one clock, or two drifting apart and by how much an hour.
- **The same from the first and last readings only**, with the drivers' raw sample positions and
  timestamps, kept for comparison because a single reading is quantised; read the fit.

If a step fails while another driver is open, it says which driver, which call, and what the driver
itself said, and cleans up. That is an answer, not a crash.

## Tests

```bash
cargo test -p gazelle-audio-aggregate-probe
```

Everything the probe decides sits behind the `Host` and `SubDriver` traits in `src/probe/mod.rs`,
with a fake PC and fake drivers in `src/probe/fake.rs`. No test opens a driver, and no test needs
hardware.
