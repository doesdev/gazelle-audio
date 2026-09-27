//! The windows themselves: Tao windows holding Wry webviews pointed at this server.
//!
//! **They run on a thread of their own.** The tray already owns the main thread and its message
//! loop, and Windows gives every thread its own message queue, so the windows take a second one
//! rather than the tray giving up the first. Tao normally insists on the main thread; on Windows
//! `with_any_thread` lifts that, which is why this is Windows-first. The event loop never returns,
//! so the thread is not joined: the process ends when the server does. All three windows are on this
//! one loop; everything else reaches them through its proxy ([`UserEvent`]).
//!
//! - **The app's window**: closing it **hides** it and leaves the server running in the tray. The
//!   tray's Open shows it again, and so does a second launch of the binary (`crate::handover`).
//! - **The recording widget**: frameless, always on top, out of the taskbar. It is dragged by its
//!   body: the page posts `drag` over Wry's IPC on a mouse-down away from its buttons, and this
//!   thread starts Windows' own move loop with Tao's `drag_window`, so the move is the system's and
//!   feels like any window's. Closing it closes it; whether it was open is remembered and restored.
//! - **The recording hub**: full screen on the monitor it was last on, else the primary. Esc in the
//!   page posts `windowed` and it becomes an ordinary window; `full-screen` puts it back. Closing it
//!   closes it.
//!
//! IPC is only for what a page asks of the very window it is in (move me, close me, leave full
//! screen), where the answer has to be immediate and there is nobody else to ask. Opening and closing
//! from elsewhere (the Recording page, the tray) goes through [`Window`]'s [`Viewports`], which the
//! HTTP routes call.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::monitor::MonitorHandle;
use tao::window::{Fullscreen, Icon, WindowBuilder};
use wry::{WebView, WebViewBuilder};

use super::place::{self, Fallback, Monitor, Rect};
use super::state::{Kind, WindowState};
use super::{Viewports, ViewportsState};

/// The icon is drawn at this size and Windows scales it down for the title bar and up for
/// Alt-Tab; a bigger one costs a few kilobytes of pixels once.
const ICON_SIZE: usize = 64;

/// At most one write of a window's position per this long, so dragging a window across a screen
/// does not write a file per frame. The last move is written once the dragging stops.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// How long after Windows closes the widget its being closed is written down; see the event loop.
const FORGET_AFTER: Duration = Duration::from_secs(5);

/// What the page's background is until it has drawn, so a window does not flash white: the dark
/// theme's surface.
const BACKGROUND: (u8, u8, u8, u8) = (0x1a, 0x1b, 0x1d, 0xff);

/// What the server tells the window thread to do.
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    /// Bring the app's window to the front: the tray's Open, or a second launch of the binary.
    Show,
    Widget(bool),
    Hub(bool),
    HubFullScreen(bool),
    /// The page in this window was pressed on somewhere that moves it.
    Drag(Kind),
}

/// What the windows need to know when they open.
pub struct Options {
    /// The server's address, with the trailing slash: `http://127.0.0.1:8420/`.
    pub url: String,
    /// Where each window is remembered.
    pub main_state: PathBuf,
    pub widget_state: PathBuf,
    pub hub_state: PathBuf,
    /// Show the app's window now; false leaves it in the tray, as a login start wants.
    pub visible: bool,
    /// Open the recording hub now, full screen: the "start in the hub" setting.
    pub hub: bool,
}

/// The running windows. Dropping this does not close them; the process's end does.
pub struct Window {
    // `EventLoopProxy` is `Send` but makes no promise about `Sync`, and the HTTP handler that
    // shows the window may be on any worker thread.
    proxy: Mutex<EventLoopProxy<UserEvent>>,
    flags: Arc<Flags>,
}

/// What is on screen, as the window thread last left it.
#[derive(Default)]
struct Flags {
    showing: AtomicBool,
    widget: AtomicBool,
    hub: AtomicBool,
    hub_full_screen: AtomicBool,
}

