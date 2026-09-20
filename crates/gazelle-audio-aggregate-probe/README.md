# gazelle-audio-aggregate-probe

Phase 0 of the aggregate driver: a coexistence probe. It answers one question before any driver is
written.

> Can both Antelope USB audio drivers be created, initialised and started in **one process**, and
> do they stay in step while the S/PDIF cable locks them together?

A DAW opens one ASIO driver at a time, so recording through both interfaces in one Cubase session
means a driver that presents itself as one device and opens both vendor drivers underneath. That
only works if the two vendor drivers tolerate living in the same process. This probe finds out.

It is a **host**, not a driver. It contains no Steinberg SDK code and compiles against none: the
driver object's vtable is declared in `gazelle-audio-stream-abi` from the interface's public shape,
and the crate builds with the SDK absent. It is MIT like the rest of the workspace, and so is the
aggregate driver that followed it: `gazelle-audio-aggregate` implements the same interface from the
same hand written declaration, and needs no SDK either.

Windows and 64 bit only.

## What it does to the hardware

It reads. It never sets a sample rate, a clock source or a buffer size, never opens a vendor
control panel, and **writes nothing but silence**: the buffer callback zeroes every output buffer
it was handed, counts, and returns. Input is never read and never copied anywhere. Still, run it
with your monitors and amplifiers down: it does start the converters.

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

If a step fails while another driver is open, it says which driver, which call, and what the driver
itself said, and cleans up. That is an answer, not a crash.

## Tests

Everything the probe decides sits behind the `Host` and `SubDriver` traits in `src/probe/mod.rs`,
with a fake PC and fake drivers in `src/probe/fake.rs`. No test opens a driver, and no test needs
hardware.
