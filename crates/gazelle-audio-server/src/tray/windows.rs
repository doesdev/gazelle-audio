//! The tray on Windows, straight on Win32: a hidden window receives the icon's messages, and the
//! menu is built from [`super::menu`] each time it opens.
//!
//! Plain `windows-sys` rather than a tray crate: the server already depends on it through
//! `hidapi`, so the tray adds no packages to audit against the Rust floor, and a hidden window
//! with one icon and one popup menu is not much code.

use std::cell::{Cell, RefCell};
use std::io;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_STATUS,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem, TrackPopupMenu,
    TranslateMessage, HICON, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, SW_SHOWNORMAL,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_NULL,
    WNDCLASSW,
};

use super::boot::{boot_program, RunKey, StartOnBoot};
use super::{device_label, menu, ui_url, Command, Context, Item, Status, ANTELOPE_SERVICE};

/// The message the icon sends to the window.
const WM_TRAY: u32 = WM_APP + 1;
/// A keyboard selection of the icon: `NIN_SELECT | NINF_KEY`, not exported by `windows-sys`.
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const ICON_ID: u32 = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A created tray, owned by the thread that created it.
pub struct Tray {
    hwnd: HWND,
}

/// Closes the tray from any thread.
#[derive(Clone)]
pub struct Closer(HWND);

// SAFETY: the handle is only ever passed to `PostMessageW`, which may be called from any thread;
// a stale handle after the window is gone makes the post fail harmlessly.
unsafe impl Send for Closer {}
unsafe impl Sync for Closer {}

impl Closer {
    pub fn close(&self) {
        unsafe { PostMessageW(self.0, WM_CLOSE, 0, 0) };
    }
}

impl Tray {
    pub fn closer(&self) -> Closer {
        Closer(self.hwnd)
    }

    /// Pump messages until the window is destroyed.
    pub fn run(self) {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        // 0 is WM_QUIT and -1 an error; either way the loop is over.
        while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        STATE.with(|s| s.borrow_mut().take());
    }
}

struct State {
    context: Context,
    boot: StartOnBoot,
    icon: HICON,
    /// Explorer broadcasts this when the taskbar is recreated, and every icon must be added again.
    taskbar_created: u32,
    last_open: Cell<Option<Instant>>,
}

thread_local! {
    // The window procedure is a plain function, so the tray's state lives with the thread that
    // owns the window. Handlers clone the `Rc` out and drop the borrow before doing anything that
    // pumps messages (a popup menu does), so a nested message never finds it borrowed.
    static STATE: RefCell<Option<Rc<State>>> = const { RefCell::new(None) };
}

fn state() -> Option<Rc<State>> {
    STATE.with(|s| s.borrow().clone())
}

pub fn start(context: Context) -> Result<Tray, String> {
    let class = wide("GazelleTray");
    let hwnd = unsafe {
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        // Registering twice fails, which only matters if the window then cannot be created.
        RegisterClassW(&wc);
        // Never shown. Not a message-only window, since those miss the taskbar broadcast.
        CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, null_mut(), null_mut(), instance, null())
    };
    if hwnd.is_null() {
        return Err(format!("creating the tray window: {}", io::Error::last_os_error()));
    }

    // The login entry runs the windowless build when it sits beside this one, so logging in opens
    // no console window.
    let program = boot_program(&context.exe, |p| p.is_file());
    let boot = StartOnBoot::new(Box::new(UserRunKey), program, &context.boot_args);
    let state = Rc::new(State {
        icon: make_icon(),
        taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
        last_open: Cell::new(None),
        context,
        boot,
    });
    if let Err(e) = add_icon(hwnd, &state) {
        unsafe {
            DestroyWindow(hwnd);
            DestroyIcon(state.icon);
        }
        return Err(e);
    }
    STATE.with(|s| *s.borrow_mut() = Some(state));
    Ok(Tray { hwnd })
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = ICON_ID;
    data
}

