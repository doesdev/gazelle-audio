//! **Gazelle Aggregate**: one low latency audio driver that a DAW opens, and several vendor
//! drivers underneath it.
//!
//! A DAW opens one driver at a time, so recording and playing through more than one interface in
//! one session means a driver that presents itself as a single device and opens the vendor drivers
//! itself. Phase 0 of this work proved at the hardware that two Antelope drivers do live in one
//! process, that they agree on buffer size and sample format, and that a pair locked by a digital
//! cable holds identical sample counts. This crate is what that was for.
//!
//! **Nothing here knows how many devices there are, or which ones.** Everything comes from
//! `%APPDATA%\gazelle\aggregate.json`: which drivers to open, what to call their channels, which
//! of them drives the callback, and how the streams are aligned. Adding another Antelope interface
//! is a line in that file.
//!
//! **No Steinberg code, and no SDK.** The interface this implements is declared by hand in
//! `gazelle-audio-stream-abi` from its public shape. Nothing from the SDK is included, copied or
//! compiled, and this crate builds with the SDK absent.
//!
//! **Licence.** MIT, like the rest of this workspace. An earlier answer had this component under
//! GPLv3, on the assumption that a driver would have to be built on Steinberg's SDK. It is not:
//! the interface is declared by us and the SDK is neither used nor needed, so the reason for that
//! answer has gone and the licence follows the workspace. This is written down as an open question
//! in the phase 1 notes for the owner to confirm.
//!
//! **Name.** Steinberg's trademark rules forbid their technology's name in a product's name, so
//! the driver calls itself "Gazelle Aggregate" and neither this crate nor the interface crate is
//! named after it.
//!
//! # How it is put together
//!
//! - [`config`] reads the configuration file: which devices, which master, which alignment.
//! - [`sub`] is the one thing a sub-device must be able to do, so that every rule in this crate is
//!   tested against fakes ([`fake`]) rather than hardware. [`daw`] is the other side of the same
//!   idea: a host made of data, for the tests to be a DAW with.
//! - [`plan`] turns the devices' own answers plus the configuration into the aggregate's channel
//!   list, its buffer size, its latencies and the padding each device needs.
//! - [`stream`] is the audio path, and the only code that runs on a callback thread. It allocates
//!   nothing, locks nothing, logs nothing and never calls back into a vendor driver.
//! - [`phase`] measures where each follower's capture pipeline settled when its stream started,
//!   over the digital cable between the interfaces, so every session can be put back into the
//!   state its trims were measured in. The deciding is pure; the listening runs in [`stream`].
//! - [`aggregate`] is everything a DAW asks for outside the audio path.
//! - [`status`] is what the driver tells the outside world: the live record in shared memory, and
//!   the small event log that keeps what happened after the driver has exited.
//! - [`watch`] is the one thread that waits for Gazelle to say the configuration has changed, and
//!   re-plans when it does. Never the audio thread.
//! - `com` (Windows only) is the COM object and the DLL's four exports.
//! - [`registration`] is what `regsvr32` writes, behind a trait so it is tested without a registry.

/// This machine's reference clock in nanoseconds, which is what a time stamp handed to a DAW must
/// be on: the same clock the vendor drivers stamp with, counting from when the machine started, not
/// from when this driver did. A DAW compares the stamp with its own reading of that clock, and one
/// that starts at zero makes a driver that meters but cannot record (seen in Cubase, 2026-09-21).
#[cfg(windows)]
pub fn now_nanos() -> i64 {
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut ticks, mut per_second) = (0i64, 0i64);
    // Safety: both take a pointer to an i64 this function owns, and Windows guarantees both succeed
    // on anything this driver can run on.
    unsafe {
        QueryPerformanceCounter(&mut ticks);
        QueryPerformanceFrequency(&mut per_second);
    }
    if per_second <= 0 {
        return 0;
    }
    // Seconds and the remainder apart, so a counter that has been running for months does not
    // overflow on the way to nanoseconds.
    (ticks / per_second) * 1_000_000_000 + (ticks % per_second) * 1_000_000_000 / per_second
}

/// Off Windows there is no performance counter to read, and no DAW to hand a stamp to either.
#[cfg(not(windows))]
pub fn now_nanos() -> i64 {
    0
}

pub mod aggregate;
pub mod config;
pub mod daw;
pub mod delay;
pub mod fake;
pub mod phase;
pub mod plan;
pub mod registration;
pub mod ring;
pub mod status;
pub mod stream;
pub mod sub;
pub mod watch;

#[cfg(test)]
mod driver_tests;

#[cfg(windows)]
pub mod com;
#[cfg(windows)]
pub mod windows_host;

/// What a DAW sees this driver called. Not the interface's name, and not a vendor's.
pub const DRIVER_NAME: &str = "Gazelle Aggregate";

/// The driver's own version, as the interface carries one: a plain number.
pub const DRIVER_VERSION: i32 = 1;

/// **This class id must never change.** It is how a DAW, and the registry, find this driver: it is
/// written into `HKLM\SOFTWARE\ASIO\Gazelle Aggregate\CLSID` and into
/// `HKLM\SOFTWARE\Classes\CLSID\{...}`, and a session that has chosen this driver remembers it by
/// this number. Changing it would orphan every project that had picked the driver and leave a dead
/// entry behind in the registry. Generated once, at random, on 2026-09-20.
pub const CLASS_ID: gazelle_audio_stream_abi::raw::Guid = gazelle_audio_stream_abi::raw::Guid {
    data1: 0xF18C_80B4,
    data2: 0x2DE1,
    data3: 0x43B8,
    data4: [0xAC, 0x84, 0x19, 0xDD, 0xC2, 0x2E, 0xBF, 0xC8],
};

/// The name of the key this driver registers itself under, which is what a DAW lists.
pub const REGISTRY_KEY: &str = "Gazelle Aggregate";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_class_id_is_the_one_written_down() {
        // A change here is a change to what every DAW has remembered. If this test fails, the
        // class id was edited, and that is the one edit this crate must never make.
        assert_eq!(CLASS_ID.to_registry_string(), "{F18C80B4-2DE1-43B8-AC84-19DDC22EBFC8}");
    }

    #[test]
    fn the_driver_is_not_named_after_anyone_elses_technology() {
        let lower = DRIVER_NAME.to_ascii_lowercase();
        assert!(!lower.contains("asio"), "the trademark rules forbid it in a product name");
        assert_eq!(DRIVER_NAME, REGISTRY_KEY);
    }
}
