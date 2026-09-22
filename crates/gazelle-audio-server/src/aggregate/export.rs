//! Exporting the aggregate's setup to the file the driver reads, and telling the driver about it.
//!
//! The workspace is where the setup lives; this is how it reaches a driver that runs inside a DAW
//! with Gazelle very possibly closed. Three things happen, in this order and only when what the
//! driver would be given has actually changed:
//!
//! 1. The document is written to a temporary file beside the real one and **renamed** over it, so
//!    a driver reading the file never sees half of one.
//! 2. The generation counter in the shared record is bumped.
//! 3. The named event is set, which is what wakes the driver's watcher thread.
//!
//! The export hangs off the workspace store rather than off a route ([`ExportingStore`]): saving
//! the workspace is the one moment the setup, the devices' names or the Mixer's names can change,
//! whichever route did it, so that is where the export belongs and nothing else has to remember.
//!
//! **The names in the file follow the devices too** (`crate::aggregate::naming`). Each entry's
//! [`AggregateKnown`] (which device it is, its model, and what its routing sends to each USB record
//! channel) is the server's alone: it is worked out again on every save from what Gazelle can see
//! now ([`Live`]), what a client sent for it is never taken, and what was known before is kept
//! while nothing better can be seen, so a restart does not take the names away. A routing change
//! does not come through a save at all, so the server calls [`ExportingStore::refresh`] when one
//! happens, whichever client made it and whether or not any page is open.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::aggregate::config::export_document;
use crate::aggregate::status::StatusLink;
use crate::device::descriptor::DeviceId;
use crate::error::ServerError;
use crate::workspace::model::{Aggregate, AggregateDevice, AggregateKnown, Workspace};
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

/// The file's text for a workspace, or nothing for one with no aggregate section.
fn document_text(workspace: &Workspace) -> Option<String> {
    let config = workspace.aggregate.as_ref()?;
    Some(serde_json::to_string_pretty(&export_document(config, workspace)).unwrap_or_default())
}

/// Write the aggregate's file and tell the driver.
pub fn export(path: &Path, workspace: &Workspace, link: &dyn StatusLink) -> Result<Exported, ServerError> {
    let text = document_text(workspace).unwrap_or_else(|| "{}".into());
    write_atomically(path, &text)?;
    // The file is what the driver needs; the signal only saves it a restart. A driver that is not
    // running cannot be signalled, and that is not a failure of the export.
    Ok(match link.signal() {
        Ok(generation) => Exported { path: path.to_path_buf(), generation: Some(generation), not_signalled: None },
        Err(why) => Exported { path: path.to_path_buf(), generation: None, not_signalled: Some(why) },
    })
}

/// What Gazelle can see now of the interface one entry is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub device_id: DeviceId,
    pub family: Option<String>,
    pub model: Option<String>,
    /// What its routing sends to each USB record channel, when that is known now.
    pub record_routing: Option<Vec<[u8; 2]>>,
}

/// Where [`Seen`] comes from: the connected devices and the routing the server has seen. Behind a
/// trait so every rule here is tested against devices made of data.
pub trait Live: Send + Sync {
    /// One per entry of the setup, in its order: what can be seen of it now, or nothing.
    fn seen(&self, config: &Aggregate) -> Vec<Option<Seen>>;
}

/// Nothing to see, which is what a store with no devices behind it has.
pub struct NothingLive;

impl Live for NothingLive {
    fn seen(&self, config: &Aggregate) -> Vec<Option<Seen>> {
        vec![None; config.devices.len()]
    }
}

/// Whether two entries are the same interface: the same class id, or the same registry key.
fn same_entry(a: &AggregateDevice, b: &AggregateDevice) -> bool {
    let same = |x: &Option<String>, y: &Option<String>| matches!((x, y), (Some(x), Some(y)) if x.trim().eq_ignore_ascii_case(y.trim()));
    same(&a.clsid, &b.clsid) || same(&a.key, &b.key)
}

/// What is known of one entry now: what can be seen, with anything it cannot say yet taken from what
/// was known of the same device before; or, with nothing to see, what was known before, unless the
/// person has since chosen a different device for it.
pub fn known_for(device: &AggregateDevice, before: Option<&AggregateKnown>, seen: Option<&Seen>) -> Option<AggregateKnown> {
    match seen {
        Some(seen) => {
            let same = before.filter(|known| known.device_id.as_ref() == Some(&seen.device_id));
            Some(AggregateKnown {
                device_id: Some(seen.device_id.clone()),
                family: seen.family.clone().or_else(|| same.and_then(|known| known.family.clone())),
                model: seen.model.clone().or_else(|| same.and_then(|known| known.model.clone())),
                record_routing: seen.record_routing.clone().or_else(|| same.and_then(|known| known.record_routing.clone())),
            })
        }
        None => match (&device.device_id, before) {
            // Chosen as another device than the one known: nothing known is about it.
            (Some(chosen), Some(known)) if known.device_id.as_ref() != Some(chosen) => Some(AggregateKnown { device_id: Some(chosen.clone()), ..AggregateKnown::default() }),
            (_, known) => known.cloned(),
        },
    }
}

