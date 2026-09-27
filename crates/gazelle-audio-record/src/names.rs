//! What a take's files are called.
//!
//! A preset names its files with a pattern: `{channel}` is the channel's name in the aggregate, the
//! same name a DAW shows for it ("Vocal mic (Quadro 1)"), `{take}` the take's number, three digits,
//! `{date}` and `{time}` when its first sample was recorded, and `{preset}` the preset's name. Every
//! pattern has `{channel}`, so the files of a take differ, and `{take}`, so takes differ.
//!
//! **A file is never overwritten.** A take's number is the first, from where the last one left off,
//! whose files are all new names; the files are then made with "create new", which fails rather than
//! replace anything that appeared in between.

use std::path::{Path, PathBuf};

/// What a new preset names its files: "2026-09-27 T001 Vocal mic (Quadro 1).wav".
pub const DEFAULT_PATTERN: &str = "{date} T{take} {channel}";
/// The longest a file's name may be, before its extension.
const NAME_MAX: usize = 180;
/// How far a take number is looked for before giving up.
const TAKES_MAX: u32 = 9_999;

/// The words for a pattern that cannot name a take's files, or nothing when it can.
pub fn pattern_problem(pattern: &str) -> Option<String> {
    if !pattern.contains("{channel}") {
        return Some("the file name pattern needs {channel}, or every channel of a take would get the same name".into());
    }
    if !pattern.contains("{take}") {
        return Some("the file name pattern needs {take}, or one take's files would get the next take's names".into());
    }
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { return Some("the file name pattern has a { with no } after it".into()) };
        let token = &rest[open + 1..open + close];
        if !["channel", "take", "date", "time", "preset"].contains(&token) {
            return Some(format!("{{{token}}} is not something a file name can hold: use {{channel}}, {{take}}, {{date}}, {{time}} or {{preset}}"));
        }
        rest = &rest[open + close + 1..];
    }
    if pattern.contains(['/', '\\']) {
        return Some("the file name pattern names a file, not a folder: leave out / and \\".into());
    }
    None
}

/// What goes in a pattern's tokens for one take.
pub struct TakeWords<'a> {
    pub preset: &'a str,
    pub take: u32,
    /// `yyyy-mm-dd`.
    pub date: &'a str,
    /// `hh-mm-ss`: a file name cannot hold a colon.
    pub time: &'a str,
}

/// One file's name, without its extension: the pattern filled in, and made safe for Windows.
pub fn file_name(pattern: &str, words: &TakeWords, channel: &str) -> String {
    let filled = pattern
        .replace("{channel}", channel)
        .replace("{take}", &format!("{:03}", words.take))
        .replace("{date}", words.date)
        .replace("{time}", words.time)
        .replace("{preset}", words.preset);
    safe(&filled)
}

/// A name Windows will take: no reserved characters, no trailing dots or spaces, not a device name.
pub fn safe(name: &str) -> String {
    let mut out: String = name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '-' } else { c }).collect();
    out = out.chars().take(NAME_MAX).collect();
    let trimmed = out.trim_end_matches(['.', ' ']).trim_start().to_string();
    let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    match (trimmed.is_empty(), reserved) {
        (true, _) => "take".into(),
        (_, true) => format!("_{trimmed}"),
        _ => trimmed,
    }
}

/// Every path a take numbered `take` would write: one WAV per channel, and its log.
pub fn take_paths(folder: &Path, pattern: &str, words: &TakeWords, channels: &[String]) -> (Vec<PathBuf>, PathBuf) {
    let files = channels.iter().map(|channel| folder.join(format!("{}.wav", file_name(pattern, words, channel)))).collect();
    let log = folder.join(format!("{}.txt", file_name(pattern, words, "log")));
    (files, log)
}

