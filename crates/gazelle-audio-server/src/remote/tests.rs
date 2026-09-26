//! Remote access without a network: a clock the test moves, addresses the test names, and a file
//! in a temporary folder. The one listener started here is on 127.0.0.1.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::addresses::LanAddress;
use super::pairing::{ALL_FAILURES, CODE_LIFETIME_MS, PEER_FAILURES, WINDOW_MS, WRONG_TRIES_PER_CODE};
use super::store::Backing;
use super::*;

const START: u64 = 1_789_700_000_000;

struct Harness {
    remote: Arc<Remote>,
    clock: Arc<AtomicU64>,
}

impl Harness {
    fn advance(&self, ms: u64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }
}

fn options(backing: Backing, exposure: Exposure, clock: Arc<AtomicU64>) -> Options {
    Options {
        backing,
        exposure,
        port: 8420,
        phone_listen: "127.0.0.1:0".parse().unwrap(),
        clock: Arc::new(move || clock.load(Ordering::SeqCst)),
        addresses: Arc::new(|| vec![LanAddress { ip: "192.168.1.5".parse().unwrap(), primary: true }, LanAddress { ip: "10.0.0.2".parse().unwrap(), primary: false }]),
        host_names: vec!["Studio-PC".into()],
    }
}

/// As if started with `--bind 0.0.0.0:8420`: other machines can reach it with no listener to start.
fn everywhere() -> Harness {
    with(Backing::Memory, Exposure::Everywhere("0.0.0.0:8420".parse().unwrap()))
}

fn with(backing: Backing, exposure: Exposure) -> Harness {
    let clock = Arc::new(AtomicU64::new(START));
    let (remote, warning) = Remote::new(options(backing, exposure, clock.clone()));
    assert_eq!(warning, None);
    Harness { remote, clock }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gazelle-remote-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("remote.json")
}

fn phone_ip() -> IpAddr {
    "192.168.1.50".parse().unwrap()
}

fn phone_at() -> SocketAddr {
    "192.168.1.50:51000".parse().unwrap()
}

#[test]
fn a_code_pairs_once_and_gives_a_token_that_works() {
    let h = everywhere();
    let code = h.remote.start_pairing().unwrap().code;
    let paired = h.remote.pair(&code.to_lowercase(), "  Pixel 9  ", phone_ip()).unwrap();
    assert_eq!(paired.phone.name, "Pixel 9");
    assert_eq!(h.remote.authenticate(&paired.token, phone_at()).as_deref(), Some(paired.phone.id.as_str()));
    assert_eq!(h.remote.pair(&code, "Again", phone_ip()), Err(PairRefusal::Refused), "a code is used once");
    assert_eq!(h.remote.status().pairing, None, "and pairing is over");
    assert_eq!(h.remote.authenticate(&token::new_token().unwrap(), phone_at()), None, "any other token is nobody");
}

#[test]
fn a_code_expires_after_five_minutes() {
    let h = everywhere();
    let code = h.remote.start_pairing().unwrap().code;
    h.advance(CODE_LIFETIME_MS);
    assert_eq!(h.remote.pair(&code, "Late", phone_ip()), Err(PairRefusal::Refused));
    assert_eq!(h.remote.status().pairing, None);

    let code = h.remote.start_pairing().unwrap().code;
    h.advance(CODE_LIFETIME_MS - 1);
    assert!(h.remote.pair(&code, "Just in time", phone_ip()).is_ok());
}

#[test]
fn starting_again_replaces_the_code_and_cancelling_ends_it() {
    let h = everywhere();
    let first = h.remote.start_pairing().unwrap().code;
    let second = h.remote.start_pairing().unwrap().code;
    if first != second {
        assert_eq!(h.remote.pair(&first, "Old", phone_ip()), Err(PairRefusal::Refused));
    }
    assert!(h.remote.cancel_pairing());
    assert_eq!(h.remote.pair(&second, "Cancelled", phone_ip()), Err(PairRefusal::Refused));
    assert!(!h.remote.cancel_pairing());
}

#[test]
fn one_address_guessing_is_stopped_before_its_code_is_looked_at() {
    let h = everywhere();
    let code = h.remote.start_pairing().unwrap().code;
    for _ in 0..PEER_FAILURES {
        assert_eq!(h.remote.pair("0000-0000", "Guess", phone_ip()), Err(PairRefusal::Refused));
    }
    assert_eq!(h.remote.pair(&code, "Right, too late", phone_ip()), Err(PairRefusal::TooMany), "even the right code waits");
    // Another phone is not held to the first one's count, and the code is still good.
    assert!(h.remote.pair(&code, "Other", "192.168.1.51".parse().unwrap()).is_ok());
    h.advance(WINDOW_MS);
    assert_eq!(h.remote.pair("0000-0000", "Later", phone_ip()), Err(PairRefusal::Refused), "a minute later it is heard again");
}

