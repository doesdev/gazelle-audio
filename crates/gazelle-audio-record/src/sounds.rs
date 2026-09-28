//! **The metronome's sounds**, made by Gazelle itself: no sample files.
//!
//! Five sounds, each in two variants: the plain one for a beat and a subdivision, and a brighter one
//! for the accent on the downbeat. Every one of them is rendered once, off the audio thread, into a
//! buffer of its own at the rate the session runs at ([`Bank::render`]); the audio thread only ever
//! reads them.
//!
//! Every rendered sound:
//!
//! - **starts and ends at silence**: a short raised-cosine attack at the front and a raised-cosine
//!   fade at the back, so its first and last samples are exactly zero and neither end is a step;
//! - **carries no DC**: whatever average the shape has is taken out along the envelope itself, so the
//!   ends stay at zero while the sum of the samples becomes zero;
//! - **peaks at exactly full scale**, so the level the metronome plays it at is its peak, and the
//!   ceiling is a statement about samples rather than a guess about a shape.

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

/// The sounds on offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sound {
    /// A short, bright click.
    #[default]
    Click,
    /// A sine beep.
    Beep,
    /// A woodblock: two or three wooden resonances that die away fast.
    Woodblock,
    /// A cowbell, the drum machine kind: two clanging tones.
    Cowbell,
    /// A short hi-hat-like tick of filtered noise.
    Tick,
}

impl Sound {
    /// Every sound, in the order they are offered.
    pub const ALL: [Sound; 5] = [Sound::Click, Sound::Beep, Sound::Woodblock, Sound::Cowbell, Sound::Tick];

    /// Its place in [`Sound::ALL`].
    pub fn index(self) -> usize {
        match self {
            Sound::Click => 0,
            Sound::Beep => 1,
            Sound::Woodblock => 2,
            Sound::Cowbell => 3,
            Sound::Tick => 4,
        }
    }

    /// The sound at `index` of [`Sound::ALL`]; anything else is the click.
    pub fn from_index(index: usize) -> Sound {
        Sound::ALL.get(index).copied().unwrap_or_default()
    }

    /// How a person names it.
    pub fn words(self) -> &'static str {
        match self {
            Sound::Click => "Click",
            Sound::Beep => "Beep",
            Sound::Woodblock => "Woodblock",
            Sound::Cowbell => "Cowbell",
            Sound::Tick => "Tick",
        }
    }
}

/// A sinusoidal partial: its frequency, its level against the others, and how fast it dies away
/// (the time it takes to fall to about a third).
type Partial = (f64, f64, f64);

/// How one variant of a sound is made.
struct Recipe {
    seconds: f64,
    attack: f64,
    release: f64,
    partials: &'static [Partial],
    /// Filtered noise: its level, how fast it dies away, and how many times it is differenced,
    /// which is a gentle high pass: more is brighter.
    noise: Option<(f64, f64, u32)>,
}

