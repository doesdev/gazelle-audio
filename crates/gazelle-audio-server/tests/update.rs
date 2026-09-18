//! The updater against a release source that is a local HTTP server, never the real one.
//!
//! The server speaks the shape of the GitHub Releases API and serves assets it holds in memory,
//! so a whole release — binaries, `SHA256SUMS`, its ed25519 signature — is built per test and
//! can be broken in exactly one way. Nothing here resolves a name or opens a socket off
//! localhost.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use ed25519_dalek::{Signer, SigningKey};
use futures_util::StreamExt;
use semver::Version;

use gazelle_audio_server::update::release::{to_hex, SIGNATURE_NAME, SUMS_NAME};
use gazelle_audio_server::update::settings::{Channel, Settings};
use gazelle_audio_server::update::{stage, State as UpdateState, Updater, TARGET};

/// The platform this test binary was built for, so asset names and the fake install match it.
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

// ---------------------------------------------------------------------------------------------
// The release source
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct ReleaseSpec {
    tag: String,
    prerelease: bool,
    /// Asset name to bytes, exactly as served.
    assets: BTreeMap<String, Vec<u8>>,
    /// Assets to serve short while claiming the full length, as a cut connection does.
    truncate: BTreeMap<String, usize>,
}

#[derive(Clone, Default)]
struct Source {
    releases: Vec<ReleaseSpec>,
    base: String,
    /// Every path the source was asked for, so a test can count requests.
    seen: Arc<Mutex<Vec<String>>>,
}

/// Build a release: the two binaries, a `SHA256SUMS` over them, and its signature.
fn release(tag: &str, prerelease: bool, body: &[u8], key: &SigningKey) -> ReleaseSpec {
    let mut assets = BTreeMap::new();
    for stem in ["gazelle-audio-server", "gazelle-audio-serverw"] {
        let mut bytes = body.to_vec();
        bytes.extend_from_slice(stem.as_bytes());
        assets.insert(asset_name(stem), bytes);
    }
    sign(tag, prerelease, assets, key)
}

/// Write the sums file over whatever assets there are, then sign it.
fn sign(tag: &str, prerelease: bool, mut assets: BTreeMap<String, Vec<u8>>, key: &SigningKey) -> ReleaseSpec {
    let sums: String = assets
        .iter()
        .map(|(name, bytes)| format!("{}  {name}\n", to_hex(&gazelle_audio_server::update::verify::sha256_bytes(bytes))))
        .collect();
    let signature = key.sign(sums.as_bytes()).to_bytes().to_vec();
    assets.insert(SUMS_NAME.into(), sums.into_bytes());
    assets.insert(SIGNATURE_NAME.into(), signature);
    ReleaseSpec { tag: tag.into(), prerelease, assets, truncate: BTreeMap::new() }
}