impl Window {
    fn send(&self, event: UserEvent) {
        let Ok(proxy) = self.proxy.lock() else { return };
        if let Err(e) = proxy.send_event(event) {
            tracing::warn!("the windows did not take {event:?}: {e}");
        }
    }

    /// Bring the app's window to the front. Safe from any thread, and a no-op if the window is gone.
    pub fn show(&self) {
        self.send(UserEvent::Show);
    }

    /// Whether the app's window is on screen rather than closed to the tray.
    pub fn is_showing(&self) -> bool {
        self.flags.showing.load(Ordering::Relaxed)
    }
}

impl Viewports for Window {
    fn state(&self) -> ViewportsState {
        ViewportsState {
            widget: self.flags.widget.load(Ordering::Relaxed),
            hub: self.flags.hub.load(Ordering::Relaxed),
            hub_full_screen: self.flags.hub.load(Ordering::Relaxed) && self.flags.hub_full_screen.load(Ordering::Relaxed),
        }
    }

    // Each says at once what it asked for, so the answer to the request that asked is what will be
    // on screen a moment later; the window thread puts it right if it could not be done.
    fn set_widget(&self, open: bool) {
        self.flags.widget.store(open, Ordering::Relaxed);
        self.send(UserEvent::Widget(open));
    }

    fn set_hub(&self, open: bool) {
        self.flags.hub.store(open, Ordering::Relaxed);
        if open {
            self.flags.hub_full_screen.store(true, Ordering::Relaxed);
        }
        self.send(UserEvent::Hub(open));
    }

    fn set_hub_full_screen(&self, full: bool) {
        self.flags.hub_full_screen.store(full, Ordering::Relaxed);
        self.send(UserEvent::HubFullScreen(full));
    }
}

/// Open the windows and return a handle that reaches them. With `options.visible` false the app's
/// window is made but left hidden, as a login start wants: the server is in the tray, and the first
/// Open shows the window already loaded. The widget comes back if it was open when Gazelle last
/// stopped, and the hub opens if `options.hub` says so, whatever `visible` is.
///
/// An error means there is no window and the server carries on headless. The likeliest reason on
/// Windows is no WebView2 runtime, which Windows 11 ships but an old Windows 10 may not.
pub fn open(options: Options) -> Result<Arc<Window>, String> {
    // The event loop must be built on the thread that runs it, so the proxy comes back from there.
    let (tx, rx) = mpsc::channel::<Result<EventLoopProxy<UserEvent>, String>>();
    let flags = Arc::new(Flags::default());
    flags.showing.store(options.visible, Ordering::Relaxed);
    let shared = flags.clone();
    std::thread::Builder::new()
        .name("gazelle-window".into())
        .spawn(move || run(options, &shared, &tx))
        .map_err(|e| format!("starting the window thread: {e}"))?;
    let proxy = rx.recv().map_err(|_| "the window thread stopped before it opened one".to_string())??;
    Ok(Arc::new(Window { proxy: Mutex::new(proxy), flags }))
}

/// One window and its webview, and when it was last written down.
struct Pane {
    kind: Kind,
    window: tao::window::Window,
    // Owned so it lives as long as the window does.
    _webview: WebView,
    path: PathBuf,
    /// What was read or last written, for what the window itself cannot say (the hub's windowed size
    /// while it is full screen).
    remembered: WindowState,
    last_save: Instant,
    /// Moved or resized since the last write.
    pending: bool,
}

impl Pane {
    /// Write where the window is now, at most once per [`SAVE_INTERVAL`] unless `now` says it must.
    fn save(&mut self, now: bool) {
        if !now && self.last_save.elapsed() < SAVE_INTERVAL {
            self.pending = true;
            return;
        }
        self.pending = false;
        self.last_save = Instant::now();
        let Some(state) = self.current() else { return };
        self.remembered = state.clone();
        if let Err(e) = state.save(&self.path) {
            tracing::warn!("remembering where the {} is in {}: {e}", self.kind.title(), self.path.display());
        }
    }

