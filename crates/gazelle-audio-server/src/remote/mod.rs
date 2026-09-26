//! Phones on the network: the setting that lets them reach Gazelle, pairing, their tokens, and the
//! second listener.
//!
//! # What this protects, and what it does not
//!
//! Gazelle listens on `127.0.0.1` unless told otherwise. With **Allow phones on this network**
//! on, it also listens on every interface at the same port, and from then on anything on the
//! network can open a connection to it. What stands between that and the devices is this module:
//!
//! - **Every request from another machine must carry a token**, as `Authorization: Bearer` or as
//!   the cookie pairing sets, except the web app's own files (which are the same for everyone and
//!   say nothing about this PC) and the one route that exchanges a pairing code for a token.
//!   Without one it gets 401. The WebSocket is a request like any other here, so it is refused the
//!   same way before it is upgraded (`guard`).
//! - **A token is only ever issued for a pairing code**, and a pairing code is only ever started
//!   from this machine, shown on its screen, and short lived (`pairing`, `token`). So being on the
//!   network is not enough: someone has to have seen this PC's screen in the last 5 minutes.
//! - **Some things stay on this machine whatever a phone holds**: updating and restarting, the
//!   window, the aggregate driver, and everything about phones themselves (the setting, pairing,
//!   the list, revoking). A paired phone gets 403 for them. A phone can do what the app's pages do
//!   to the devices, which is the point of it, and no more.
//! - **Revoking a phone is immediate**: its next request is refused and its open WebSocket is
//!   closed.
//! - **Who is asking is the socket's peer address**, and nothing else. No header (`X-Forwarded-For`
//!   and the like) is ever believed, since anyone can send one. A request with no peer address at
//!   all counts as coming from the network.
//! - **This machine is trusted without a token**, which is what keeps the desktop window and local
//!   scripts as they were. That makes a page in a browser on this machine the thing to guard
//!   against, since it can be made to send requests to `127.0.0.1`. Two checks do it (`guard`):
//!   the `Host` must be a name this server answers to, which defeats DNS rebinding, and an
//!   `Origin`, when a browser sends one, must be this server's own, which stops another site's
//!   page from posting to it or opening its WebSocket.
//!
//! What it does **not** protect: the traffic is plain HTTP. Anyone on the same network who can see
//! the traffic (a shared or open Wi-Fi, a compromised router) can read everything a phone and
//! Gazelle say to each other, the token included, and could then use that token until the phone
//! is revoked. The token limits who can control Gazelle, not who can watch. So phones belong on a
//! network you trust, which is also what Windows' firewall asks when it first sees Gazelle listen:
//! allow it on private networks only.
//!
//! # How `--bind` fits
//!
//! - The default, a loopback address: the setting decides whether the second listener runs, and
//!   it can be turned on and off while Gazelle runs.
//! - A wildcard (`0.0.0.0:8420`): Gazelle is already on every interface, loopback included, so
//!   there is no second listener and the setting has no say. Other machines need a token, as
//!   everywhere.
//! - One network address (`192.168.1.5:8420`): the same, and Gazelle also listens on `127.0.0.1`
//!   at that port (`main.rs`), since otherwise nothing on this machine could reach it without a
//!   token, and nothing could start the pairing that gives one.

pub mod addresses;
pub mod guard;
pub mod pairing;
pub mod seam;
pub mod store;
pub mod token;

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Router;
use serde::Serialize;
use tokio::sync::{broadcast, oneshot};

use addresses::LanAddress;
use pairing::{Limiter, Pairing, WRONG_TRIES_PER_CODE};
use store::{Backing, RemoteFile, StoredPhone};

/// The cookie pairing sets.
pub const COOKIE: &str = "gazelle_token";
/// How long the cookie is kept by the phone's browser: ten years, so in practice until revoked.
pub const COOKIE_MAX_AGE_S: u64 = 10 * 365 * 24 * 3600;
/// The longest name kept for a phone, in characters.
pub const NAME_LIMIT: usize = 60;
/// The most phones that may be paired at once.
pub const PHONE_LIMIT: usize = 32;
/// How long between writes of a phone's "last seen" when nothing else about it changed.
pub const SEEN_SAVE_MS: u64 = 60 * 1000;

/// Milliseconds since the Unix epoch. A parameter so tests can move time.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;
/// Where this PC's network addresses come from. A parameter so tests name their own.
pub type AddressSource = Arc<dyn Fn() -> Vec<LanAddress> + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(crate::driver::now_ms)
}