fn recipe(sound: Sound, accent: bool) -> Recipe {
    match (sound, accent) {
        (Sound::Click, false) => Recipe { seconds: 0.012, attack: 0.000_25, release: 0.003, partials: &[(2_400.0, 1.0, 0.002_5), (3_700.0, 0.6, 0.001_5)], noise: None },
        (Sound::Click, true) => Recipe { seconds: 0.014, attack: 0.000_25, release: 0.003, partials: &[(3_300.0, 1.0, 0.003), (5_100.0, 0.6, 0.001_8)], noise: None },
        (Sound::Beep, false) => Recipe { seconds: 0.05, attack: 0.002, release: 0.015, partials: &[(880.0, 1.0, 0.08)], noise: None },
        (Sound::Beep, true) => Recipe { seconds: 0.05, attack: 0.002, release: 0.015, partials: &[(1_760.0, 1.0, 0.08), (3_520.0, 0.15, 0.03)], noise: None },
        (Sound::Woodblock, false) => Recipe { seconds: 0.045, attack: 0.000_4, release: 0.008, partials: &[(880.0, 1.0, 0.012), (2_130.0, 0.45, 0.006), (3_600.0, 0.15, 0.003)], noise: None },
        (Sound::Woodblock, true) => Recipe { seconds: 0.045, attack: 0.000_4, release: 0.008, partials: &[(1_175.0, 1.0, 0.012), (2_840.0, 0.5, 0.006), (4_700.0, 0.2, 0.003)], noise: None },
        (Sound::Cowbell, false) => {
            Recipe { seconds: 0.14, attack: 0.000_5, release: 0.025, partials: &[(540.0, 0.8, 0.05), (800.0, 1.0, 0.05), (1_620.0, 0.3, 0.03), (2_400.0, 0.35, 0.025)], noise: None }
        }
        (Sound::Cowbell, true) => {
            Recipe { seconds: 0.14, attack: 0.000_5, release: 0.025, partials: &[(587.0, 0.8, 0.05), (870.0, 1.0, 0.05), (1_761.0, 0.45, 0.03), (2_610.0, 0.5, 0.025)], noise: None }
        }
        (Sound::Tick, false) => Recipe { seconds: 0.035, attack: 0.000_2, release: 0.006, partials: &[], noise: Some((1.0, 0.006, 2)) },
        (Sound::Tick, true) => Recipe { seconds: 0.045, attack: 0.000_2, release: 0.008, partials: &[], noise: Some((1.0, 0.009, 3)) },
    }
}

/// One variant of a sound at `rate`, peaking at exactly 1.0, with no DC and silence at both ends.
///
/// Off the audio thread only: it allocates.
pub fn render(sound: Sound, accent: bool, rate: f64) -> Vec<f32> {
    let recipe = recipe(sound, accent);
    let rate = if rate.is_finite() && rate > 0.0 { rate } else { 48_000.0 };
    let length = ((recipe.seconds * rate).round() as usize).max(8);
    let attack = ((recipe.attack * rate).round() as usize).clamp(2, length / 4);
    let release = ((recipe.release * rate).round() as usize).clamp(2, length / 2);

    let mut shape = vec![0.0f64; length];
    for (index, value) in shape.iter_mut().enumerate() {
        let t = index as f64 / rate;
        *value = recipe.partials.iter().map(|&(hz, level, decay)| level * (-t / decay).exp() * (2.0 * PI * hz * t).sin()).sum();
    }
    if let Some((level, decay, passes)) = recipe.noise {
        // The same noise every time, so every render of a sound is the same sound.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut noise: Vec<f64> = (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
            })
            .collect();
        for _ in 0..passes {
            for index in (1..noise.len()).rev() {
                noise[index] -= noise[index - 1];
            }
        }
        for (index, value) in shape.iter_mut().enumerate() {
            *value += level * (-(index as f64 / rate) / decay).exp() * noise[index];
        }
    }

    // The envelope: up from silence over the attack, down to silence over the release.
    let envelope: Vec<f64> = (0..length)
        .map(|index| {
            let rise = if index < attack { 0.5 - 0.5 * (PI * index as f64 / attack as f64).cos() } else { 1.0 };
            let into_release = index as i64 - (length - release) as i64;
            let fall = if into_release >= 0 { 0.5 + 0.5 * (PI * into_release as f64 / (release - 1) as f64).cos() } else { 1.0 };
            rise * fall
        })
        .collect();
    let mut samples: Vec<f64> = shape.iter().zip(&envelope).map(|(s, e)| s * e).collect();

    // No DC: take the average out along the envelope, which is zero at both ends, so the ends stay
    // exactly where they are and the samples sum to nothing.
    let total: f64 = samples.iter().sum();
    let weight: f64 = envelope.iter().sum();
    if weight > 0.0 {
        let per = total / weight;
        for (sample, e) in samples.iter_mut().zip(&envelope) {
            *sample -= per * e;
        }
    }

    let peak = samples.iter().fold(0.0f64, |loudest, s| loudest.max(s.abs()));
    let scale = if peak > 0.0 { 1.0 / peak } else { 0.0 };
    let mut out: Vec<f32> = samples.into_iter().map(|s| (s * scale) as f32).collect();
    // Exactly silence at the ends, whatever the arithmetic left there.
    if let Some(first) = out.first_mut() {
        *first = 0.0;
    }
    if let Some(last) = out.last_mut() {
        *last = 0.0;
    }
    out
}

