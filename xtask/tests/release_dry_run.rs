//! A whole release, cut and consumed, without publishing anything.
//!
//! The individual pieces are tested elsewhere: `xtask`'s own unit tests check that what `sign`
//! writes is what `update::verify` accepts, and the server's `tests/update.rs` drives the
//! updater against an in-memory release source. What neither covers is the **path a real
//! release takes end to end**, and that is exactly where a first release can go wrong:
//!
//! 1. `xtask keygen` into a path outside the repository — this test never writes a key inside
//!    one, and neither does the tool;
//! 2. a directory of real files, signed by the **real `xtask` binary** over the command line a
//!    person would type;
//! 3. an HTTP server serving **that directory from disk** in the shape the updater expects;
//! 4. a real [`Updater`] pointed at it, which checks, downloads, verifies, and stages;
//! 5. the staged binary **runs**, which is what a restart would do.
//!
//! The server crate's fake release source could not be reused as it stands: it lives inside
//! that crate's test tree, and it serves assets from a map in memory rather than the signed
//! directory on disk, which is the thing being proved here. The wire shape — the releases
//! listing and the asset URLs — is the same, deliberately.
//!
//! Nothing here resolves a name or opens a socket off localhost, nothing is uploaded, and the
//! "binaries" are copies of `xtask` itself, so a staged file can be executed to prove which
//! bytes ended up in place.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gazelle_audio_server::update::release::{parse_sums, to_hex, SIGNATURE_NAME, SUMS_NAME};
use gazelle_audio_server::update::settings::{Channel, Settings};
use gazelle_audio_server::update::{stage, verify, State, Updater};

const XTASK: &str = env!("CARGO_BIN_EXE_xtask");
/// The triple the assets are named for. The updater asks for the one it was built for, and this
/// test builds the release for that same one, as a real release for this platform would.
const TARGET: &str = gazelle_audio_server::update::TARGET;

fn exe_suffix() -> &'static str {
    if TARGET.contains("windows") {
        ".exe"
    } else {
        ""
    }
}

fn asset_name(stem: &str) -> String {
    format!("{stem}-{TARGET}{}", exe_suffix())
}

/// A scratch directory, **outside the repository** — `keygen` refuses to write a key inside a
/// git working tree, and this test would be pointless if it did.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("gazelle-release-dry-run-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------------------------
// Step 1 and 2: the key, and the signed directory
// ---------------------------------------------------------------------------------------------

