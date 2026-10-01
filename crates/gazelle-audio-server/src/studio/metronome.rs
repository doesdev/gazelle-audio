//! The metronome's settings: `metronome.json` in the config directory, beside `recording.json`
//! (`%APPDATA%\gazelle\metronome.json` on Windows).
//!
//! **One metronome setting for this PC, not one per preset, and not in the workspace.** Its outputs
//! are this PC's wiring (which outputs reach the headphones here), exactly as auto-arm is this PC's
//! habit: a workspace carried to another machine, or restored from a backup, must not start playing
//! a click into whatever that machine has on those outputs. Tempo, signature and sound go with them in
//! the one file because they are one thing the person sets up once and nudges per song; a tempo per
//! preset would be wrong more often than right, since a preset is about channels, not songs.
//!
//! A missing file is the defaults: 120 BPM in 4/4, the click, -18 dBFS, no outputs chosen (so the
//! metronome cannot play until someone chooses them), no count-in, not following Record, no latency
//! offset. An
//! unreadable one is the same and a warning. `--no-persist` keeps it in memory.

use std::sync::Mutex;

use gazelle_calibrate::Pick;
use gazelle_record::latency::OFFSET_MS_MAX;
use gazelle_record::metronome::{Params, Subdivision, COUNT_IN_MAX, DEFAULT_VOLUME_DBFS};
use gazelle_record::sounds::Sound;
use serde::{Deserialize, Serialize};

use super::settings::Backing;

/// The file's contents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetronomeSettings {
    /// Quarter notes a minute, 20 to 400, in steps of 0.1.
    pub tempo: f64,
    pub numerator: u32,
    /// 2, 4, 8 or 16: the note a beat is.
    pub denominator: u32,
    /// The downbeat played brighter and louder than the other beats.
    pub accent: bool,
    pub subdivision: Subdivision,
    pub sound: Sound,
    /// The peak of the loudest click, in dBFS. Never louder than -6 dBFS, whatever it says.
    pub volume_db: f64,
    /// Where it plays, by interface and that interface's own output, from zero.
    pub outputs: Vec<Pick>,
    /// Bars counted before a take starts: 0 to 4.
    pub count_in_bars: u32,
    /// The click runs whenever a take does, and stops with it.
    pub follow_record: bool,
    /// Milliseconds, either way, added to the latencies the aggregate reports when a take's Downbeat
    /// is placed after a count-in: -100 to 100, 0 by default. This PC's, as its wiring is.
    pub latency_offset_ms: f64,
}

impl Default for MetronomeSettings {
    fn default() -> MetronomeSettings {
        let params = Params::default();
        MetronomeSettings {
            tempo: params.tempo(),
            numerator: params.numerator,
            denominator: params.denominator,
            accent: params.accent,
            subdivision: params.subdivision,
            sound: params.sound,
            volume_db: DEFAULT_VOLUME_DBFS,
            outputs: Vec::new(),
            count_in_bars: 0,
            follow_record: false,
            latency_offset_ms: 0.0,
        }
    }
}

impl MetronomeSettings {
    /// What the engine plays.
    pub fn params(&self) -> Params {
        Params {
            tempo_tenths: Params::tenths(self.tempo),
            numerator: self.numerator,
            denominator: self.denominator,
            accent: self.accent,
            subdivision: self.subdivision,
            sound: self.sound,
            volume_dbfs: self.volume_db,
        }
    }

    /// Everything wrong with these, in a sentence.
    pub fn problem(&self) -> Option<String> {
        if let Some(why) = self.params().problem() {
            return Some(why);
        }
        if !self.volume_db.is_finite() {
            return Some("the volume has to be a number of dB".into());
        }
        if self.count_in_bars > COUNT_IN_MAX {
            return Some(format!("a count-in is 0 to {COUNT_IN_MAX} bars, not {}", self.count_in_bars));
        }
        if !(self.latency_offset_ms.is_finite() && (-OFFSET_MS_MAX..=OFFSET_MS_MAX).contains(&self.latency_offset_ms)) {
            return Some(format!("the latency offset is between -{OFFSET_MS_MAX} and {OFFSET_MS_MAX} ms, not {}", self.latency_offset_ms));
        }
        if self.outputs.iter().any(|pick| pick.device < 0 || pick.channel < 0) {
            return Some("an output is an interface and one of its outputs, counted from zero".into());
        }
        None
    }
}

/// Where the file lives, beside the workspace in the config directory.
pub fn default_metronome_path(var: impl Fn(&str) -> Option<String>) -> std::path::PathBuf {
    crate::config::config_dir(var).map_or_else(|| std::path::PathBuf::from("metronome.json"), |dir| dir.join("metronome.json"))
}

