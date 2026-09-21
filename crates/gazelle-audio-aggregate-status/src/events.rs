//! The durable event log: what is in it, what a line looks like, and how it is kept small.
//!
//! The shared record says what is happening **now**, and it says nothing at all once the driver
//! has exited. This file is the other half: a handful of lines, written when something happens and
//! never on a timer, that a person can read the next morning to find out why last night's session
//! would not start.
//!
//! Only the driver writes it. Gazelle reads it, and so does anyone with a text editor, which is
//! why a line is plain words in the order a person reads them and not a structure.

/// What happened. These are the only things worth keeping after the fact: a refusal, a device
/// going away and coming back, the first block a session lost, a session beginning and ending, and
/// a change of plan being taken up or turned down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The driver would not do something, and here is what it told the DAW.
    Refused,
    /// A device stopped calling back and was muted.
    Stalled,
    /// It came back.
    Recovered,
    /// The first block a device lost in this session, dropped or missing. Only the first: the
    /// count of all of them belongs to [`Event::SessionEnded`], and a line per lost block would
    /// bury everything else in the file at exactly the moment a person needs to read it.
    Glitched,
    /// A DAW started the audio.
    SessionStarted,
    /// It stopped. Its line carries how long it ran and what each interface lost.
    SessionEnded,
    /// A new configuration was taken up.
    Adopted,
    /// A new configuration arrived while a DAW was streaming, so the host was asked to reset.
    ResetAsked,
    /// What a session's phase measurement came to, per interface: what was measured, what was
    /// applied, or why nothing was. The measurement itself is gone the moment the driver is
    /// released, so this line is the only thing that survives it.
    Phase,
}

impl Event {
    /// The one word a line carries. One word, no spaces, so a line splits the same way whatever is
    /// in its detail.
    pub fn word(self) -> &'static str {
        match self {
            Event::Refused => "refused",
            Event::Stalled => "stalled",
            Event::Recovered => "recovered",
            Event::Glitched => "glitched",
            Event::SessionStarted => "session-started",
            Event::SessionEnded => "session-ended",
            Event::Adopted => "adopted",
            Event::ResetAsked => "reset-asked",
            Event::Phase => "phase",
        }
    }

    pub fn from_word(word: &str) -> Option<Event> {
        ALL.into_iter().find(|event| event.word() == word)
    }
}

/// Every kind of line the log can hold. One list, so that a new event cannot be written by a
/// driver and then read back as nothing by the same build.
pub const ALL: [Event; 9] = [
    Event::Refused,
    Event::Stalled,
    Event::Recovered,
    Event::Glitched,
    Event::SessionStarted,
    Event::SessionEnded,
    Event::Adopted,
    Event::ResetAsked,
    Event::Phase,
];

/// One line of the log, taken apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventLine {
    /// When it happened, as the writer wrote it: `YYYY-MM-DD HH:MM:SS`, local time, because the
    /// person reading it is the person it happened to.
    pub at: String,
    pub event: Event,
    /// The rest of the line, in the same words the DAW or the person was given.
    pub detail: String,
}

/// How many lines the file keeps. A few hundred is more than one night's worth of real events and
/// still a file anyone can open.
pub const KEEP_LINES: usize = 400;

/// The clock format the log is written in.
pub const TIME_FORMAT: &str = "YYYY-MM-DD HH:MM:SS";

