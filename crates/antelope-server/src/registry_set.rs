//! Per-model command registries.
//!
//! The command surface differs by device model, so the server holds one registry per model
//! and selects by application pid. Quadro exposes 63 in-scope commands; Studio+ exposes the
//! shared 35 (see `.agent/reference/devices.md`).

use antelope_protocol::registry::{from_json_doc, Registry};
use std::collections::HashMap;
use std::sync::Arc;

/// Zen Quadro Synergy Core application pid.
pub const PID_QUADRO: u16 = 0xa2f9;
/// Zen Studio+ application pid.
pub const PID_STUDIO: u16 = 0xa100;

/// A named registry plus the model it belongs to.
#[derive(Clone)]
pub struct ModelRegistry {
    /// Device slug, e.g. `zenquadrosc_usb2`.
    pub slug: &'static str,
    /// Human-readable model name.
    pub model: &'static str,
    pub registry: Arc<Registry>,
}

/// All registries the server knows, keyed by application pid.
#[derive(Clone, Default)]
pub struct RegistrySet {
    by_pid: HashMap<u16, ModelRegistry>,
}

impl RegistrySet {
    /// Load the built-in registries from the recovered schemas.
    ///
    /// Both files are compile-time embedded so the binary is self-contained and cannot
    /// silently pick up a different schema at runtime.
    pub fn builtin() -> Result<RegistrySet, String> {
        const QUADRO_JSON: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../refs/schemas/in_scope_commands.json"
        ));
        const STUDIO_JSON: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../refs/schemas/studio_commands.json"
        ));

        let mut set = RegistrySet::default();
        set.insert(PID_QUADRO, "zenquadrosc_usb2", "Zen Quadro Synergy Core", QUADRO_JSON)?;
        set.insert(PID_STUDIO, "zenstudiotb", "Zen Studio+", STUDIO_JSON)?;
        Ok(set)
    }

    fn insert(
        &mut self,
        pid: u16,
        slug: &'static str,
        model: &'static str,
        json: &str,
    ) -> Result<(), String> {
        let doc: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("{slug}: schema is not valid JSON: {e}"))?;
        let registry = from_json_doc(&doc)
            .map_err(|e| format!("{slug}: schema could not be loaded: {e:?}"))?;
        self.by_pid.insert(
            pid,
            ModelRegistry { slug, model, registry: Arc::new(registry) },
        );
        Ok(())
    }

    /// The registry for a device's application pid, if its model is known.
    ///
    /// Returns `None` for an unrecognised model rather than guessing a surface — sending a
    /// Quadro command set to an unknown device is exactly the sort of blind driving the
    /// project's risk posture forbids.
    pub fn for_pid(&self, pid: u16) -> Option<&ModelRegistry> {
        self.by_pid.get(&pid)
    }

    pub fn models(&self) -> impl Iterator<Item = (&u16, &ModelRegistry)> {
        self.by_pid.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registries_load() {
        let set = RegistrySet::builtin().expect("builtin registries must load");
        let q = set.for_pid(PID_QUADRO).expect("quadro registry");
        let s = set.for_pid(PID_STUDIO).expect("studio registry");
        assert_eq!(q.registry.len(), 63, "Quadro in-scope surface is 63 commands");
        assert_eq!(s.registry.len(), 35, "Studio+ shares 35 in-scope commands");
        assert_eq!(q.slug, "zenquadrosc_usb2");
        assert_eq!(s.slug, "zenstudiotb");
    }

    #[test]
    fn unknown_pid_has_no_registry() {
        let set = RegistrySet::builtin().unwrap();
        assert!(set.for_pid(0x0000).is_none());
    }

    #[test]
    fn quadro_only_commands_absent_from_studio() {
        let set = RegistrySet::builtin().unwrap();
        let q = &set.for_pid(PID_QUADRO).unwrap().registry;
        let s = &set.for_pid(PID_STUDIO).unwrap().registry;
        // set_mixer is Quadro-only; set_routing is shared.
        assert!(q.get("set_mixer").is_some());
        assert!(s.get("set_mixer").is_none(), "set_mixer is Quadro-only");
        assert!(q.get("set_routing").is_some());
        assert!(s.get("set_routing").is_some(), "set_routing is shared");
    }
}