/// The settings, and where they are kept.
pub struct MetronomeStore {
    backing: Mutex<Backing>,
    settings: Mutex<MetronomeSettings>,
}

impl MetronomeStore {
    pub fn in_memory() -> MetronomeStore {
        MetronomeStore { backing: Mutex::new(Backing::Memory), settings: Mutex::new(MetronomeSettings::default()) }
    }

    /// Read them from `backing`, and keep them there from now on. A warning to log when the file
    /// could not be used; the settings are then the defaults.
    pub fn load(&self, backing: Backing) -> Option<String> {
        let (mut settings, mut warning): (MetronomeSettings, Option<String>) = backing.load_as("metronome settings", "the metronome is at its defaults, with no outputs");
        if let Some(why) = settings.problem() {
            warning = Some(format!("the metronome settings were not used ({why}); the metronome is at its defaults, with no outputs"));
            settings = MetronomeSettings::default();
        }
        *self.settings.lock().unwrap_or_else(|p| p.into_inner()) = settings;
        *self.backing.lock().unwrap_or_else(|p| p.into_inner()) = backing;
        warning
    }

    pub fn get(&self) -> MetronomeSettings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Keep `settings` and write them back. Nothing changes when the file cannot be written.
    pub fn put(&self, settings: MetronomeSettings) -> Result<(), String> {
        let backing = self.backing.lock().map_err(|_| "the metronome settings are unavailable".to_string())?;
        backing.save_as(&settings).map_err(|e| format!("saving the metronome settings: {e}"))?;
        *self.settings.lock().map_err(|_| "the metronome settings are unavailable".to_string())? = settings;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_defaults_play_nowhere_at_minus_18_in_four_four_at_120() {
        let settings = MetronomeSettings::default();
        assert_eq!((settings.tempo, settings.numerator, settings.denominator), (120.0, 4, 4));
        assert_eq!(settings.volume_db, -18.0);
        assert!(settings.outputs.is_empty(), "no outputs until somebody chooses them");
        assert_eq!(settings.count_in_bars, 0);
        assert!(!settings.follow_record);
        assert_eq!(settings.latency_offset_ms, 0.0);
        assert_eq!(settings.problem(), None);
    }

    #[test]
    fn nonsense_is_refused_in_words() {
        let with = |change: fn(&mut MetronomeSettings)| {
            let mut settings = MetronomeSettings::default();
            change(&mut settings);
            settings.problem()
        };
        assert!(with(|s| s.tempo = 19.9).unwrap().contains("between 20 and 400"));
        assert!(with(|s| s.tempo = 400.04).is_none(), "400.04 is 400.0 in tenths");
        assert!(with(|s| s.denominator = 3).is_some());
        assert!(with(|s| s.numerator = 0).is_some());
        assert!(with(|s| s.count_in_bars = 5).unwrap().contains("0 to 4"));
        assert!(with(|s| s.volume_db = f64::INFINITY).is_some());
        assert!(with(|s| s.outputs = vec![Pick::new(0, -1)]).is_some());
        assert!(with(|s| s.latency_offset_ms = 100.5).unwrap().contains("between -100 and 100 ms"));
        assert!(with(|s| s.latency_offset_ms = f64::NAN).is_some());
        assert!(with(|s| s.latency_offset_ms = -100.0).is_none());
    }

    #[test]
    fn they_persist_through_the_file_and_a_damaged_one_plays_nowhere() {
        let dir = std::env::temp_dir().join(format!("gazelle-metronome-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("metronome.json");
        let store = MetronomeStore::in_memory();
        assert_eq!(store.load(Backing::File(path.clone())), None, "a missing file is the defaults, said nothing about");
        let chosen = MetronomeSettings { tempo: 97.5, sound: Sound::Cowbell, outputs: vec![Pick::new(0, 6), Pick::new(0, 7)], count_in_bars: 2, latency_offset_ms: -2.5, ..MetronomeSettings::default() };
        store.put(chosen.clone()).unwrap();
        let again = MetronomeStore::in_memory();
        assert_eq!(again.load(Backing::File(path.clone())), None);
        assert_eq!(again.get(), chosen);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"sound\": \"cowbell\"") && text.contains("\"subdivision\": \"none\""), "{text}");

        std::fs::write(&path, r#"{"tempo": 900}"#).unwrap();
        let damaged = MetronomeStore::in_memory();
        assert!(damaged.load(Backing::File(path.clone())).unwrap().contains("defaults, with no outputs"));
        assert!(damaged.get().outputs.is_empty());
        assert_eq!(default_metronome_path(|k| (k == "APPDATA").then(|| r"C:\A".to_string())), PathBuf::from(r"C:\A").join("gazelle").join("metronome.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