async fn list(State(source): State<Source>) -> Response {
    source.seen.lock().unwrap().push("releases".into());
    let json: Vec<serde_json::Value> = source
        .releases
        .iter()
        .map(|r| {
            serde_json::json!({
                "tag_name": r.tag,
                "prerelease": r.prerelease,
                "html_url": format!("{}/releases/{}", source.base, r.tag),
                "assets": r.assets.keys().map(|name| serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("{}/dl/{}/{name}", source.base, r.tag),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    axum::Json(json).into_response()
}

async fn download(State(source): State<Source>, UrlPath((tag, name)): UrlPath<(String, String)>) -> Response {
    source.seen.lock().unwrap().push(format!("dl/{tag}/{name}"));
    let Some(spec) = source.releases.iter().find(|r| r.tag == tag) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(bytes) = spec.assets.get(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match spec.truncate.get(&name) {
        // Promise the whole length, send part of it and then fail: the connection is cut
        // mid-download, which is what the client has to survive.
        Some(&at) => {
            let head = bytes[..at.min(bytes.len())].to_vec();
            let stream = futures_util::stream::once(async move { Ok::<Vec<u8>, std::io::Error>(head) })
                .chain(futures_util::stream::once(async { Err(std::io::Error::other("the connection was cut")) }));
            Response::builder()
                .header(header::CONTENT_LENGTH, bytes.len())
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }
        None => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes.clone()).into_response(),
    }
}

/// A running source, on a runtime of its own thread so a `#[tokio::test]` can drive a blocking
/// client against it. Dropping it stops the server.
struct Fake {
    base: String,
    seen: Arc<Mutex<Vec<String>>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fake {
    fn start(releases: Vec<ReleaseSpec>) -> Fake {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let served = seen.clone();
        let (address, bound) = std::sync::mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind::<SocketAddr>("127.0.0.1:0".parse().unwrap()).await.unwrap();
                let base = format!("http://{}", listener.local_addr().unwrap());
                address.send(base.clone()).unwrap();
                let app = axum::Router::new()
                    .route("/repos/{owner}/{repo}/releases", get(list))
                    .route("/dl/{tag}/{name}", get(download))
                    .with_state(Source { releases, base, seen: served });
                let _ = axum::serve(listener, app).with_graceful_shutdown(async { let _ = stopped.await; }).await;
            });
        });
        Fake { base: bound.recv().unwrap(), seen, stop: Some(stop), thread: Some(thread) }
    }

    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The fake install
// ---------------------------------------------------------------------------------------------

/// A directory holding both binaries, as a portable install does.
struct Install(PathBuf);

impl Install {
    fn new(name: &str) -> Install {
        let dir = std::env::temp_dir().join(format!("gazelle-update-it-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for stem in ["gazelle-audio-server", "gazelle-audio-serverw"] {
            std::fs::write(dir.join(format!("{stem}{}", exe_suffix())), format!("running {stem}")).unwrap();
        }
        Install(dir)
    }

    fn exe(&self) -> PathBuf {
        self.0.join(format!("gazelle-audio-server{}", exe_suffix()))
    }

    fn windowless(&self) -> PathBuf {
        self.0.join(format!("gazelle-audio-serverw{}", exe_suffix()))
    }

    /// Anything left lying around that is not one of the two binaries.
    fn litter(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != &format!("gazelle-audio-server{}", exe_suffix()) && n != &format!("gazelle-audio-serverw{}", exe_suffix()))
            .collect();
        names.sort();
        names
    }
}

impl Drop for Install {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[42u8; 32])
}

fn updater(fake: &Fake, install: &Install, current: &str, channel: Channel, public_key: Option<String>) -> Updater {
    let settings = Settings {
        channel,
        api_base: fake.base.clone(),
        repo: "gazelle/test".into(),
        auto_download: false,
        ..Settings::default()
    };
    Updater::new(settings, install.exe(), Version::parse(current).unwrap(), TARGET.to_string(), public_key)
}

fn public_key() -> Option<String> {
    Some(to_hex(key().verifying_key().as_bytes()))
}

fn failure(state: &UpdateState) -> String {
    match state {
        UpdateState::Failed { message } => message.clone(),
        other => panic!("expected a failure, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Checking
// ---------------------------------------------------------------------------------------------

#[test]
fn a_newer_version_is_found_and_nothing_is_downloaded() {
    let install = Install::new("newer");
    let fake = Fake::start(vec![release("v0.1.0", false, b"one", &key()), release("v0.2.0", false, b"two", &key())]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    let state = updater.check(true);

    assert_eq!(state, UpdateState::Available { version: "0.2.0".into(), page: format!("{}/releases/v0.2.0", fake.base) });
    assert_eq!(updater.available().as_deref(), Some("0.2.0"));
    assert_eq!(fake.requests(), ["releases"], "a check asks once and downloads nothing");
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "running gazelle-audio-server");
    assert!(install.litter().is_empty(), "{:?}", install.litter());
}

#[test]
fn the_newest_release_being_the_running_one_is_up_to_date() {
    let install = Install::new("same");
    let fake = Fake::start(vec![release("v0.2.0", false, b"two", &key())]);
    let updater = updater(&fake, &install, "0.2.0", Channel::Stable, public_key());

    assert_eq!(updater.check(true), UpdateState::UpToDate);
    assert_eq!(updater.available(), None);
    assert_eq!(fake.requests(), ["releases"]);
}

#[test]
fn a_pre_release_is_ignored_unless_the_channel_is_switched_on() {
    let releases = vec![release("v0.2.0", false, b"two", &key()), release("v0.3.0-rc.1", true, b"three", &key())];

    let stable_install = Install::new("prerelease-off");
    let stable_source = Fake::start(releases.clone());
    let stable = updater(&stable_source, &stable_install, "0.1.0", Channel::Stable, public_key());
    assert_eq!(stable.check(true), UpdateState::Available { version: "0.2.0".into(), page: format!("{}/releases/v0.2.0", stable_source.base) });

    let early_install = Install::new("prerelease-on");
    let early_source = Fake::start(releases);
    let early = updater(&early_source, &early_install, "0.1.0", Channel::Prerelease, public_key());
    assert_eq!(early.check(true).line(), "Update available: 0.3.0-rc.1");
}

#[test]
fn a_repeat_check_is_answered_from_the_last_one_unless_it_is_asked_for() {
    let install = Install::new("cache");
    let fake = Fake::start(vec![release("v0.2.0", false, b"two", &key())]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    updater.check(true);
    updater.check(false);
    updater.check(false);
    assert_eq!(fake.requests(), ["releases"], "a tray poll must cost nothing");

    updater.check(true);
    assert_eq!(fake.requests(), ["releases", "releases"], "the user asking always asks the source");
}

#[test]
fn a_source_that_cannot_be_reached_is_reported_and_is_not_fatal() {
    let install = Install::new("unreachable");
    let fake = Fake::start(vec![]);
    let base = fake.base.clone();
    drop(fake);
    let settings = Settings { api_base: base, repo: "gazelle/test".into(), ..Settings::default() };
    let updater = Updater::new(settings, install.exe(), Version::parse("0.1.0").unwrap(), TARGET.to_string(), public_key());

    let state = updater.check(true);
    assert!(matches!(state, UpdateState::Failed { .. }), "{state:?}");
    assert!(state.line().starts_with("Update check failed:"), "{}", state.line());
}

// ---------------------------------------------------------------------------------------------
// Downloading, verifying, staging
// ---------------------------------------------------------------------------------------------

#[test]
fn a_verified_update_is_staged_for_the_next_start_and_the_old_binaries_are_kept() {
    let install = Install::new("stage");
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &key())]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    updater.check(true);
    assert_eq!(updater.download(), UpdateState::Staged { version: "0.2.0".into() });
    assert_eq!(updater.staged().as_deref(), Some("0.2.0"));

    // Both binaries are replaced: Start on boot runs the windowless one (P78), which must not
    // be left a version behind.
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "NEW gazelle-audio-server");
    assert_eq!(std::fs::read_to_string(install.windowless()).unwrap(), "NEW gazelle-audio-serverw");
    assert_eq!(std::fs::read_to_string(stage::old_path(&install.exe())).unwrap(), "running gazelle-audio-server");
    assert_eq!(std::fs::read_to_string(stage::old_path(&install.windowless())).unwrap(), "running gazelle-audio-serverw");

    // Nothing is left half-downloaded.
    assert_eq!(
        install.litter(),
        [
            format!("gazelle-audio-server{}.old", exe_suffix()),
            format!("gazelle-audio-serverw{}.old", exe_suffix())
        ]
    );

    // The next start sweeps the displaced binaries up, and a start after that finds nothing.
    assert_eq!(gazelle_audio_server::update::clean_up_after(&install.exe()), 2);
    assert!(install.litter().is_empty(), "{:?}", install.litter());
    assert_eq!(gazelle_audio_server::update::clean_up_after(&install.exe()), 0);
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "NEW gazelle-audio-server", "the staged binary survives the sweep");
}

#[test]
fn a_download_whose_hash_is_wrong_is_deleted_and_nothing_is_replaced() {
    let install = Install::new("bad-hash");
    // The sums file is signed over the honest digests, then one asset is swapped for other bytes.
    let mut spec = release("v0.2.0", false, b"NEW ", &key());
    spec.assets.insert(asset_name("gazelle-audio-server"), b"tampered".to_vec());
    let fake = Fake::start(vec![spec]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    updater.check(true);
    let message = failure(&updater.download());

    assert!(message.contains("does not match SHA256SUMS"), "{message}");
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "running gazelle-audio-server");
    assert!(install.litter().is_empty(), "a failed download is deleted: {:?}", install.litter());
}

#[test]
fn a_sums_file_signed_by_another_key_is_refused_and_nothing_is_replaced() {
    let install = Install::new("bad-signature");
    // Everything hashes correctly; only the signature is by a key the build does not know.
    let impostor = SigningKey::from_bytes(&[7u8; 32]);
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &impostor)]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    updater.check(true);
    let message = failure(&updater.download());

    assert!(message.contains("not made by the release signing key"), "{message}");
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "running gazelle-audio-server");
    assert!(install.litter().is_empty(), "an unverified download is deleted: {:?}", install.litter());
}

