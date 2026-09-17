//! Per-model command registries.
//!
//! The command surface differs by device model, so the server holds one registry per model
//! and selects by application pid. Quadro exposes 199 in-scope commands (the shared 35 +
//! 28 Quadro-only + a parameter set and get for each of 68 effect types); Studio+ exposes 115
//! (the shared 35 + 8 Studio+-only + 36 effect types' pairs) (see `.agent/reference/devices.md`).

use gazelle_audio_protocol::registry::{from_json_doc, Registry};
use std::collections::HashMap;
use std::sync::Arc;

/// Zen Quadro Synergy Core application pid.
pub const PID_QUADRO: u16 = 0xa2f9;
/// Zen Studio+ application pid.
pub const PID_STUDIO: u16 = 0xa100;

/// A named registry plus the model it belongs to.
#[derive(Clone)]
pub struct ModelRegistry {
    /// Stable short key clients switch on: `quadro` or `studio`.
    pub family: &'static str,
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
            "/../../refs/schemas/quadro_commands.json"
        ));
        const STUDIO_JSON: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../refs/schemas/studio_commands.json"
        ));

        let mut set = RegistrySet::default();
        set.insert(PID_QUADRO, "quadro", "zenquadrosc_usb2", "Zen Quadro Synergy Core", QUADRO_JSON)?;
        set.insert(PID_STUDIO, "studio", "zenstudiotb", "Zen Studio+", STUDIO_JSON)?;
        Ok(set)
    }

    fn insert(
        &mut self,
        pid: u16,
        family: &'static str,
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
            ModelRegistry { family, slug, model, registry: Arc::new(registry) },
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
        assert_eq!(q.registry.len(), 199, "Quadro in-scope surface is 199 commands");
        assert_eq!(s.registry.len(), 115, "Studio+ in-scope surface is 115 commands");
        assert_eq!(q.slug, "zenquadrosc_usb2");
        assert_eq!(s.slug, "zenstudiotb");
    }

    #[test]
    fn unknown_pid_has_no_registry() {
        let set = RegistrySet::builtin().unwrap();
        assert!(set.for_pid(0x0000).is_none());
    }

    /// Studio+'s own mix/monitoring commands, which a shared-only scope silently dropped.
    const STUDIO_ONLY: [&str; 8] = [
        "get_lines_links", "set_line_gain", "set_mixer_cfg", "set_pre_phaseinv",
        "set_talk", "set_tbk_enable", "set_tbk_vol", "set_trim",
    ];

    #[test]
    fn each_family_exposes_exactly_its_scope() {
        let set = RegistrySet::builtin().unwrap();
        let quadro = &set.for_pid(PID_QUADRO).unwrap().registry;
        let studio = &set.for_pid(PID_STUDIO).unwrap().registry;

        assert_eq!(quadro.len(), 63 + 2 * 68, "shared 35 + Quadro-only 28 + 68 effect types' set and get");
        assert_eq!(studio.len(), 43 + 2 * 36, "shared 35 + Studio+-only 8 + 36 effect types' set and get");
        for name in STUDIO_ONLY {
            assert!(studio.get(name).is_some(), "Studio+ is missing {name}");
            assert!(quadro.get(name).is_none(), "Quadro unexpectedly has {name}");
        }
        for name in ["set_mixer", "set_trim_config", "set_dim"] {
            assert!(quadro.get(name).is_some(), "Quadro is missing {name}");
            assert!(studio.get(name).is_none(), "Studio+ unexpectedly has {name}");
        }
    }
}