    /// Where the window is now, as it would be remembered. `None` when it is somewhere it will not
    /// come back to: hidden or minimised.
    fn current(&self) -> Option<WindowState> {
        let window = &self.window;
        if !window.is_visible() || window.is_minimized() {
            return None;
        }
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
        let position = window.outer_position().ok();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (width, height) = (size.width.max(0.0) as u32, size.height.max(0.0) as u32);
        let state = match self.kind {
            Kind::Main => WindowState { width, height, x: position.map(|p| p.x), y: position.map(|p| p.y), maximised: window.is_maximized(), ..WindowState::first_run(Kind::Main) },
            Kind::Widget => WindowState { width, height, x: position.map(|p| p.x), y: position.map(|p| p.y), open: true, ..WindowState::first_run(Kind::Widget) },
            // The hub is remembered by its monitor. Its windowed size is kept for when it leaves full
            // screen, and a full-screen size is not one.
            Kind::Hub => {
                let monitor = window.current_monitor();
                let (width, height) = if window.fullscreen().is_some() { (self.remembered.width, self.remembered.height) } else { (width, height) };
                WindowState {
                    width,
                    height,
                    x: monitor.as_ref().map(|m| m.position().x),
                    y: monitor.as_ref().map(|m| m.position().y),
                    monitor: monitor.and_then(|m| m.name()),
                    ..WindowState::first_run(Kind::Hub)
                }
            }
        };
        Some(state.sanitised(self.kind))
    }
}