#[test]
fn a_sums_file_edited_after_signing_is_refused() {
    let install = Install::new("edited-sums");
    let mut spec = release("v0.2.0", false, b"NEW ", &key());
    // Re-hash the tampered binary into the sums file, leaving the old signature in place: the
    // hash check now passes and only the signature can catch it.
    let tampered = b"tampered".to_vec();
    let name = asset_name("gazelle-audio-server");
    let line = format!("{}  {name}\n", to_hex(&gazelle_audio_server::update::verify::sha256_bytes(&tampered)));
    let sums = String::from_utf8(spec.assets[SUMS_NAME].clone()).unwrap();
    let rewritten: String = sums.lines().filter(|l| !l.ends_with(&name)).map(|l| format!("{l}\n")).chain([line]).collect();
    spec.assets.insert(SUMS_NAME.into(), rewritten.into_bytes());
    spec.assets.insert(name, tampered);
    let fake = Fake::start(vec![spec]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    updater.check(true);
    let message = failure(&updater.download());

    assert!(message.contains("not made by the release signing key"), "{message}");
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "running gazelle-audio-server");
    assert!(install.litter().is_empty(), "{:?}", install.litter());
}

#[test]
fn a_download_cut_short_is_deleted_and_nothing_is_replaced() {
    let install = Install::new("truncated");
    let mut spec = release("v0.2.0", false, &[b'N'; 4_000_000], &key());
    let name = asset_name("gazelle-audio-server");
    spec.truncate.insert(name, 1_500_000);
    let fake = Fake::start(vec![spec]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    updater.check(true);
    let message = failure(&updater.download());

    assert!(message.contains(&asset_name("gazelle-audio-server")), "{message}");
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "running gazelle-audio-server");
    assert!(install.litter().is_empty(), "a part-downloaded file is deleted: {:?}", install.litter());
}

