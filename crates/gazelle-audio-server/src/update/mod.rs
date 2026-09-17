//! The in-app updater for portable builds.
//!
//! The app ships as a bare executable, so nothing else is going to update it. This module asks
//! GitHub Releases what the newest build for this platform is, fetches it when asked, checks it
//! twice, and puts it in place for the **next** start. It never restarts the process: the
//! running one holds a USB handle, a port and possibly a window, and only the user decides when
//! that is a good moment. The tray offers the restart; this module only stages.
//!
//! # What is trusted
//!
//! A release carries the binary for each platform, a `SHA256SUMS` listing their digests, and a
//! detached ed25519 signature of that sums file made when the release was cut. The public half
//! of the signing key is compiled in ([`verify::built_in_public_key`]). The download is checked
//! against the sums file first, then the sums file against the key; **either failing deletes the
//! download**. A build with no key compiled in refuses to download at all, rather than applying
//! something it cannot check.
//!
//! The release source is therefore not trusted, only the key is, which is why the settings may
//! point `api_base` at a mirror and why the tests can point it at a local server.
//!
//! # Why the asset is the executable, not an archive
//!
//! Unpacking an archive from the network is code that runs before anything has been verified.
//! Assets are the executables themselves, so applying an update is two renames and no parser.
//! A zip is still published for people downloading by hand; the updater ignores it.
//!
//! # TLS
//!
//! `ureq` with rustls and the ring provider — A35's crypto choice, a different client from
//! `drive`'s `reqwest` because reqwest 0.13's tree would lift this crate off the Rust 1.82
//! floor. Certificates verify against ureq's bundled Mozilla roots rather than the OS trust
//! store (`rustls-platform-verifier` also needs 1.85). That is stricter in the common case and
//! weaker in one: an update check will not go through a TLS-inspecting corporate proxy. The
//! check failing is harmless — the app says so and carries on — and nothing is ever applied
//! without the signature, which the proxy cannot forge.

pub mod release;
pub mod settings;
pub mod stage;
pub mod verify;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use semver::Version;
use serde::Serialize;

use release::{binary_asset_name, Release, SIGNATURE_NAME, SUMS_NAME};
use settings::Settings;

/// The target triple this binary was built for, from `build.rs`.
pub const TARGET: &str = env!("GAZELLE_TARGET");

/// The binaries a release carries for each platform. The running one is always updated; the
/// other is updated too when it sits beside it, so Start on boot (which runs the windowless
/// build, P78) does not quietly stay a version behind.
pub const BINARIES: [&str; 2] = ["gazelle-audio-server", "gazelle-audio-serverw"];

/// How long a check's answer stands. A tray menu opening, a web UI polling and a background
/// timer all share it, so none of them costs a request.
pub const CACHE: Duration = Duration::from_secs(15 * 60);

/// Caps on what is read from the network, so a hostile or broken source cannot fill the disk.
const MAX_LISTING: u64 = 4 * 1024 * 1024;
const MAX_SUMS: u64 = 256 * 1024;
const MAX_BINARY: u64 = 256 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(120);

/// Where the updater has got to. Serialised as `{"state": "...", ...}` for the web UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// Nothing has been checked yet.
    Unknown,
    Checking,
    UpToDate,
    Available { version: String, page: String },
    Downloading { version: String },
    /// Verified and in place; it runs after a restart.
    Staged { version: String },
    Failed { message: String },
}

impl State {
    /// The one line the tray shows.
    pub fn line(&self) -> String {
        match self {
            State::Unknown => "Updates: not checked yet".into(),
            State::Checking => "Updates: checking…".into(),
            State::UpToDate => "Updates: this is the newest version".into(),
            State::Available { version, .. } => format!("Update available: {version}"),
            State::Downloading { version } => format!("Downloading {version}…"),
            State::Staged { version } => format!("Update {version} is ready — restart to use it"),
            State::Failed { message } => format!("Update check failed: {message}"),
        }
    }
}

