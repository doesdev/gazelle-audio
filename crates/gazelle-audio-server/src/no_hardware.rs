//! The refusal that keeps a harness off the user's own interfaces.
//!
//! `--backend` defaults to `usb` (decision `0018`), which is right for the shipped app and wrong
//! for everything that starts a server in a test: a harness that forgets the flag would open the
//! devices on the developer's desk. The flag is passed explicitly everywhere it matters, but
//! "everywhere" is exactly the kind of rule a new harness quietly breaks.
//!
//! So the harnesses also set one environment variable, and a server that sees it refuses the USB
//! backend outright and says why. It is the simplest guard that catches the mistake **where the
//! mistake would do harm**: in the server itself, whatever spawned it and however it was
//! spawned, before a device is opened, rather than at each of the places that must remember
//! something.

/// The variable. Set to anything but `0`, `false` or the empty string, no server started under it
/// will open real devices.
pub const VAR: &str = "GAZELLE_NO_HARDWARE";

/// Whether this value of [`VAR`] forbids hardware. Unset does not; `0`, `false` or nothing at all
/// does not, so a shell that always exports the name can still turn it off.
pub fn forbidden(value: Option<&str>) -> bool {
    !matches!(value.map(str::trim), None | Some("") | Some("0") | Some("false"))
}

/// What to say, and stop for, when this run asked for hardware it is forbidden.
///
/// `backend` is `--backend` as it is spelled on the command line. `None` means carry on.
pub fn refusal(backend: &str, value: Option<&str>) -> Option<String> {
    (backend == "usb" && forbidden(value)).then(|| {
        format!(
            "{VAR} is set, so this server will not open real devices, but --backend is usb, \
             which is now the default. Pass --backend loopback to run against the \
             emulator, or set {VAR}=0 to drive the hardware deliberately: cargo sets it \
             to 1 for everything it starts in this repository, so unsetting it is not enough."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_meaningful_value_forbids_hardware() {
        assert!(forbidden(Some("1")));
        assert!(forbidden(Some("yes")));
        assert!(!forbidden(None));
        assert!(!forbidden(Some("")));
        assert!(!forbidden(Some("0")));
        assert!(!forbidden(Some("false")));
        assert!(!forbidden(Some("  ")), "whitespace is as good as unset");
    }

    #[test]
    fn the_refusal_is_the_usb_backend_under_the_variable_and_nothing_else() {
        let message = refusal("usb", Some("1")).expect("usb under the variable is refused");
        assert!(message.contains(VAR), "{message}");
        assert!(message.contains("--backend loopback"), "it says what to do instead: {message}");
        // The loopback opens nothing, so the variable has no quarrel with it.
        assert_eq!(refusal("loopback", Some("1")), None);
        // Without the variable, usb is an ordinary deliberate run.
        assert_eq!(refusal("usb", None), None);
        assert_eq!(refusal("usb", Some("0")), None);
    }
}