/// The window thread: build everything, report back, then pump events until the process ends.
fn run(options: Options, flags: &Arc<Flags>, tx: &mpsc::Sender<Result<EventLoopProxy<UserEvent>, String>>) {
    let mut builder = EventLoopBuilder::<UserEvent>::with_user_event();
    #[cfg(windows)]
    {
        use tao::platform::windows::EventLoopBuilderExtWindows;
        // The tray has the main thread and its message loop; these windows have this one.
        builder.with_any_thread(true);
    }
    let event_loop = builder.build();
    let proxy = event_loop.create_proxy();

    let mut main = match build(&event_loop, &proxy, Kind::Main, &options.url, &options.main_state, options.visible) {
        Ok(pane) => pane,
        Err(why) => {
            let _ = tx.send(Err(why));
            return;
        }
    };
    if tx.send(Ok(event_loop.create_proxy())).is_err() {
        // Nobody is waiting for the window, so nobody wants it.
        return;
    }
    let hidden = if options.visible { "" } else { ", hidden until the tray's Open" };
    tracing::info!(
        "window open on {} ({}x{}{hidden}); --no-window to run without one",
        options.url,
        main.remembered.width,
        main.remembered.height
    );

    let mut widget: Option<Pane> = None;
    if WindowState::load(&options.widget_state, Kind::Widget).open {
        tracing::info!("the recording widget was open when Gazelle last stopped; opening it again");
        widget = open_widget(&event_loop, &proxy, &options, flags);
    }
    let mut hub: Option<Pane> = None;
    if options.hub {
        tracing::info!("starting in the recording hub, as the recording settings say");
        hub = open_hub(&event_loop, &proxy, &options, flags);
    }

    let mut maximise_on_show = !options.visible && main.remembered.maximised;
    // When to write down that the widget was closed by Windows, rather than by a person in Gazelle.
    let mut forget_widget_at: Option<Instant> = None;
    let flags = flags.clone();
    event_loop.run(move |event, target, control_flow| {
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                for pane in std::iter::once(&mut main).chain(widget.as_mut()).chain(hub.as_mut()) {
                    if pane.pending && pane.last_save.elapsed() >= SAVE_INTERVAL {
                        pane.save(true);
                    }
                }
                if forget_widget_at.is_some_and(|at| Instant::now() >= at) {
                    forget_widget_at = None;
                    if widget.is_none() && !super::shutting_down() {
                        remember_widget_closed(&options.widget_state);
                    }
                }
            }
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                if window_id == main.window.id() {
                    // Closing leaves the server running in the tray; the tray's Open shows it again.
                    main.save(true);
                    main.window.set_visible(false);
                    flags.showing.store(false, Ordering::Relaxed);
                } else if widget.as_ref().is_some_and(|p| p.window.id() == window_id) {
                    // Windows asked, not the page, the tray or the app: Alt+F4, or `taskkill`
                    // asking every window to close on its way to stopping Gazelle. It closes now;
                    // that it is closed is written a moment later, unless Gazelle is stopping by
                    // then, so a restart brings back a widget that was open.
                    close(&mut widget, &flags.widget, false);
                    forget_widget_at = Some(Instant::now() + FORGET_AFTER);
                } else if hub.as_ref().is_some_and(|p| p.window.id() == window_id) {
                    close(&mut hub, &flags.hub, true);
                }
            }
            Event::WindowEvent { window_id, event: WindowEvent::Resized(_) | WindowEvent::Moved(_), .. } => {
                for pane in std::iter::once(&mut main).chain(widget.as_mut()).chain(hub.as_mut()) {
                    if pane.window.id() == window_id {
                        pane.save(false);
                    }
                }
            }
            Event::UserEvent(UserEvent::Show) => {
                main.window.set_visible(true);
                flags.showing.store(true, Ordering::Relaxed);
                if std::mem::take(&mut maximise_on_show) {
                    main.window.set_maximized(true);
                }
                main.window.set_minimized(false);
                main.window.set_focus();
            }
            Event::UserEvent(UserEvent::Widget(true)) => {
                forget_widget_at = None;
                if widget.is_none() {
                    widget = open_widget(target, &proxy, &options, &flags);
                }
            }
            Event::UserEvent(UserEvent::Widget(false)) => {
                forget_widget_at = None;
                close(&mut widget, &flags.widget, true);
            }
            Event::UserEvent(UserEvent::Hub(true)) => match hub.as_mut() {
                Some(pane) => {
                    full_screen(target, pane, true);
                    flags.hub_full_screen.store(true, Ordering::Relaxed);
                    pane.window.set_focus();
                }
                None => hub = open_hub(target, &proxy, &options, &flags),
            },
            Event::UserEvent(UserEvent::Hub(false)) => close(&mut hub, &flags.hub, true),
            Event::UserEvent(UserEvent::HubFullScreen(full)) => {
                if let Some(pane) = hub.as_mut() {
                    full_screen(target, pane, full);
                    flags.hub_full_screen.store(full, Ordering::Relaxed);
                }
            }
            Event::UserEvent(UserEvent::Drag(kind)) => {
                let pane = match kind {
                    Kind::Main => Some(&main),
                    Kind::Widget => widget.as_ref(),
                    Kind::Hub => hub.as_ref(),
                };
                if let Some(pane) = pane {
                    // Windows' own move loop: it runs until the button is let go, and the window's
                    // Moved events save where it ends up.
                    if let Err(e) = pane.window.drag_window() {
                        tracing::debug!("dragging the {}: {e}", kind.title());
                    }
                }
            }
            _ => {}
        }
        // Nothing here animates; the loop sleeps until the next event, or until a move that was
        // not written yet is due to be.
        let due = std::iter::once(&main)
            .chain(widget.as_ref())
            .chain(hub.as_ref())
            .filter(|p| p.pending)
            .map(|p| p.last_save + SAVE_INTERVAL)
            .chain(forget_widget_at)
            .min();
        *control_flow = match due {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        };
    });
}