/// Every sound in both variants at one rate, rendered once at session start.
pub struct Bank {
    /// `Sound::index() * 2`, plus one for the accent.
    voices: Vec<Box<[f32]>>,
}

impl Bank {
    /// Off the audio thread: every sound, both variants, at `rate`.
    pub fn render(rate: f64) -> Bank {
        let voices = Sound::ALL.iter().flat_map(|&sound| [false, true].map(|accent| render(sound, accent, rate).into_boxed_slice())).collect();
        Bank { voices }
    }

    /// One variant, to read from. Never allocates.
    pub fn get(&self, sound: Sound, accent: bool) -> &[f32] {
        &self.voices[sound.index() * 2 + usize::from(accent)]
    }

    /// The longest of them, in samples.
    pub fn longest(&self) -> usize {
        self.voices.iter().map(|voice| voice.len()).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [f64; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];

    #[test]
    fn every_sound_peaks_at_exactly_full_scale_and_carries_no_dc() {
        for rate in RATES {
            for sound in Sound::ALL {
                for accent in [false, true] {
                    let samples = render(sound, accent, rate);
                    let peak = samples.iter().fold(0.0f32, |loudest, s| loudest.max(s.abs()));
                    assert!((peak - 1.0).abs() < 1e-6, "{sound:?} accent {accent} at {rate}: peak {peak}");
                    let mean = samples.iter().map(|&s| f64::from(s)).sum::<f64>() / samples.len() as f64;
                    assert!(mean.abs() < 1e-6, "{sound:?} accent {accent} at {rate}: DC of {mean}");
                }
            }
        }
    }

    #[test]
    fn every_sound_starts_and_ends_at_silence_without_a_step() {
        for rate in RATES {
            for sound in Sound::ALL {
                for accent in [false, true] {
                    let samples = render(sound, accent, rate);
                    assert_eq!(samples[0], 0.0, "{sound:?} starts at silence");
                    assert_eq!(*samples.last().unwrap(), 0.0, "{sound:?} ends at silence");
                    // No jump at either end: the first and last few samples rise from and fall to zero.
                    assert!(samples[1].abs() < 0.2, "{sound:?} accent {accent} at {rate}: first step {}", samples[1]);
                    assert!(samples[samples.len() - 2].abs() < 0.05, "{sound:?} accent {accent} at {rate}: last step {}", samples[samples.len() - 2]);
                }
            }
        }
    }

    #[test]
    fn the_accent_is_its_own_brighter_variant_and_every_sound_is_short() {
        for sound in Sound::ALL {
            let (plain, accent) = (render(sound, false, 48_000.0), render(sound, true, 48_000.0));
            assert_ne!(plain, accent, "{sound:?} has an accent of its own");
            // Brighter: more of its energy is in the sample-to-sample changes.
            let brightness = |s: &[f32]| s.windows(2).map(|w| f64::from(w[1] - w[0]).powi(2)).sum::<f64>() / s.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
            assert!(brightness(&accent) > brightness(&plain), "{sound:?}: the accent is brighter");
            assert!(plain.len() <= 48_000 * 15 / 100, "{sound:?} is short");
        }
        assert_eq!(render(Sound::Click, false, 48_000.0), render(Sound::Click, false, 48_000.0), "the same every time");
    }

    #[test]
    fn a_bank_holds_both_variants_of_every_sound_at_its_rate() {
        let bank = Bank::render(96_000.0);
        for sound in Sound::ALL {
            assert_eq!(bank.get(sound, true), render(sound, true, 96_000.0).as_slice());
            assert_eq!(bank.get(sound, false), render(sound, false, 96_000.0).as_slice());
        }
        assert_eq!(bank.longest(), (0.14f64 * 96_000.0).round() as usize, "the cowbell is the longest");
        assert_eq!(Sound::from_index(3), Sound::Cowbell);
        assert_eq!(Sound::from_index(99), Sound::Click);
    }
}
