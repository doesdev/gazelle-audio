# Gazelle Aggregate: the status format

The wire format between the **Gazelle Aggregate driver** and **Gazelle**, written down here so that
both sides agree and neither has to depend on the other. The driver runs inside whatever process
opened it, which is a DAW; Gazelle is a separate program that may not be running at all.

Windows only. MIT, like the rest of this workspace. Unreleased: nothing here is installed,
registered or mentioned in the release notes.

There are three things in it, and they are deliberately different shapes.

| | What it is | Written | Read |
| --- | --- | --- | --- |
| Live state | A fixed size record in a named shared section | By the driver, every block and whenever the plan changes | By Gazelle, as often as it likes |
| Durable events | A small text file | By the driver, only when something happens | By Gazelle, or by a person with a text editor |
| A request | A counter in the same record, plus a named event | By Gazelle | By the driver's watcher thread |

**Why shared memory and not a file.** Once a section is mapped, writing it is a handful of atomic
stores: no allocation, no lock anything slow can hold, and no system call. That is what makes it
safe from an audio callback. A file written on a timer from an audio process is at the mercy of
antivirus and of the filesystem's own metadata churn, and it caps how often anything can be
reported.

**Why a file as well.** Shared memory says nothing once the driver has exited, and that is exactly
when somebody wants to know why last night's session would not start.

## The names

All three are worked out in `names.rs`, and every shared name carries the format version, so a
driver and a Gazelle built against different versions of this crate do not find each other at all
rather than finding each other and misreading.

| | Now |
| --- | --- |
| Shared section | `Local\gazelle-aggregate-status-1` |
| Reload event | `Local\gazelle-aggregate-reload-1` |
| Configuration file | `%APPDATA%\gazelle\aggregate.json` |
| Event log | `%APPDATA%\gazelle\aggregate-events.log` |

`Local\` is per logon session. The driver runs inside the DAW and Gazelle runs as the same person,
and nothing here should be reachable from another account.

## The record

`StatusRecord`, `#[repr(C)]`, 1936 bytes, 8 byte aligned. Fixed size, no pointers, no allocation.
Names are fixed byte arrays with a length, so a reader maps it and reads it without trusting a word
of the writer's memory. A test asserts the size of every structure in it, because a size that moved
silently is the one mistake this crate can make that nothing else would catch.

```
StatusRecord
  magic            u64    "GZAGGST1" as little endian bytes
  sequence         u64    the seqlock, atomic
  format_version   u32    1
  record_size      u32    1936
  gate             u32    the driver's own writer gate, atomic
  reserved         u32
  control          ControlArea    64 bytes, written only by Gazelle
  driver           DriverArea   1840 bytes, written only by the driver
```

### Who writes what

- **The driver writes `driver` and nothing else.** It reads `control` and never writes it.
- **Gazelle writes `control` and nothing else.** It reads `driver` and never writes it.
- **The header is written once**, by whoever created the section, and then never again.

Neither side reads the other's area expecting it to hold still. `control` is two atomics, so it
needs no protocol; `driver` is bigger than any one atomic, so it has one.

### `ControlArea`, which Gazelle writes

| Field | Type | What it is |
| --- | --- | --- |
| `generation` | `u64` | Bumped by Gazelle **after** it has written the configuration file. Starts at zero, and the first request is one. |
| `requested_nanos` | `i64` | When that request was made, on the machine's clock (`QueryPerformanceCounter` in nanoseconds), for a Gazelle that wants to show how long a reload took. |
| `reserved` | `[u64; 6]` | Zero. Room for a later version without a new record size. |

`Reader::request(nanos)` does both stores in the right order and answers the generation the driver
is now expected to reach.

### `DriverArea`, which the driver writes