/// Build one window of `kind` over its route. The app's window is built hidden or shown as asked;
/// the widget shown, without taking the focus from whatever has it; the hub hidden, to be put full
/// screen on its monitor before it is shown.
fn build(target: &EventLoopWindowTarget<UserEvent>, proxy: &EventLoopProxy<UserEvent>, kind: Kind, url: &str, path: &Path, visible: bool) -> Result<Pane, String> {
    let saved = WindowState::load(path, kind);
    let (min_width, min_height) = kind.min_size();
    let mut builder = WindowBuilder::new()
        .with_title(kind.title())
        .with_inner_size(LogicalSize::new(f64::from(saved.width), f64::from(saved.height)))
        .with_min_inner_size(LogicalSize::new(f64::from(min_width), f64::from(min_height)))
        .with_window_icon(icon())
        .with_visible(visible);
    builder = match kind {
        // Maximising a window shows it, so a hidden one is maximised when it is first shown.
        Kind::Main => builder.with_maximized(visible && saved.maximised),
        Kind::Widget => {
            let builder = builder.with_decorations(false).with_always_on_top(true).with_focused(false);
            #[cfg(windows)]
            let builder = {
                use tao::platform::windows::WindowBuilderExtWindows;
                // Always on top already; a taskbar button for it would be one more thing to tidy.
                builder.with_skip_taskbar(true).with_undecorated_shadow(true)
            };
            builder
        }
        Kind::Hub => builder,
    };
    let window = builder.build(target).map_err(|e| format!("creating the {}: {e}", kind.title()))?;
    // A remembered position is restored after building, so the size above is not re-centred, and
    // only where enough of the window would be on a monitor that is there now.
    if let (Some(x), Some(y), Kind::Main | Kind::Widget) = (saved.x, saved.y, kind) {
        let outer = window.outer_size();
        let rect = Rect { x, y, width: outer.width, height: outer.height };
        let fallback = if kind == Kind::Widget { Fallback::TopRight } else { Fallback::Centre };
        let (x, y) = place::keep_on_screen(rect, &monitors(target).0, fallback);
        window.set_outer_position(PhysicalPosition::new(x, y));
    } else if kind == Kind::Widget {
        // A first run: out of the way, in the primary monitor's top-right corner.
        let outer = window.outer_size();
        let rect = Rect { x: i32::MIN / 2, y: i32::MIN / 2, width: outer.width, height: outer.height };
        let (x, y) = place::keep_on_screen(rect, &monitors(target).0, Fallback::TopRight);
        window.set_outer_position(PhysicalPosition::new(x, y));
    }

    // The webview is built before the server starts answering. That is fine and deliberate: the
    // listener is already bound by then, so the first request waits in the accept backlog for the
    // moment it takes to attach devices, rather than failing to connect.
    let asks = proxy.clone();
    let webview = WebViewBuilder::new()
        .with_url(format!("{url}{}", kind.route()))
        .with_background_color(BACKGROUND)
        .with_focused(kind != Kind::Widget)
        .with_ipc_handler(move |request| {
            let event = match request.body().as_str() {
                "drag" => Some(UserEvent::Drag(kind)),
                "close" if kind == Kind::Widget => Some(UserEvent::Widget(false)),
                "close" if kind == Kind::Hub => Some(UserEvent::Hub(false)),
                "windowed" if kind == Kind::Hub => Some(UserEvent::HubFullScreen(false)),
                "full-screen" if kind == Kind::Hub => Some(UserEvent::HubFullScreen(true)),
                _ => None,
            };
            if let Some(event) = event {
                let _ = asks.send_event(event);
            }
        })
        .build(&window)
        .map_err(|e| format!("creating the webview for the {} (is the WebView2 runtime installed?): {e}", kind.title()))?;
    Ok(Pane { kind, window, _webview: webview, path: path.to_path_buf(), remembered: saved, last_save: Instant::now(), pending: false })
}

/// Open the widget where it was, and remember that it is open.
fn open_widget(target: &EventLoopWindowTarget<UserEvent>, proxy: &EventLoopProxy<UserEvent>, options: &Options, flags: &Flags) -> Option<Pane> {
    match build(target, proxy, Kind::Widget, &options.url, &options.widget_state, true) {
        Ok(mut pane) => {
            pane.save(true);
            flags.widget.store(true, Ordering::Relaxed);
            tracing::info!("recording widget open");
            Some(pane)
        }
        Err(why) => {
            flags.widget.store(false, Ordering::Relaxed);
            tracing::warn!("no recording widget: {why}");
            None
        }
    }
}