#[test]
fn a_code_dies_after_ten_wrong_tries_from_anywhere() {
    let h = everywhere();
    let code = h.remote.start_pairing().unwrap().code;
    for i in 0..WRONG_TRIES_PER_CODE {
        let from: IpAddr = format!("192.168.2.{}", i + 1).parse().unwrap();
        let _ = h.remote.pair("0000-0000", "Guess", from);
    }
    assert_eq!(h.remote.status().pairing, None);
    assert_eq!(h.remote.pair(&code, "Owner", phone_ip()), Err(PairRefusal::Refused));
}

#[test]
fn many_addresses_guessing_together_are_stopped_too() {
    let h = everywhere();
    let mut refused = 0;
    for i in 0..(ALL_FAILURES + 5) {
        // A fresh pairing each time, so the per code limit is not what stops them.
        if i % 5 == 0 {
            h.remote.start_pairing().unwrap();
        }
        let from: IpAddr = format!("10.1.{}.{}", i / 200, i % 200 + 1).parse().unwrap();
        if h.remote.pair("0000-0000", "Guess", from) == Err(PairRefusal::TooMany) {
            refused += 1;
        }
    }
    assert_eq!(refused, 5, "after {ALL_FAILURES} failures in a minute, everyone waits");
}

#[test]
fn pairing_is_refused_while_no_phone_could_reach_gazelle() {
    let h = with(Backing::Memory, Exposure::Loopback);
    assert!(!h.remote.accepts_remote());
    let refused = h.remote.start_pairing().unwrap_err();
    assert!(refused.contains("Allow phones"), "{refused}");
}

#[test]
fn revoking_ends_a_phone_at_once() {
    let h = everywhere();
    let code = h.remote.start_pairing().unwrap().code;
    let paired = h.remote.pair(&code, "Pixel", phone_ip()).unwrap();
    assert!(h.remote.is_valid(&paired.phone.id));
    assert_eq!(h.remote.revoke(&paired.phone.id), Ok(true));
    assert_eq!(h.remote.authenticate(&paired.token, phone_at()), None);
    assert!(!h.remote.is_valid(&paired.phone.id));
    assert_eq!(h.remote.revoke(&paired.phone.id), Ok(false), "a second revoke finds nothing");
}

#[tokio::test]
async fn an_open_session_ends_when_its_phone_is_revoked_and_not_before() {
    let h = everywhere();
    let code = h.remote.start_pairing().unwrap().code;
    let keep = h.remote.pair(&code, "Keep", phone_ip()).unwrap();
    let code = h.remote.start_pairing().unwrap().code;
    let lose = h.remote.pair(&code, "Lose", phone_ip()).unwrap();
    let kept = tokio::spawn(h.remote.session(keep.phone.id.clone()).ended());
    let lost = tokio::spawn(h.remote.session(lose.phone.id.clone()).ended());
    tokio::task::yield_now().await;
    h.remote.revoke(&lose.phone.id).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), lost).await.expect("the revoked phone's session ended").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!kept.is_finished(), "the other phone stays connected");
    kept.abort();
}

#[test]
fn phones_and_the_setting_persist_as_hashes_across_a_restart() {
    let path = temp("persist");
    let h = with(Backing::File(path.clone()), Exposure::Everywhere("0.0.0.0:8420".parse().unwrap()));
    let code = h.remote.start_pairing().unwrap().code;
    let paired = h.remote.pair(&code, "Pixel", phone_ip()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(&paired.token), "the token itself is never written");
    assert!(text.contains(&crate::update::release::to_hex(&token::hash(&paired.token))));

    let again = with(Backing::File(path), Exposure::Everywhere("0.0.0.0:8420".parse().unwrap()));
    assert_eq!(again.remote.phones().len(), 1);
    assert_eq!(again.remote.authenticate(&paired.token, phone_at()).as_deref(), Some(paired.phone.id.as_str()));
}

#[test]
fn the_setting_persists_and_is_refused_where_bind_decides() {
    let path = temp("setting");
    let h = with(Backing::File(path.clone()), Exposure::Loopback);
    assert!(!h.remote.allow_phones());
    h.remote.set_allow_phones(true).unwrap();
    let again = with(Backing::File(path), Exposure::Loopback);
    assert!(again.remote.allow_phones(), "the setting survives a restart");

    let fixed = with(Backing::Memory, Exposure::Everywhere("0.0.0.0:8420".parse().unwrap()));
    assert!(fixed.remote.set_allow_phones(true).unwrap_err().contains("--bind"));
    assert_eq!(fixed.remote.status().fixed_by_bind.as_deref(), Some("0.0.0.0:8420"));
}

