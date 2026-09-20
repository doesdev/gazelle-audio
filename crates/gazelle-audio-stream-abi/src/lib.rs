//! The low latency audio driver interface, declared here once for everything in this workspace
//! that touches it.
//!
//! **No Steinberg code, and none of their SDK.** The driver object this describes is a COM object
//! whose vtable, after `IUnknown`'s three entries, holds twenty one calls in a fixed order. That
//! order, the layout of the structures passed across it, and the numbering of its constants are
//! the interface's public shape, and they are declared in Rust here from that shape. Nothing from
//! the SDK is included, copied or compiled, and every crate that uses this one builds with the SDK
//! absent. The SDK headers under `refs/` were read to check the order of the calls and the layout
//! of the structures; no code, no comment and no text was taken from them. This crate is MIT, like
//! the rest of the workspace.
//!
//! **Trademarks.** The interface's usual name is a Steinberg trademark and their rules forbid it
//! in a product's name, so nothing here, and no crate that builds on it, is named after it.
//!
//! **Two sides.** The probe crate *calls* a driver through [`raw::Vtable`]. The aggregate crate
//! *implements* it: it builds a vtable of its own functions and hands a pointer to it out of
//! `DllGetClassObject`. Both need the same declaration, which is why it lives here rather than in
//! either.
//!
//! **Types.** In the 64 bit Windows build of this interface every `long` is 32 bits ([`i32`]), a
//! sample rate is an IEEE 754 double passed by value, and a sample position is a pair of 32 bit
//! words, high word first. `extern "system"` on x86_64 Windows is the convention a C++ virtual
//! call uses, with the object pointer first.

pub mod entry;
pub mod raw;
pub mod sample;

#[cfg(windows)]
pub mod registry;

pub use entry::Entry;
pub use raw::{
    BufferInfoRaw, CallbacksRaw, ChannelInfoRaw, ClockSourceRaw, Guid, Object, Samples, Time, TimeCode, TimeInfo, Vtable,
};

/// Whether two class id strings name the same class, whatever their case or their braces.
pub fn same_clsid(a: &str, b: &str) -> bool {
    fn bare(s: &str) -> String {
        s.trim().trim_start_matches('{').trim_end_matches('}').to_ascii_lowercase()
    }
    bare(a) == bare(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_compare_without_case_or_braces() {
        assert!(same_clsid("{12217625-CB57-11EE-908D-7085C2FB2DD5}", " 12217625-cb57-11ee-908d-7085c2fb2dd5 "));
        assert!(!same_clsid("{12217625-CB57-11EE-908D-7085C2FB2DD5}", "{AE4A4452-A316-11E5-A113-080027F6C1F4}"));
    }
}