fn add_icon(hwnd: HWND, state: &State) -> Result<(), String> {
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = state.icon;
    let tip = wide(&format!("Gazelle — {}", ui_url(state.context.address)));
    let n = tip.len().min(data.szTip.len() - 1);
    data.szTip[..n].copy_from_slice(&tip[..n]);
    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
        return Err("the shell did not accept a notification icon (no desktop session?)".into());
    }
    // Version 4 reports clicks as NIN_SELECT and the menu key as WM_CONTEXTMENU.
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
    Ok(())
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(state) = state() else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_TRAY => {
            // Version 4: the event is the low word of `lparam`.
            match (lparam as u32) & 0xffff {
                NIN_SELECT | NIN_KEYSELECT => open_ui(&state),
                WM_CONTEXTMENU => show_menu(hwnd, &state),
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
            DestroyIcon(state.icon);
            PostQuitMessage(0);
            0
        }
        m if m == state.taskbar_created && m != 0 => {
            if let Err(e) = add_icon(hwnd, &state) {
                tracing::warn!("re-adding the tray icon after the taskbar restarted: {e}");
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn open_ui(state: &State) {
    if !state.context.web_ui {
        return;
    }
    // A double-click selects twice; one browser tab is what was meant.
    let double_click = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
    if state.last_open.get().is_some_and(|t| t.elapsed() < double_click) {
        return;
    }
    state.last_open.set(Some(Instant::now()));
    let url = ui_url(state.context.address);
    if let Err(code) = shell_open(&url) {
        tracing::warn!("opening {url} in the browser failed (ShellExecute returned {code})");
    }
}

/// Open a URL or a folder as Explorer would.
fn shell_open(target: &str) -> Result<(), usize> {
    let result = unsafe {
        ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(target).as_ptr(), null(), null(), SW_SHOWNORMAL)
    };
    // ShellExecute reports success as a value above 32.
    if (result as usize) <= 32 {
        Err(result as usize)
    } else {
        Ok(())
    }
}

fn status(state: &State) -> Status {
    let c = &state.context;
    Status {
        address: c.address,
        backend: c.backend.clone(),
        dry_run: c.dry_run,
        web_ui: c.web_ui,
        devices: c.devices.descriptors().iter().map(device_label).collect(),
        antelope_service_running: service_running(ANTELOPE_SERVICE),
        start_on_boot: state.boot.is_enabled(),
        log_file: c.log_dir.is_some(),
        can_rescan: c.rescan.is_some(),
    }
}

fn show_menu(hwnd: HWND, state: &Rc<State>) {
    let items = menu(&status(state));
    let picked = unsafe {
        let popup = CreatePopupMenu();
        if popup.is_null() {
            return;
        }
        for item in &items {
            match item {
                Item::Separator => {
                    AppendMenuW(popup, MF_SEPARATOR, 0, null());
                }
                Item::Info(text) => {
                    AppendMenuW(popup, MF_STRING | MF_GRAYED, 0, wide(text).as_ptr());
                }
                Item::Action { command, label, enabled, checked, default } => {
                    let mut flags = MF_STRING;
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    if *checked == Some(true) {
                        flags |= MF_CHECKED;
                    }
                    AppendMenuW(popup, flags, command.id(), wide(label).as_ptr());
                    if *default {
                        SetMenuDefaultItem(popup, command.id() as u32, 0);
                    }
                }
            }
        }
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        // Without the foreground window the menu does not close when clicking elsewhere, and
        // without the trailing message it can fail to show the second time.
        SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(popup, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, at.x, at.y, 0, hwnd, null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(popup);
        picked
    };
    match Command::from_id(picked as usize) {
        Some(Command::Open) => open_ui(state),
        Some(Command::StartOnBoot) => match state.boot.toggle() {
            Ok(true) => tracing::info!("start on boot: on ({})", state.boot.command()),
            Ok(false) => tracing::info!("start on boot: off"),
            Err(e) => tracing::warn!("changing start on boot: {e}"),
        },
        Some(Command::OpenLogFolder) => {
            if let Some(dir) = &state.context.log_dir {
                if let Err(code) = shell_open(&dir.display().to_string()) {
                    tracing::warn!("opening the log folder {} failed (ShellExecute returned {code})", dir.display());
                }
            }
        }
        // The scan runs on its own thread; the next time the menu opens, its lines show the result.
        Some(Command::Rescan) => {
            if let Some(rescan) = &state.context.rescan {
                tracing::info!("rescanning devices, from the tray");
                rescan();
            }
        }
        Some(Command::Quit) => {
            tracing::info!("quit from the tray");
            (state.context.quit)();
            unsafe { DestroyWindow(hwnd) };
        }
        None => {}
    }
}

/// Whether a Windows service is running. Only reads its status; anything that cannot be read
/// (not installed, no access) counts as not running.
pub(super) fn service_running(name: &str) -> bool {
    unsafe {
        let manager = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return false;
        }
        let service = OpenServiceW(manager, wide(name).as_ptr(), SERVICE_QUERY_STATUS);
        let mut status: SERVICE_STATUS = std::mem::zeroed();
        let running = !service.is_null() && QueryServiceStatus(service, &mut status) != 0 && status.dwCurrentState == SERVICE_RUNNING;
        if !service.is_null() {
            CloseServiceHandle(service);
        }
        CloseServiceHandle(manager);
        running
    }
}

/// The current user's login entries, `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
struct UserRunKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

impl RunKey for UserRunKey {
    fn read(&self, name: &str) -> io::Result<Option<String>> {
        let (key, name) = (wide(RUN_KEY), wide(name));
        let mut bytes = 0u32;
        let query = |data: *mut u16, bytes: &mut u32| unsafe {
            RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), data.cast(), bytes)
        };
        match query(null_mut(), &mut bytes) {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(None),
            e => return Err(io::Error::from_raw_os_error(e as i32)),
        }
        let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
        match query(buf.as_mut_ptr(), &mut bytes) {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(None),
            e => return Err(io::Error::from_raw_os_error(e as i32)),
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(Some(String::from_utf16_lossy(&buf[..len])))
    }

    fn write(&self, name: &str, command: &str) -> io::Result<()> {
        let data = wide(command);
        let result = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(name).as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        match result {
            ERROR_SUCCESS => Ok(()),
            e => Err(io::Error::from_raw_os_error(e as i32)),
        }
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), wide(name).as_ptr()) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            e => Err(io::Error::from_raw_os_error(e as i32)),
        }
    }
}