/// `xtask keygen --out <file>`, returning the public half it printed.
fn keygen(out: &Path) -> String {
    let run = Command::new(XTASK).args(["keygen", "--out"]).arg(out).output().expect("running xtask keygen");
    assert!(run.status.success(), "keygen failed: {}", String::from_utf8_lossy(&run.stderr));
    let printed = String::from_utf8(run.stdout).unwrap();
    // The tool prints the whole command line to build with; the key is the value in it.
    let key = printed
        .split("GAZELLE_UPDATE_PUBKEY=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("keygen printed no public key:\n{printed}"))
        .to_string();
    assert_eq!(key.len(), 64, "a public key is 32 hex-encoded bytes");
    assert!(out.is_file(), "the private half is written where it was asked for");
    key
}

/// A binary that can be run and told apart from another: this test's own executable plus a
/// marker appended past the end of the image, which Windows ignores and a digest does not.
fn a_binary(path: &Path, marker: &str) {
    let mut bytes = std::fs::read(XTASK).unwrap();
    bytes.extend_from_slice(format!("\n// {marker}\n").as_bytes());
    std::fs::write(path, bytes).unwrap();
}

/// Everything the release carries, under the names the updater asks for.
fn build_release(dist: &Path, marker: &str) {
    for stem in ["gazelle-audio-server", "gazelle-audio-serverw"] {
        a_binary(&dist.join(asset_name(stem)), &format!("{stem} {marker}"));
    }
    // A release also carries a zip for people downloading by hand. The updater ignores it; that
    // it is signed along with everything else is the point of it being here.
    std::fs::write(dist.join(format!("gazelle-audio-{TARGET}.zip")), b"PK\x03\x04 not a real zip, but it is signed").unwrap();
}

/// `xtask sign --dir <dist> --key <file>`.
fn sign(dist: &Path, key: &Path) {
    let run = Command::new(XTASK).args(["sign", "--dir"]).arg(dist).arg("--key").arg(key).output().expect("running xtask sign");
    assert!(run.status.success(), "sign failed: {}", String::from_utf8_lossy(&run.stderr));
}

// ---------------------------------------------------------------------------------------------
// Step 3: a release source that serves the signed directory from disk
// ---------------------------------------------------------------------------------------------

/// A blocking HTTP/1.1 server on localhost speaking the two routes the updater uses:
/// `GET /repos/{owner}/{repo}/releases` and `GET /dl/{name}`. Files come off disk, so what the
/// updater downloads is exactly what `xtask sign` hashed.
struct Source {
    base: String,
    stop: Arc<AtomicBool>,
    /// Every path asked for, so a test can say what was *not* fetched.
    seen: Arc<Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Source {
    fn serving(dist: PathBuf, tag: &str, prerelease: bool) -> Source {
        let listener = TcpListener::bind::<SocketAddr>("127.0.0.1:0".parse().unwrap()).unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));

        let (listing_base, tag, stopping, asked) = (base.clone(), tag.to_string(), stop.clone(), seen.clone());
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve_one(stream, &dist, &listing_base, &tag, prerelease, &asked),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(5)),
                    Err(e) => panic!("accepting: {e}"),
                }
            }
        });
        Source { base, stop, seen, thread: Some(thread) }
    }

    /// The paths fetched so far.
    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    /// The assets fetched so far — the only requests that move megabytes.
    fn downloads(&self) -> Vec<String> {
        self.requests().into_iter().filter(|p| p.starts_with("/dl/")).collect()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve_one(mut stream: TcpStream, dist: &Path, base: &str, tag: &str, prerelease: bool, asked: &Mutex<Vec<String>>) {
    stream.set_nonblocking(false).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request = String::new();
    reader.read_line(&mut request).unwrap();
    // Drain the headers; nothing here reads a body.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
            break;
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap().to_string();
    asked.lock().unwrap().push(path.clone());

    let (status, kind, body) = if path.contains("/releases") {
        let listing = listing(dist, base, tag, prerelease);
        ("200 OK", "application/json", listing.into_bytes())
    } else if let Some(name) = path.strip_prefix("/dl/") {
        // The name came off a URL this server itself wrote, but it still reaches the filesystem,
        // so anything with a separator in it is refused rather than joined.
        match std::fs::read(dist.join(name)).ok().filter(|_| !name.contains('/') && !name.contains('\\')) {
            Some(bytes) => ("200 OK", "application/octet-stream", bytes),
            None => ("404 Not Found", "text/plain", b"no such asset".to_vec()),
        }
    } else {
        ("404 Not Found", "text/plain", b"no such route".to_vec())
    };

    let head = format!("HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    // Close gracefully rather than dropping the socket: on Windows, closing with anything still
    // unread resets the connection, and the client sees a reset instead of the answer it was
    // sent. Say "no more from me", then read to the end before letting the socket go.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
    let mut drain = [0u8; 512];
    while matches!(stream.read(&mut drain), Ok(n) if n > 0) {}
}

/// One release, in the shape `GET /repos/{owner}/{repo}/releases` returns.
fn listing(dist: &Path, base: &str, tag: &str, prerelease: bool) -> String {
    let assets: Vec<String> = std::fs::read_dir(dist)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            format!(r#"{{"name":"{name}","browser_download_url":"{base}/dl/{name}"}}"#)
        })
        .collect();
    format!(
        r#"[{{"tag_name":"{tag}","prerelease":{prerelease},"html_url":"{base}/releases/{tag}","assets":[{}]}}]"#,
        assets.join(",")
    )
}

// ---------------------------------------------------------------------------------------------
// Step 4 and 5: the updater, and the staged binary running
// ---------------------------------------------------------------------------------------------

/// An installed copy: a directory holding both binaries, as `--install` leaves one.
fn install(dir: &Path, marker: &str) {
    for stem in ["gazelle-audio-server", "gazelle-audio-serverw"] {
        a_binary(&dir.join(format!("{stem}{}", exe_suffix())), &format!("{stem} {marker}"));
    }
}

fn updater(install: &Path, base: &str, current: &str, key: Option<&str>) -> Updater {
    let settings = Settings {
        check: true,
        channel: Channel::Stable,
        interval_hours: 6,
        auto_download: false,
        repo: "doesdev/gazelle-audio".into(),
        api_base: base.to_string(),
    };
    Updater::new(
        settings,
        install.join(format!("gazelle-audio-server{}", exe_suffix())),
        current.parse().unwrap(),
        TARGET.to_string(),
        key.map(str::to_string),
    )
}

#[test]
fn a_release_cut_with_xtask_is_found_verified_and_staged_by_the_updater() {
    let scratch = Scratch::new("whole");
    // Step 1. Outside the repository, which is where the tool insists it goes.
    let key = scratch.join("release.key");
    let public = keygen(&key);

    // Step 2. The release directory, then the real signing command.
    let dist = scratch.dir("dist");
    build_release(&dist, "0.2.0");
    sign(&dist, &key);

    // The two files a release is checked against, verified with the code the app itself uses.
    let sums = std::fs::read(dist.join(SUMS_NAME)).unwrap();
    let signature = std::fs::read(dist.join(SIGNATURE_NAME)).unwrap();
    assert_eq!(signature.len(), 64, "a detached ed25519 signature is 64 raw bytes");
    assert_eq!(verify::verify_signature(&public, &sums, &signature), Ok(()), "the release does not verify against its own key");
    let listed = parse_sums(&String::from_utf8(sums.clone()).unwrap());
    assert_eq!(listed.len(), 3, "both binaries and the zip are listed, and the sums file is not listed in itself");
    for (name, digest) in &listed {
        assert_eq!(&to_hex(&verify::sha256_file(&dist.join(name)).unwrap()), &to_hex(digest), "{name}");
    }

    // Step 3 and 4. An install one version behind, and the updater pointed at the directory.
    let installed = scratch.dir("install");
    install(&installed, "0.1.0");
    let before = std::fs::read(installed.join(format!("gazelle-audio-server{}", exe_suffix()))).unwrap();
    let source = Source::serving(dist.clone(), "v0.2.0", false);
    let updater = updater(&installed, &source.base, "0.1.0", Some(&public));

    assert_eq!(updater.check(true), State::Available { version: "0.2.0".into(), page: format!("{}/releases/v0.2.0", source.base) });
    assert_eq!(updater.download(), State::Staged { version: "0.2.0".into() }, "the release should verify and stage");

    // Both binaries were replaced, byte for byte, with the release's own assets, and each
    // previous one is beside it waiting for the next start to delete it.
    for stem in ["gazelle-audio-server", "gazelle-audio-serverw"] {
        let target = installed.join(format!("{stem}{}", exe_suffix()));
        assert_eq!(std::fs::read(&target).unwrap(), std::fs::read(dist.join(asset_name(stem))).unwrap(), "{stem} was not replaced");
        assert!(stage::old_path(&target).is_file(), "{stem}'s previous binary should be kept as .old");
    }
    assert_eq!(std::fs::read(stage::old_path(&installed.join(format!("gazelle-audio-server{}", exe_suffix())))).unwrap(), before);
    // Nothing is left half-downloaded beside them.
    let leftovers: Vec<String> = std::fs::read_dir(&installed)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("download") || n.contains("tmp"))
        .collect();
    assert!(leftovers.is_empty(), "a finished update leaves no temporary files: {leftovers:?}");

    // Step 5. What a restart runs is the staged binary. These stand-ins are copies of xtask, so
    // running one prints its usage — proving the file at the install path is now executable and
    // is the downloaded one, not the displaced one.
    let staged = installed.join(format!("gazelle-audio-server{}", exe_suffix()));
    let run = Command::new(&staged).output().expect("the staged binary should run");
    assert!(run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("gazelle release helper"),
        "the staged file did not run as the binary that was downloaded"
    );

    // And the next start sweeps the displaced copies, which is `clean_up_after_previous_update`.
    for stem in ["gazelle-audio-server", "gazelle-audio-serverw"] {
        let target = installed.join(format!("{stem}{}", exe_suffix()));
        assert!(stage::clean_old(&target).unwrap());
        assert!(!stage::old_path(&target).exists());
    }
}