/// Everything the tray and the HTTP endpoint report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    /// The version running now.
    pub version: String,
    pub target: String,
    pub channel: &'static str,
    /// Whether unattended checks happen at all.
    pub check: bool,
    pub auto_download: bool,
    /// Whether this build can verify a download. False means it will not fetch one.
    pub can_verify: bool,
    pub state: State,
}

struct Inner {
    state: State,
    found: Option<Release>,
    last_check: Option<Instant>,
}

pub struct Updater {
    settings: Settings,
    current: Version,
    target: String,
    /// The running binary, which is what gets replaced.
    exe: PathBuf,
    public_key: Option<String>,
    agent: ureq::Agent,
    inner: Mutex<Inner>,
}

impl Updater {
    /// An updater for this build: the running executable, `CARGO_PKG_VERSION`, the compiled-in
    /// target triple and signing key.
    pub fn for_this_build(settings: Settings) -> Result<Updater, String> {
        let exe = std::env::current_exe().map_err(|e| format!("finding the running binary: {e}"))?;
        let current = Version::parse(crate::VERSION).map_err(|e| format!("{} is not a version: {e}", crate::VERSION))?;
        Ok(Updater::new(settings, exe, current, TARGET.to_string(), verify::built_in_public_key().map(str::to_string)))
    }