/// What `--bind` already exposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// A loopback address: nothing else reaches Gazelle unless the setting says so.
    Loopback,
    /// A wildcard address: every interface, whatever the setting says.
    Everywhere(SocketAddr),
    /// One network address, plus loopback.
    Address(SocketAddr),
}

impl Exposure {
    pub fn of(bind: SocketAddr) -> Self {
        let ip = bind.ip().to_canonical();
        if ip.is_loopback() {
            Exposure::Loopback
        } else if ip.is_unspecified() {
            Exposure::Everywhere(bind)
        } else {
            Exposure::Address(bind)
        }
    }
}

/// How a remote access server is put together.
pub struct Options {
    pub backing: Backing,
    pub exposure: Exposure,
    /// The port the main listener really bound.
    pub port: u16,
    /// Where the phone listener binds: every interface at `port`, unless a test says otherwise
    /// (`seam`).
    pub phone_listen: SocketAddr,
    pub clock: Clock,
    pub addresses: AddressSource,
    /// This computer's own names, which a `Host` header may carry (`guard`).
    pub host_names: Vec<String>,
}

impl Options {
    /// The options for this PC: its clock, its adapters and its name.
    pub fn for_this_pc(backing: Backing, bind: SocketAddr, port: u16) -> Self {
        Self {
            backing,
            exposure: Exposure::of(bind),
            port,
            phone_listen: seam::phone_listen(port),
            clock: system_clock(),
            addresses: Arc::new(addresses::this_pc),
            host_names: std::env::var("COMPUTERNAME").ok().into_iter().collect(),
        }
    }
}

/// A paired phone as the list shows it. No token and no hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PhoneView {
    pub id: String,
    pub name: String,
    pub paired_ms: u64,
    pub last_seen_ms: Option<u64>,
    pub last_address: Option<String>,
}

/// A QR code as rows of modules, `1` dark and `0` light, for the page to draw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Qr {
    pub size: usize,
    pub rows: Vec<String>,
}

/// The pairing running now, as this computer's page shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PairingView {
    pub code: String,
    pub expires_ms: u64,
    /// One per address a phone could use, the likeliest first. Empty when none was found.
    pub pair_urls: Vec<String>,
    /// The first of those, as a QR code.
    pub qr: Option<Qr>,
}

/// Everything the Phones section shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub allow_phones: bool,
    /// The `--bind` address, when that decides who can reach Gazelle rather than the setting.
    pub fixed_by_bind: Option<String>,
    /// Whether a phone can reach Gazelle now.
    pub listening: bool,
    /// Why the phone listener is not running although the setting is on.
    pub error: Option<String>,
    pub port: u16,
    pub addresses: Vec<LanAddress>,
    pub urls: Vec<String>,
    pub phones: Vec<PhoneView>,
    pub pairing: Option<PairingView>,
    pub now_ms: u64,
}

/// Why a pairing code was not taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairRefusal {
    /// Too many wrong tries lately, from this address or from everyone.
    TooMany,
    /// No pairing running, the code expired, or the code is wrong. One answer for all three, so
    /// a guesser learns nothing about whether a code exists.
    Refused,
    /// The list is full.
    Full,
    /// Something on this machine failed: no randomness, or the file could not be written.
    Failed(String),
}

/// What pairing gives a phone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paired {
    pub token: String,
    pub phone: PhoneView,
}

struct Phone {
    stored: StoredPhone,
    hash: [u8; 32],
    /// The "last seen" last written to the file.
    saved_seen_ms: Option<u64>,
}

struct Inner {
    allow_phones: bool,
    phones: Vec<Phone>,
    pairing: Option<Pairing>,
    limiter: Limiter,
}

/// The phone listener while it runs.
struct Running {
    stop: oneshot::Sender<()>,
    address: SocketAddr,
}

#[derive(Default)]
struct Listener {
    app: Option<Router>,
    runtime: Option<tokio::runtime::Handle>,
    running: Option<Running>,
    error: Option<String>,
    /// Set once the server is stopping, so nothing starts the listener again.
    closed: bool,
}

