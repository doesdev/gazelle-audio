//! Exporting the aggregate's setup to the file the driver reads, and telling the driver about it.
//!
//! The workspace is where the setup lives; this is how it reaches a driver that runs inside a DAW
//! with Gazelle very possibly closed. Three things happen, in this order and only when the
//! section has actually changed:
//!
//! 1. The document is written to a temporary file beside the real one and **renamed** over it, so
//!    a driver reading the file never sees half of one.
//! 2. The generation counter in the shared record is bumped.
//! 3. The named event is set, which is what wakes the driver's watcher thread.
//!
//! The export hangs off the workspace store rather than off a route ([`ExportingStore`]): saving
//! the workspace is the one moment the section can change, whichever route did it, so that is
//! where the export belongs and nothing else has to remember.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::aggregate::config::export_document;
use crate::aggregate::status::StatusLink;
use crate::error::ServerError;
use crate::workspace::model::{Aggregate, Workspace};
use crate::workspace::store::WorkspaceStore;

/// What one export came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exported {
    pub path: PathBuf,
    /// The generation the driver was told about, when it could be told.
    pub generation: Option<u64>,
    /// Why the driver could not be told, which is the ordinary case: it is not running.
    pub not_signalled: Option<String>,
}

/// Write the document to `path` through a temporary file and a rename.
pub fn write_atomically(path: &Path, text: &str) -> Result<(), ServerError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ServerError::Storage(format!("creating {}: {e}", dir.display())))?;
    }
    // A sibling, so the rename stays on one filesystem and is therefore atomic.
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes()).map_err(|e| ServerError::Storage(format!("writing {}: {e}", temporary.display())))?;
    std::fs::rename(&temporary, path).map_err(|e| ServerError::Storage(format!("renaming into {}: {e}", path.display())))
}

/// Write the aggregate's file and tell the driver.
pub fn export(path: &Path, config: &Aggregate, link: &dyn StatusLink) -> Result<Exported, ServerError> {
    let document = export_document(config);
    let text = serde_json::to_string_pretty(&document).map_err(|e| ServerError::Storage(format!("building the aggregate's file: {e}")))?;
    write_atomically(path, &text)?;
    // The file is what the driver needs; the signal only saves it a restart. A driver that is not
    // running cannot be signalled, and that is not a failure of the export.
    Ok(match link.signal() {
        Ok(generation) => Exported { path: path.to_path_buf(), generation: Some(generation), not_signalled: None },
        Err(why) => Exported { path: path.to_path_buf(), generation: None, not_signalled: Some(why) },
    })
}

/// A workspace store that exports the aggregate's section whenever it changes.
///
/// It wraps another store rather than replacing one, so every route, every test and the whole of
/// `AppState` are untouched by it: a server that has no aggregate simply does not wrap.
pub struct ExportingStore {
    inner: Arc<dyn WorkspaceStore>,
    path: PathBuf,
    link: Arc<dyn StatusLink>,
}