    pub fn new(settings: Settings, exe: PathBuf, current: Version, target: String, public_key: Option<String>) -> Updater {
        ensure_crypto_provider();
        let agent = ureq::config::Config::builder()
            .user_agent(format!("gazelle-audio-server/{current}"))
            .timeout_global(Some(TIMEOUT))
            // Production settings name an https source; a plain-http one is only ever a local
            // stand-in, and even then the signature is what decides whether anything is applied.
            .https_only(settings.api_base.starts_with("https://"))
            .build()
            .new_agent();
        Updater {
            settings,
            current,
            target,
            exe,
            public_key,
            agent,
            inner: Mutex::new(Inner { state: State::Unknown, found: None, last_check: None }),
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn state(&self) -> State {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn status(&self) -> Status {
        Status {
            version: self.current.to_string(),
            target: self.target.clone(),
            channel: self.settings.channel.as_str(),
            check: self.settings.check,
            auto_download: self.settings.auto_download,
            can_verify: self.public_key.is_some(),
            state: self.state(),
        }
    }

    /// The version staged and waiting for a restart, if there is one.
    pub fn staged(&self) -> Option<String> {
        match self.state() {
            State::Staged { version } => Some(version),
            _ => None,
        }
    }

    /// The version that could be downloaded, if one was found and has not been fetched.
    pub fn available(&self) -> Option<String> {
        match self.state() {
            State::Available { version, .. } => Some(version),
            _ => None,
        }
    }

    /// Ask the release source what the newest build is. Nothing is downloaded.
    ///
    /// Within [`CACHE`] of the last check this returns the answer already held and makes no
    /// request, so a tray menu or a polling web UI costs nothing. `force` is the user asking.
    pub fn check(&self, force: bool) -> State {
        {
            let inner = self.inner.lock().unwrap();
            let fresh = inner.last_check.is_some_and(|t| t.elapsed() < CACHE);
            if !force && fresh {
                return inner.state.clone();
            }
            // A staged update is the end of the road until a restart; re-checking would only
            // offer the same version again.
            if matches!(inner.state, State::Staged { .. } | State::Downloading { .. }) {
                return inner.state.clone();
            }
        }
        self.set_state(State::Checking);
        let state = match self.look() {
            Ok(Some(found)) => {
                let state = State::Available { version: found.version.to_string(), page: found.page.clone() };
                self.inner.lock().unwrap().found = Some(found);
                state
            }
            Ok(None) => {
                self.inner.lock().unwrap().found = None;
                State::UpToDate
            }
            Err(message) => State::Failed { message },
        };
        {
            let mut inner = self.inner.lock().unwrap();
            inner.last_check = Some(Instant::now());
            inner.state = state.clone();
        }
        if self.settings.auto_download && matches!(state, State::Available { .. }) {
            return self.download();
        }
        state
    }

    /// Fetch the release found by the last check, verify it, and put it in place for the next
    /// start. Anything that fails deletes what was downloaded and says why.
    pub fn download(&self) -> State {
        let Some(found) = self.inner.lock().unwrap().found.clone() else {
            return self.set_state(State::Failed { message: "nothing to download: no newer release has been found".into() });
        };
        let Some(key) = self.public_key.clone() else {
            return self.set_state(State::Failed {
                message: "this build carries no release signing key, so an update cannot be verified; download it by hand".into(),
            });
        };
        self.set_state(State::Downloading { version: found.version.to_string() });
        match self.fetch_and_stage(&found, &key) {
            Ok(()) => self.set_state(State::Staged { version: found.version.to_string() }),
            Err(message) => self.set_state(State::Failed { message }),
        }
    }

    fn set_state(&self, state: State) -> State {
        self.inner.lock().unwrap().state = state.clone();
        state
    }

    /// One request to the release source, then a purely local decision.
    fn look(&self) -> Result<Option<Release>, String> {
        let url = format!("{}/repos/{}/releases?per_page=30", self.settings.api_base.trim_end_matches('/'), self.settings.repo);
        let body = self.get(&url, MAX_LISTING)?;
        let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("the release list is not JSON: {e}"))?;
        let releases = release::parse_releases(&json);
        let asset = binary_asset_name(self.stem(), &self.target);
        Ok(release::newest(&releases, self.settings.channel, &self.current, &asset).cloned())
    }

    /// The running binary's name without its extension, which is also its asset's name.
    fn stem(&self) -> &str {
        self.exe.file_stem().and_then(|s| s.to_str()).unwrap_or(BINARIES[0])
    }

    /// Which files on disk this update replaces: the running binary, and any sibling from
    /// [`BINARIES`] that is installed beside it. The running one is first and is required;
    /// a sibling that the release does not carry, or that cannot be replaced, is logged and
    /// skipped rather than failing the update.
    fn targets(&self) -> Vec<(PathBuf, String)> {
        let stem = self.stem().to_string();
        let mut targets = vec![(self.exe.clone(), binary_asset_name(&stem, &self.target))];
        let (Some(dir), Some(extension)) = (self.exe.parent(), self.exe.extension()) else {
            return targets;
        };
        for sibling in BINARIES.iter().filter(|s| **s != stem) {
            let path = dir.join(sibling).with_extension(extension);
            if path.is_file() {
                targets.push((path, binary_asset_name(sibling, &self.target)));
            }
        }
        targets
    }

    fn fetch_and_stage(&self, found: &Release, key: &str) -> Result<(), String> {
        let sums_asset = found.asset(SUMS_NAME).ok_or("the release has no SHA256SUMS")?;
        let signature_asset = found.asset(SIGNATURE_NAME).ok_or("the release has no SHA256SUMS.sig")?;
        let sums_bytes = self.get(&sums_asset.url, MAX_SUMS)?;
        let signature = self.get(&signature_asset.url, MAX_SUMS)?;
        let sums = release::parse_sums(&String::from_utf8_lossy(&sums_bytes));

        let mut verified_signature = false;
        let mut staged = Vec::new();
        for (index, (target, asset_name)) in self.targets().into_iter().enumerate() {
            let required = index == 0;
            let evidence = Evidence { sums: &sums, sums_bytes: &sums_bytes, signature: &signature, key };
            match self.fetch_one(found, &asset_name, &evidence, &target, &mut verified_signature) {
                Ok(()) => staged.push(target),
                Err(e) if required => return Err(e),
                Err(e) => tracing::warn!("{} was not updated: {e}", target.display()),
            }
        }
        tracing::info!(
            "update {} staged: {} — restart to use it",
            found.version,
            staged.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        );
        Ok(())
    }

    /// Download one asset beside where it will live, check its hash against the sums file,
    /// then — the first time round — check the sums file's signature. Only then is it moved
    /// into place. A failure anywhere deletes the download.
    fn fetch_one(
        &self,
        found: &Release,
        asset_name: &str,
        evidence: &Evidence,
        target: &Path,
        verified_signature: &mut bool,
    ) -> Result<(), String> {
        let asset = found.asset(asset_name).ok_or_else(|| format!("the release has no {asset_name}"))?;
        let expected = *evidence.sums.get(asset_name).ok_or_else(|| format!("SHA256SUMS does not list {asset_name}"))?;
        let temporary = temporary_path(target);

        let check = (|| -> Result<(), String> {
            self.get_to_file(&asset.url, &temporary, MAX_BINARY)?;
            let digest = verify::sha256_file(&temporary).map_err(|e| format!("reading {} back: {e}", temporary.display()))?;
            if digest != expected {
                return Err(format!(
                    "{asset_name} does not match SHA256SUMS (got {}, expected {})",
                    release::to_hex(&digest),
                    release::to_hex(&expected)
                ));
            }
            if !*verified_signature {
                verify::verify_signature(evidence.key, evidence.sums_bytes, evidence.signature)?;
            }
            Ok(())
        })();
        if let Err(e) = check {
            let _ = std::fs::remove_file(&temporary);
            return Err(e);
        }
        *verified_signature = true;
        stage::stage(&temporary, target).map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            format!("putting {} in place: {e}", target.display())
        })
    }

    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        let mut response = self
            .agent
            .get(url)
            .header("accept", "application/octet-stream, application/vnd.github+json, */*")
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| format!("reading {url}: {e}"))
    }

    fn get_to_file(&self, url: &str, path: &Path, limit: u64) -> Result<(), String> {
        let mut response = self.agent.get(url).header("accept", "application/octet-stream").call().map_err(|e| format!("{url}: {e}"))?;
        let mut file = std::fs::File::create(path).map_err(|e| format!("creating {}: {e}", path.display()))?;
        let mut reader = response.body_mut().with_config().limit(limit).reader();
        std::io::copy(&mut reader, &mut file).map_err(|e| format!("downloading {url}: {e}"))?;
        Ok(())
    }
}

