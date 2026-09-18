//! Turning `assets/gazelle.ico` into a Windows resource the linker puts in both binaries.
//!
//! Explorer, the taskbar, the Start Menu shortcut, the Add/Remove Programs entry and the
//! window's own title bar all read the icon out of the executable's `.rsrc` section. Nothing in
//! Rust puts one there, so something has to hand the linker an object file that carries it.
//!
//! # Why this is written by hand
//!
//! The usual route is a `.rc` script compiled by `rc.exe` (Windows SDK) or `llvm-rc`, driven by
//! a crate such as `embed-resource` or `winresource`. That is a build dependency **and** an
//! external tool that has to be found on every machine that builds a release; `rc.exe` is not on
//! `PATH` here, it lives under a versioned Windows Kits directory. What the tool produces,
//! though, is a COFF object holding two sections (the resource directory and the resource
//! bytes), and that is about two hundred lines of plain structure writing. So this writes it,
//! with no new dependency, no tool lookup, and byte-identical output on any machine.
//!
//! The layout is the one `cvtres` emits, and the linker's `$`-suffix ordering is what makes it
//! work: `.rsrc$01` (the directory) sorts before `.rsrc$02` (the data), and the two are merged
//! into one `.rsrc` section. Each `IMAGE_RESOURCE_DATA_ENTRY` has to name its bytes by RVA,
//! which is only known at link time, so each one carries an image-relative relocation against
//! the `.rsrc$02` section symbol.
//!
//! # What goes in
//!
//! One `RT_ICON` per image in the `.ico` (ids 1..n, in the file's order) and one `RT_GROUP_ICON`
//! named `1`. The group is what `LoadIcon`/`ExtractIcon` and the shell ask for, and **the
//! lowest-numbered group icon is the one Explorer shows for the file**, which is why it is 1.
//! The group's entries are the `.ico` directory's, with the 4-byte file offset replaced by the
//! 2-byte resource id. That substitution is the entire difference between the two formats.

use std::path::Path;

/// Resource types, from `winuser.h`.
const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
/// en-US, which is what `rc.exe` stamps on a resource with no `LANGUAGE` statement.
const LANGUAGE: u32 = 0x0409;

/// One image inside an `.ico`: its directory entry, and the bytes the entry points at.
#[derive(Debug)]
pub struct Image {
    /// The 16-byte `ICONDIRENTRY`, whose last 4 bytes (the file offset) the group icon replaces.
    pub entry: [u8; 16],
    pub body: Vec<u8>,
}

/// Split an `.ico` into its images, or say what is wrong with it.
///
/// Deliberately strict: a build that silently embedded half an icon would show the default one
/// and look exactly like a build that embedded none.
pub fn read_ico(bytes: &[u8]) -> Result<Vec<Image>, String> {
    if bytes.len() < 6 {
        return Err("not an .ico: shorter than its own header".into());
    }
    let reserved = u16::from_le_bytes([bytes[0], bytes[1]]);
    let kind = u16::from_le_bytes([bytes[2], bytes[3]]);
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if reserved != 0 || kind != 1 {
        return Err(format!("not an .ico: reserved {reserved}, type {kind} (an icon file is 0 and 1)"));
    }
    if count == 0 {
        return Err("the .ico holds no images".into());
    }
    let mut images = Vec::with_capacity(count);
    for i in 0..count {
        let at = 6 + i * 16;
        let entry: [u8; 16] = bytes
            .get(at..at + 16)
            .ok_or_else(|| format!("the .ico claims {count} images but stops inside entry {i}"))?
            .try_into()
            .unwrap();
        let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let body = bytes
            .get(offset..offset.checked_add(size).ok_or("an image's size and offset overflow")?)
            .ok_or_else(|| format!("image {i} runs past the end of the .ico"))?
            .to_vec();
        images.push(Image { entry, body });
    }
    Ok(images)
}

/// The `RT_GROUP_ICON` body: the `.ico` directory with each 4-byte offset replaced by the
/// 2-byte id of the `RT_ICON` holding those bytes.
pub fn group(images: &[Image]) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + 14 * images.len());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    for (i, image) in images.iter().enumerate() {
        out.extend_from_slice(&image.entry[..12]);
        out.extend_from_slice(&(icon_id(i)).to_le_bytes());
    }
    out
}

/// `RT_ICON` ids run from 1, in the order the `.ico` lists them.
fn icon_id(index: usize) -> u16 {
    index as u16 + 1
}