#[test]
fn last_seen_is_noted_and_written_at_most_once_a_minute_from_one_address() {
    let path = temp("seen");
    let h = with(Backing::File(path.clone()), Exposure::Everywhere("0.0.0.0:8420".parse().unwrap()));
    let code = h.remote.start_pairing().unwrap().code;
    let paired = h.remote.pair(&code, "Pixel", phone_ip()).unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    h.advance(1_000);
    h.remote.authenticate(&paired.token, phone_at()).unwrap();
    assert_eq!(h.remote.phones()[0].last_seen_ms, Some(START + 1_000), "noted in memory at once");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), written, "not written again within the minute");
    h.advance(SEEN_SAVE_MS);
    h.remote.authenticate(&paired.token, "192.168.1.60:1".parse().unwrap()).unwrap();
    let phone = &h.remote.phones()[0];
    assert_eq!(phone.last_address.as_deref(), Some("192.168.1.60"));
    assert!(std::fs::read_to_string(&path).unwrap().contains("192.168.1.60"));
}

#[test]
fn the_status_lists_addresses_urls_and_a_scannable_pairing() {
    let h = everywhere();
    let status = h.remote.status();
    assert_eq!(status.urls, ["http://192.168.1.5:8420/", "http://10.0.0.2:8420/"]);
    assert!(status.listening);
    let pairing = h.remote.start_pairing().unwrap();
    assert_eq!(pairing.pair_urls[0], format!("http://192.168.1.5:8420/pair#code={}", pairing.code));
    assert_eq!(pairing.expires_ms, START + CODE_LIFETIME_MS);
    let qr = pairing.qr.expect("a QR code of the first address");
    assert_eq!(qr.rows.len(), qr.size);
    assert!(qr.rows.iter().all(|row| row.len() == qr.size && row.chars().all(|c| c == '0' || c == '1')));
    // The finder pattern's top row: seven dark modules in the corner.
    assert!(qr.rows[0].starts_with("1111111"));
    assert_eq!(h.remote.status().pairing, Some(PairingView { qr: Some(qr), ..pairing }));
}

#[test]
fn bound_to_one_address_that_is_the_address_offered() {
    let h = with(Backing::Memory, Exposure::Address("192.168.1.77:9000".parse().unwrap()));
    assert_eq!(h.remote.status().urls, ["http://192.168.1.77:9000/"]);
}

#[test]
fn names_are_cleaned_and_never_empty() {
    assert_eq!(clean_name("  Pixel\u{7} 9 "), "Pixel 9");
    assert_eq!(clean_name("   "), "Phone");
    assert_eq!(clean_name(&"x".repeat(200)).chars().count(), NAME_LIMIT);
}

#[test]
fn a_damaged_hash_in_the_file_drops_that_phone_and_says_so() {
    let path = temp("damaged-hash");
    std::fs::write(&path, r#"{"allow_phones":false,"phones":[{"id":"1","name":"Old","token_sha256":"zz","paired_ms":1}]}"#).unwrap();
    let clock = Arc::new(AtomicU64::new(START));
    let (remote, warning) = Remote::new(options(Backing::File(path), Exposure::Loopback, clock));
    assert!(remote.phones().is_empty());
    assert!(warning.unwrap().contains("Old"));
}

/// The listener on every interface is started and stopped while the server runs. Here it binds
/// 127.0.0.1 on a port of its own, which is as close as a test may come.
#[tokio::test(flavor = "multi_thread")]
async fn turning_phones_on_and_off_starts_and_stops_the_listener_at_once() {
    let h = with(Backing::Memory, Exposure::Loopback);
    let app = axum::Router::new().route("/api/v1/health", axum::routing::get(|| async { "ok" }));
    h.remote.serve_phones(app, tokio::runtime::Handle::current());
    assert_eq!(h.remote.phone_address(), None, "off: nothing listens beyond the main listener");

    let remote = h.remote.clone();
    tokio::task::spawn_blocking(move || remote.set_allow_phones(true)).await.unwrap().unwrap();
    let address = h.remote.phone_address().expect("on: the phone listener runs");
    assert!(address.ip().is_loopback(), "tests only ever listen on loopback");
    assert!(h.remote.accepts_remote());
    tokio::net::TcpStream::connect(address).await.expect("something answers there");

    let remote = h.remote.clone();
    tokio::task::spawn_blocking(move || remote.set_allow_phones(false)).await.unwrap().unwrap();
    assert_eq!(h.remote.phone_address(), None);
    assert!(!h.remote.accepts_remote());
    // The port is let go: connecting now fails (after a moment for the task to finish).
    let mut gone = false;
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(address).await.is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(gone, "the listener on {address} was closed");
}