/// What a downloaded file is checked against: the release's `SHA256SUMS`, the bytes of that
/// file as they arrived, the detached signature over them, and the key that signs it.
struct Evidence<'a> {
    sums: &'a std::collections::BTreeMap<String, [u8; 32]>,
    sums_bytes: &'a [u8],
    signature: &'a [u8],
    key: &'a str,
}

/// Where a download is written: beside the file it will replace, so the move into place is a
/// rename within one directory and never a copy across volumes.
pub fn temporary_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".download");
    PathBuf::from(name)
}

/// rustls needs a process-wide crypto provider before any client is built (A35). Idempotent:
/// installing a second time is the error this ignores.
pub fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Tidy up after a previous update: delete the binary it displaced. Called once at start, when
/// nothing holds the old image any more. Never fatal — the file is inert, and a failure (a
/// virus scanner still reading it, say) is retried at the next start.
pub fn clean_up_after_previous_update() {
    let Ok(exe) = std::env::current_exe() else { return };
    for binary in BINARIES {
        let Some(path) = exe.parent().map(|d| match exe.extension() {
            Some(extension) => d.join(binary).with_extension(extension),
            None => d.join(binary),
        }) else {
            continue;
        };
        match stage::clean_old(&path) {
            Ok(true) => tracing::info!("removed {}, left by a previous update", stage::old_path(&path).display()),
            Ok(false) => {}
            Err(e) => tracing::warn!("could not remove {}: {e}", stage::old_path(&path).display()),
        }
    }
}