/// The workspace with every entry's `known` worked out again. What the workspace came with is never
/// read for it: a client's copy may be minutes old. What was known is taken from `before`, the
/// workspace as the store last held it, entry by entry.
pub fn with_known(mut workspace: Workspace, before: Option<&Workspace>, seen: &[Option<Seen>]) -> Workspace {
    let earlier: &[AggregateDevice] = before.and_then(|w| w.aggregate.as_ref()).map_or(&[], |config| config.devices.as_slice());
    if let Some(config) = workspace.aggregate.as_mut() {
        for (at, device) in config.devices.iter_mut().enumerate() {
            let known = earlier.iter().find(|old| same_entry(old, device)).and_then(|old| old.known.as_ref());
            device.known = known_for(device, known, seen.get(at).and_then(Option::as_ref));
        }
    }
    workspace
}

/// A workspace store that exports the aggregate's setup whenever what the driver would be given
/// changes, and keeps each entry's [`AggregateKnown`] itself.
///
/// It wraps another store rather than replacing one, so every route, every test and the whole of
/// `AppState` are untouched by it: a server that has no aggregate simply does not wrap.
pub struct ExportingStore {
    inner: Arc<dyn WorkspaceStore>,
    path: PathBuf,
    link: Arc<dyn StatusLink>,
    live: Arc<dyn Live>,
    /// One save or refresh at a time, so a refresh reading the store cannot put back a workspace a
    /// client saved while it was working.
    lock: Mutex<()>,
    /// Told after every save, so whoever reads the routing an export needs hears of a new entry.
    saved: Notify,
}

impl ExportingStore {
    pub fn new(inner: Arc<dyn WorkspaceStore>, path: impl Into<PathBuf>, link: Arc<dyn StatusLink>) -> Self {
        ExportingStore { inner, path: path.into(), link, live: Arc::new(NothingLive), lock: Mutex::new(()), saved: Notify::new() }
    }

    /// The same store, seeing the devices through `live`.
    pub fn with_live(mut self, live: Arc<dyn Live>) -> Self {
        self.live = live;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Told after every save.
    pub fn saved(&self) -> &Notify {
        &self.saved
    }

    /// Write the file now if what is on disk is not what the workspace says, whatever the reason.
    /// Called once at startup, so a workspace restored from a backup reaches the driver without
    /// anyone having to open the page and change something. What was last known of each device is
    /// in the workspace, so this gives the driver the same names it had before the restart.
    pub fn sync(&self) -> Result<Option<Exported>, ServerError> {
        let workspace = self.inner.load()?;
        let Some(wanted) = document_text(&workspace) else { return Ok(None) };
        if std::fs::read_to_string(&self.path).is_ok_and(|on_disk| on_disk == wanted) {
            return Ok(None);
        }
        export(&self.path, &workspace, self.link.as_ref()).map(Some)
    }

    /// Work out again what is known of every entry, from the devices as they are now, and save and
    /// export only if anything changed. The server calls this when a device's routing changes or a
    /// device comes or goes. True when the workspace changed.
    pub fn refresh(&self) -> Result<bool, ServerError> {
        let _one = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = self.inner.load()?;
        let next = self.settle(current.clone(), Some(&current));
        if next.aggregate == current.aggregate {
            return Ok(false);
        }
        self.inner.save(&next)?;
        self.export_if_changed(Some(&current), &next);
        Ok(true)
    }

    /// The workspace with its entries' `known` worked out from what can be seen now.
    fn settle(&self, workspace: Workspace, before: Option<&Workspace>) -> Workspace {
        let seen = match &workspace.aggregate {
            Some(config) if !config.devices.is_empty() => self.live.seen(config),
            _ => Vec::new(),
        };
        with_known(workspace, before, &seen)
    }

    /// Export the file when what the driver would be given is not what it was.
    fn export_if_changed(&self, before: Option<&Workspace>, next: &Workspace) {
        let Some(wanted) = document_text(next) else {
            if before.is_some_and(|before| before.aggregate.is_some()) {
                // The section was removed. The file is left where it is: a driver that is running
                // keeps working, and taking the setup out of the workspace is not the same as
                // asking for the aggregate to stop.
                tracing::info!("the workspace no longer has an aggregate section; {} was left as it was", self.path.display());
            }
            return;
        };
        if before.and_then(document_text).as_ref() == Some(&wanted) {
            return;
        }
        match export(&self.path, next, self.link.as_ref()) {
            Ok(exported) => tracing::info!("wrote the aggregate's setup to {}", exported.path.display()),
            // The workspace is saved either way: refusing it because a file could not be written
            // would lose the person's edit as well.
            Err(why) => tracing::warn!("the workspace was saved and its aggregate setup could not be exported: {why}"),
        }
    }
}

impl WorkspaceStore for ExportingStore {
    fn load(&self) -> Result<Workspace, ServerError> {
        self.inner.load()
    }

