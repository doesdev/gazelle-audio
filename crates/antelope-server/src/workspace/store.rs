//! Workspace persistence.

use std::path::{Path, PathBuf};

use crate::error::ServerError;
use crate::workspace::model::Workspace;

/// Where workspace state is kept.
///
/// A trait so the backend can change without touching callers. The data is a single
/// document that is always read and written whole, which is what a JSON file is good at;
/// a database would buy nothing until cross-document queries or history arrive.
pub trait WorkspaceStore: Send + Sync {
    fn load(&self) -> Result<Workspace, ServerError>;
    fn save(&self, workspace: &Workspace) -> Result<(), ServerError>;
}

/// A workspace stored as one JSON file, written atomically.
pub struct JsonFileStore {
    path: PathBuf,
}

impl JsonFileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        JsonFileStore { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl WorkspaceStore for JsonFileStore {
    fn load(&self) -> Result<Workspace, ServerError> {
        match std::fs::read_to_string(&self.path) {
            // A missing file is the normal first-run case, not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Workspace::default()),
            Err(e) => Err(ServerError::Storage(format!(
                "reading {}: {e}",
                self.path.display()
            ))),
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                ServerError::Storage(format!("parsing {}: {e}", self.path.display()))
            }),
        }
    }

    fn save(&self, workspace: &Workspace) -> Result<(), ServerError> {
        let text = serde_json::to_string_pretty(workspace)
            .map_err(|e| ServerError::Storage(format!("serialising workspace: {e}")))?;

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                ServerError::Storage(format!("creating {}: {e}", dir.display()))
            })?;
        }

        // Write to a sibling temp file and rename, so an interrupted save cannot leave a
        // half-written workspace behind.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|e| ServerError::Storage(format!("writing {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            ServerError::Storage(format!("renaming into {}: {e}", self.path.display()))
        })
    }
}

/// An in-memory store, for tests and for `--no-persist`.
#[derive(Default)]
pub struct MemoryStore {
    inner: std::sync::RwLock<Workspace>,
}

impl WorkspaceStore for MemoryStore {
    fn load(&self) -> Result<Workspace, ServerError> {
        Ok(self.inner.read().unwrap().clone())
    }
    fn save(&self, workspace: &Workspace) -> Result<(), ServerError> {
        *self.inner.write().unwrap() = workspace.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::{Group, WORKSPACE_VERSION};

    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("antelope-ws-test-{}-{}.json", std::process::id(), name));
        p
    }

    #[test]
    fn missing_file_loads_a_default_workspace() {
        let store = JsonFileStore::new(temp_path("missing"));
        let w = store.load().expect("missing file is not an error");
        assert_eq!(w.version, WORKSPACE_VERSION);
        assert!(w.groups.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let store = JsonFileStore::new(&path);

        let mut w = Workspace::default();
        w.groups.push(Group {
            id: "g1".into(),
            name: "Drums".into(),
            collapsed: true,
            hidden: false,
            members: vec![],
            children: vec![],
        });
        store.save(&w).expect("save");

        let back = store.load().expect("load");
        assert_eq!(back.groups.len(), 1);
        assert_eq!(back.groups[0].name, "Drums");
        assert!(back.groups[0].collapsed);

        // No temp file is left behind by a successful save.
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn memory_store_roundtrips() {
        let store = MemoryStore::default();
        let mut w = Workspace::default();
        w.aliases.insert(crate::device::descriptor::DeviceId::loopback(0), "Main".into());
        store.save(&w).unwrap();
        assert_eq!(store.load().unwrap().aliases.len(), 1);
    }
}