/// Make one line. Anything in `detail` that would make it two lines is flattened, because one
/// event is one line and a reader counts on that.
pub fn line(at: &str, event: Event, detail: &str) -> String {
    let flat: String = detail
        .chars()
        .map(|c| if c == '\r' || c == '\n' || c == '\t' { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.is_empty() {
        format!("{at} {}", event.word())
    } else {
        format!("{at} {} {flat}", event.word())
    }
}

/// Take one line apart again. A line this version does not understand reads as nothing rather than
/// as something wrong.
pub fn parse(text: &str) -> Option<EventLine> {
    let text = text.trim_end();
    // The timestamp is a date and a time, which is two pieces with a space between them.
    let mut pieces = text.splitn(4, ' ');
    let date = pieces.next()?;
    let time = pieces.next()?;
    let word = pieces.next()?;
    let detail = pieces.next().unwrap_or("").trim().to_string();
    if date.len() != 10 || time.len() != 8 {
        return None;
    }
    Some(EventLine { at: format!("{date} {time}"), event: Event::from_word(word)?, detail })
}

/// The last `keep` lines of a file, which is what is written back when it is opened. Blank lines
/// go, because a file that was half written when a machine lost power should not grow a hole that
/// stays for ever.
pub fn trimmed(text: &str, keep: usize) -> Vec<String> {
    let lines: Vec<String> =
        text.lines().map(|line| line.trim_end().to_string()).filter(|line| !line.is_empty()).collect();
    let from = lines.len().saturating_sub(keep);
    lines[from..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_the_time_then_one_word_then_the_reason() {
        let made = line("2026-09-20 21:14:07", Event::Refused, "Studio+ will not run at 96000 Hz");
        assert_eq!(made, "2026-09-20 21:14:07 refused Studio+ will not run at 96000 Hz");
        let back = parse(&made).expect("what was written reads back");
        assert_eq!(back.at, "2026-09-20 21:14:07");
        assert_eq!(back.event, Event::Refused);
        assert_eq!(back.detail, "Studio+ will not run at 96000 Hz");
    }

    #[test]
    fn an_event_with_nothing_to_add_is_still_a_whole_line() {
        let made = line("2026-09-20 21:14:07", Event::SessionEnded, "   ");
        assert_eq!(made, "2026-09-20 21:14:07 session-ended");
        assert_eq!(parse(&made).unwrap().detail, "");
        assert_eq!(parse(&made).unwrap().event, Event::SessionEnded);
    }

    #[test]
    fn a_reason_with_a_line_break_in_it_is_still_one_line() {
        let made = line("2026-09-20 21:14:07", Event::Refused, "one thing\nand\r\nanother\ttoo");
        assert_eq!(made.lines().count(), 1);
        assert_eq!(parse(&made).unwrap().detail, "one thing and  another too");
    }

    #[test]
    fn every_event_word_is_one_word_and_reads_back_as_itself() {
        for event in ALL {
            assert!(!event.word().contains(' '), "{}", event.word());
            assert_eq!(Event::from_word(event.word()), Some(event));
        }
        assert_eq!(Event::from_word("exploded"), None);
        let words: std::collections::BTreeSet<&str> = ALL.iter().map(|event| event.word()).collect();
        assert_eq!(words.len(), ALL.len(), "two events sharing a word would read back as one of them");
    }

    #[test]
    fn a_line_from_a_version_this_one_does_not_know_reads_as_nothing() {
        assert_eq!(parse("2026-09-20 21:14:07 tea-break the kettle"), None);
        assert_eq!(parse("not a log line at all"), None);
        assert_eq!(parse(""), None);
        // The right shape but a date that is not one.
        assert_eq!(parse("yesterday 21:14:07 refused something"), None);
    }

    #[test]
    fn opening_the_file_keeps_the_last_few_hundred_lines_and_no_blank_ones() {
        let long: String = (0..1000)
            .map(|n| format!("2026-09-20 21:14:07 stalled device {n}\n"))
            .collect::<Vec<_>>()
            .concat();
        let kept = trimmed(&long, KEEP_LINES);
        assert_eq!(kept.len(), KEEP_LINES);
        assert!(kept.first().unwrap().ends_with("device 600"), "the oldest kept line is the 601st");
        assert!(kept.last().unwrap().ends_with("device 999"), "the newest is kept");

        let holed = "2026-09-20 21:14:07 stalled a\n\n\n2026-09-20 21:14:08 recovered a\n";
        assert_eq!(trimmed(holed, KEEP_LINES).len(), 2);
    }

    #[test]
    fn a_file_shorter_than_the_limit_is_left_alone() {
        let short = "2026-09-20 21:14:07 session-started 40 in, 40 out\n";
        assert_eq!(trimmed(short, KEEP_LINES), vec!["2026-09-20 21:14:07 session-started 40 in, 40 out"]);
        assert_eq!(trimmed("", KEEP_LINES), Vec::<String>::new());
    }
}
