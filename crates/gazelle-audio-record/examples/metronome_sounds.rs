//! One bar of each of the metronome's sounds, played by its own generator, written as WAV files to
//! listen to: `cargo run -p gazelle-audio-record --example metronome_sounds -- <folder>`.
//!
//! 4/4 at 120 BPM with the accent on the downbeat and eighth-note subdivisions, at the default
//! -18 dBFS and 48 kHz, one second of silence after the bar so the last click rings out.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use gazelle_record::metronome::{Generator, Params, Subdivision};
use gazelle_record::sounds::Sound;
use gazelle_record::wav::{Bext, SampleFormat, WavWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = std::env::args().nth(1).map(PathBuf::from).ok_or("say which folder to write the files into")?;
    std::fs::create_dir_all(&folder)?;
    const RATE: u32 = 48_000;
    const BLOCK: usize = 480;
    for sound in Sound::ALL {
        let params = Params { sound, subdivision: Subdivision::Eighths, ..Params::default() };
        let generator = Generator::new(f64::from(RATE), BLOCK, params);
        generator.count_in(1, gazelle_record::metronome::Then::Stop, false);
        let mut samples = Vec::new();
        let mut block = vec![0.0f32; BLOCK];
        // Two seconds of 4/4 at 120 is one bar, then a second for the last click to ring out.
        for _ in 0..(3 * RATE as usize) / BLOCK {
            // Safety: this is the generator's one caller.
            unsafe { generator.fill(&mut block) };
            samples.extend(block.iter().map(|&s| (f64::from(s) * f64::from(i32::MAX)) as i32));
        }
        let path = folder.join(format!("metronome-{}.wav", sound.words().to_lowercase()));
        let bext = Bext {
            description: format!("Gazelle metronome: {}, one bar of 4/4 at 120 BPM", sound.words()),
            originator: "Gazelle".into(),
            originator_reference: String::new(),
            origination_date: String::new(),
            origination_time: String::new(),
            time_reference: 0,
            coding_history: String::new(),
        };
        let mut writer = WavWriter::create(BufWriter::new(File::create(&path)?), SampleFormat::Int24, RATE, &bext)?;
        writer.write(&samples)?;
        writer.finish()?;
        println!("{}", path.display());
    }
    Ok(())
}