/// The first take number from `from` whose files and log are all new, and those paths. Two channels
/// with one name (a person named two channels alike) are told apart by their number in the take.
pub fn next_take(
    folder: &Path,
    pattern: &str,
    words: &mut TakeWords,
    channels: &[String],
    from: u32,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<(u32, Vec<PathBuf>, PathBuf), String> {
    let mut named: Vec<String> = Vec::with_capacity(channels.len());
    for (at, channel) in channels.iter().enumerate() {
        let taken = named.iter().any(|earlier| earlier.eq_ignore_ascii_case(channel));
        named.push(if taken { format!("{channel} ({})", at + 1) } else { channel.clone() });
    }
    for take in from.max(1)..=TAKES_MAX {
        words.take = take;
        let (files, log) = take_paths(folder, pattern, words, &named);
        if !files.iter().chain(std::iter::once(&log)).any(|path| exists(path)) {
            return Ok((take, files, log));
        }
    }
    Err(format!("{} already holds takes up to {TAKES_MAX} with these names: choose another folder or pattern", folder.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> TakeWords<'static> {
        TakeWords { preset: "Band", take: 7, date: "2026-09-27", time: "14-03-21" }
    }

    #[test]
    fn a_pattern_fills_in_and_the_default_names_a_file_after_its_channel() {
        assert_eq!(file_name(DEFAULT_PATTERN, &words(), "Vocal mic (Quadro 1)"), "2026-09-27 T007 Vocal mic (Quadro 1)");
        assert_eq!(file_name("{preset} {time} {take} {channel}", &words(), "Kick"), "Band 14-03-21 007 Kick");
    }

    #[test]
    fn a_name_windows_would_refuse_is_made_one_it_takes() {
        assert_eq!(safe("S/PDIF 1: L?"), "S-PDIF 1- L-");
        assert_eq!(safe("Take. . "), "Take");
        assert_eq!(safe("con"), "_con");
        assert_eq!(safe("COM3.wav"), "_COM3.wav");
        assert_eq!(safe("Comet"), "Comet");
        assert_eq!(safe("   "), "take");
        assert_eq!(safe(&"x".repeat(400)).len(), NAME_MAX);
    }

    #[test]
    fn a_pattern_that_cannot_name_a_take_says_why() {
        assert_eq!(pattern_problem(DEFAULT_PATTERN), None);
        assert!(pattern_problem("{date} {take}").unwrap().contains("{channel}"));
        assert!(pattern_problem("{date} {channel}").unwrap().contains("{take}"));
        assert!(pattern_problem("{take} {channel} {mood}").unwrap().contains("{mood}"));
        assert!(pattern_problem("{take} {channel} {date").unwrap().contains("no }"));
        assert!(pattern_problem("takes/{take} {channel}").unwrap().contains("folder"));
    }

    #[test]
    fn the_take_number_moves_past_any_name_already_there_and_never_reuses_one() {
        let folder = Path::new("C:/Recordings");
        let channels = vec!["Kick".to_string(), "Snare".to_string()];
        let taken = [folder.join("2026-09-27 T001 Kick.wav"), folder.join("2026-09-27 T002 log.txt")];
        let exists = |path: &Path| taken.iter().any(|t| t == path);
        let mut w = words();
        let (take, files, log) = next_take(folder, DEFAULT_PATTERN, &mut w, &channels, 1, &exists).unwrap();
        assert_eq!(take, 3, "take 1 has a file there, and take 2 its log");
        assert_eq!(files[1], folder.join("2026-09-27 T003 Snare.wav"));
        assert_eq!(log, folder.join("2026-09-27 T003 log.txt"));
    }

    #[test]
    fn two_channels_named_alike_get_two_files() {
        let folder = Path::new("C:/Recordings");
        let channels = vec!["Mic".to_string(), "mic".to_string()];
        let mut w = words();
        let (_, files, _) = next_take(folder, DEFAULT_PATTERN, &mut w, &channels, 1, &|_| false).unwrap();
        assert_eq!(files[0], folder.join("2026-09-27 T001 Mic.wav"));
        assert_eq!(files[1], folder.join("2026-09-27 T001 mic (2).wav"));
    }
}