#[test]
fn a_build_with_no_signing_key_refuses_to_download_at_all() {
    let install = Install::new("no-key");
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &key())]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, None);

    assert_eq!(updater.check(true).line(), "Update available: 0.2.0");
    let message = failure(&updater.download());

    assert!(message.contains("no release signing key"), "{message}");
    assert_eq!(fake.requests(), ["releases"], "it does not even fetch what it could not check");
    assert!(!updater.status().can_verify);
}

#[test]
fn nothing_is_downloaded_before_something_has_been_found() {
    let install = Install::new("nothing-found");
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &key())]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    assert!(failure(&updater.download()).contains("nothing to download"));
    assert_eq!(fake.requests(), Vec::<String>::new());
}

#[test]
fn asking_to_download_it_is_what_fetches_it_and_checking_again_keeps_the_staged_answer() {
    let install = Install::new("explicit");
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &key())]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    updater.check(true);
    assert_eq!(fake.requests().len(), 1, "the default settings download nothing by themselves");
    updater.download();

    // A check after staging says the same thing rather than offering the version again.
    assert_eq!(updater.check(true), UpdateState::Staged { version: "0.2.0".into() });
}

#[test]
fn the_status_says_what_is_running_and_how_it_is_set_up() {
    let install = Install::new("status");
    let fake = Fake::start(vec![]);
    let updater = updater(&fake, &install, "0.4.2", Channel::Prerelease, public_key());
    let status = updater.status();

    assert_eq!(status.version, "0.4.2");
    assert_eq!(status.target, TARGET);
    assert_eq!(status.channel, "prerelease");
    assert!(status.can_verify);
    assert!(!status.auto_download);
    assert_eq!(status.state, UpdateState::Unknown);
    assert_eq!(status.state.line(), "Updates: not checked yet");
}

#[test]
fn switched_on_auto_download_stages_without_being_asked_twice() {
    let install = Install::new("auto");
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &key())]);
    let settings = Settings {
        api_base: fake.base.clone(),
        repo: "gazelle/test".into(),
        auto_download: true,
        ..Settings::default()
    };
    let updater = Updater::new(settings, install.exe(), Version::parse("0.1.0").unwrap(), TARGET.to_string(), public_key());

    assert_eq!(updater.check(true), UpdateState::Staged { version: "0.2.0".into() });
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "NEW gazelle-audio-server");
}