/// The machine and relocation type for a target architecture, or `None` where this has not been
/// worked out, in which case the build embeds nothing rather than emitting a bad object.
pub fn machine(arch: &str) -> Option<(u16, u16)> {
    match arch {
        // IMAGE_FILE_MACHINE_AMD64, IMAGE_REL_AMD64_ADDR32NB
        "x86_64" => Some((0x8664, 0x0003)),
        // IMAGE_FILE_MACHINE_I386, IMAGE_REL_I386_DIR32NB
        "x86" => Some((0x014C, 0x0007)),
        // IMAGE_FILE_MACHINE_ARM64, IMAGE_REL_ARM64_ADDR32NB
        "aarch64" => Some((0xAA64, 0x0002)),
        _ => None,
    }
}

/// Everything a resource tree holds: a type, a name and the bytes, in the order they are laid
/// out. Only integer ids are used here; named resources would need a string directory too.
struct Resource {
    kind: u32,
    id: u32,
    body: Vec<u8>,
}

/// A COFF object holding the icon, ready to be handed to the linker.
pub fn coff(images: &[Image], arch: &str) -> Result<Vec<u8>, String> {
    let (machine, relocation) = machine(arch).ok_or_else(|| format!("no COFF machine type is known for {arch}"))?;

    let mut resources: Vec<Resource> = images
        .iter()
        .enumerate()
        .map(|(i, image)| Resource { kind: RT_ICON, id: icon_id(i) as u32, body: image.body.clone() })
        .collect();
    resources.push(Resource { kind: RT_GROUP_ICON, id: 1, body: group(images) });
    // The resource directory is a search tree: every level must be sorted by its key, because
    // the loader binary-searches it.
    resources.sort_by_key(|r| (r.kind, r.id));

    let types: Vec<u32> = {
        let mut kinds: Vec<u32> = resources.iter().map(|r| r.kind).collect();
        kinds.dedup();
        kinds
    };

    // ----- the data section, and where each resource lands in it -----
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(resources.len());
    for resource in &resources {
        offsets.push(data.len() as u32);
        data.extend_from_slice(&resource.body);
        while data.len() % 8 != 0 {
            data.push(0);
        }
    }

    // ----- the directory section -----
    // Three levels of directory, then one data entry per resource. Sizes are known up front,
    // so the offsets can be written in one pass.
    let level1 = 16 + 8 * types.len();
    let level2: usize = 16 * types.len() + 8 * resources.len();
    let level3 = resources.len() * (16 + 8);
    let entries_at = level1 + level2 + level3;

    let mut directory = Vec::with_capacity(entries_at + 16 * resources.len());
    let mut relocations: Vec<u32> = Vec::new();

    // Level 1: one entry per type, each pointing at a name directory.
    write_directory(&mut directory, types.len());
    let mut names_at = level1;
    for kind in &types {
        let count = resources.iter().filter(|r| r.kind == *kind).count();
        write_entry(&mut directory, *kind, names_at as u32, true);
        names_at += 16 + 8 * count;
    }
    // Level 2: one entry per resource name, each pointing at a language directory.
    let mut languages_at = level1 + level2;
    for kind in &types {
        let of_this_kind: Vec<&Resource> = resources.iter().filter(|r| r.kind == *kind).collect();
        write_directory(&mut directory, of_this_kind.len());
        for resource in of_this_kind {
            write_entry(&mut directory, resource.id, languages_at as u32, true);
            languages_at += 16 + 8;
        }
    }
    // Level 3: one language each, pointing at the data entry. A leaf, so no high bit.
    for (index, _) in resources.iter().enumerate() {
        write_directory(&mut directory, 1);
        write_entry(&mut directory, LANGUAGE, (entries_at + 16 * index) as u32, false);
    }
    // The data entries themselves. `OffsetToData` is an RVA the linker fills in, so it is
    // written as the offset within the data section and relocated against that section.
    for (index, resource) in resources.iter().enumerate() {
        relocations.push(directory.len() as u32);
        directory.extend_from_slice(&offsets[index].to_le_bytes());
        directory.extend_from_slice(&(resource.body.len() as u32).to_le_bytes());
        directory.extend_from_slice(&0u32.to_le_bytes()); // CodePage: none, these are binary
        directory.extend_from_slice(&0u32.to_le_bytes()); // Reserved
    }
    debug_assert_eq!(directory.len(), entries_at + 16 * resources.len());

    // ----- the object file -----
    const HEADER: usize = 20;
    const SECTION_HEADER: usize = 40;
    const RELOCATION: usize = 10;
    const SYMBOL: usize = 18;
    // Two section symbols, each with one auxiliary record: `.rsrc$01` is 0 and 1, `.rsrc$02`
    // is 2 and 3. The relocations name index 2.
    const DATA_SYMBOL: u32 = 2;
    const SYMBOLS: usize = 4;

    let sections_at = HEADER + 2 * SECTION_HEADER;
    let directory_at = sections_at;
    let data_at = directory_at + directory.len();
    let relocations_at = data_at + data.len();
    let symbols_at = relocations_at + relocations.len() * RELOCATION;

    let mut out = Vec::with_capacity(symbols_at + SYMBOLS * SYMBOL + 4);
    out.extend_from_slice(&machine.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // two sections
    out.extend_from_slice(&0u32.to_le_bytes()); // no timestamp: the same input gives the same file
    out.extend_from_slice(&(symbols_at as u32).to_le_bytes());
    out.extend_from_slice(&(SYMBOLS as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // no optional header
    out.extend_from_slice(&0u16.to_le_bytes()); // no characteristics

    // IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_ALIGN_8BYTES | IMAGE_SCN_MEM_READ
    const CHARACTERISTICS: u32 = 0x0000_0040 | 0x0040_0000 | 0x4000_0000;
    write_section(&mut out, b".rsrc$01", directory.len(), directory_at, relocations.len(), relocations_at, CHARACTERISTICS);
    write_section(&mut out, b".rsrc$02", data.len(), data_at, 0, 0, CHARACTERISTICS);

    out.extend_from_slice(&directory);
    out.extend_from_slice(&data);
    for at in &relocations {
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&DATA_SYMBOL.to_le_bytes());
        out.extend_from_slice(&relocation.to_le_bytes());
    }
    write_section_symbol(&mut out, b".rsrc$01", 1, directory.len(), relocations.len());
    write_section_symbol(&mut out, b".rsrc$02", 2, data.len(), 0);
    // The string table, which is empty but whose four-byte length must be there.
    out.extend_from_slice(&4u32.to_le_bytes());
    Ok(out)
}

/// `IMAGE_RESOURCE_DIRECTORY`: no named entries anywhere here, only integer ids.
fn write_directory(out: &mut Vec<u8>, entries: usize) {
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
    out.extend_from_slice(&0u16.to_le_bytes()); // MajorVersion
    out.extend_from_slice(&0u16.to_le_bytes()); // MinorVersion
    out.extend_from_slice(&0u16.to_le_bytes()); // NumberOfNamedEntries
    out.extend_from_slice(&(entries as u16).to_le_bytes());
}

/// `IMAGE_RESOURCE_DIRECTORY_ENTRY`. The high bit of the offset says "another directory".
fn write_entry(out: &mut Vec<u8>, id: u32, offset: u32, subdirectory: bool) {
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(if subdirectory { offset | 0x8000_0000 } else { offset }).to_le_bytes());
}

fn write_section(out: &mut Vec<u8>, name: &[u8; 8], size: usize, at: usize, relocations: usize, relocations_at: usize, flags: u32) {
    out.extend_from_slice(name);
    out.extend_from_slice(&0u32.to_le_bytes()); // VirtualSize: zero in an object
    out.extend_from_slice(&0u32.to_le_bytes()); // VirtualAddress: the linker decides
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(at as u32).to_le_bytes());
    out.extend_from_slice(&(relocations_at as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // PointerToLinenumbers
    out.extend_from_slice(&(relocations as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // NumberOfLinenumbers
    out.extend_from_slice(&flags.to_le_bytes());
}

/// A static symbol for a section, plus the auxiliary section-definition record the format
/// requires. The relocations point at the second of these.
fn write_section_symbol(out: &mut Vec<u8>, name: &[u8; 8], section: i16, length: usize, relocations: usize) {
    out.extend_from_slice(name);
    out.extend_from_slice(&0u32.to_le_bytes()); // Value: the start of the section
    out.extend_from_slice(&section.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // Type
    out.push(3); // IMAGE_SYM_CLASS_STATIC
    out.push(1); // one auxiliary record

    out.extend_from_slice(&(length as u32).to_le_bytes());
    out.extend_from_slice(&(relocations as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // NumberOfLinenumbers
    out.extend_from_slice(&0u32.to_le_bytes()); // CheckSum
    out.extend_from_slice(&0u16.to_le_bytes()); // Number
    out.push(0); // Selection: not a COMDAT
    out.extend_from_slice(&[0, 0, 0]);
}

/// Write the object beside the build's other outputs and say where it is, or say why not.
///
/// The caller decides what to do with a failure. Embedding nothing is not fatal (the app runs
/// without an icon), but it is never silent.
pub fn write_object(ico: &Path, out: &Path, arch: &str) -> Result<(), String> {
    let bytes = std::fs::read(ico).map_err(|e| format!("reading {}: {e}", ico.display()))?;
    let images = read_ico(&bytes)?;
    let object = coff(&images, arch)?;
    std::fs::write(out, object).map_err(|e| format!("writing {}: {e}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed icon, read the way the build reads it.
    fn committed() -> Vec<u8> {
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/gazelle.ico")).unwrap()
    }

    #[test]
    fn the_committed_icon_holds_the_four_sizes_windows_asks_for() {
        let images = read_ico(&committed()).unwrap();
        let sizes: Vec<u32> = images
            .iter()
            .map(|i| if i.entry[0] == 0 { 256 } else { i.entry[0] as u32 })
            .collect();
        assert_eq!(sizes, [16, 32, 48, 256], "the shortcut, the taskbar and Explorer's large views each want their own");
        for image in &images {
            let declared = u32::from_le_bytes(image.entry[8..12].try_into().unwrap()) as usize;
            assert_eq!(image.body.len(), declared, "an image's bytes must be the length its entry claims");
        }
    }

    #[test]
    fn a_truncated_or_foreign_file_is_refused_rather_than_half_embedded() {
        assert!(read_ico(b"").unwrap_err().contains("shorter"));
        assert!(read_ico(b"\x89PNG\r\n\x1a\n").unwrap_err().contains("not an .ico"));
        assert!(read_ico(&[0, 0, 1, 0, 0, 0]).unwrap_err().contains("no images"));
        // One entry promised, none there.
        assert!(read_ico(&[0, 0, 1, 0, 1, 0]).unwrap_err().contains("stops inside entry 0"));
        // An entry whose image runs off the end.
        let mut bytes = committed();
        bytes.truncate(bytes.len() - 1);
        assert!(read_ico(&bytes).unwrap_err().contains("past the end"));
    }

    #[test]
    fn the_group_replaces_each_file_offset_with_the_id_of_the_icon_holding_those_bytes() {
        let images = read_ico(&committed()).unwrap();
        let group = group(&images);
        assert_eq!(&group[..6], &[0, 0, 1, 0, images.len() as u8, 0]);
        for (i, image) in images.iter().enumerate() {
            let at = 6 + 14 * i;
            assert_eq!(&group[at..at + 12], &image.entry[..12], "everything but the offset is copied through");
            assert_eq!(u16::from_le_bytes(group[at + 12..at + 14].try_into().unwrap()), i as u16 + 1);
        }
        assert_eq!(group.len(), 6 + 14 * images.len());
    }

    /// Walk the directory the way the Windows loader does, and find the icons.
    #[test]
    fn the_object_carries_a_resource_tree_the_loader_can_walk() {
        let images = read_ico(&committed()).unwrap();
        let object = coff(&images, "x86_64").unwrap();

        // The file header, then the two sections the linker merges.
        assert_eq!(u16::from_le_bytes(object[0..2].try_into().unwrap()), 0x8664);
        assert_eq!(u16::from_le_bytes(object[2..4].try_into().unwrap()), 2);
        assert_eq!(&object[20..28], b".rsrc$01");
        assert_eq!(&object[60..68], b".rsrc$02");

        let directory_at = u32::from_le_bytes(object[20 + 20..20 + 24].try_into().unwrap()) as usize;
        let directory_len = u32::from_le_bytes(object[20 + 16..20 + 20].try_into().unwrap()) as usize;
        let data_at = u32::from_le_bytes(object[60 + 20..60 + 24].try_into().unwrap()) as usize;
        let tree = &object[directory_at..directory_at + directory_len];
        let data = &object[data_at..];

        // Level 1: RT_ICON (3) then RT_GROUP_ICON (14), sorted, both subdirectories.
        assert_eq!(u16::from_le_bytes(tree[14..16].try_into().unwrap()), 2, "two resource types");
        let types: Vec<(u32, u32)> = (0..2)
            .map(|i| {
                let at = 16 + 8 * i;
                (
                    u32::from_le_bytes(tree[at..at + 4].try_into().unwrap()),
                    u32::from_le_bytes(tree[at + 4..at + 8].try_into().unwrap()),
                )
            })
            .collect();
        assert_eq!(types.iter().map(|t| t.0).collect::<Vec<_>>(), [RT_ICON, RT_GROUP_ICON]);
        assert!(types.iter().all(|t| t.1 & 0x8000_0000 != 0), "both are subdirectories");

        // Follow RT_ICON down to each leaf and check the bytes are the .ico's, unchanged.
        let icons = (types[0].1 & 0x7FFF_FFFF) as usize;
        assert_eq!(u16::from_le_bytes(tree[icons + 14..icons + 16].try_into().unwrap()) as usize, images.len());
        for (i, image) in images.iter().enumerate() {
            let at = icons + 16 + 8 * i;
            assert_eq!(u32::from_le_bytes(tree[at..at + 4].try_into().unwrap()), i as u32 + 1, "ids run from 1");
            let languages = (u32::from_le_bytes(tree[at + 4..at + 8].try_into().unwrap()) & 0x7FFF_FFFF) as usize;
            assert_eq!(u16::from_le_bytes(tree[languages + 14..languages + 16].try_into().unwrap()), 1);
            let entry = u32::from_le_bytes(tree[languages + 20..languages + 24].try_into().unwrap()) as usize;
            assert_eq!(entry & 0x8000_0000, 0, "a language entry is a leaf, not another directory");
            let offset = u32::from_le_bytes(tree[entry..entry + 4].try_into().unwrap()) as usize;
            let size = u32::from_le_bytes(tree[entry + 4..entry + 8].try_into().unwrap()) as usize;
            assert_eq!(&data[offset..offset + size], image.body.as_slice());
        }

        // The group is id 1, which is what makes it the icon Explorer shows for the file.
        let groups = (types[1].1 & 0x7FFF_FFFF) as usize;
        assert_eq!(u16::from_le_bytes(tree[groups + 14..groups + 16].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(tree[groups + 16..groups + 20].try_into().unwrap()), 1);
    }

    #[test]
    fn every_data_entrys_rva_field_is_relocated_against_the_data_section() {
        let images = read_ico(&committed()).unwrap();
        let object = coff(&images, "x86_64").unwrap();
        let count = u16::from_le_bytes(object[20 + 32..20 + 34].try_into().unwrap()) as usize;
        assert_eq!(count, images.len() + 1, "one per icon, plus the group");

        let at = u32::from_le_bytes(object[20 + 24..20 + 28].try_into().unwrap()) as usize;
        let directory_len = u32::from_le_bytes(object[20 + 16..20 + 20].try_into().unwrap()) as usize;
        for i in 0..count {
            let r = at + 10 * i;
            let field = u32::from_le_bytes(object[r..r + 4].try_into().unwrap()) as usize;
            assert!(field + 4 <= directory_len, "the relocated field is inside the directory section");
            // Every data entry is 16 bytes and the relocation is on its first field, so the
            // fields sit at the very end of the section, one per resource.
            assert_eq!((directory_len - field) % 16, 0);
            assert_eq!(u32::from_le_bytes(object[r + 4..r + 8].try_into().unwrap()), 2, "the .rsrc$02 section symbol");
            assert_eq!(u16::from_le_bytes(object[r + 8..r + 10].try_into().unwrap()), 3, "IMAGE_REL_AMD64_ADDR32NB");
        }
    }

    #[test]
    fn the_symbol_table_defines_both_sections_and_the_string_table_is_present() {
        let images = read_ico(&committed()).unwrap();
        let object = coff(&images, "x86_64").unwrap();
        let at = u32::from_le_bytes(object[8..12].try_into().unwrap()) as usize;
        assert_eq!(u32::from_le_bytes(object[12..16].try_into().unwrap()), 4, "two symbols, each with one aux record");
        assert_eq!(&object[at..at + 8], b".rsrc$01");
        assert_eq!(i16::from_le_bytes(object[at + 12..at + 14].try_into().unwrap()), 1);
        assert_eq!(object[at + 16], 3, "IMAGE_SYM_CLASS_STATIC");
        assert_eq!(object[at + 17], 1, "one auxiliary record");
        assert_eq!(&object[at + 36..at + 44], b".rsrc$02");
        assert_eq!(i16::from_le_bytes(object[at + 48..at + 50].try_into().unwrap()), 2);
        assert_eq!(object.len(), at + 4 * 18 + 4, "the four-byte empty string table ends the file");
    }

    #[test]
    fn the_object_is_the_same_bytes_every_run_and_is_written_only_for_known_architectures() {
        let images = read_ico(&committed()).unwrap();
        assert_eq!(coff(&images, "x86_64").unwrap(), coff(&images, "x86_64").unwrap());
        assert_ne!(coff(&images, "x86_64").unwrap(), coff(&images, "aarch64").unwrap());
        assert_eq!(machine("x86"), Some((0x014C, 0x0007)));
        assert!(machine("riscv64").is_none());
        assert!(coff(&images, "riscv64").unwrap_err().contains("riscv64"));
    }
}
