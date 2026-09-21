# gazelle-audio-stream-abi

The low latency audio driver interface, declared once, by hand, for everything in this workspace
that touches it.

Gazelle Aggregate is a driver of this kind, and the phase 0 probe is a host of it. Both need the
same description of the driver object: its vtable, the structures that cross it, the numbering of
its constants and its sample types. That description lives here so that neither has to depend on
the other, and so that there is exactly one place where it could be wrong.

Windows and 64 bit only. MIT, like the rest of this workspace.

## No Steinberg code, and no SDK

**No Steinberg SDK code is included, copied or needed, and the SDK must never be committed to
this repository.**

The driver object is a COM object whose vtable, after `IUnknown`'s three entries, holds twenty one
calls in a fixed order. That order, the layout of the structures passed across it and the numbering
of its constants are the interface's public shape, and this crate declares them in Rust from that
shape. Nothing from the SDK is included, copied or compiled, and every crate that uses this one
builds with the SDK absent. The SDK headers were read to check the order of the calls and the
layout of the structures; no code, no comment and no text was taken from them. A local copy kept
for that purpose belongs in `refs/sdk/`, which `.gitignore` keeps out of the repository.

This is the licensing story for the whole aggregate driver: because the interface is declared here
and the SDK is neither used nor needed, the driver, the probe and this crate are all MIT like the
rest of the workspace. The module docs in [`src/lib.rs`](src/lib.rs) say the same, and so do
[`gazelle-audio-aggregate`'s README](../gazelle-audio-aggregate/README.md#licence) and its
`src/lib.rs`.

**Why nothing is named after it.** The interface's usual name is a Steinberg trademark, and
Steinberg's trademark rules forbid it in a product's name. So neither this crate, nor any crate
built on it, nor the driver itself ("Gazelle Aggregate") is named after the technology. The name
still appears where the platform itself puts it, such as the `HKLM\SOFTWARE\ASIO` key every driver
of this kind registers under; that is a fact about Windows, not a name of ours.

## What is in it

| Module | What it holds |
| --- | --- |
| [`raw`](src/raw.rs) | The boundary itself: every structure `#[repr(C)]`, every function pointer `extern "system"`, the vtable, the class id type and the constants. |
| [`sample`](src/sample.rs) | What each sample type code means, how wide it is, and how to carry any of them through a full scale `i32` without changing what it sounds like. |
| [`entry`](src/entry.rs) | One driver as the registry lists it, as plain data, so that anything reasoning about a list of drivers does so without a registry underneath it. |
| [`registry`](src/registry.rs) | Windows only: reading where drivers of this kind publish themselves. It only reads; the aggregate's own registration, which writes, lives in the aggregate crate behind a trait. |

The type rules for the 64 bit Windows build (every `long` is 32 bits, a sample rate is a double
passed by value, a sample position is two 32 bit words, high word first) are in the module docs in
[`src/lib.rs`](src/lib.rs).

## What depends on it

It depends on nothing in the workspace, and only on `windows-sys` on Windows.

- [`gazelle-audio-aggregate-probe`](../gazelle-audio-aggregate-probe/README.md) **calls** a driver
  through `raw::Vtable`.
- [`gazelle-audio-aggregate`](../gazelle-audio-aggregate/README.md) **implements** it: it builds a
  vtable of its own functions and hands it out of `DllGetClassObject`. It also reads the registry
  through this crate to find the vendor drivers it opens.
- [`gazelle-audio-calibrate`](../gazelle-audio-calibrate/README.md) drives the aggregate as a host
  does, so it uses the same types.

## Testing

```bash
cargo test -p gazelle-audio-stream-abi
cargo clippy -p gazelle-audio-stream-abi --all-targets
```

No test opens a driver or reads the registry. What is tested is what can be checked without one:
that every structure is the size the interface says, that a sample count survives the two halves
it is carried in, that a class id prints the way the registry holds it, and that every sample type
converts both ways without changing value.

## Before changing it

- **A layout change here is an ABI change on both sides at once.** A field moved or resized, or a
  call out of order, and the aggregate hands a DAW a vtable it will call into wrongly, while the
  probe calls into a vendor driver wrongly. The size test is the guard; extend it whenever a
  structure is added.
- **Declare from the public shape, never by pasting.** If a question can only be answered by the
  SDK, read it and write the answer in your own words and your own Rust. Do not commit the SDK, any
  file from it, or text taken from it.
- **Keep it free of policy.** This crate says what the interface is. What to do with a driver
  belongs to the probe or the aggregate.