#[test]
fn a_release_missing_the_windowless_build_still_updates_the_running_one() {
    let install = Install::new("one-binary");
    let mut assets = BTreeMap::new();
    assets.insert(asset_name("gazelle-audio-server"), b"NEW console".to_vec());
    let fake = Fake::start(vec![sign("v0.2.0", false, assets, &key())]);
    let updater = updater(&fake, &install, "0.1.0", Channel::Stable, public_key());

    updater.check(true);
    assert_eq!(updater.download(), UpdateState::Staged { version: "0.2.0".into() });
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "NEW console");
    assert_eq!(
        std::fs::read_to_string(install.windowless()).unwrap(),
        "running gazelle-audio-serverw",
        "a binary the release does not carry is left alone"
    );
}

// ---------------------------------------------------------------------------------------------
// The HTTP surface the web UI sees
// ---------------------------------------------------------------------------------------------

/// The routes as `main.rs` merges them, over a server with no devices.
fn api(updater: Updater) -> axum::Router {
    use gazelle_audio_server::registry_set::RegistrySet;
    use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
    let state = gazelle_audio_server::AppState {
        devices: gazelle_audio_server::device::manager::DeviceManager::new(RegistrySet::builtin().unwrap()),
        store: Arc::new(MemoryStore::default()) as Arc<dyn WorkspaceStore>,
        force_dry_run: false,
        backend: "loopback".into(),
        themes_dir: None,
    };
    gazelle_audio_server::http::router(state).merge(gazelle_audio_server::http::update::routes(Arc::new(updater)))
}

async fn call(app: &axum::Router, method: &str, uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let request = axum::http::Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

#[tokio::test]
async fn the_endpoint_reports_the_running_version_and_checks_when_asked() {
    let install = Install::new("http");
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &key())]);
    let app = api(updater(&fake, &install, "0.1.0", Channel::Stable, public_key()));

    let (status, body) = call(&app, "GET", "/api/v1/update").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["version"], "0.1.0");
    assert_eq!(body["channel"], "stable");
    assert_eq!(body["can_verify"], true);
    assert_eq!(body["state"]["state"], "unknown");
    assert_eq!(fake.requests(), Vec::<String>::new(), "reading the status asks nothing of the source");

    let (status, body) = call(&app, "POST", "/api/v1/update/check").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["state"]["state"], "available");
    assert_eq!(body["version"], "0.1.0", "the version reported is the one running");
    assert_eq!(fake.requests(), ["releases"]);
}

#[tokio::test]
async fn the_endpoint_downloads_only_when_asked_and_says_what_came_of_it() {
    let install = Install::new("http-download");
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &key())]);
    let app = api(updater(&fake, &install, "0.1.0", Channel::Stable, public_key()));

    call(&app, "POST", "/api/v1/update/check").await;
    let (status, body) = call(&app, "POST", "/api/v1/update/download").await;

    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["state"]["state"], "staged");
    assert_eq!(std::fs::read_to_string(install.exe()).unwrap(), "NEW gazelle-audio-server");
    assert_eq!(call(&app, "GET", "/api/v1/update").await.1["state"]["state"], "staged");
}

#[tokio::test]
async fn a_failed_download_is_reported_through_the_endpoint_as_it_is_to_the_tray() {
    let install = Install::new("http-failure");
    let fake = Fake::start(vec![release("v0.2.0", false, b"NEW ", &SigningKey::from_bytes(&[3u8; 32]))]);
    let app = api(updater(&fake, &install, "0.1.0", Channel::Stable, public_key()));

    call(&app, "POST", "/api/v1/update/check").await;
    let (_, body) = call(&app, "POST", "/api/v1/update/download").await;

    assert_eq!(body["state"]["state"], "failed");
    assert!(body["state"]["message"].as_str().unwrap().contains("release signing key"), "{body}");
}

/// `main.rs` merges these routes only on a loopback bind; without them the paths are simply not
/// served, and health still names the version.
#[tokio::test]
async fn without_the_updater_the_routes_are_not_there_but_health_still_names_the_version() {
    use gazelle_audio_server::registry_set::RegistrySet;
    use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
    let app = gazelle_audio_server::http::router(gazelle_audio_server::AppState {
        devices: gazelle_audio_server::device::manager::DeviceManager::new(RegistrySet::builtin().unwrap()),
        store: Arc::new(MemoryStore::default()) as Arc<dyn WorkspaceStore>,
        force_dry_run: false,
        backend: "loopback".into(),
        themes_dir: None,
    });

    assert_eq!(call(&app, "GET", "/api/v1/update").await.0, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "POST", "/api/v1/update/check").await.0, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "GET", "/api/v1/health").await.1["version"], gazelle_audio_server::VERSION);
}
