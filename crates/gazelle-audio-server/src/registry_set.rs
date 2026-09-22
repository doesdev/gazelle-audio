//! Per-model command registries.
//!
//! The command surface differs by device model, so the server holds one registry per model
//! and selects by application pid. The Quadro's schema has 199 in-scope commands (the shared 35 +
//! 28 Quadro-only + a parameter set and get for each of 68 effect types) and the Studio+'s 117
//! (the shared 35 + 8 Studio+-only + 37 effect types' pairs); see `docs/protocol.md`, "Command
//! surface". The server serves all of them except [`OUT_OF_SCOPE`]: 195 and 116.

use gazelle_audio_protocol::registry::{from_json_doc, Registry};
use std::collections::HashMap;
use std::sync::Arc;

/// Zen Quadro Synergy Core application pid.
pub const PID_QUADRO: u16 = 0xa2f9;
/// Zen Studio+ application pid.
pub const PID_STUDIO: u16 = 0xa100;

/// Commands in the schemas that the server never serves, lists or sends, on any model.
///
/// Licence management is out of scope: these change what a device is licensed for, or take part
/// in assigning it to an account, which is the vendor's business and not a mixer's. Leaving them
/// out of the registry means the HTTP API, the WebSocket and the command listings all refuse or
/// omit them alike (`unknown_command`), dry run included. `get_feature_mask` stays: the app reads
/// it to know which microphone emulations the device may use. The web client's generated types
/// leave out the same names (`web/tools/gen-types`), and its integration test checks the two
/// agree.
pub const OUT_OF_SCOPE: [&str; 4] = [
    "set_config_feature",
    "get_cmd_set_assignment",
    "get_assignment_request",
    "get_assignment_status",
];

/// What Gazelle knows about one model beyond its command schema. A model Gazelle learns to drive
/// adds one line to [`MODELS`], and everything that names a model reads it from there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFacts {
    pub pid: u16,
    /// Stable short key clients switch on: `quadro` or `studio`.
    pub family: &'static str,
    /// Device slug, e.g. `zenquadrosc_usb2`.
    pub slug: &'static str,
    /// Human-readable model name, as the sidebar shows a device nobody has named.
    pub model: &'static str,
    /// The model's short form, which is what the aggregate's channels carry in a DAW for a device
    /// nobody has named: "Quadro 3" fits the interface's 31 characters where the full name does not.
    pub short: &'static str,
}

/// Every model Gazelle knows.
pub const MODELS: &[ModelFacts] = &[
    ModelFacts { pid: PID_QUADRO, family: "quadro", slug: "zenquadrosc_usb2", model: "Zen Quadro Synergy Core", short: "Quadro" },
    ModelFacts { pid: PID_STUDIO, family: "studio", slug: "zenstudiotb", model: "Zen Studio+", short: "Studio+" },
];

/// The facts of a model, by its family key.
pub fn model_facts(family: &str) -> Option<&'static ModelFacts> {
    MODELS.iter().find(|facts| facts.family == family)
}

/// A named registry plus the model it belongs to.
#[derive(Clone)]
pub struct ModelRegistry {
    /// Stable short key clients switch on: `quadro` or `studio`.
    pub family: &'static str,
    /// Device slug, e.g. `zenquadrosc_usb2`.
    pub slug: &'static str,
    /// Human-readable model name.
    pub model: &'static str,
    /// The model's short form ([`ModelFacts::short`]).
    pub short: &'static str,
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
        for facts in MODELS {
            let json = match facts.family {
                "quadro" => QUADRO_JSON,
                "studio" => STUDIO_JSON,
                other => return Err(format!("{other}: no schema is built in for this model")),
            };
            set.insert(facts, json)?;
        }
        Ok(set)
    }

    fn insert(&mut self, facts: &ModelFacts, json: &str) -> Result<(), String> {
        let ModelFacts { pid, family, slug, model, short } = *facts;
        let mut doc: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("{slug}: schema is not valid JSON: {e}"))?;
        if let Some(commands) = doc.get_mut("commands").and_then(|c| c.as_object_mut()) {
            for name in OUT_OF_SCOPE {
                commands.remove(name);
            }
        }
        let registry = from_json_doc(&doc)
            .map_err(|e| format!("{slug}: schema could not be loaded: {e:?}"))?;
        self.by_pid.insert(
            pid,
            ModelRegistry { family, slug, model, short, registry: Arc::new(registry) },
        );
        Ok(())
    }

    /// The registry for a device's application pid, if its model is known.
    ///
    /// Returns `None` for an unrecognised model rather than guessing a surface. Sending a
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
        assert_eq!(q.registry.len(), 199 - 4, "the Quadro's 199 in-scope commands, less the four licence ones");
        assert_eq!(s.registry.len(), 117 - 1, "the Studio+'s 117 in-scope commands, less set_config_feature");
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

        assert_eq!(quadro.len(), 63 + 2 * 68 - 4, "shared 35 + Quadro-only 28 + 68 effect types' set and get, less licence management");
        assert_eq!(studio.len(), 43 + 2 * 37 - 1, "shared 35 + Studio+-only 8 + 37 effect types' set and get, less licence management");
        for name in STUDIO_ONLY {
            assert!(studio.get(name).is_some(), "Studio+ is missing {name}");
            assert!(quadro.get(name).is_none(), "Quadro unexpectedly has {name}");
        }
        for name in ["set_mixer", "set_trim_config", "set_dim"] {
            assert!(quadro.get(name).is_some(), "Quadro is missing {name}");
            assert!(studio.get(name).is_none(), "Studio+ unexpectedly has {name}");
        }
    }

    /// Every name in [`OUT_OF_SCOPE`] is in some schema, so the list is not stale, and none of them
    /// is in any registry the server builds.
    #[test]
    fn licence_management_is_left_out() {
        let schemas = [
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/quadro_commands.json")),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../refs/schemas/studio_commands.json")),
        ]
        .map(|json| serde_json::from_str::<serde_json::Value>(json).unwrap());
        for name in OUT_OF_SCOPE {
            assert!(schemas.iter().any(|doc| doc["commands"].get(name).is_some()), "{name} is in no schema");
        }
        let set = RegistrySet::builtin().unwrap();
        for (_, model) in set.models() {
            for name in OUT_OF_SCOPE {
                assert!(model.registry.get(name).is_none(), "{} serves {name}", model.slug);
            }
            assert_eq!(model.registry.get("get_feature_mask").is_some(), model.family == "quadro", "the feature mask is still read");
        }
    }
}
