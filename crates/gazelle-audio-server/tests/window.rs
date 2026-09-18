//! The handover a second launch makes: a real server on an ephemeral port, and the same code
//! path a second copy of the binary takes when the port is already ours.
//!
//! No window is created here — only the decision, the request and the reply, which is all a
//! second instance ever sees.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use gazelle_audio_server::device::manager::DeviceManager;
use gazelle_audio_server::handover::{self, Outcome};
use gazelle_audio_server::registry_set::RegistrySet;
use gazelle_audio_server::workspace::store::{MemoryStore, WorkspaceStore};
use gazelle_audio_server::{http, AppState};

/// A server on an ephemeral loopback port, with or without a window to show.
async fn serve(show_window: Option<Arc<dyn Fn() + Send + Sync>>) -> SocketAddr {
    let devices = DeviceManager::new(RegistrySet::builtin().expect("registries"));
    let store: Arc<dyn WorkspaceStore> = Arc::new(MemoryStore::default());
    let app = http::router(AppState {
        devices,
        store,
        force_dry_run: false, enable_recall: false,
        backend: "loopback".into(),
        themes_dir: None,
        snapshots: std::sync::Arc::new(gazelle_audio_server::snapshot::store::MemorySnapshotStore::default()),
        show_window,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { http::serve(listener, app).await.unwrap() });
    address
}

#[tokio::test]
async fn a_second_launch_brings_the_running_window_to_the_front() {
    let shown = Arc::new(AtomicUsize::new(0));
    let counter = shown.clone();
    let address = serve(Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })))
    .await;

    // The blocking request is what the second process really makes, so it runs off the runtime.
    let outcome = tokio::task::spawn_blocking(move || handover::hand_over(address)).await.unwrap();
    assert_eq!(outcome.expect("the running server answered"), Outcome::Shown);
    assert_eq!(shown.load(Ordering::SeqCst), 1, "the window was shown once");
}

#[tokio::test]
async fn a_second_launch_against_a_headless_server_is_told_so() {
    let address = serve(None).await;
    let outcome = tokio::task::spawn_blocking(move || handover::hand_over(address)).await.unwrap();
    match outcome.expect("the running server answered") {
        Outcome::Headless(reason) => assert!(reason.contains("without a window"), "{reason}"),
        other => panic!("expected a headless answer, got {other:?}"),
    }
}

#[tokio::test]
async fn a_port_held_by_something_else_is_not_mistaken_for_gazelle() {
    // A listener that accepts and says nothing: the handover must fail rather than hang or claim
    // a Gazelle is running, so the second launch reports the port as taken.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        // Hold the connection open until the test ends.
        std::future::pending::<()>().await;
        drop(stream);
    });
    let outcome = tokio::task::spawn_blocking(move || handover::hand_over(address)).await.unwrap();
    assert!(outcome.is_err(), "a silent listener is not a Gazelle: {outcome:?}");
}

#[tokio::test]
async fn nothing_listening_is_an_error_not_a_handover() {
    // Bound and dropped, so the port is almost certainly free.
    let address = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };
    let outcome = tokio::task::spawn_blocking(move || handover::hand_over(address)).await.unwrap();
    assert!(outcome.is_err(), "{outcome:?}");
}
