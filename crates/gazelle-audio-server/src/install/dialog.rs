//! The setup dialog on Windows: a task dialog with buttons that say what they do.
//!
//! `TaskDialogIndirect` lives only in version 6 of the common controls, which the application
//! manifest asks for (`assets/gazelle.manifest`). It is looked up by name at run time rather than
//! imported, so a binary that somehow lost its manifest still starts and falls back to a plain
//! message box, instead of failing to load at all over a missing import.
//!
//! No decisions are made here: [`ask`] shows a [`Question`] and returns the [`Choice`] clicked.

use super::setup::{Choice, Question, Tone, TITLE};

use windows_sys::core::{BOOL, HRESULT, PCSTR, PCWSTR};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Controls::{
    TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_USE_HICON_MAIN, TD_ERROR_ICON,
    TD_WARNING_ICON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    LoadIconW, MessageBoxW, IDNO, IDOK, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK,
    MB_OKCANCEL, MB_SETFOREGROUND, MB_YESNOCANCEL,
};

type TaskDialogIndirect =
    unsafe extern "system" fn(config: *const TASKDIALOGCONFIG, button: *mut i32, radio: *mut i32, checked: *mut BOOL) -> HRESULT;

/// Custom button ids start here, clear of the common ids (`IDOK`, `IDCANCEL` and the rest).
const FIRST_BUTTON: i32 = 100;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Show `question` and wait for an answer.
pub fn ask(question: &Question) -> Choice {
    match task_dialog() {
        Some(show) => ask_with_task_dialog(show, question),
        None => ask_with_message_box(question),
    }
}

fn task_dialog() -> Option<TaskDialogIndirect> {
    let name = wide("comctl32.dll");
    // With the manifest's activation context this is version 6 of the library.
    let library = unsafe { LoadLibraryW(name.as_ptr()) };
    if library.is_null() {
        return None;
    }
    let found = unsafe { GetProcAddress(library, c"TaskDialogIndirect".as_ptr() as PCSTR) }?;
    // SAFETY: the export of that name has exactly this signature (commctrl.h).
    Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, TaskDialogIndirect>(found) })
}

fn ask_with_task_dialog(show: TaskDialogIndirect, question: &Question) -> Choice {
    let title = wide(TITLE);
    let text = wide(&question.text);
    let labels: Vec<Vec<u16>> = question.choices.iter().map(|c| wide(c.label())).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| TASKDIALOG_BUTTON { nButtonID: FIRST_BUTTON + i as i32, pszButtonText: label.as_ptr() })
        .collect();

    let mut config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: title.as_ptr(),
        pszContent: text.as_ptr(),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: FIRST_BUTTON,
        ..Default::default()
    };
    match question.tone {
        // The app's own icon: resource 1, the group the build embeds.
        Tone::Question => {
            let icon = unsafe { LoadIconW(GetModuleHandleW(std::ptr::null()), 1 as PCWSTR) };
            if !icon.is_null() {
                config.dwFlags |= TDF_USE_HICON_MAIN;
                config.Anonymous1.hMainIcon = icon;
            }
        }
        Tone::Warning => config.Anonymous1.pszMainIcon = TD_WARNING_ICON,
        Tone::Error => config.Anonymous1.pszMainIcon = TD_ERROR_ICON,
    }

    let mut clicked = 0;
    let result = unsafe { show(&config, &mut clicked, std::ptr::null_mut(), std::ptr::null_mut()) };
    if result < 0 {
        return ask_with_message_box(question);
    }
    usize::try_from(clicked - FIRST_BUTTON)
        .ok()
        .and_then(|i| question.choices.get(i).copied())
        .unwrap_or_else(|| question.dismissed())
}

/// The fallback: a message box's fixed buttons stand for the question's, in order, and the text
/// says which is which when there are three.
fn ask_with_message_box(question: &Question) -> Choice {
    let choices = &question.choices;
    let (buttons, text) = match choices.len() {
        0 | 1 => (MB_OK, question.text.clone()),
        2 => (MB_OKCANCEL, format!("{}\n\nOK: {}.", question.text, choices[0].label())),
        _ => (
            MB_YESNOCANCEL,
            format!("{}\n\nYes: {}. No: {}.", question.text, choices[0].label(), choices[1].label()),
        ),
    };
    let icon = match question.tone {
        Tone::Question => MB_ICONINFORMATION,
        Tone::Warning => MB_ICONWARNING,
        Tone::Error => MB_ICONERROR,
    };
    let answer = unsafe {
        MessageBoxW(std::ptr::null_mut(), wide(&text).as_ptr(), wide(TITLE).as_ptr(), buttons | icon | MB_SETFOREGROUND)
    };
    let index = match answer {
        IDOK | IDYES => 0,
        IDNO => 1,
        // IDCANCEL, or the box closed some other way.
        _ => return question.dismissed(),
    };
    choices.get(index).copied().unwrap_or_else(|| question.dismissed())
}