#[test]
fn a_release_signed_with_another_key_is_refused_and_nothing_is_staged() {
    let scratch = Scratch::new("wrong-key");
    let theirs = scratch.join("theirs.key");
    keygen(&theirs);
    let ours = keygen(&scratch.join("ours.key"));

    let dist = scratch.dir("dist");
    build_release(&dist, "0.2.0");
    sign(&dist, &theirs);

    let installed = scratch.dir("install");
    install(&installed, "0.1.0");
    let before = std::fs::read(installed.join(format!("gazelle-audio-server{}", exe_suffix()))).unwrap();

    let source = Source::serving(dist, "v0.2.0", false);
    // Built with our key; the release is signed with someone else's.
    let updater = updater(&installed, &source.base, "0.1.0", Some(&ours));
    assert!(matches!(updater.check(true), State::Available { .. }), "a check reads the listing, which is not signed");
    match updater.download() {
        State::Failed { message, detail } => {
            assert_eq!(message, "the download could not be verified", "the tray gets the short summary");
            assert!(detail.contains("signature"), "the detail should name the signature: {detail}");
        }
        other => panic!("a release signed with another key must not be staged: {other:?}"),
    }
    assert_eq!(std::fs::read(installed.join(format!("gazelle-audio-server{}", exe_suffix()))).unwrap(), before, "nothing replaced");
    assert!(!stage::old_path(&installed.join(format!("gazelle-audio-server{}", exe_suffix()))).exists(), "nothing displaced");
}