/// Remote access: shared by the guard, the routes, the WebSocket and the tray.
pub struct Remote {
    inner: Mutex<Inner>,
    listener: Mutex<Listener>,
    /// Serialises writes of the file, so an older copy never lands after a newer one.
    saving: Mutex<()>,
    backing: Backing,
    exposure: Exposure,
    port: u16,
    phone_listen: SocketAddr,
    clock: Clock,
    addresses: AddressSource,
    host_names: Vec<String>,
    /// Told whenever a phone may have lost its right to be connected; each open phone WebSocket
    /// then checks its own and closes if it has.
    kicks: broadcast::Sender<()>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding one of these leaves plain data behind, never half an invariant that
    // matters more than refusing everything would.
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Remote {
    /// Build it from what is on disk. The second value is a warning to log about the file.
    pub fn new(options: Options) -> (Arc<Self>, Option<String>) {
        let (file, warning) = options.backing.load();
        let mut warning = warning;
        let phones = file
            .phones
            .into_iter()
            .filter_map(|stored| match from_hex_32(&stored.token_sha256) {
                Some(hash) => Some(Phone { saved_seen_ms: stored.last_seen_ms, stored, hash }),
                None => {
                    warning.get_or_insert_with(|| format!("a paired phone ({}) had a damaged token hash and was dropped; pair it again", stored.name));
                    None
                }
            })
            .collect();
        let remote = Arc::new(Self {
            inner: Mutex::new(Inner { allow_phones: file.allow_phones, phones, pairing: None, limiter: Limiter::default() }),
            listener: Mutex::new(Listener::default()),
            saving: Mutex::new(()),
            backing: options.backing,
            exposure: options.exposure,
            port: options.port,
            phone_listen: options.phone_listen,
            clock: options.clock,
            addresses: options.addresses,
            host_names: options.host_names.into_iter().map(|n| n.to_ascii_lowercase()).collect(),
            kicks: broadcast::channel(16).0,
        });
        (remote, warning)
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    pub fn exposure(&self) -> Exposure {
        self.exposure
    }

    /// This computer's own names, lower case.
    pub fn host_names(&self) -> &[String] {
        &self.host_names
    }

    pub fn allow_phones(&self) -> bool {
        lock(&self.inner).allow_phones
    }

    /// Whether `--bind` decides who can reach Gazelle, rather than the setting.
    pub fn fixed_by_bind(&self) -> Option<SocketAddr> {
        match self.exposure {
            Exposure::Loopback => None,
            Exposure::Everywhere(bind) | Exposure::Address(bind) => Some(bind),
        }
    }

    /// Whether anything but this machine may be answered at all. Off, with a loopback bind, even a
    /// request that somehow arrived from elsewhere is refused.
    pub fn accepts_remote(&self) -> bool {
        match self.exposure {
            Exposure::Loopback => self.allow_phones() && lock(&self.listener).running.is_some(),
            _ => true,
        }
    }

    fn file(inner: &Inner) -> RemoteFile {
        RemoteFile { allow_phones: inner.allow_phones, phones: inner.phones.iter().map(|p| p.stored.clone()).collect() }
    }

    /// Write the file as it is now.
    fn save(&self) -> std::io::Result<()> {
        let _saving = lock(&self.saving);
        let file = Self::file(&lock(&self.inner));
        self.backing.save(&file)
    }

    // -----------------------------------------------------------------------------------------
    // The setting and the listener
    // -----------------------------------------------------------------------------------------

    /// Hand over what the phone listener serves, and start it if the setting is on. Called once,
    /// when the app is built; before this the setting is remembered but nothing listens.
    ///
    /// A port still held at start is tried again for a few seconds: a restart into an update starts
    /// the new copy moments after the old one let go, and the system may not have finished closing
    /// the old listener yet.
    pub fn serve_phones(self: &Arc<Self>, app: Router, runtime: tokio::runtime::Handle) {
        {
            let mut listener = lock(&self.listener);
            listener.app = Some(app);
            listener.runtime = Some(runtime.clone());
        }
        self.apply();
        if lock(&self.listener).error.is_some() {
            let remote = self.clone();
            runtime.spawn(async move {
                for _ in 0..20 {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    let remote = remote.clone();
                    let retried = tokio::task::spawn_blocking(move || {
                        remote.apply();
                        let listener = lock(&remote.listener);
                        listener.running.is_some() || listener.error.is_none()
                    });
                    if retried.await.unwrap_or(true) {
                        return;
                    }
                }
            });
        }
    }

    /// Turn phones on or off: saved, then acted on at once. `Err` when `--bind` decides instead,
    /// or the file could not be written (and then nothing changed).
    pub fn set_allow_phones(&self, on: bool) -> Result<(), String> {
        if let Some(bind) = self.fixed_by_bind() {
            return Err(format!("Gazelle was started with --bind {bind}, which decides who can reach it; start it without --bind to use this setting"));
        }
        let was = std::mem::replace(&mut lock(&self.inner).allow_phones, on);
        if let Err(e) = self.save() {
            lock(&self.inner).allow_phones = was;
            return Err(format!("the setting could not be saved: {e}"));
        }
        tracing::info!("phones on this network: {}", if on { "allowed" } else { "not allowed" });
        self.apply();
        Ok(())
    }

    /// Start or stop the phone listener to match the setting.
    fn apply(&self) {
        let want = self.exposure == Exposure::Loopback && self.allow_phones();
        let mut listener = lock(&self.listener);
        if listener.closed {
            return;
        }
        match (want, listener.running.is_some()) {
            (true, false) => {
                let (Some(app), Some(runtime)) = (listener.app.clone(), listener.runtime.clone()) else { return };
                match start_listener(self.phone_listen, app, &runtime) {
                    Ok(running) => {
                        tracing::info!("listening for phones on http://{}", running.address);
                        listener.running = Some(running);
                        listener.error = None;
                    }
                    Err(e) => {
                        tracing::warn!("phones are allowed, but Gazelle could not listen on {}: {e}", self.phone_listen);
                        listener.error = Some(format!("Gazelle could not listen on the network at port {}: {e}", self.phone_listen.port()));
                    }
                }
            }
            (false, true) => {
                if let Some(running) = listener.running.take() {
                    let _ = running.stop.send(());
                    tracing::info!("stopped listening for phones on http://{}", running.address);
                }
                listener.error = None;
                drop(listener);
                // Every phone connected through it loses its right to be, so its WebSocket closes.
                self.kick();
            }
            (false, false) => listener.error = None,
            (true, true) => {}
        }
    }

    /// Stop the phone listener for good: the server is stopping, and the port must be free before
    /// a restart into an update starts the next copy.
    pub fn stop(&self) {
        let mut listener = lock(&self.listener);
        listener.closed = true;
        if let Some(running) = listener.running.take() {
            let _ = running.stop.send(());
        }
        drop(listener);
        self.kick();
    }

    /// The address the phone listener is on, while it runs.
    pub fn phone_address(&self) -> Option<SocketAddr> {
        lock(&self.listener).running.as_ref().map(|r| r.address)
    }

    /// The port a phone would use now.
    fn phone_port(&self) -> u16 {
        match self.exposure {
            Exposure::Loopback => self.phone_address().map_or(self.port, |a| a.port()),
            Exposure::Everywhere(bind) | Exposure::Address(bind) => if bind.port() == 0 { self.port } else { bind.port() },
        }
    }

    /// The addresses a phone could use now.
    fn phone_addresses(&self) -> Vec<LanAddress> {
        match self.exposure {
            Exposure::Address(bind) => vec![LanAddress { ip: bind.ip(), primary: true }],
            _ => (self.addresses)(),
        }
    }

    // -----------------------------------------------------------------------------------------
    // What the page shows
    // -----------------------------------------------------------------------------------------

    pub fn status(&self) -> Status {
        let now = self.now();
        let port = self.phone_port();
        let addresses = self.phone_addresses();
        let urls: Vec<String> = addresses.iter().map(|a| format!("http://{}/", SocketAddr::new(a.ip, port))).collect();
        let (listening, error) = {
            let listener = lock(&self.listener);
            (listener.running.is_some(), listener.error.clone())
        };
        let mut inner = lock(&self.inner);
        if inner.pairing.as_ref().is_some_and(|p| p.expired(now)) {
            inner.pairing = None;
        }
        let pairing = inner.pairing.as_ref().map(|p| pairing_view(p, &urls));
        Status {
            allow_phones: inner.allow_phones,
            fixed_by_bind: self.fixed_by_bind().map(|b| b.to_string()),
            listening: match self.exposure {
                Exposure::Loopback => listening,
                _ => true,
            },
            error,
            port,
            addresses,
            urls,
            phones: inner.phones.iter().map(|p| view(&p.stored)).collect(),
            pairing,
            now_ms: now,
        }
    }

    // -----------------------------------------------------------------------------------------
    // Pairing
    // -----------------------------------------------------------------------------------------

    /// Start pairing, replacing any code already running. Refused while no phone could reach
    /// Gazelle to use it.
    pub fn start_pairing(&self) -> Result<PairingView, String> {
        if !self.accepts_remote() {
            return Err("Turn on Allow phones on this network first: a phone cannot reach Gazelle to pair".into());
        }
        let code = token::new_code()?;
        let pairing = Pairing::new(code, self.now());
        let urls = self.status().urls;
        let shown = pairing_view(&pairing, &urls);
        lock(&self.inner).pairing = Some(pairing);
        tracing::info!("pairing started; the code lasts {} minutes", pairing::CODE_LIFETIME_MS / 60_000);
        Ok(shown)
    }

    /// Stop pairing. Answers whether there was one to stop.
    pub fn cancel_pairing(&self) -> bool {
        lock(&self.inner).pairing.take().is_some()
    }

    /// Exchange a code for a token. `from` is the socket's peer.
    pub fn pair(&self, code: &str, name: &str, from: IpAddr) -> Result<Paired, PairRefusal> {
        let now = self.now();
        let paired = {
            let mut inner = lock(&self.inner);
            if !inner.limiter.allows(from, now) {
                return Err(PairRefusal::TooMany);
            }
            if inner.pairing.as_ref().is_some_and(|p| p.expired(now)) {
                inner.pairing = None;
            }
            let given = token::normalise_code(code).unwrap_or_default();
            let matches = inner.pairing.as_ref().is_some_and(|p| token::same(p.code.as_bytes(), given.as_bytes()));
            if !matches {
                inner.limiter.failed(from, now);
                if let Some(pairing) = inner.pairing.as_mut() {
                    pairing.wrong += 1;
                    if pairing.wrong >= WRONG_TRIES_PER_CODE {
                        inner.pairing = None;
                        tracing::warn!("the pairing code was thrown away after {WRONG_TRIES_PER_CODE} wrong tries");
                    }
                }
                tracing::info!("a pairing try from {from} was refused");
                return Err(PairRefusal::Refused);
            }
            if inner.phones.len() >= PHONE_LIMIT {
                return Err(PairRefusal::Full);
            }
            let token = token::new_token().map_err(PairRefusal::Failed)?;
            let id = token::new_phone_id().map_err(PairRefusal::Failed)?;
            let hash = token::hash(&token);
            let stored = StoredPhone {
                id,
                name: clean_name(name),
                token_sha256: crate::update::release::to_hex(&hash),
                paired_ms: now,
                last_seen_ms: Some(now),
                last_address: Some(from.to_string()),
            };
            // Used once, whatever happens next.
            inner.pairing = None;
            inner.phones.push(Phone { saved_seen_ms: stored.last_seen_ms, hash, stored: stored.clone() });
            Paired { token, phone: view(&stored) }
        };
        if let Err(e) = self.save() {
            // Not kept, so not handed out: a token that would not survive a restart would look
            // like a revocation nobody made.
            lock(&self.inner).phones.retain(|p| p.stored.id != paired.phone.id);
            return Err(PairRefusal::Failed(format!("the paired phone could not be saved: {e}")));
        }
        tracing::info!("paired a phone, \"{}\", from {from}", paired.phone.name);
        Ok(paired)
    }

    // -----------------------------------------------------------------------------------------
    // Tokens and phones
    // -----------------------------------------------------------------------------------------

    /// The phone a token belongs to, noting when and where it was seen. Every stored hash is
    /// compared, in constant time, whether or not an earlier one matched.
    pub fn authenticate(&self, token: &str, from: SocketAddr) -> Option<String> {
        let hash = token::hash(token);
        let now = self.now();
        let (id, save) = {
            let mut inner = lock(&self.inner);
            let mut found = None;
            for (i, phone) in inner.phones.iter().enumerate() {
                if token::same(&phone.hash, &hash) {
                    found = Some(i);
                }
            }
            let phone = &mut inner.phones[found?];
            let address = from.ip().to_string();
            let moved = phone.stored.last_address.as_deref() != Some(address.as_str());
            phone.stored.last_seen_ms = Some(now);
            phone.stored.last_address = Some(address);
            let save = moved || phone.saved_seen_ms.is_none_or(|saved| now.saturating_sub(saved) >= SEEN_SAVE_MS);
            if save {
                phone.saved_seen_ms = Some(now);
            }
            (phone.stored.id.clone(), save)
        };
        if save {
            if let Err(e) = self.save() {
                tracing::warn!("noting when a phone was last seen: {e}");
            }
        }
        Some(id)
    }

    /// Whether this phone may still be connected: still paired, and still able to reach Gazelle.
    pub fn is_valid(&self, phone: &str) -> bool {
        lock(&self.inner).phones.iter().any(|p| p.stored.id == phone) && self.accepts_remote()
    }

    /// Unpair a phone. Its next request is refused and its open WebSocket closes now. Answers
    /// whether there was such a phone.
    pub fn revoke(&self, phone: &str) -> Result<bool, String> {
        let removed = {
            let mut inner = lock(&self.inner);
            let before = inner.phones.len();
            inner.phones.retain(|p| p.stored.id != phone);
            inner.phones.len() != before
        };
        if !removed {
            return Ok(false);
        }
        self.kick();
        tracing::info!("revoked a phone ({phone})");
        self.save().map_err(|e| format!("the phone was revoked, but the list could not be saved: {e}"))?;
        Ok(true)
    }

    pub fn phones(&self) -> Vec<PhoneView> {
        lock(&self.inner).phones.iter().map(|p| view(&p.stored)).collect()
    }

    fn kick(&self) {
        let _ = self.kicks.send(());
    }

    /// A handle for one phone's connection, which ends when the phone should no longer be
    /// connected.
    pub fn session(self: &Arc<Self>, phone: String) -> PhoneSession {
        PhoneSession { remote: self.clone(), phone }
    }
}

/// Who a request from the network is, for the WebSocket to follow.
#[derive(Clone)]
pub struct PhoneSession {
    remote: Arc<Remote>,
    phone: String,
}

impl PhoneSession {
    pub fn phone(&self) -> &str {
        &self.phone
    }

    /// Resolves when this phone is revoked, or phones are turned off.
    pub async fn ended(self) {
        let mut kicks = self.remote.kicks.subscribe();
        loop {
            // Checked after subscribing, so a revocation between the two is not missed.
            if !self.remote.is_valid(&self.phone) {
                return;
            }
            match kicks.recv().await {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
            }
        }
    }
}

fn view(stored: &StoredPhone) -> PhoneView {
    PhoneView {
        id: stored.id.clone(),
        name: stored.name.clone(),
        paired_ms: stored.paired_ms,
        last_seen_ms: stored.last_seen_ms,
        last_address: stored.last_address.clone(),
    }
}

fn pairing_view(pairing: &Pairing, urls: &[String]) -> PairingView {
    let pair_urls: Vec<String> = urls.iter().map(|u| format!("{u}pair#code={}", pairing.code)).collect();
    let qr = pair_urls.first().and_then(|u| qr(u));
    PairingView { code: pairing.code.clone(), expires_ms: pairing.expires_ms, pair_urls, qr }
}

/// A QR code of `text`, as rows of modules.
pub fn qr(text: &str) -> Option<Qr> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let size = code.width();
    let colors = code.to_colors();
    let rows = colors
        .chunks(size)
        .map(|row| row.iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect())
        .collect();
    Some(Qr { size, rows })
}

/// A phone's name as it will be shown: trimmed, without control characters, not too long, and
/// never empty.
pub fn clean_name(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(NAME_LIMIT).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() { "Phone".into() } else { cleaned }
}

fn from_hex_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// Bind and serve the phone listener. The bind is done here, on the caller's thread, so a
/// failure (the port taken on another interface, say) is known before this returns.
fn start_listener(address: SocketAddr, app: Router, runtime: &tokio::runtime::Handle) -> std::io::Result<Running> {
    let socket = std::net::TcpListener::bind(address)?;
    socket.set_nonblocking(true)?;
    let bound = socket.local_addr()?;
    let _entered = runtime.enter();
    let listener = tokio::net::TcpListener::from_std(socket)?;
    let (stop, stopped) = oneshot::channel::<()>();
    runtime.spawn(async move {
        let shutdown = async move {
            let _ = stopped.await;
        };
        if let Err(e) = crate::http::serve_with_shutdown(listener, app, shutdown).await {
            tracing::warn!("the phone listener stopped: {e}");
        }
    });
    Ok(Running { stop, address: bound })
}

#[cfg(test)]
mod tests;
