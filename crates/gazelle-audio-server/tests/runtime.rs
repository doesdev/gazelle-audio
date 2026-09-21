//! Both shipped programs start on a clean Windows, with nothing installed beforehand.
//!
//! Rust's MSVC target links Microsoft's C runtime as a DLL (`vcruntime140.dll`) unless it is told
//! to build it in, and a PC that has never had the Visual C++ Redistributable installed then
//! refuses to start the program at all ("VCRUNTIME140.dll was not found"). `.cargo/config.toml`
//! builds it in (`+crt-static`); this reads each **built** executable's import table and fails if
//! either still asks Windows for a runtime DLL.
//!
//! The aggregate driver is held to the same rule, since a DAW loading it on such a PC would fail
//! the same way: the copy cargo builds beside the server whenever the workspace is built, and the
//! copy a release build carries inside the server, which is the one that ships.

#[cfg(windows)]
fn u16_at(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap()) as usize
}

#[cfg(windows)]
fn u32_at(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

/// The DLL names in a PE32+ executable's or DLL's import directory, lower-cased.
#[cfg(windows)]
fn imported_dlls(b: &[u8]) -> Vec<String> {
    let pe = u32_at(b, 0x3c);
    assert_eq!(&b[pe..pe + 4], b"PE\0\0", "a PE executable");
    let sections = u16_at(b, pe + 6);
    let optional = pe + 24;
    let optional_len = u16_at(b, pe + 20);
    assert_eq!(u16_at(b, optional), 0x20b, "PE32+ (64-bit)");
    // Data directory 1 is the import table; the directories start 112 bytes into a PE32+ header.
    let import_rva = u32_at(b, optional + 112 + 8);
    let table = optional + optional_len;
    let file_offset = |rva: usize| -> usize {
        (0..sections)
            .map(|i| table + 40 * i)
            .find_map(|s| {
                let (virtual_address, raw_size, raw_at) = (u32_at(b, s + 12), u32_at(b, s + 16), u32_at(b, s + 20));
                (rva >= virtual_address && rva < virtual_address + raw_size.max(u32_at(b, s + 8))).then(|| rva - virtual_address + raw_at)
            })
            .expect("an address inside a section")
    };
    let mut names = Vec::new();
    let mut descriptor = file_offset(import_rva);
    // Each descriptor is 20 bytes; an all-zero one ends the table. Its name is at offset 12.
    while b[descriptor..descriptor + 20].iter().any(|&x| x != 0) {
        let name_at = file_offset(u32_at(b, descriptor + 12));
        let end = b[name_at..].iter().position(|&x| x == 0).unwrap();
        names.push(String::from_utf8_lossy(&b[name_at..name_at + end]).to_ascii_lowercase());
        descriptor += 20;
    }
    names
}

/// Fail if the PE file `bytes`, called `what`, asks Windows for a C runtime DLL.
#[cfg(windows)]
fn assert_no_runtime_dll(what: &str, bytes: &[u8]) {
    let dlls = imported_dlls(bytes);
    assert!(dlls.iter().any(|d| d == "kernel32.dll"), "the import table was read: {dlls:?}");
    let runtime: Vec<_> = dlls.iter().filter(|d| d.starts_with("vcruntime") || d.starts_with("msvcp") || d.starts_with("api-ms-win-crt")).collect();
    assert!(runtime.is_empty(), "{what} still needs {runtime:?} at run time; see .cargo/config.toml");
}

#[cfg(windows)]
#[test]
fn neither_program_needs_the_visual_c_runtime_installed() {
    for binary in [env!("CARGO_BIN_EXE_gazelle-audio-server"), env!("CARGO_BIN_EXE_gazelle-audio-serverw")] {
        assert_no_runtime_dll(binary, &std::fs::read(binary).unwrap());
    }
}

/// The driver cargo built beside the server, when it did (`cargo test --workspace` builds it; a
/// test of this package alone may not), and the driver this build carries, when it carries one.
/// The release runs this test on the build it ships, which always carries one.
#[cfg(windows)]
#[test]
fn nor_does_the_aggregate_driver() {
    let beside = std::path::Path::new(env!("CARGO_BIN_EXE_gazelle-audio-server")).with_file_name("gazelle_aggregate.dll");
    let mut checked = 0;
    if beside.is_file() {
        assert_no_runtime_dll(&beside.display().to_string(), &std::fs::read(&beside).unwrap());
        checked += 1;
    }
    if let Some(carried) = gazelle_audio_server::aggregate::bundled::carried() {
        assert_no_runtime_dll("the aggregate driver this build carries", carried);
        checked += 1;
    }
    if checked == 0 {
        eprintln!("no aggregate driver to check: none beside {} and none carried", beside.display());
    }
}