| Field | Type | What it is |
| --- | --- | --- |
| `session_nanos` | `i64` | When the current session started, machine clock. Zero when there is no session. |
| `last_block_nanos` | `i64` | When the last block went through. A figure that has stopped moving while `streaming` is one is a driver that has stopped. |
| `callbacks` | `u64` | Blocks the master has driven this session. |
| `position` | `u64` | Samples handed to the DAW this session. |
| `generation_in_force` | `u64` | The generation the driver is actually running. Equal to `control.generation` means what Gazelle asked for is what is happening. |
| `refused_generation` | `u64` | The generation that was refused, if one was. With `refusal` it says which request went wrong and why. |
| `sample_rate` | `f64` | |
| `open` | `u32` | A DAW has the driver open with its buffers made. |
| `streaming` | `u32` | Audio is running. |
| `device_count` | `u32` | How many of `devices` mean anything. Clamp it to `MAX_DEVICES` (8) before using it. |
| `master` | `u32` | Which device drives the DAW's callback. |
| `buffer_size` | `i32` | |
| `alignment` | `u32` | 0 aligned, 1 lowest latency. An unknown number reads as aligned. |
| `input_latency`, `output_latency` | `i32` | The one figure each way the aggregate reports for the whole of itself. |
| `daw_inputs`, `daw_outputs` | `u32` | How many channels the DAW actually asked for. |
| `refusal` | 256 bytes + length | The last thing the driver refused to do, in the same words the DAW was given. Empty when nothing has been refused. |
| `config_source` | 256 bytes + length | Where the configuration in force was read from. |
| `devices` | `[DeviceStatus; 8]` | |

`DeviceStatus`, 152 bytes each:

| Field | Type | What it is |
| --- | --- | --- |
| `callbacks` | `u64` | How many times this device has called back this session. |
| `dropped` | `u64` | Blocks thrown away because its ring was full. |
| `starved` | `u64` | Blocks that were not there when they were wanted, which is what a person hears as a click. |
| `gap` | `i64` | **The number the person most wants**: this device's sample count minus the master's. Zero while the two are locked, and growing in one direction when they are not. Negative means behind. Meaningful only while both are streaming. |
| `name` | 32 bytes + length | What the configuration file calls it. |
| `driver_name` | 32 bytes + length | What its own driver calls itself. |
| `inputs`, `outputs` | `u32` | How many channels of it are exposed. |
| `streaming` | `u32` | It is calling back. |
| `stalled` | `u32` | The driver has given up on it for the moment: its inputs read as silence and its outputs are muted until it comes back. |
| `is_master` | `u32` | It drives the DAW's callback. |
| `latency_in`, `latency_out` | `i32` | What its own driver reports, plus the trim from the configuration file. |
| `pad_in`, `pad_out` | `i32` | Samples it is held back by so that every device lines up. |

A string field is bytes plus a length. Read it through `Text::get`, which clamps the length to the
array and answers nothing at all for bytes that are not text: a reader of a record it cannot trust
should say less, not guess.

## The seqlock

The driver's area is bigger than any one atomic, so it is published under a sequence counter.

- **The writer** makes `sequence` **odd** before it touches the area and **even** again afterwards.
- **The reader** takes `sequence`, copies the area, takes `sequence` again, and keeps what it copied
  only if the counter was even both times and did not change. If it moved, it tries again; after
  `read::ATTEMPTS` (64) tries it answers `StatusError::Busy` rather than handing back something half
  old. A driver killed part way through a write leaves the counter odd for ever, and this is what
  stops a reader spinning on it.

No reader ever blocks a writer and no writer ever waits for a reader, which is what makes it safe
to do from an audio callback. Every field of `DriverArea` is an integer, a float or a byte, so
every bit pattern is a value of its type; whether a value **means** anything is what the counter
decides, afterwards.

There is a second, much smaller rule for the writer's own side. The driver has two threads that
write: the audio thread every block, and the watcher thread when the plan or a refusal changes. The
`gate` word is taken by whichever is writing.

- The watcher **waits** for it (`Publisher::update`), spinning for a moment.
- The audio thread **never waits** (`Publisher::try_update`). If it cannot take the gate it skips
  that block's update and writes the next one instead. Losing one block's worth of counters is not
  worth a moment of jitter.

## The event log

`%APPDATA%\gazelle\aggregate-events.log`. One event, one line, plain words in the order a person
reads them:

