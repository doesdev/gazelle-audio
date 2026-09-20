//! What `regsvr32` writes, and what it takes away again.
//!
//! Registering is a separate, optional step that needs administrator rights, and it is the only
//! thing in this crate that writes outside the user's own profile. It touches nothing Gazelle
//! installs: Gazelle's own install is per user and does not go near `HKLM`.
//!
//! Every write goes through [`RegistryWriter`], so the whole of it is tested against a registry
//! made of a list.

use crate::{CLASS_ID, DRIVER_NAME, REGISTRY_KEY};

/// Where a driver of this kind publishes itself.
pub const ASIO_ROOT: &str = r"SOFTWARE\ASIO";
/// Where COM keeps its classes.
pub const CLSID_ROOT: &str = r"SOFTWARE\Classes\CLSID";

/// A place to write keys and values. The real one writes under `HKEY_LOCAL_MACHINE`.
pub trait RegistryWriter {
    fn create_key(&mut self, path: &str) -> Result<(), String>;
    fn set_value(&mut self, path: &str, name: &str, value: &str) -> Result<(), String>;
    /// Remove a key and everything under it. A key that was not there is not an error.
    fn delete_tree(&mut self, path: &str) -> Result<(), String>;
}

/// The key this driver's own entry lives under.
pub fn asio_key() -> String {
    format!(r"{ASIO_ROOT}\{REGISTRY_KEY}")
}

/// The key COM finds the DLL through.
pub fn class_key() -> String {
    format!(r"{CLSID_ROOT}\{}", CLASS_ID.to_registry_string())
}

pub fn inproc_key() -> String {
    format!(r"{}\InprocServer32", class_key())
}

/// Write everything a DAW needs to find this driver. `dll` is the full path of the DLL being
/// registered, which is the one thing that is not known until it is on a PC.
pub fn register(writer: &mut dyn RegistryWriter, dll: &str) -> Result<(), String> {
    if dll.trim().is_empty() {
        return Err("the DLL being registered has no path".to_string());
    }

    // The driver's own entry, which is the name a DAW lists.
    writer.create_key(&asio_key())?;
    writer.set_value(&asio_key(), "CLSID", &CLASS_ID.to_registry_string())?;
    writer.set_value(&asio_key(), "Description", DRIVER_NAME)?;

    // The class itself, and the DLL behind it.
    writer.create_key(&class_key())?;
    writer.set_value(&class_key(), "", DRIVER_NAME)?;
    writer.create_key(&inproc_key())?;
    writer.set_value(&inproc_key(), "", dll)?;
    // Both vendor drivers register Apartment, and this driver calls them from the thread it was
    // made on, so it must be the same.
    writer.set_value(&inproc_key(), "ThreadingModel", "Apartment")?;
    Ok(())
}

/// Take it all away again, leaving nothing behind.
pub fn unregister(writer: &mut dyn RegistryWriter) -> Result<(), String> {
    writer.delete_tree(&asio_key())?;
    writer.delete_tree(&class_key())?;
    Ok(())
}

/// A registry made of a list, for tests and for saying what registering would do.
#[derive(Default)]
pub struct Recorded {
    /// Every key made, in order.
    pub keys: Vec<String>,
    /// Every value set, as key, name, value.
    pub values: Vec<(String, String, String)>,
    pub deleted: Vec<String>,
    /// A path whose write fails, as a locked down PC would.
    pub refuse: Option<String>,
}

impl Recorded {
    pub fn value(&self, path: &str, name: &str) -> Option<&str> {
        self.values.iter().find(|(k, n, _)| k == path && n == name).map(|(_, _, v)| v.as_str())
    }
}

impl RegistryWriter for Recorded {
    fn create_key(&mut self, path: &str) -> Result<(), String> {
        if self.refuse.as_deref() == Some(path) {
            return Err(format!("{path} could not be created: access is denied"));
        }
        self.keys.push(path.to_string());
        Ok(())
    }

    fn set_value(&mut self, path: &str, name: &str, value: &str) -> Result<(), String> {
        if !self.keys.iter().any(|key| key == path) {
            return Err(format!("{path} was written to before it was made"));
        }
        self.values.push((path.to_string(), name.to_string(), value.to_string()));
        Ok(())
    }

    fn delete_tree(&mut self, path: &str) -> Result<(), String> {
        self.deleted.push(path.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registering_writes_the_two_keys_a_daw_looks_in() {
        let mut registry = Recorded::default();
        register(&mut registry, r"C:\Tools\gazelle_aggregate.dll").expect("an ordinary registration");

        assert_eq!(registry.value(r"SOFTWARE\ASIO\Gazelle Aggregate", "CLSID"), Some("{F18C80B4-2DE1-43B8-AC84-19DDC22EBFC8}"));
        assert_eq!(registry.value(r"SOFTWARE\ASIO\Gazelle Aggregate", "Description"), Some("Gazelle Aggregate"));

        let class = r"SOFTWARE\Classes\CLSID\{F18C80B4-2DE1-43B8-AC84-19DDC22EBFC8}";
        let inproc = format!(r"{class}\InprocServer32");
        assert_eq!(registry.value(class, ""), Some("Gazelle Aggregate"));
        assert_eq!(registry.value(&inproc, ""), Some(r"C:\Tools\gazelle_aggregate.dll"));
        assert_eq!(registry.value(&inproc, "ThreadingModel"), Some("Apartment"));
    }

    #[test]
    fn every_key_is_made_before_it_is_written_to() {
        let mut registry = Recorded::default();
        register(&mut registry, r"C:\x.dll").expect("an ordinary registration");
        assert_eq!(registry.keys.len(), 3, "the entry, the class, and the DLL under it");
        assert!(registry.deleted.is_empty(), "registering deletes nothing");
    }

    #[test]
    fn unregistering_takes_away_exactly_what_was_written() {
        let mut registry = Recorded::default();
        register(&mut registry, r"C:\x.dll").expect("an ordinary registration");
        let written: Vec<String> = registry.keys.clone();
        let mut registry = Recorded::default();
        unregister(&mut registry).expect("an ordinary unregistration");
        assert_eq!(registry.deleted, vec![asio_key(), class_key()]);
        // Everything written sits under one of the two trees that are removed.
        for key in written {
            assert!(
                key.starts_with(&asio_key()) || key.starts_with(&class_key()),
                "{key} would be left behind"
            );
        }
    }

    #[test]
    fn a_registry_that_says_no_is_passed_on_rather_than_half_written() {
        let mut registry = Recorded { refuse: Some(asio_key()), ..Recorded::default() };
        let error = register(&mut registry, r"C:\x.dll").expect_err("access denied");
        assert!(error.contains("access is denied"), "{error}");
        assert!(registry.values.is_empty(), "nothing was written");
    }

    #[test]
    fn registering_needs_to_know_where_the_dll_is() {
        let mut registry = Recorded::default();
        assert!(register(&mut registry, "   ").is_err());
        assert!(registry.keys.is_empty());
    }

    #[test]
    fn nothing_gazelle_installs_is_touched() {
        let mut registry = Recorded::default();
        register(&mut registry, r"C:\x.dll").expect("an ordinary registration");
        for key in registry.keys.iter().chain(registry.values.iter().map(|(k, _, _)| k)) {
            let lower = key.to_ascii_lowercase();
            assert!(
                lower.starts_with("software\\asio") || lower.starts_with("software\\classes\\clsid"),
                "{key} is outside the two places a driver registers"
            );
        }
    }
}
