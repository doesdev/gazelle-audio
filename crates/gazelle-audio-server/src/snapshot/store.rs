//! Where snapshots are kept: one JSON document each, beside the workspace.
//!
//! Decision 0011 puts full device state in JSON documents behind a storage trait, and the
//! workspace deliberately does not hold them: a
//! snapshot is ~100 KB, so keeping them inside `workspace.json` would re-send every one of them
//! on every debounced rename, and two clients capturing at once would overwrite each other.

use std::path::{Path, PathBuf};

use crate::error::ServerError;
use crate::snapshot::model::{Snapshot, SNAPSHOT_VERSION};

/// The longest a snapshot id may be. Ids are the server's own (`Snapshot::new_id`); the limit is
/// for ones that arrive from a client, in a URL or an imported file.
const MAX_ID: usize = 64;

/// Check an id before it is ever used as a file name.
///
/// Ids reach this from a URL path segment, so a lax check is a path traversal: `../../workspace`
/// would read and delete the workspace. Only `a-z A-Z 0-9 _ -`, and never empty.
pub fn check_id(id: &str) -> Result<(), ServerError> {
    if id.is_empty() || id.len() > MAX_ID || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(ServerError::BadValue(format!(
            "{id:?} is not a snapshot id: letters, digits, '-' and '_', up to {MAX_ID} characters"
        )));
    }
    Ok(())
}

/// A document is refused when it is not a snapshot this server reads, rather than half-loaded.
pub fn check_version(version: u32) -> Result<(), ServerError> {
    if (1..=SNAPSHOT_VERSION).contains(&version) {
        Ok(())
    } else {
        Err(ServerError::BadValue(format!(
            "snapshot version {version} is not one this server reads ({SNAPSHOT_VERSION})"
        )))
    }
}

/// Where snapshots are kept. A trait so the backend can change without touching callers, as
/// [`crate::workspace::store::WorkspaceStore`] is.
pub trait SnapshotStore: Send + Sync {
    /// Every snapshot, newest first. Whole documents: there are tens of them, not thousands, and
    /// the caller projects [`Snapshot::summary`] for the list.
    fn list(&self) -> Result<Vec<Snapshot>, ServerError>;
    fn load(&self, id: &str) -> Result<Snapshot, ServerError>;
    fn save(&self, snapshot: &Snapshot) -> Result<(), ServerError>;
    fn delete(&self, id: &str) -> Result<(), ServerError>;
}

/// One JSON file per snapshot in a directory, each written atomically.
pub struct JsonDirStore {
    dir: PathBuf,
}

impl JsonDirStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        JsonDirStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> Result<PathBuf, ServerError> {
        check_id(id)?;
        Ok(self.dir.join(format!("{id}.json")))
    }
}

impl SnapshotStore for JsonDirStore {
    fn list(&self) -> Result<Vec<Snapshot>, ServerError> {
        let entries = match std::fs::read_dir(&self.dir) {
            // No directory is the normal first-run case: no snapshots have been taken.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ServerError::Storage(format!("reading {}: {e}", self.dir.display()))),
            Ok(entries) => entries,
        };
        let mut snapshots = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            // A file that is not a snapshot (an older format, a newer version, something the user
            // dropped in) is skipped with a warning rather than failing the whole list: one bad
            // file must not hide every good one.
            match std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Snapshot>(&t).ok()) {
                Some(snapshot) if check_version(snapshot.version).is_ok() => snapshots.push(snapshot),
                _ => tracing::warn!("{} is not a snapshot this server reads; skipping it", path.display()),
            }
        }
        // Newest first. `created` is RFC 3339 in UTC, so it sorts lexicographically.
        snapshots.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
        Ok(snapshots)
    }

    fn load(&self, id: &str) -> Result<Snapshot, ServerError> {
        let path = self.path(id)?;
        let text = match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ServerError::UnknownSnapshot(id.to_string())),
            Err(e) => return Err(ServerError::Storage(format!("reading {}: {e}", path.display()))),
            Ok(text) => text,
        };
        let snapshot: Snapshot = serde_json::from_str(&text)
            .map_err(|e| ServerError::Storage(format!("parsing {}: {e}", path.display())))?;
        check_version(snapshot.version)?;
        Ok(snapshot)
    }

    fn save(&self, snapshot: &Snapshot) -> Result<(), ServerError> {
        let path = self.path(&snapshot.id)?;
        let text = serde_json::to_string_pretty(snapshot)
            .map_err(|e| ServerError::Storage(format!("serialising snapshot {}: {e}", snapshot.id)))?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| ServerError::Storage(format!("creating {}: {e}", self.dir.display())))?;
        // Write beside it and rename, so an interrupted save cannot leave half a snapshot behind
        // that a later recall would read as device state.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|e| ServerError::Storage(format!("writing {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| ServerError::Storage(format!("renaming into {}: {e}", path.display())))
    }

    fn delete(&self, id: &str) -> Result<(), ServerError> {
        let path = self.path(id)?;
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ServerError::UnknownSnapshot(id.to_string())),
            Err(e) => Err(ServerError::Storage(format!("deleting {}: {e}", path.display()))),
            Ok(()) => Ok(()),
        }
    }
}

