//! The app's own icon: in the file it is generated from, and in both binaries that ship.
//!
//! Three things can go wrong and each would be invisible until someone looked at a Start Menu
//! entry on a stranger's machine:
//!
//! 1. the committed `.ico` drifts from the icon the tray and the window draw (`icon::rgba`);
//! 2. the generator stops producing the committed file, so nobody can regenerate it;
//! 3. the resource stops reaching one of the binaries — most likely the windowless one, which
//!    is the one the shortcut and the Add/Remove Programs entry name.
//!
//! So this reads the `.ico` back, compares every pixel with the drawn icon, re-runs the
//! generator where Python is available, and walks the resource directory of both **built**
//! executables looking for the icon group.

use std::path::{Path, PathBuf};

const ICO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/gazelle.ico");
const SIZES: [u32; 4] = [16, 32, 48, 256];

// ---------------------------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------------------------

struct Entry {
    size: u32,
    body: Vec<u8>,
}

fn entries(bytes: &[u8]) -> Vec<Entry> {
    assert_eq!(&bytes[0..4], &[0, 0, 1, 0], "an .ico begins with a zero and a type of 1");
    let count = u16::from_le_bytes(bytes[4..6].try_into().unwrap()) as usize;
    (0..count)
        .map(|i| {
            let at = 6 + 16 * i;
            let size = if bytes[at] == 0 { 256 } else { bytes[at] as u32 };
            let length = u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(bytes[at + 12..at + 16].try_into().unwrap()) as usize;
            assert_eq!(bytes[at + 1] as u32, size % 256, "width and height are the same");
            Entry { size, body: bytes[offset..offset + length].to_vec() }
        })
        .collect()
}

/// The pixels of a 32-bit BITMAPINFOHEADER image, back in `icon::rgba`'s order: top row first.
fn bmp_pixels(size: u32, body: &[u8]) -> Vec<[u8; 4]> {
    assert_eq!(u32::from_le_bytes(body[0..4].try_into().unwrap()), 40, "a BITMAPINFOHEADER");
    assert_eq!(u32::from_le_bytes(body[4..8].try_into().unwrap()), size);
    assert_eq!(u32::from_le_bytes(body[8..12].try_into().unwrap()), size * 2, "the height counts the mask rows");
    assert_eq!(u16::from_le_bytes(body[14..16].try_into().unwrap()), 32, "32 bits per pixel");
    let (size, mut out) = (size as usize, Vec::new());
    for y in (0..size).rev() {
        for x in 0..size {
            let at = 40 + (y * size + x) * 4;
            let (b, g, r, a) = (body[at], body[at + 1], body[at + 2], body[at + 3]);
            out.push([r, g, b, a]);
        }
    }
    out
}

#[test]
fn the_committed_icon_is_the_icon_the_tray_and_the_window_draw() {
    let bytes = std::fs::read(ICO).expect("assets/gazelle.ico is missing; run refs/tools/scripts/make_icon.py");
    let entries = entries(&bytes);
    assert_eq!(entries.iter().map(|e| e.size).collect::<Vec<_>>(), SIZES);

    for entry in &entries {
        if entry.size == 256 {
            // The largest image is a PNG, which this test does not decode; that it is one, and
            // that it is square and 256 wide, is what the shell reads out of its own header.
            assert_eq!(&entry.body[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(u32::from_be_bytes(entry.body[16..20].try_into().unwrap()), 256);
            assert_eq!(u32::from_be_bytes(entry.body[20..24].try_into().unwrap()), 256);
            assert_eq!(entry.body[24], 8, "eight bits a channel");
            assert_eq!(entry.body[25], 6, "colour type 6: RGBA");
            continue;
        }
        let drawn = gazelle_audio_server::icon::rgba(entry.size as usize);
        assert_eq!(
            bmp_pixels(entry.size, &entry.body),
            drawn,
            "the {}px image in the .ico is not what icon::rgba draws — regenerate it with refs/tools/scripts/make_icon.py",
            entry.size
        );
    }
}

/// The generator has to be able to write the file that is committed, or nobody can change the
/// icon. Skipped where Python is not installed rather than failing someone else's build.
#[test]
fn the_generator_writes_exactly_the_committed_file() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../refs/tools/scripts/make_icon.py");
    assert!(script.is_file(), "the icon's generator is part of the tree: {}", script.display());

    let Some(python) = python() else {
        eprintln!("skipped: no python on PATH, so the generator could not be re-run");
        return;
    };
    let out = std::env::temp_dir().join(format!("gazelle-icon-{}.ico", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let run = std::process::Command::new(&python)
        .arg(&script)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap_or_else(|e| panic!("running {}: {e}", script.display()));
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));

    let written = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    assert_eq!(
        written,
        std::fs::read(ICO).unwrap(),
        "refs/tools/scripts/make_icon.py no longer writes the committed assets/gazelle.ico"
    );
}

fn python() -> Option<PathBuf> {
    ["python", "python3"].into_iter().find_map(|name| {
        std::process::Command::new(name)
            .arg("--version")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|_| PathBuf::from(name))
    })
}

// ---------------------------------------------------------------------------------------------
// The binaries
// ---------------------------------------------------------------------------------------------

