//! The window itself: a Tao window holding a Wry webview pointed at this server.
//!
//! **It runs on a thread of its own.** The tray already owns the main thread and its message
//! loop (P70), and Windows gives every thread its own message queue, so the window takes a second
//! one rather than the tray giving up the first. Tao normally insists on the main thread; on
//! Windows `with_any_thread` lifts that, which is why this is Windows-first. The event loop never
//! returns, so the thread is not joined: the process ends when the server does.
//!
//! Closing the window **hides** it and leaves the server running in the tray, which is what the
//! tray is for. The tray's Open shows it again, and so does a second launch of the binary
//! (`crate::handover`).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Icon, WindowBuilder};
use wry::WebViewBuilder;

use super::state::{WindowState, MIN_HEIGHT, MIN_WIDTH};
use super::TITLE;

/// The icon is drawn at this size and Windows scales it down for the title bar and up for
/// Alt-Tab; a bigger one costs a few kilobytes of pixels once.
const ICON_SIZE: usize = 64;

/// At most one write of the window's position per this long, so dragging a window across a screen
/// does not write a file per frame. The last position before the process ends may be lost, which
/// is a pixel or two of accuracy, not a setting.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// What the server tells the window thread to do. Only one thing so far.
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    /// Come to the front: the tray's Open, or a second launch of the binary.
    Show,
}

/// A running window. Dropping it does not close the window; the process's end does.
pub struct Window {
    // `EventLoopProxy` is `Send` but makes no promise about `Sync`, and the HTTP handler that
    // shows the window may be on any worker thread.
    proxy: Mutex<EventLoopProxy<UserEvent>>,
}

impl Window {
    /// Bring the window to the front. Safe from any thread, and a no-op if the window is gone.
    pub fn show(&self) {
        let Ok(proxy) = self.proxy.lock() else { return };
        if let Err(e) = proxy.send_event(UserEvent::Show) {
            tracing::warn!("the window did not take the request to show itself: {e}");
        }
    }
}

/// Open the window on `url` and return a handle that brings it to the front.
///
/// An error means there is no window and the server carries on headless — the likeliest reason on
/// Windows is no WebView2 runtime, which Windows 11 ships but an old Windows 10 may not.
pub fn open(url: String, state_path: PathBuf) -> Result<Arc<Window>, String> {
    // The event loop must be built on the thread that runs it, so the proxy comes back from there.
    let (tx, rx) = mpsc::channel::<Result<EventLoopProxy<UserEvent>, String>>();
    std::thread::Builder::new()
        .name("gazelle-window".into())
        .spawn(move || run(&url, state_path, &tx))
        .map_err(|e| format!("starting the window thread: {e}"))?;
    let proxy = rx.recv().map_err(|_| "the window thread stopped before it opened one".to_string())??;
    Ok(Arc::new(Window { proxy: Mutex::new(proxy) }))
}

/// The window thread: build everything, report back, then pump events until the process ends.
fn run(url: &str, state_path: PathBuf, tx: &mpsc::Sender<Result<EventLoopProxy<UserEvent>, String>>) {
    let mut builder = EventLoopBuilder::<UserEvent>::with_user_event();
    #[cfg(windows)]
    {
        use tao::platform::windows::EventLoopBuilderExtWindows;
        // The tray has the main thread and its message loop; this window has this one.
        builder.with_any_thread(true);
    }
    let event_loop = builder.build();
    let saved = WindowState::load(&state_path);

    let window = match WindowBuilder::new()
        .with_title(TITLE)
        .with_inner_size(LogicalSize::new(f64::from(saved.width), f64::from(saved.height)))
        .with_min_inner_size(LogicalSize::new(f64::from(MIN_WIDTH), f64::from(MIN_HEIGHT)))
        .with_window_icon(icon())
        .with_maximized(saved.maximised)
        .build(&event_loop)
    {
        Ok(window) => window,
        Err(e) => {
            let _ = tx.send(Err(format!("creating the window: {e}")));
            return;
        }
    };
    // A remembered position is restored after building, so the size above is not re-centred.
    if let (Some(x), Some(y)) = (saved.x, saved.y) {
        window.set_outer_position(PhysicalPosition::new(x, y));
    }

    // The webview is built before the server starts answering. That is fine and deliberate: the
    // listener is already bound by then, so the first request waits in the accept backlog for the
    // moment it takes to attach devices, rather than failing to connect.
    let webview = match WebViewBuilder::new().with_url(url).build(&window) {
        Ok(webview) => webview,
        Err(e) => {
            let _ = tx.send(Err(format!("creating the webview (is the WebView2 runtime installed?): {e}")));
            return;
        }
    };

    if tx.send(Ok(event_loop.create_proxy())).is_err() {
        // Nobody is waiting for the window, so nobody wants it.
        return;
    }
    tracing::info!("window open on {url} ({}x{}) — --no-window to run without one", saved.width, saved.height);

    let mut last_save = Instant::now();
    event_loop.run(move |event, _target, control_flow| {
        // Nothing here animates; the loop sleeps until the next event.
        *control_flow = ControlFlow::Wait;
        // The webview is owned by this closure so it lives as long as the window does.
        let _ = &webview;
        match event {
            Event::NewEvents(StartCause::Init) => {}
            // Closing leaves the server running in the tray; the tray's Open shows it again.
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                save(&window, &state_path);
                last_save = Instant::now();
                window.set_visible(false);
            }
            Event::WindowEvent { event: WindowEvent::Resized(_) | WindowEvent::Moved(_), .. } => {
                if last_save.elapsed() >= SAVE_INTERVAL {
                    save(&window, &state_path);
                    last_save = Instant::now();
                }
            }
            Event::UserEvent(UserEvent::Show) => {
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
            }
            _ => {}
        }
    });
}

/// Write where the window is now. A window that is hidden or minimised reports a position that is
/// not where it will come back, so those are left as they were.
fn save(window: &tao::window::Window, path: &Path) {
    if !window.is_visible() || window.is_minimized() {
        return;
    }
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let position = window.outer_position().ok();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let state = WindowState {
        width: size.width.max(0.0) as u32,
        height: size.height.max(0.0) as u32,
        x: position.map(|p| p.x),
        y: position.map(|p| p.y),
        maximised: window.is_maximized(),
    }
    .sanitised();
    if let Err(e) = state.save(path) {
        tracing::warn!("remembering the window's place in {}: {e}", path.display());
    }
}

/// The app's icon at [`ICON_SIZE`], or none if it cannot be made — a window with the system's
/// default icon is better than no window.
fn icon() -> Option<Icon> {
    let rgba: Vec<u8> = crate::icon::rgba(ICON_SIZE).into_iter().flatten().collect();
    Icon::from_rgba(rgba, ICON_SIZE as u32, ICON_SIZE as u32).ok()
}
