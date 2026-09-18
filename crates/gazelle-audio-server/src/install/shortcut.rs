//! A Start Menu shortcut, written as bytes rather than through COM.
//!
//! A `.lnk` is the Shell Link Binary File Format (MS-SHLLINK): a fixed header, an optional
//! `LinkInfo` naming the target, and a run of counted strings. Writing it directly rather than
//! driving `IShellLink` costs about a hundred lines and buys two things. `windows-sys` (the only
//! Windows crate here, and one the server already had through `hidapi`) carries no COM
//! interfaces at all, so the alternative was a hand-rolled vtable call, which is *more* unsafe
//! code than this and none of it testable. And a shortcut that is bytes is a shortcut a test can
//! read back: [`target_of`] parses what [`shell_link`] wrote, and an integration test hands the
//! same file to `WScript.Shell`, which resolves it through the very `IShellLink` Explorer uses.
//!
//! The one piece not written here is the target's item ID list, which the shell builds
//! ([`target_id_list`]). It is what `IShellLink::GetPath` reads, and a link without one has no
//! target however complete its `LinkInfo`: found by that test, not by reading the format.
//!
//! The target path is written twice, ANSI and UTF-16, with a `LinkInfoHeaderSize` of `0x24` so
//! readers know the Unicode copy is there. Without it a path holding a character the local code
//! page cannot spell (a user folder named in Greek, say) would arrive mangled.

use std::path::Path;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

/// `{00021401-0000-0000-C000-000000000046}`, the shell link class.
const CLSID: [u8; 16] = [0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];

const HAS_LINK_TARGET_ID_LIST: u32 = 0x0000_0001;
const HAS_LINK_INFO: u32 = 0x0000_0002;
const HAS_NAME: u32 = 0x0000_0004;
const HAS_WORKING_DIR: u32 = 0x0000_0010;
const HAS_ARGUMENTS: u32 = 0x0000_0020;
const HAS_ICON_LOCATION: u32 = 0x0000_0040;
const IS_UNICODE: u32 = 0x0000_0080;

const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
const SW_SHOWNORMAL: u32 = 1;
const DRIVE_FIXED: u32 = 3;

/// The fixed header, which everything else follows.
const HEADER_SIZE: usize = 0x4C;
/// A `LinkInfo` header that carries the two Unicode offsets as well as the ANSI ones.
const LINK_INFO_HEADER: u32 = 0x24;
/// `VolumeIDAndLocalBasePath`.
const VOLUME_ID_AND_LOCAL_BASE_PATH: u32 = 0x0000_0001;
/// A `VolumeID` with an empty ANSI label: five fields and one terminator.
const VOLUME_ID_SIZE: u32 = 0x11;

