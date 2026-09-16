//! The server's log: always the console, and a file for a server nobody is watching.
//!
//! A server started from the tray's Start on boot or from Explorer has no terminal, so without a
//! file whatever it said is gone. The file is written only when the tray is on or `--log-dir` is
//! given ([`file_log_dir`]): test harnesses run `--no-tray` and must not fill the user's log folder.
//!
//! The file is capped rather than dated: `gazelle.log` grows to [`MAX_FILE_BYTES`], then moves to
//! `gazelle.1.log` (and that to `gazelle.2.log`, and so on) and a fresh one starts, keeping
//! [`KEPT_FILES`] old ones, so the folder never holds much more than 10 MB whatever the server
//! does. A new run appends to the current file. Each record is written whole and unbuffered, so the
//! last lines before a crash are on disk.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

/// The current log file, in the log folder.
pub const FILE_NAME: &str = "gazelle.log";
/// The size a log file grows to before a new one starts.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// How many full log files are kept beside the current one.
pub const KEPT_FILES: usize = 4;
/// What is logged, to the console and the file alike, unless the `RUST_LOG` environment variable
/// sets another filter. Targets match by prefix, so `gazelle_audio_server` also covers the
/// windowless build's own lines, whose target is `gazelle_audio_serverw::server`.
pub const DEFAULT_FILTER: &str = "gazelle_audio_server=info,tower_http=info";

/// The folder a log file is written to, if any: the one `--log-dir` names, whether or not the tray
/// is on; else, for a run with the tray, the platform default (`default`); else none.
pub fn file_log_dir(tray: bool, log_dir: Option<PathBuf>, default: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    match log_dir {
        Some(dir) => Some(dir),
        None if tray => default(),
        None => None,
    }
}

/// Install the global subscriber: the console, plus the file in `file_dir` when there is one.
/// Returns the folder really being written to; `None` when there is none, including when the file
/// could not be opened, which is logged to the console and is never a reason to stop the server.
pub fn init(file_dir: Option<PathBuf>) -> Option<PathBuf> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| DEFAULT_FILTER.into());
    let opened = file_dir.map(|dir| LogFile::open(&dir).map_err(|e| (dir, e)));
    let (file, failed) = match opened {
        Some(Ok(file)) => (Some(file), None),
        Some(Err(failed)) => (None, Some(failed)),
        None => (None, None),
    };
    let dir = file.as_ref().map(|f| f.dir().to_path_buf());
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(file.map(file_layer))
        .init();
    match (&dir, failed) {
        (Some(dir), _) => tracing::info!(
            "{} {} logging to {}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            dir.join(FILE_NAME).display()
        ),
        (None, Some((dir, e))) => tracing::warn!("no log file: opening {} failed: {e}", dir.join(FILE_NAME).display()),
        (None, None) => {}
    }
    dir
}

/// The layer that writes to a log file: the console's format without its colour codes.
pub fn file_layer<S>(file: LogFile) -> impl Layer<S>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer().with_ansi(false).with_writer(file)
}

/// A size-capped log file in a folder, shared by every thread that logs.
pub struct LogFile {
    dir: PathBuf,
    max_bytes: u64,
    kept: usize,
    current: Mutex<Current>,
}

struct Current {
    file: File,
    len: u64,
}