impl ExportingStore {
    pub fn new(inner: Arc<dyn WorkspaceStore>, path: impl Into<PathBuf>, link: Arc<dyn StatusLink>) -> Self {
        ExportingStore { inner, path: path.into(), link: link.clone() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write the file now if what is on disk is not what the workspace says, whatever the reason.
    /// Called once at startup, so a workspace restored from a backup reaches the driver without
    /// anyone having to open the page and change something.
    pub fn sync(&self) -> Result<Option<Exported>, ServerError> {
        let workspace = self.inner.load()?;
        let Some(config) = &workspace.aggregate else { return Ok(None) };
        let wanted = serde_json::to_string_pretty(&export_document(config)).unwrap_or_default();
        if std::fs::read_to_string(&self.path).is_ok_and(|on_disk| on_disk == wanted) {
            return Ok(None);
        }
        export(&self.path, config, self.link.as_ref()).map(Some)
    }
}

impl WorkspaceStore for ExportingStore {
    fn load(&self) -> Result<Workspace, ServerError> {
        self.inner.load()
    }

    fn save(&self, workspace: &Workspace) -> Result<(), ServerError> {
        // What was there before, so an unchanged section writes nothing and signals nothing. A
        // store that cannot be read is not a reason to refuse the save: it only means the section
        // counts as changed, and the file is written.
        let before = self.inner.load().ok().and_then(|workspace| workspace.aggregate);
        self.inner.save(workspace)?;
        if before.as_ref() == workspace.aggregate.as_ref() {
            return Ok(());
        }
        let Some(config) = &workspace.aggregate else {
            // The section was removed. The file is left where it is: a driver that is running
            // keeps working, and taking the setup out of the workspace is not the same as asking
            // for the aggregate to stop.
            tracing::info!("the workspace no longer has an aggregate section; {} was left as it was", self.path.display());
            return Ok(());
        };
        match export(&self.path, config, self.link.as_ref()) {
            Ok(exported) => {
                tracing::info!("wrote the aggregate's setup to {}", exported.path.display());
                Ok(())
            }
            // The workspace is saved either way: refusing it because a file could not be written
            // would lose the person's edit as well.
            Err(why) => {
                tracing::warn!("the workspace was saved and its aggregate setup could not be exported: {why}");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::status::FakeLink;
    use crate::workspace::model::AggregateDevice;
    use crate::workspace::store::MemoryStore;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gazelle-aggregate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with_device(name: &str) -> Workspace {
        Workspace {
            aggregate: Some(Aggregate {
                devices: vec![AggregateDevice { key: Some(name.into()), ..AggregateDevice::default() }],
                ..Aggregate::default()
            }),
            ..Workspace::default()
        }
    }

    #[test]
    fn saving_a_changed_section_writes_the_file_and_signals_the_driver_once() {
        let dir = temp_dir("changed");
        let path = dir.join("aggregate.json");
        let link = Arc::new(FakeLink::default());
        let store = ExportingStore::new(Arc::new(MemoryStore::default()), &path, link.clone());

        store.save(&Workspace::default()).unwrap();
        assert!(!path.exists(), "a workspace with no aggregate section writes no file");

        store.save(&with_device("Quadro")).unwrap();
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!({ "devices": [{ "key": "Quadro" }] }));
        assert_eq!(link.signalled(), vec![1], "the generation was bumped and the event set, once");

        // The same section again changes nothing, so nothing is written and nothing is signalled.
        store.save(&with_device("Quadro")).unwrap();
        assert_eq!(link.signalled(), vec![1]);

        store.save(&with_device("Studio+")).unwrap();
        assert_eq!(link.signalled(), vec![1, 2]);
        assert!(std::fs::read_to_string(&path).unwrap().contains("Studio+"));
        assert!(!path.with_extension("json.tmp").exists(), "no temporary file is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_is_renamed_into_place_so_a_reader_never_sees_half_of_one() {
        let dir = temp_dir("atomic");
        let path = dir.join("aggregate.json");
        std::fs::write(&path, "{\"devices\":[]}").unwrap();
        write_atomically(&path, "{\n  \"devices\": [\n    {}\n  ]\n}").unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("devices"));
        assert!(!path.with_extension("json.tmp").exists());
        // Into a directory that is not there yet, which is the first run.
        let fresh = dir.join("deeper").join("aggregate.json");
        write_atomically(&fresh, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rename is the whole point, so it is tested as the driver would meet it: a reader going
    /// at the file while it is written over and over. Every read that succeeds is a whole
    /// document, never half of one, which is not true of writing the file in place.
    #[test]
    fn a_reader_going_at_the_file_while_it_is_written_never_sees_half_of_one() {
        let dir = temp_dir("racing");
        let path = dir.join("aggregate.json");
        // Big enough that writing it in place cannot finish between two of the reader's looks.
        let long = |what: &str| format!("{{\n  \"devices\": [{}],\n  \"callback_master\": \"{what}\"\n}}", vec!["{ \"key\": \"Quadro\" }"; 4000].join(", "));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let whole = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let torn = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader = {
            let (path, stop, whole, torn) = (path.clone(), stop.clone(), whole.clone(), torn.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    // A read that fails is the file not being there yet, which is not a torn read.
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(_) => whole.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                            Err(_) => torn.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                        };
                    }
                }
            })
        };
        for round in 0..60 {
            write_atomically(&path, &long(&round.to_string())).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        reader.join().unwrap();
        assert_eq!(torn.load(std::sync::atomic::Ordering::Relaxed), 0, "a reader saw a file that was not whole JSON");
        assert!(whole.load(std::sync::atomic::Ordering::Relaxed) > 0, "the reader never got a look in, so this proved nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_driver_that_cannot_be_signalled_still_gets_its_file() {
        let dir = temp_dir("unsignalled");
        let path = dir.join("aggregate.json");
        let link = FakeLink { signal_fails: Some("nothing is listening".into()), ..FakeLink::default() };
        let exported = export(&path, &with_device("Quadro").aggregate.unwrap(), &link).unwrap();
        assert_eq!(exported.generation, None);
        assert_eq!(exported.not_signalled.as_deref(), Some("nothing is listening"));
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_workspace_restored_from_a_backup_reaches_the_driver_at_startup() {
        let dir = temp_dir("sync");
        let path = dir.join("aggregate.json");
        let inner = Arc::new(MemoryStore::default());
        inner.save(&with_device("Quadro")).unwrap();
        let link = Arc::new(FakeLink::default());
        let store = ExportingStore::new(inner, &path, link.clone());

        assert!(store.sync().unwrap().is_some(), "the file is not there, so it is written");
        assert!(path.exists());
        assert!(store.sync().unwrap().is_none(), "and a second start writes nothing");
        assert_eq!(link.signalled(), vec![1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_the_section_leaves_the_file_alone() {
        let dir = temp_dir("removed");
        let path = dir.join("aggregate.json");
        let link = Arc::new(FakeLink::default());
        let store = ExportingStore::new(Arc::new(MemoryStore::default()), &path, link.clone());
        store.save(&with_device("Quadro")).unwrap();
        store.save(&Workspace::default()).unwrap();
        assert!(path.exists(), "a running driver keeps the setup it was given");
        assert_eq!(link.signalled(), vec![1], "and it is not told about a change that is not one");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