/// Walk a built executable's resource directory and return every `RT_ICON` body and the one
/// `RT_GROUP_ICON` body, decoded from the PE exactly as the loader would find them.
#[cfg(windows)]
fn resources_in(path: &str) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let image = std::fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    let pe = u32::from_le_bytes(image[0x3c..0x40].try_into().unwrap()) as usize;
    assert_eq!(&image[pe..pe + 4], b"PE\0\0");
    let sections = u16::from_le_bytes(image[pe + 6..pe + 8].try_into().unwrap()) as usize;
    let optional = u16::from_le_bytes(image[pe + 20..pe + 22].try_into().unwrap()) as usize;
    // The resource directory is data directory 2, and PE32+ puts the directories 16 bytes
    // further on than PE32 does.
    let magic = u16::from_le_bytes(image[pe + 24..pe + 26].try_into().unwrap());
    let directories = pe + 24 + if magic == 0x20b { 112 } else { 96 };
    let rva = u32::from_le_bytes(image[directories + 16..directories + 20].try_into().unwrap()) as usize;
    assert_ne!(rva, 0, "{path} has no resource directory at all");

    // RVA to file offset, through the section table.
    let headers = pe + 24 + optional;
    let mut base = None;
    for i in 0..sections {
        let at = headers + 40 * i;
        let virtual_address = u32::from_le_bytes(image[at + 12..at + 16].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(image[at + 8..at + 12].try_into().unwrap()) as usize;
        let raw = u32::from_le_bytes(image[at + 20..at + 24].try_into().unwrap()) as usize;
        if rva >= virtual_address && rva < virtual_address + size {
            base = Some((virtual_address, raw));
        }
    }
    let (virtual_address, raw) = base.expect("the resource directory's RVA is in no section");
    let file_of = |rva: usize| rva - virtual_address + raw;
    let root = file_of(rva);

    let entries_of = |at: usize| -> Vec<(u32, u32)> {
        let named = u16::from_le_bytes(image[at + 12..at + 14].try_into().unwrap()) as usize;
        let ids = u16::from_le_bytes(image[at + 14..at + 16].try_into().unwrap()) as usize;
        (0..named + ids)
            .map(|i| {
                let e = at + 16 + 8 * i;
                (
                    u32::from_le_bytes(image[e..e + 4].try_into().unwrap()),
                    u32::from_le_bytes(image[e + 4..e + 8].try_into().unwrap()),
                )
            })
            .collect()
    };

    let mut icons = Vec::new();
    let mut groups = Vec::new();
    for (kind, offset) in entries_of(root) {
        if kind != 3 && kind != 14 {
            continue;
        }
        for (_, names) in entries_of(root + (offset & 0x7fff_ffff) as usize) {
            for (_, leaf) in entries_of(root + (names & 0x7fff_ffff) as usize) {
                let entry = root + leaf as usize;
                let at = file_of(u32::from_le_bytes(image[entry..entry + 4].try_into().unwrap()) as usize);
                let size = u32::from_le_bytes(image[entry + 4..entry + 8].try_into().unwrap()) as usize;
                let body = image[at..at + size].to_vec();
                if kind == 3 {
                    icons.push(body);
                } else {
                    groups.push(body);
                }
            }
        }
    }
    (icons, groups)
}

/// Both shipped binaries carry the icon: the console one is what Explorer and the taskbar show,
/// the windowless one is what the Start Menu shortcut and the Add/Remove Programs entry name
/// (`install/registry.rs`'s `DisplayIcon`), and a release that lost either would look wrong in
/// a different place.
#[cfg(windows)]
#[test]
fn both_binaries_carry_the_icon() {
    let ico = std::fs::read(ICO).unwrap();
    let expected = entries(&ico);

    for binary in [env!("CARGO_BIN_EXE_gazelle-audio-server"), env!("CARGO_BIN_EXE_gazelle-audio-serverw")] {
        let (icons, groups) = resources_in(binary);
        assert_eq!(icons.len(), expected.len(), "{binary} should carry one RT_ICON per image in the .ico");
        assert_eq!(groups.len(), 1, "{binary} should carry exactly one RT_GROUP_ICON");

        for image in &expected {
            assert!(
                icons.contains(&image.body),
                "{binary} does not carry the {}px image, byte for byte, from assets/gazelle.ico",
                image.size
            );
        }

        // The group is what the shell reads. Its entries name the sizes and the ids of the
        // RT_ICONs above, so a group naming an icon that is not there would show nothing.
        let group = &groups[0];
        assert_eq!(&group[0..4], &[0, 0, 1, 0]);
        let count = u16::from_le_bytes(group[4..6].try_into().unwrap()) as usize;
        assert_eq!(count, expected.len());
        let sizes: Vec<u32> = (0..count).map(|i| if group[6 + 14 * i] == 0 { 256 } else { group[6 + 14 * i] as u32 }).collect();
        assert_eq!(sizes, SIZES, "{binary}'s icon group must offer every size");
        for i in 0..count {
            let at = 6 + 14 * i;
            let length = u32::from_le_bytes(group[at + 8..at + 12].try_into().unwrap()) as usize;
            let id = u16::from_le_bytes(group[at + 12..at + 14].try_into().unwrap());
            assert!(id >= 1 && (id as usize) <= icons.len(), "{binary}'s group names icon {id}, which is not there");
            assert_eq!(icons[id as usize - 1].len(), length, "{binary}'s group disagrees with the icon about its length");
        }
    }
}
