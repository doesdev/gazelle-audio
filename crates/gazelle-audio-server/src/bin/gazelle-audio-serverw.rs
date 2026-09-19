//! The Gazelle control server without a console window: what Start on boot runs, and what to
//! double-click in Explorer. The same server as `gazelle-audio-server`, compiled from the same
//! `main.rs`, with the same options; only the Windows subsystem differs, so Windows never creates
//! a console for it, not even for a moment.
//!
//! It is the tray and the log file that show what it is doing (a tray run writes one by default).
//! Run `gazelle-audio-server` from a terminal to see the log there instead: a windowless program
//! started from a terminal does not write to it. A server that stops with an error says so in a
//! message box, since nothing else would show it.
//!
//! It is also the setup program: the release's `Gazelle-Setup.exe` is this file under another
//! name, and double-clicked with no arguments it offers to install itself (`install::setup`).
//!
//! On other platforms the subsystem does not exist and this is the ordinary server.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[path = "../main.rs"]
mod server;

fn main() -> std::process::ExitCode {
    // Double-clicked as the release's setup file, or for the first time from an unzipped release:
    // offer to install before anything else. Never with arguments (`install::setup`).
    use gazelle_audio_server::install::setup::{from_windowless, Next};
    if from_windowless() == Some(Next::Exit) {
        return std::process::ExitCode::SUCCESS;
    }
    match server::main() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            report(&format!("Gazelle stopped: {e}"));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn report(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(message).as_ptr(), wide("Gazelle").as_ptr(), MB_OK | MB_ICONERROR) };
}

#[cfg(not(windows))]
fn report(message: &str) {
    eprintln!("{message}");
}