    fn save(&self, workspace: &Workspace) -> Result<(), ServerError> {
        {
            let _one = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            // What was there before, so an unchanged setup writes nothing and signals nothing. A
            // store that cannot be read is not a reason to refuse the save: it only means the file
            // counts as changed, and is written.
            let before = self.inner.load().ok();
            let next = self.settle(workspace.clone(), before.as_ref());
            self.inner.save(&next)?;
            self.export_if_changed(before.as_ref(), &next);
        }
        self.saved.notify_one();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::status::FakeLink;
    use crate::workspace::model::{DeviceMixer, MixerChannel, RouteSource};
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

    /// Devices made of data: what is seen of each entry, by registry key, changed at will.
    #[derive(Default)]
    struct FakeLive {
        seen: Mutex<Vec<(String, Seen)>>,
    }

    impl FakeLive {
        fn set(&self, key: &str, seen: Seen) {
            let mut all = self.seen.lock().unwrap();
            all.retain(|(k, _)| k != key);
            all.push((key.into(), seen));
        }
    }

    impl Live for FakeLive {
        fn seen(&self, config: &Aggregate) -> Vec<Option<Seen>> {
            let all = self.seen.lock().unwrap();
            config.devices.iter().map(|device| all.iter().find(|(key, _)| Some(key) == device.key.as_ref()).map(|(_, seen)| seen.clone())).collect()
        }
    }

    fn quadro(routing: Option<Vec<[u8; 2]>>) -> Seen {
        Seen { device_id: DeviceId::from_serial("Q"), family: Some("quadro".into()), model: Some("Zen Quadro Synergy Core".into()), record_routing: routing }
    }

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
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
        assert_eq!(read(&path), serde_json::json!({ "devices": [{ "key": "Quadro", "name": "Quadro" }] }), "a device nothing is known of is called by its key");
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
        let exported = export(&path, &with_device("Quadro"), &link).unwrap();
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

    /// What Gazelle knows of each device is the server's: a client's copy of it is never taken, and
    /// what was known is kept while nothing better can be seen.
    #[test]
    fn what_is_known_of_a_device_is_the_servers_and_a_clients_copy_is_never_taken() {
        let dir = temp_dir("known");
        let path = dir.join("aggregate.json");
        let live = Arc::new(FakeLive::default());
        let store = ExportingStore::new(Arc::new(MemoryStore::default()), &path, Arc::new(FakeLink::default())).with_live(live.clone());
        live.set("Zen Quadro Synergy Core", quadro(Some(vec![[0, 0]])));
        store.save(&with_device("Zen Quadro Synergy Core")).unwrap();
        let known = store.load().unwrap().aggregate.unwrap().devices[0].known.clone().expect("what was seen");
        assert_eq!(known.device_id, Some(DeviceId::from_serial("Q")));
        assert_eq!(known.record_routing, Some(vec![[0, 0]]));

        // A client sends a stale copy, and the device cannot be seen now: what was known stays.
        *live.seen.lock().unwrap() = Vec::new();
        let mut stale = with_device("Zen Quadro Synergy Core");
        stale.aggregate.as_mut().unwrap().devices[0].known = Some(AggregateKnown { model: Some("Something made up".into()), ..AggregateKnown::default() });
        store.save(&stale).unwrap();
        assert_eq!(store.load().unwrap().aggregate.unwrap().devices[0].known, Some(known), "the client's copy counts for nothing");
        // And the file still names the device and its channels from what was known.
        assert_eq!(read(&path)["devices"][0]["name"], "Zen Quadro Synergy Core");
        assert_eq!(read(&path)["devices"][0]["input_names"]["0"], "PREAMP 1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The owner's case: the routing changes through Gazelle, and the file's names follow without
    /// anybody saving anything, while a name the person typed stays exactly as it was.
    #[test]
    fn a_routing_change_reaches_the_file_through_a_refresh_and_a_typed_name_is_left_alone() {
        let dir = temp_dir("reroute");
        let path = dir.join("aggregate.json");
        let live = Arc::new(FakeLive::default());
        let link = Arc::new(FakeLink::default());
        let store = ExportingStore::new(Arc::new(MemoryStore::default()), &path, link.clone()).with_live(live.clone());
        live.set("Zen Quadro Synergy Core", quadro(Some(vec![[0, 0], [0, 1]])));
        let mut workspace = with_device("Zen Quadro Synergy Core");
        workspace.aggregate.as_mut().unwrap().devices[0].input_names.insert(1, "Talkback".into());
        workspace.mixers.insert(
            DeviceId::from_serial("Q"),
            DeviceMixer {
                channels: vec![MixerChannel { id: "c".into(), name: "Vocal mic".into(), group: None, color: None, slot: 6, source: Some(RouteSource { group: 0, channel: 0 }), main_mix: Some(0), sends: Vec::new() }],
                ..DeviceMixer::default()
            },
        );
        store.save(&workspace).unwrap();
        assert_eq!(read(&path)["devices"][0]["input_names"]["0"], "Vocal mic");
        assert_eq!(read(&path)["devices"][0]["input_names"]["1"], "Talkback");
        assert_eq!(link.signalled(), vec![1]);

        assert!(!store.refresh().unwrap(), "nothing changed, so nothing is saved or written");
        assert_eq!(link.signalled(), vec![1]);

        // USB A REC 1 is routed from AFX OUT 3 now.
        live.set("Zen Quadro Synergy Core", quadro(Some(vec![[5, 2], [0, 1]])));
        assert!(store.refresh().unwrap());
        assert_eq!(read(&path)["devices"][0]["input_names"]["0"], "AFX OUT 3", "the automatic name follows the routing");
        assert_eq!(read(&path)["devices"][0]["input_names"]["1"], "Talkback", "and the typed one is untouched");
        assert_eq!(store.load().unwrap().aggregate.unwrap().devices[0].input_names.get(&0), None, "nothing automatic is ever written over the typed names");
        assert_eq!(link.signalled(), vec![1, 2], "the driver is told, to take it at its next reset");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A device renamed in Gazelle is a save of the workspace's names, not of the aggregate section,
    /// and still reaches the file, with the callback master following it.
    #[test]
    fn renaming_the_device_in_gazelle_renames_it_in_the_file_and_the_master_follows() {
        let dir = temp_dir("renamed");
        let path = dir.join("aggregate.json");
        let live = Arc::new(FakeLive::default());
        let store = ExportingStore::new(Arc::new(MemoryStore::default()), &path, Arc::new(FakeLink::default())).with_live(live.clone());
        live.set("Zen Quadro Synergy Core", quadro(None));
        let mut workspace = with_device("Zen Quadro Synergy Core");
        workspace.aggregate.as_mut().unwrap().callback_master = Some("Zen Quadro Synergy Core".into());
        store.save(&workspace).unwrap();
        assert_eq!(read(&path)["devices"][0]["name"], "Zen Quadro Synergy Core");
        assert_eq!(read(&path)["callback_master"], "Zen Quadro Synergy Core");

        workspace.aliases.insert(DeviceId::from_serial("Q"), "Desk".into());
        store.save(&workspace).unwrap();
        assert_eq!(read(&path)["devices"][0]["name"], "Desk");
        assert_eq!(read(&path)["callback_master"], "Desk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_device_chosen_as_another_forgets_what_was_known_of_the_first() {
        let before = AggregateKnown { device_id: Some(DeviceId::from_serial("Q")), family: Some("quadro".into()), model: None, record_routing: Some(vec![[0, 0]]) };
        let chosen = AggregateDevice { key: Some("k".into()), device_id: Some(DeviceId::from_serial("Q2")), ..AggregateDevice::default() };
        assert_eq!(known_for(&chosen, Some(&before), None), Some(AggregateKnown { device_id: Some(DeviceId::from_serial("Q2")), ..AggregateKnown::default() }));
        let same = AggregateDevice { device_id: Some(DeviceId::from_serial("Q")), ..chosen.clone() };
        assert_eq!(known_for(&same, Some(&before), None), Some(before.clone()), "the same device keeps what was known");
        // Seen now, with its routing not read since it came back: what was known of it fills in.
        let seen = Seen { device_id: DeviceId::from_serial("Q"), family: Some("quadro".into()), model: Some("Zen Quadro Synergy Core".into()), record_routing: None };
        assert_eq!(known_for(&same, Some(&before), Some(&seen)).unwrap().record_routing, Some(vec![[0, 0]]));
    }
}