/// The bytes of a shell link pointing at `target`.
///
/// `arguments` may be empty, in which case the flag is left out rather than an empty string
/// written. `icon` is the file the icon is taken from, with `icon_index` selecting which.
///
/// `id_list` is the shell's own item ID list for the target, from [`target_id_list`]. **It is
/// what `IShellLink::GetPath` reads**, and so what Explorer launches: a link with only a
/// `LinkInfo` is parsed happily and shows its name, its working directory and its icon, and has
/// no target at all. That was found by the test that asks the shell to read the file back, not
/// by reading the specification, which describes the list as optional.
pub fn shell_link(
    target: &Path,
    arguments: &str,
    working_dir: &Path,
    description: &str,
    icon: &Path,
    icon_index: i32,
    id_list: Option<&[u8]>,
) -> Vec<u8> {
    let mut flags = HAS_LINK_INFO | HAS_NAME | HAS_WORKING_DIR | HAS_ICON_LOCATION | IS_UNICODE;
    if !arguments.is_empty() {
        flags |= HAS_ARGUMENTS;
    }
    if id_list.is_some() {
        flags |= HAS_LINK_TARGET_ID_LIST;
    }

    let mut out = Vec::new();
    out.extend(0x4Cu32.to_le_bytes());
    out.extend(CLSID);
    out.extend(flags.to_le_bytes());
    out.extend(FILE_ATTRIBUTE_NORMAL.to_le_bytes());
    // Creation, access and write times: zero means "unknown", which is what a fresh link knows.
    out.extend([0u8; 24]);
    out.extend(0u32.to_le_bytes()); // FileSize, advisory only
    out.extend(icon_index.to_le_bytes());
    out.extend(SW_SHOWNORMAL.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // HotKey
    out.extend([0u8; 10]); // Reserved, Reserved2, Reserved3
    debug_assert_eq!(out.len(), HEADER_SIZE);

    if let Some(id_list) = id_list {
        out.extend((id_list.len() as u16).to_le_bytes());
        out.extend(id_list);
    }
    out.extend(link_info(target));

    // The order is fixed by the format: name, relative path, working directory, arguments, icon.
    string_data(&mut out, description);
    string_data(&mut out, &working_dir.display().to_string());
    if !arguments.is_empty() {
        string_data(&mut out, arguments);
    }
    string_data(&mut out, &icon.display().to_string());

    out.extend(0u32.to_le_bytes()); // the terminal block: no ExtraData
    out
}

/// The `LinkInfo` block naming a local target, with the path in both encodings.
fn link_info(target: &Path) -> Vec<u8> {
    let path = target.display().to_string();
    let ansi: Vec<u8> = path.chars().map(|c| if c.is_ascii() { c as u8 } else { b'?' }).collect();
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    let volume_id_offset = LINK_INFO_HEADER;
    let local_base_path_offset = volume_id_offset + VOLUME_ID_SIZE;
    let common_path_suffix_offset = local_base_path_offset + ansi.len() as u32 + 1;
    let local_base_path_offset_unicode = common_path_suffix_offset + 1;
    let common_path_suffix_offset_unicode = local_base_path_offset_unicode + (wide.len() * 2) as u32;
    let size = common_path_suffix_offset_unicode + 2;

    let mut out = Vec::with_capacity(size as usize);
    out.extend(size.to_le_bytes());
    out.extend(LINK_INFO_HEADER.to_le_bytes());
    out.extend(VOLUME_ID_AND_LOCAL_BASE_PATH.to_le_bytes());
    out.extend(volume_id_offset.to_le_bytes());
    out.extend(local_base_path_offset.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // no CommonNetworkRelativeLink
    out.extend(common_path_suffix_offset.to_le_bytes());
    out.extend(local_base_path_offset_unicode.to_le_bytes());
    out.extend(common_path_suffix_offset_unicode.to_le_bytes());

    out.extend(VOLUME_ID_SIZE.to_le_bytes());
    out.extend(DRIVE_FIXED.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // drive serial number, not checked by anything
    out.extend(0x10u32.to_le_bytes()); // VolumeLabelOffset
    out.push(0); // an empty label

    out.extend(&ansi);
    out.push(0);
    out.push(0); // CommonPathSuffix, empty
    for unit in &wide {
        out.extend(unit.to_le_bytes());
    }
    out.extend(0u16.to_le_bytes()); // CommonPathSuffixUnicode, empty

    debug_assert_eq!(out.len(), size as usize);
    out
}

/// One `StringData` entry: the count in UTF-16 code units, then the characters, with no
/// terminator.
fn string_data(out: &mut Vec<u8>, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    out.extend((wide.len() as u16).to_le_bytes());
    for unit in wide {
        out.extend(unit.to_le_bytes());
    }
}

/// The target path a shell link names, read back out of its `LinkInfo`.
///
/// Prefers the Unicode copy when the block carries one. Returns `None` for anything this module
/// did not write: a link with a `LinkTargetIDList` and no `LinkInfo`, or a truncated file.
pub fn target_of(bytes: &[u8]) -> Option<String> {
    let u32_at = |at: usize| -> Option<u32> { bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())) };
    if u32_at(0)? != HEADER_SIZE as u32 || bytes.get(4..20)? != CLSID {
        return None;
    }
    let flags = u32_at(20)?;
    if flags & HAS_LINK_INFO == 0 {
        return None;
    }
    // The item ID list, when there is one, sits between the header and `LinkInfo`.
    let mut at = HEADER_SIZE;
    if flags & HAS_LINK_TARGET_ID_LIST != 0 {
        let size = bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))? as usize;
        at += 2 + size;
    }
    let offset = |field: usize| -> Option<usize> { u32_at(at + field).map(|o| at + o as usize) };
    if u32_at(at + 4)? >= LINK_INFO_HEADER {
        let from = offset(0x1C)?;
        let units: Vec<u16> = bytes
            .get(from..)?
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|&u| u != 0)
            .collect();
        return Some(String::from_utf16_lossy(&units));
    }
    let from = offset(0x10)?;
    let end = bytes.get(from..)?.iter().position(|&b| b == 0)? + from;
    Some(String::from_utf8_lossy(&bytes[from..end]).into_owned())
}