```
2026-09-20 21:14:07 session-started 40 in, 40 out at 96000 Hz
2026-09-20 21:31:44 stalled Studio+
2026-09-20 21:31:46 recovered Studio+
2026-09-20 21:47:02 glitched Studio+ dropped a block, the first this session has lost: it was handing them over faster than they could be taken
2026-09-20 22:02:11 session-ended ran for 47 minutes 12 seconds. Quadro lost nothing; Studio+ dropped 3 blocks and missed 1 block
2026-09-21 09:14:02 refused Studio+ will not run at 96000 Hz, so neither will the aggregate
2026-09-21 09:15:30 adopted C:\Users\someone\AppData\Roaming\gazelle\aggregate.json
```

- The time is `YYYY-MM-DD HH:MM:SS`, **local**, because the person reading it is the person it
  happened to.
- The second piece is one word, never two, so a line splits the same way whatever is in its detail.
  The words are `refused`, `stalled`, `recovered`, `glitched`, `session-started`, `session-ended`,
  `adopted`, `reset-asked`. `events::ALL` is the list, and it is the one both sides read.
- **`glitched` is the first block a session loses, and only the first.** The rest are counted, and
  the totals go in that session's `session-ended` line, along with how long it ran. The live record
  above holds the counts while the driver is running; those two lines are what is left of them once
  it has exited.
- The rest of the line is the detail, with anything that would make it two lines flattened to
  spaces.
- A line this version does not understand reads as nothing, rather than as something wrong.

**Nothing is ever written on a timer.** Every line in the file is something that happened. The file
is trimmed to its last `events::KEEP_LINES` (400) lines when the driver opens it, which is the only
time it is ever rewritten.

## Asking the driver to change

1. Gazelle writes `%APPDATA%\gazelle\aggregate.json`.
2. Gazelle calls `Reader::request(nanos)`, which bumps `control.generation`.
3. Gazelle signals the reload event (`windows::ReloadEvent::open()` then `signal()`).
4. The driver's watcher thread wakes, re-reads the file and compares the plan with the one in
   force.
5. Gazelle watches `driver.generation_in_force` catch up, or `driver.refusal` and
   `driver.refused_generation` say why it did not.

The driver's side of step 4 is in the driver's own crate, and its rules are:

- A configuration that does not parse, or that names a device this PC does not have, is **refused**.
  What is in force stays in force and the DAW is left alone.
- If nothing is streaming, the new plan is **adopted** there and then, quietly.
- If a DAW is streaming, the host is **asked to reset**, which is the message a vendor driver sends
  when its own settings change, and the new plan is taken up when the DAW comes back for buffers.
  Audio drops for a moment exactly as a buffer size change does.

The watcher also looks around on a timeout, every 250 ms, which is how a device that stalled gets
its line in the event log.

## Reading it from Gazelle

```rust
use gazelle_audio_aggregate_status::{Reader, StatusError};
use gazelle_audio_aggregate_status::windows::Section;

match Section::open().map_err(|_| StatusError::NotMapped).and_then(|s| Reader::map(Box::new(s))) {
    Ok(reader) => match reader.read() {
        Ok(seen) => {
            for device in seen.devices() {
                println!("{} gap {}", device.name.get(), device.gap);
            }
        }
        Err(why) => println!("{why}"),
    },
    Err(why) => println!("{why}"),
}
```

Every refusal has a sentence of its own (`StatusError` implements `Display`), and
`Section::open` failing simply means the driver is not loaded in anything.

## Testing

Everything above the Windows layer is decided against `map::Scratch`, a mapping made of ordinary
memory, so no test makes a real section, opens a real file or touches a device. Two of the tests do
make a real named section and a real named event, because the thin Windows layer is worth one pass
each; they cost a page of memory and a handle.

The one thing about a seqlock that has to be proven rather than reasoned about is that a reader
catching a torn write tries again and gets a whole one. `Reader::read_watching` exists for exactly
that: a test is the writer that tears the first attempt, and the record that comes back is all from
one write and not a mixture of two.