#[test]
fn a_build_with_no_key_compiled_in_will_not_even_fetch_the_release() {
    let scratch = Scratch::new("no-key");
    let key = scratch.join("release.key");
    keygen(&key);
    let dist = scratch.dir("dist");
    build_release(&dist, "0.2.0");
    sign(&dist, &key);

    let installed = scratch.dir("install");
    install(&installed, "0.1.0");
    let source = Source::serving(dist, "v0.2.0", false);
    // No `GAZELLE_UPDATE_PUBKEY` at build time is exactly this: `public_key` is `None`.
    let updater = updater(&installed, &source.base, "0.1.0", None);

    assert!(matches!(updater.check(true), State::Available { .. }));
    match updater.download() {
        State::Failed { message, detail } => {
            assert_eq!(message, "this build cannot verify a download", "the tray gets the short summary");
            assert!(detail.contains("signing key"), "{detail}");
        }
        other => panic!("a build that cannot verify must not download: {other:?}"),
    }
    // Not one byte of the release was fetched: it refuses before it starts, rather than
    // downloading and then discovering it cannot check what it has.
    assert!(source.downloads().is_empty(), "nothing should have been fetched: {:?}", source.downloads());
    assert!(!updater.status().can_verify, "and it says so, so a release build without the key is obvious");
}

/// The hand-cut release is a directory someone assembles. If a file is missing from it, or a
/// name is wrong, the updater has to say so rather than staging what it did find.
#[test]
fn a_release_missing_this_platforms_binary_is_not_offered_at_all() {
    let scratch = Scratch::new("incomplete");
    let key = scratch.join("release.key");
    let public = keygen(&key);
    let dist = scratch.dir("dist");
    // Only the windowless build and the zip — the console binary, which is what the updater
    // asks for, was left out of the upload.
    a_binary(&dist.join(asset_name("gazelle-audio-serverw")), "serverw 0.2.0");
    std::fs::write(dist.join(format!("gazelle-audio-{TARGET}.zip")), b"PK\x03\x04").unwrap();
    sign(&dist, &key);

    let installed = scratch.dir("install");
    install(&installed, "0.1.0");
    let source = Source::serving(dist, "v0.2.0", false);
    let updater = updater(&installed, &source.base, "0.1.0", Some(&public));

    assert_eq!(updater.check(true), State::UpToDate, "a release with nothing for this platform is not an update");
}

