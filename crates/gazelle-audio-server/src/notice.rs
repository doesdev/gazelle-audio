//! What the server has to say about itself, beyond the device list.
//!
//! Both of them so far are about an empty device list, which is the one thing that must never be
//! all the app says. Antelope's own Manager Service opens the interfaces exclusively while it
//! runs, so a USB server started beside it attaches nothing (`reference/usb-access.md`): the
//! commonest way the app looks broken when it is not. And with the USB backend the default
//! (decision `0018`), a first run with nothing plugged in is empty for the ordinary reason, which
//! is worth saying too.
//!
//! The rule is a pure function of what the server can see, evaluated wherever it is asked for
//! rather than tracked: a device attaching, or the service stopping, changes the answer at once.

use serde::Serialize;

/// Something the server wants said, in the window and in the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Notice {
    /// A stable identifier, so a client can recognise this notice without matching prose.
    pub code: &'static str,
    /// One or two sentences: what is wrong, and what to do about it.
    pub message: String,
}

/// The code of [`service_conflict`]'s notice.
pub const ANTELOPE_SERVICE: &str = "antelope-service-holds-devices";

/// Antelope's service is running, and it is why nothing attached.
///
/// Only for the USB backend and only while no device is open: with one open the service either is
/// not running or is not in the way, and the loopback never touches hardware.
pub fn service_conflict(backend: &str, devices: usize, service_running: bool) -> Option<Notice> {
    (backend == "usb" && devices == 0 && service_running).then(|| Notice {
        code: ANTELOPE_SERVICE,
        message: "Antelope's Manager Service is running and holds the interfaces, so Gazelle cannot \
                  open them. Stop the Antelope Manager Service and the devices attach within a few \
                  seconds."
            .into(),
    })
}

/// The code of [`nothing_attached`]'s notice.
pub const NO_DEVICE: &str = "no-device-attached";

/// Nothing is attached, and Antelope's service is not the reason.
///
/// This became worth saying when `--backend usb` became the default (decision `0018`): before
/// then an empty device list meant the person had asked for hardware, so they knew what they were
/// looking at. Now it is what someone sees on first run with nothing plugged in, and an app with
/// no explanation for its own emptiness looks broken.
pub fn nothing_attached(backend: &str, devices: usize, service_running: bool) -> Option<Notice> {
    (backend == "usb" && devices == 0 && !service_running).then(|| Notice {
        code: NO_DEVICE,
        message: "No Antelope interface is attached, so there is nothing to control yet. Plug one \
                  in and it appears within a few seconds. To see the app without one, start the \
                  server with --backend loopback."
            .into(),
    })
}

/// Everything true of the server right now. `service_running` is a parameter so the rule is tested
/// without a service control manager; [`crate::tray::antelope_service_running`] supplies it.
///
/// The two device notices are alternatives. "Nothing is attached" and "something is attached and
/// Antelope's service is holding it" are answers to the same question, and the second is the more
/// useful one whenever it applies.
pub fn current(backend: &str, devices: usize, service_running: bool) -> Vec<Notice> {
    service_conflict(backend, devices, service_running)
        .or_else(|| nothing_attached(backend, devices, service_running))
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_is_named_only_when_it_is_the_reason_nothing_attached() {
        let notice = service_conflict("usb", 0, true).expect("the service holds the devices");
        assert_eq!(notice.code, ANTELOPE_SERVICE);
        assert!(notice.message.contains("Antelope's Manager Service"), "{}", notice.message);
        assert!(notice.message.contains("Stop"), "says what to do: {}", notice.message);
        // A device is open, so whatever the service is doing it is not in the way.
        assert_eq!(service_conflict("usb", 1, true), None);
        // The service is not running: nothing attached for some other reason.
        assert_eq!(service_conflict("usb", 0, false), None);
        // The loopback never opens a device, so the service is irrelevant to it.
        assert_eq!(service_conflict("loopback", 0, true), None);
    }

    #[test]
    fn nothing_attached_and_nothing_holding_it_says_so_rather_than_showing_an_empty_app() {
        let notice = nothing_attached("usb", 0, false).expect("no device, no service");
        assert_eq!(notice.code, NO_DEVICE);
        assert!(notice.message.contains("loopback"), "it offers the way to see the UI anyway: {}", notice.message);
        // The service being up is the other notice's business, and it is the more useful one.
        assert_eq!(nothing_attached("usb", 0, true), None);
        assert_eq!(nothing_attached("usb", 1, false), None);
        // The loopback always has its devices; an empty list there is not this.
        assert_eq!(nothing_attached("loopback", 0, false), None);
    }

    #[test]
    fn current_is_the_notices_that_apply_and_never_both_reasons_at_once() {
        assert_eq!(current("usb", 0, true), vec![service_conflict("usb", 0, true).unwrap()]);
        assert_eq!(current("usb", 0, false), vec![nothing_attached("usb", 0, false).unwrap()]);
        assert!(current("usb", 2, false).is_empty(), "devices are attached: nothing to say");
        assert!(current("loopback", 0, true).is_empty());
    }
}
