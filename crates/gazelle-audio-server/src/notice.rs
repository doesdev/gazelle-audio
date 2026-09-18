//! What the server has to say about itself, beyond the device list.
//!
//! There is one such thing so far, and it is the commonest way the app looks broken when it is
//! not: Antelope's own Manager Service opens the interfaces exclusively while it runs, so a
//! `--backend usb` server started beside it attaches nothing (`reference/usb-access.md`). An
//! empty device list is the one thing that must not be all the app says then.
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

/// Everything true of the server right now. `service_running` is a parameter so the rule is tested
/// without a service control manager; [`crate::tray::antelope_service_running`] supplies it.
pub fn current(backend: &str, devices: usize, service_running: bool) -> Vec<Notice> {
    service_conflict(backend, devices, service_running).into_iter().collect()
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
    fn current_is_the_notices_that_apply() {
        assert_eq!(current("usb", 0, true), vec![service_conflict("usb", 0, true).unwrap()]);
        assert!(current("loopback", 0, true).is_empty());
    }
}