/// `sign` is run over the whole directory, so the digests have to be taken after the last
/// file is put there. Signing an old directory and then adding a binary is the mistake this
/// catches; the release would download and then fail its hash check, which is the right end
/// but a confusing one, so the checklist says sign last.
#[test]
fn an_asset_changed_after_signing_fails_its_hash_and_stages_nothing() {
    let scratch = Scratch::new("stale-sums");
    let key = scratch.join("release.key");
    let public = keygen(&key);
    let dist = scratch.dir("dist");
    build_release(&dist, "0.2.0");
    sign(&dist, &key);
    // Rebuilt after signing, as an "oh, one more fix" would.
    a_binary(&dist.join(asset_name("gazelle-audio-server")), "gazelle-audio-server 0.2.0 rebuilt");

    let installed = scratch.dir("install");
    install(&installed, "0.1.0");
    let before = std::fs::read(installed.join(format!("gazelle-audio-server{}", exe_suffix()))).unwrap();
    let source = Source::serving(dist, "v0.2.0", false);
    let updater = updater(&installed, &source.base, "0.1.0", Some(&public));

    assert!(matches!(updater.check(true), State::Available { .. }));
    match updater.download() {
        State::Failed { message, detail } => {
            assert_eq!(message, "the download could not be verified", "the tray gets the short summary");
            assert!(detail.contains("SHA256SUMS"), "{detail}");
        }
        other => panic!("an asset that does not match the sums file must not be staged: {other:?}"),
    }
    assert_eq!(std::fs::read(installed.join(format!("gazelle-audio-server{}", exe_suffix()))).unwrap(), before);
}

/// A sanity check on the thing that would silently break a release: the sums file is what the
/// signature covers, and `sign` never lists its own outputs.
#[test]
fn the_sums_file_lists_every_asset_and_neither_of_the_tools_own_outputs() {
    let scratch = Scratch::new("sums");
    let key = scratch.join("release.key");
    keygen(&key);
    let dist = scratch.dir("dist");
    build_release(&dist, "0.2.0");
    sign(&dist, &key);
    // Signing twice is what a re-cut release does; it must not start listing SHA256SUMS itself.
    sign(&dist, &key);

    let text = std::fs::read_to_string(dist.join(SUMS_NAME)).unwrap();
    let names: Vec<&str> = text.lines().map(|line| line.split_at(66).1).collect();
    assert_eq!(
        names,
        [asset_name("gazelle-audio-server"), asset_name("gazelle-audio-serverw"), format!("gazelle-audio-{TARGET}.zip")]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        "sorted, so the same directory always signs to the same bytes"
    );
    assert!(!text.contains(SUMS_NAME) && !text.contains(SIGNATURE_NAME));
}

/// Nothing above should have gone near the network. This is cheap insurance on a test that
/// drives a real HTTP client.
#[test]
fn the_release_source_is_always_localhost() {
    let scratch = Scratch::new("localhost");
    let dist = scratch.dir("dist");
    std::fs::write(dist.join("anything"), b"x").unwrap();
    let source = Source::serving(dist, "v0.0.1", false);
    assert!(source.base.starts_with("http://127.0.0.1:"), "{}", source.base);

    // And it really is serving: a bare request comes back with the listing.
    let mut stream = TcpStream::connect(source.base.trim_start_matches("http://")).unwrap();
    stream.write_all(b"GET /repos/doesdev/gazelle-audio/releases HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    assert!(answer.contains("\"tag_name\":\"v0.0.1\""), "{answer}");
}