/// Open the hub full screen on its monitor, and bring it to the front.
fn open_hub(target: &EventLoopWindowTarget<UserEvent>, proxy: &EventLoopProxy<UserEvent>, options: &Options, flags: &Flags) -> Option<Pane> {
    match build(target, proxy, Kind::Hub, &options.url, &options.hub_state, false) {
        Ok(mut pane) => {
            full_screen(target, &mut pane, true);
            pane.window.set_visible(true);
            pane.window.set_focus();
            pane.save(true);
            flags.hub.store(true, Ordering::Relaxed);
            flags.hub_full_screen.store(true, Ordering::Relaxed);
            tracing::info!("recording hub open");
            Some(pane)
        }
        Err(why) => {
            flags.hub.store(false, Ordering::Relaxed);
            tracing::warn!("no recording hub: {why}");
            None
        }
    }
}

/// Close the widget or the hub, remembering where it was, and, with `closed`, that it is closed.
fn close(pane: &mut Option<Pane>, open: &AtomicBool, closed: bool) {
    open.store(false, Ordering::Relaxed);
    let Some(mut pane) = pane.take() else { return };
    pane.save(true);
    if pane.kind == Kind::Widget && closed {
        remember_widget_closed(&pane.path);
    }
    tracing::info!("{} closed", pane.kind.title());
    // Dropping the pane destroys the window and its webview.
}

/// Write down that the widget is closed, so the next start does not open it.
fn remember_widget_closed(path: &Path) {
    let closed = WindowState { open: false, ..WindowState::load(path, Kind::Widget) };
    if let Err(e) = closed.save(path) {
        tracing::warn!("remembering that the recording widget is closed: {e}");
    }
}

/// Put the hub into full screen on the monitor it belongs on, or out of it into an ordinary window
/// in the middle of that monitor.
fn full_screen(target: &EventLoopWindowTarget<UserEvent>, pane: &mut Pane, full: bool) {
    let (list, handles) = monitors(target);
    let saved = &pane.remembered;
    let at = pane
        .window
        .current_monitor()
        .filter(|_| pane.window.is_visible())
        .and_then(|current| handles.iter().position(|h| *h == current))
        .or_else(|| place::monitor_for(saved.monitor.as_deref(), saved.x.zip(saved.y), &list));
    let monitor = at.and_then(|at| handles.get(at).cloned());
    if full {
        pane.window.set_fullscreen(Some(Fullscreen::Borderless(monitor)));
        return;
    }
    pane.window.set_fullscreen(None);
    let size = LogicalSize::new(f64::from(saved.width), f64::from(saved.height));
    pane.window.set_inner_size(size);
    if let Some(screen) = at.and_then(|at| list.get(at)) {
        let outer = pane.window.outer_size();
        let rect = Rect { x: i32::MIN / 2, y: i32::MIN / 2, width: outer.width, height: outer.height };
        let (x, y) = place::keep_on_screen(rect, std::slice::from_ref(&Monitor { primary: true, ..screen.clone() }), Fallback::Centre);
        pane.window.set_outer_position(PhysicalPosition::new(x, y));
    }
    pane.window.set_focus();
}

/// The monitors there are now, as plain data and as Tao's handles, in the same order.
fn monitors(target: &EventLoopWindowTarget<UserEvent>) -> (Vec<Monitor>, Vec<MonitorHandle>) {
    let primary = target.primary_monitor();
    let handles: Vec<MonitorHandle> = target.available_monitors().collect();
    let list = handles
        .iter()
        .map(|handle| {
            let (position, size) = (handle.position(), handle.size());
            Monitor { name: handle.name(), x: position.x, y: position.y, width: size.width, height: size.height, primary: primary.as_ref() == Some(handle) }
        })
        .collect();
    (list, handles)
}

/// The app's icon at [`ICON_SIZE`], or none if it cannot be made: a window with the system's
/// default icon is better than no window.
fn icon() -> Option<Icon> {
    let rgba: Vec<u8> = crate::icon::rgba(ICON_SIZE).into_iter().flatten().collect();
    Icon::from_rgba(rgba, ICON_SIZE as u32, ICON_SIZE as u32).ok()
}