impl LogFile {
    /// Open (creating the folder if needed) with the standard limits.
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::with_limits(dir, MAX_FILE_BYTES, KEPT_FILES)
    }

    pub fn with_limits(dir: &Path, max_bytes: u64, kept: usize) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = OpenOptions::new().create(true).append(true).open(dir.join(FILE_NAME))?;
        let len = file.metadata()?.len();
        Ok(Self { dir: dir.to_path_buf(), max_bytes, kept, current: Mutex::new(Current { file, len }) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `gazelle.log`, then `gazelle.1.log`, `gazelle.2.log`, … oldest last.
    pub fn path(&self, age: usize) -> PathBuf {
        self.dir.join(if age == 0 { FILE_NAME.to_string() } else { format!("gazelle.{age}.log") })
    }

    /// Write one record whole. A record that would take the file past the cap starts a new file
    /// first, unless the file is empty: one oversized record still goes in, in one piece.
    fn write_record(&self, record: &[u8]) -> io::Result<()> {
        // A thread that panicked mid-write leaves nothing half-updated worth refusing to log over.
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if current.len > 0 && current.len + record.len() as u64 > self.max_bytes {
            self.rotate(&mut current)?;
        }
        current.file.write_all(record)?;
        current.len += record.len() as u64;
        Ok(())
    }

    /// Age every file by one, dropping the oldest, and start the current one afresh. A file that
    /// cannot be moved (another program holding it) is overwritten rather than left to grow, so the
    /// cap holds either way.
    fn rotate(&self, current: &mut Current) -> io::Result<()> {
        // Oldest first, each move replacing the file one older, so the oldest kept is overwritten.
        for age in (0..self.kept).rev() {
            let _ = fs::rename(self.path(age), self.path(age + 1));
        }
        current.file = OpenOptions::new().create(true).write(true).truncate(true).open(self.path(0))?;
        current.len = 0;
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogFile {
    type Writer = Record<'a>;

    fn make_writer(&'a self) -> Record<'a> {
        Record(self)
    }
}

/// A writer for one event. The formatter hands over each event in one write, which goes to the
/// file whole.
pub struct Record<'a>(&'a LogFile);

impl Write for Record<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_record(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the system temp directory, removed again when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gazelle-logging-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read(path: PathBuf) -> Option<String> {
        fs::read_to_string(path).ok()
    }

    fn write(file: &LogFile, record: &str) {
        file.make_writer().write_all(record.as_bytes()).unwrap();
    }

    #[test]
    fn file_logging_is_for_tray_runs_unless_a_folder_is_named() {
        let default = || Some(PathBuf::from("/default/logs"));
        assert_eq!(file_log_dir(true, None, default), Some(PathBuf::from("/default/logs")));
        assert_eq!(file_log_dir(false, None, default), None, "a --no-tray run (a test harness) writes no file");
        assert_eq!(file_log_dir(false, Some("/mine".into()), default), Some(PathBuf::from("/mine")));
        assert_eq!(file_log_dir(true, Some("/mine".into()), default), Some(PathBuf::from("/mine")));
        assert_eq!(file_log_dir(true, None, || None), None, "nowhere to put it, no file");
    }

    #[test]
    fn the_folder_is_created_and_a_new_run_appends() {
        let tmp = TempDir::new("append");
        let dir = tmp.0.join("nested").join("logs");
        write(&LogFile::open(&dir).unwrap(), "first run\n");
        write(&LogFile::open(&dir).unwrap(), "second run\n");
        assert_eq!(read(dir.join(FILE_NAME)).as_deref(), Some("first run\nsecond run\n"));
    }

    #[test]
    fn a_full_file_moves_aside_and_only_so_many_are_kept() {
        let tmp = TempDir::new("rotate");
        let file = LogFile::with_limits(&tmp.0, 10, 2).unwrap();
        for n in 1..=5 {
            write(&file, &format!("record {n}\n"));
        }
        assert_eq!(read(file.path(0)).as_deref(), Some("record 5\n"));
        assert_eq!(read(file.path(1)).as_deref(), Some("record 4\n"));
        assert_eq!(read(file.path(2)).as_deref(), Some("record 3\n"));
        assert_eq!(read(file.path(3)), None, "the oldest is dropped");
        assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 3);
    }

    #[test]
    fn records_fill_a_file_up_to_the_cap_exactly() {
        let tmp = TempDir::new("exact");
        let file = LogFile::with_limits(&tmp.0, 10, 2).unwrap();
        write(&file, "12345");
        write(&file, "67890");
        assert_eq!(read(file.path(0)).as_deref(), Some("1234567890"));
        assert_eq!(read(file.path(1)), None);
        write(&file, "x");
        assert_eq!(read(file.path(1)).as_deref(), Some("1234567890"));
        assert_eq!(read(file.path(0)).as_deref(), Some("x"));
    }

    #[test]
    fn a_record_is_never_split_even_past_the_cap() {
        let tmp = TempDir::new("whole");
        let file = LogFile::with_limits(&tmp.0, 10, 1).unwrap();
        let long = "a record much longer than the cap\n";
        write(&file, long);
        assert_eq!(read(file.path(0)).as_deref(), Some(long), "an empty file takes it whole");
        assert_eq!(read(file.path(1)), None, "and is not first moved aside, empty");
        write(&file, "next\n");
        assert_eq!(read(file.path(1)).as_deref(), Some(long));
        assert_eq!(read(file.path(0)).as_deref(), Some("next\n"));
    }

    #[test]
    fn a_file_left_full_by_an_earlier_run_moves_aside_on_the_first_record() {
        let tmp = TempDir::new("reopen");
        write(&LogFile::with_limits(&tmp.0, 10, 1).unwrap(), "0123456789");
        let file = LogFile::with_limits(&tmp.0, 10, 1).unwrap();
        write(&file, "new run\n");
        assert_eq!(read(file.path(1)).as_deref(), Some("0123456789"));
        assert_eq!(read(file.path(0)).as_deref(), Some("new run\n"));
    }

    #[test]
    fn with_nothing_kept_a_full_file_starts_over() {
        let tmp = TempDir::new("none-kept");
        let file = LogFile::with_limits(&tmp.0, 10, 0).unwrap();
        write(&file, "0123456789");
        write(&file, "again\n");
        assert_eq!(read(file.path(0)).as_deref(), Some("again\n"));
        assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 1);
    }

    #[test]
    fn events_reach_the_file_without_colour_codes() {
        use tracing_subscriber::layer::SubscriberExt;
        let tmp = TempDir::new("layer");
        let subscriber = tracing_subscriber::registry().with(file_layer(LogFile::open(&tmp.0).unwrap()));
        tracing::subscriber::with_default(subscriber, || tracing::warn!("the tray could not be added"));
        let text = read(tmp.0.join(FILE_NAME)).unwrap();
        assert!(text.contains("WARN"), "{text}");
        assert!(text.contains("the tray could not be added"), "{text}");
        assert!(!text.contains('\x1b'), "{text:?}");
        assert!(text.ends_with('\n'));
    }
}