/// Snapshots held in memory only, for tests and for `--no-persist`.
#[derive(Default)]
pub struct MemorySnapshotStore {
    inner: std::sync::RwLock<std::collections::BTreeMap<String, Snapshot>>,
}

impl SnapshotStore for MemorySnapshotStore {
    fn list(&self) -> Result<Vec<Snapshot>, ServerError> {
        let mut snapshots: Vec<Snapshot> = self.inner.read().unwrap().values().cloned().collect();
        snapshots.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
        Ok(snapshots)
    }
    fn load(&self, id: &str) -> Result<Snapshot, ServerError> {
        check_id(id)?;
        self.inner.read().unwrap().get(id).cloned().ok_or_else(|| ServerError::UnknownSnapshot(id.to_string()))
    }
    fn save(&self, snapshot: &Snapshot) -> Result<(), ServerError> {
        check_id(&snapshot.id)?;
        self.inner.write().unwrap().insert(snapshot.id.clone(), snapshot.clone());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<(), ServerError> {
        check_id(id)?;
        self.inner
            .write()
            .unwrap()
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| ServerError::UnknownSnapshot(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::Workspace;

    fn temp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("gazelle-snap-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn snapshot(name: &str, created: &str) -> Snapshot {
        let mut s = Snapshot::new(name.into(), String::new(), Workspace::default());
        s.created = created.into();
        s
    }

    #[test]
    fn saved_snapshots_survive_the_store_being_rebuilt_and_come_back_newest_first() {
        let dir = temp_dir("restart");
        let store = JsonDirStore::new(&dir);
        assert!(store.list().expect("an empty directory is not an error").is_empty());
        store.save(&snapshot("Older", "2026-09-16T10:00:00Z")).expect("save");
        store.save(&snapshot("Newer", "2026-09-17T10:00:00Z")).expect("save");
        drop(store);

        // A new store over the same directory is what a restarted server has.
        let store = JsonDirStore::new(&dir);
        let listed = store.list().expect("list");
        assert_eq!(listed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Newer", "Older"]);
        let loaded = store.load(&listed[0].id).expect("load");
        assert_eq!(loaded.name, "Newer");
        assert!(!dir.join(format!("{}.json.tmp", listed[0].id)).exists(), "a successful save leaves no temp file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_removes_one_snapshot_and_a_missing_one_is_not_found() {
        let dir = temp_dir("delete");
        let store = JsonDirStore::new(&dir);
        let one = snapshot("One", "2026-09-17T10:00:00Z");
        store.save(&one).expect("save");
        store.delete(&one.id).expect("delete");
        assert!(store.list().expect("list").is_empty());
        assert!(matches!(store.delete(&one.id), Err(ServerError::UnknownSnapshot(_))));
        assert!(matches!(store.load(&one.id), Err(ServerError::UnknownSnapshot(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_id_that_would_escape_the_directory_is_refused_before_any_file_is_touched() {
        let store = JsonDirStore::new(temp_dir("escape"));
        for bad in ["../workspace", "a/b", "", "c:\\windows\\system32", &"x".repeat(65)] {
            assert!(matches!(store.load(bad), Err(ServerError::BadValue(_))), "{bad:?} was not refused");
            assert!(matches!(store.delete(bad), Err(ServerError::BadValue(_))), "{bad:?} was not refused");
        }
        assert!(check_id("snap-199f2c3a4b5-0001").is_ok());
    }

    #[test]
    fn a_file_that_is_not_a_readable_snapshot_is_skipped_rather_than_failing_the_list() {
        let dir = temp_dir("mixed");
        let store = JsonDirStore::new(&dir);
        let good = snapshot("Good", "2026-09-17T10:00:00Z");
        store.save(&good).expect("save");
        std::fs::write(dir.join("notes.txt"), b"not json").expect("write");
        std::fs::write(dir.join("broken.json"), b"{").expect("write");
        std::fs::write(dir.join("future.json"), br#"{"version":99,"id":"future","name":"n","created":"2026-09-18T10:00:00Z","workspace":{"version":1},"devices":{}}"#).expect("write");
        let listed = store.list().expect("list");
        assert_eq!(listed.len(), 1, "only the readable snapshot is listed: {:?}", listed.iter().map(|s| &s.name).collect::<Vec<_>>());
        assert_eq!(listed[0].name, "Good");
        // Fetched by name, a version this server does not read says so rather than loading half of it.
        assert!(matches!(store.load("future"), Err(ServerError::BadValue(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_memory_store_behaves_like_the_file_one() {
        let store = MemorySnapshotStore::default();
        let one = snapshot("One", "2026-09-17T10:00:00Z");
        store.save(&one).expect("save");
        assert_eq!(store.list().expect("list").len(), 1);
        assert_eq!(store.load(&one.id).expect("load").name, "One");
        store.delete(&one.id).expect("delete");
        assert!(matches!(store.load(&one.id), Err(ServerError::UnknownSnapshot(_))));
        assert!(matches!(store.load("../workspace"), Err(ServerError::BadValue(_))));
    }

    #[test]
    fn only_versions_this_server_reads_are_accepted() {
        assert!(check_version(SNAPSHOT_VERSION).is_ok());
        assert!(check_version(0).is_err());
        assert!(check_version(SNAPSHOT_VERSION + 1).is_err());
    }
}