/// The shell's own item ID list for a filesystem path, ready to be written into a link.
///
/// Built by `shell32`'s `ILCreateFromPathW` rather than by hand: an item ID list is a chain of
/// shell-defined structures whose exact contents are the shell's business, and asking the shell
/// for it is both shorter and correct by construction. `None` when the shell will not parse the
/// path, and on any platform that has no shell; a link is then written with `LinkInfo` alone,
/// which is all that can be offered there.
#[cfg(windows)]
pub fn target_id_list(target: &Path) -> Option<Vec<u8>> {
    use windows_sys::Win32::UI::Shell::{ILCreateFromPathW, ILFree, ILGetSize};
    let wide: Vec<u16> = target.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let pidl = ILCreateFromPathW(wide.as_ptr());
        if pidl.is_null() {
            return None;
        }
        let size = ILGetSize(pidl) as usize;
        let bytes = std::slice::from_raw_parts(pidl.cast::<u8>(), size).to_vec();
        ILFree(pidl);
        // A list that is nothing but its terminator names nothing.
        (size > 2).then_some(bytes)
    }
}

#[cfg(not(windows))]
pub fn target_id_list(_target: &Path) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn link() -> Vec<u8> {
        shell_link(
            Path::new(r"C:\Users\u\AppData\Local\Programs\Gazelle\gazelle-audio-serverw.exe"),
            "",
            Path::new(r"C:\Users\u\AppData\Local\Programs\Gazelle"),
            "Gazelle",
            Path::new(r"C:\Users\u\AppData\Local\Programs\Gazelle\gazelle-audio-serverw.exe"),
            0,
            None,
        )
    }

    #[test]
    fn the_header_is_the_one_the_shell_expects() {
        let bytes = link();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 0x4C, "HeaderSize");
        assert_eq!(bytes[4..20], CLSID, "the shell link class");
        let flags = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        assert_eq!(flags & IS_UNICODE, IS_UNICODE, "the strings are UTF-16");
        assert_eq!(flags & HAS_ARGUMENTS, 0, "no arguments, so no empty argument string");
        assert_eq!(i32::from_le_bytes(bytes[56..60].try_into().unwrap()), 0, "IconIndex");
        assert_eq!(u32::from_le_bytes(bytes[60..64].try_into().unwrap()), SW_SHOWNORMAL, "ShowCommand");
    }

    #[test]
    fn the_target_is_readable_from_the_bytes_that_were_written() {
        let target = r"C:\Users\u\AppData\Local\Programs\Gazelle\gazelle-audio-serverw.exe";
        assert_eq!(target_of(&link()).as_deref(), Some(target));
    }

    /// The whole reason for the `0x24` header: a path the local code page cannot spell survives.
    #[test]
    fn a_path_outside_ascii_survives_in_the_unicode_copy() {
        let target = PathBuf::from(r"C:\Users\Ελένη\Programs\Gazelle\gazelle-audio-serverw.exe");
        let bytes = shell_link(&target, "", target.parent().unwrap(), "Gazelle", &target, 0, None);
        assert_eq!(target_of(&bytes).as_deref(), Some(target.display().to_string().as_str()));
        // The ANSI copy is still there for an older reader, with the unspellable part replaced.
        let at = HEADER_SIZE + u32::from_le_bytes(bytes[HEADER_SIZE + 0x10..HEADER_SIZE + 0x14].try_into().unwrap()) as usize;
        let end = bytes[at..].iter().position(|&b| b == 0).unwrap() + at;
        assert_eq!(std::str::from_utf8(&bytes[at..end]).unwrap(), r"C:\Users\?????\Programs\Gazelle\gazelle-audio-serverw.exe");
    }

    #[test]
    fn arguments_are_written_only_when_there_are_some() {
        let exe = Path::new(r"C:\g\gazelle-audio-server.exe");
        let bytes = shell_link(exe, "--backend usb", exe.parent().unwrap(), "Gazelle", exe, 0, None);
        let flags = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        assert_eq!(flags & HAS_ARGUMENTS, HAS_ARGUMENTS);
        let text: String = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .map(|u| char::from_u32(u32::from(u)).unwrap_or('.'))
            .collect();
        assert!(text.contains("--backend usb"), "the arguments are in the string data: {text}");
    }

    #[test]
    fn the_size_the_block_declares_is_the_size_it_takes() {
        let bytes = link();
        let size = u32::from_le_bytes(bytes[HEADER_SIZE..HEADER_SIZE + 4].try_into().unwrap()) as usize;
        // What follows LinkInfo is the description, whose count is the length of "Gazelle".
        let at = HEADER_SIZE + size;
        assert_eq!(u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap()), 7, "the description follows the block");
        assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 0, 0], "the terminal block closes the file");
    }

    #[test]
    fn anything_this_module_did_not_write_is_not_read_as_a_shortcut() {
        assert_eq!(target_of(&[]), None);
        assert_eq!(target_of(&[0; 200]), None, "no header size, no class");
        let mut bytes = link();
        bytes[20] = 0; // clear HasLinkInfo
        assert_eq!(target_of(&bytes), None);
    }
}