/// The icon, drawn rather than shipped: a light "G" on a dark grey disc, at the small-icon size.
fn make_icon() -> HICON {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as usize;
    let pixels = icon_pixels(size);
    let mut color = Vec::with_capacity(size * size * 4);
    // The AND mask is one bit per pixel, rows padded to 16 bits; set where fully transparent.
    let row = size.div_ceil(16) * 2;
    let mut mask = vec![0u8; row * size];
    for (i, [r, g, b, a]) in pixels.into_iter().enumerate() {
        color.extend([b, g, r, a]);
        if a == 0 {
            mask[(i / size) * row + (i % size) / 8] |= 0x80 >> ((i % size) % 8);
        }
    }
    unsafe {
        CreateIcon(GetModuleHandleW(null()), size as i32, size as i32, 1, 32, mask.as_ptr(), color.as_ptr())
    }
}

/// RGBA, top row first, antialiased by sampling each pixel 4x4.
fn icon_pixels(size: usize) -> Vec<[u8; 4]> {
    const DISC: [f32; 3] = [0x2b as f32, 0x2d as f32, 0x31 as f32];
    const MARK: [f32; 3] = [0xe8 as f32, 0xa8 as f32, 0x38 as f32];
    const SAMPLES: usize = 4;
    let mut out = Vec::with_capacity(size * size);
    for py in 0..size {
        for px in 0..size {
            let (mut disc, mut mark) = (0.0f32, 0.0f32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    // -1..1 across the icon, y up.
                    let x = ((px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32) * 2.0 - 1.0;
                    let y = 1.0 - ((py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32) * 2.0;
                    let r = (x * x + y * y).sqrt();
                    if r <= 0.97 {
                        disc += 1.0;
                        let angle = y.atan2(x).to_degrees();
                        // A ring open on the right above the bar, plus the bar itself.
                        let ring = (0.40..=0.66).contains(&r) && !(0.0..60.0).contains(&angle);
                        let bar = (-0.06..=0.12).contains(&y) && (0.05..=0.66).contains(&x);
                        if ring || bar {
                            mark += 1.0;
                        }
                    }
                }
            }
            let n = (SAMPLES * SAMPLES) as f32;
            let (coverage, m) = (disc / n, if disc > 0.0 { mark / disc } else { 0.0 });
            let mix = |i: usize| (DISC[i] + (MARK[i] - DISC[i]) * m).round() as u8;
            out.push([mix(0), mix(1), mix(2), (coverage * 255.0).round() as u8]);
        }
    }
    out
}
