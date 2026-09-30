//! **A Cubase track archive for each take**, so a take goes into a Cubase project with one import:
//! File > Import > Track Archive.
//!
//! A track archive is the XML Cubase writes with File > Export > Selected Tracks. Gazelle does not
//! write one from nothing. It is made from a **seed**: an archive the owner exported from their own
//! tracking template, holding a folder track with its own group channel (a "folder with group") and
//! at least one mono audio track in it with a recording on it. Each take's archive is that seed with
//! the audio tracks swapped for one per recorded file, so every setting Cubase keeps on a track,
//! inserts, sends, colour and the routing into the folder's group, is the owner's own.
//!
//! # What an archive is made of
//!
//! - The root, `tracklist2`, holds a list named `track` (what was exported), then `ExternalRouting`
//!   (the buses outside the archive its tracks use: inputs, and the group), `PArrangeSetup` (the
//!   project's rate, bit depth and length), the tempo and signature tracks, and then objects the
//!   tracks refer to: each audio clip, its volume curve, its quick controls, its automation.
//! - **Objects are written once and referred to after.** An `obj` element with a `class` and an `ID`
//!   is an object; an `obj` with an `ID` and no class, always empty, refers back (or forward) to it.
//!   IDs are unique within the file and mean nothing outside it. The `track` list also names tracks
//!   already written inside the folder by `<item value="ID"/>`.
//! - The folder (`MFolderTrack`) holds its tracks in `Node` > `Tracks`: first its group channel
//!   (`MDeviceTrackEvent`, which the folder names as its `GroupTrackEvent`), then the audio tracks
//!   (`MAudioTrackEvent`). An audio track is routed to the group by its channel's `OutputBusValue`
//!   holding the group's own bus UID (`OwnInputBus` > `Bus UID` on the group).
//! - An audio track's recording is an `MAudioEvent` (start, length and offset in samples) naming a
//!   `PAudioClip`, whose `AudioCluster` holds one `AudioFile` (frames, sample size, rate, where the
//!   audio starts in the file) naming an `FNPath` (the file's name, and its folder as an absolute
//!   path with a trailing backslash).
//!
//! # How a take's archive is made
//!
//! - Everything outside the folder's audio tracks is copied through as it is, byte for byte.
//! - Each recorded file gets a copy of one of the seed's mono audio tracks: the first file the first,
//!   the second the second, and so on, with the last one used again once they run out. Every object
//!   in the copy gets a fresh ID and every reference inside it follows, while references to what is
//!   shared (the tempo track, the group) are left alone. The seed's own audio tracks, and the objects
//!   only they used, are left out.
//! - The copy is named after the channel, its one event starts at the project start and runs the
//!   length of the file, and its clip names the take's WAV by its absolute path, where it is. Nothing
//!   is copied into a project folder.
//! - What Cubase expects to be unique in a project is made unique across the copies: runtime IDs,
//!   each channel's own bus UID and internal names, plug-in instance names, and each clip's UID.
//! - The result is read back and checked before it is used: every object's ID is unique and every
//!   reference finds its object ([`check_archive`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use roxmltree::{Document, Node, NodeId, ParsingOptions};
use serde::Serialize;

use crate::wav::SampleFormat;

/// One file of a take, as its track in the archive needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct TakeFile {
    /// The channel's name, which the track takes.
    pub channel: String,
    /// The WAV, whole.
    pub path: PathBuf,
    /// Its length in samples.
    pub frames: u64,
    pub rate: f64,
    pub format: SampleFormat,
    /// Where the audio starts in the file, in bytes.
    pub data_offset: u64,
}

/// What a usable seed holds, for the page.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SeedSummary {
    /// The folder the takes' tracks go into.
    pub folder: String,
    /// The folder's own group channel.
    pub group: String,
    /// The folder's mono audio tracks with a recording on them, which the takes' tracks are copied from.
    pub tracks: usize,
    /// The project's rate, as the seed says it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
}

impl SeedSummary {
    /// How the page says it.
    pub fn words(&self) -> String {
        let tracks = if self.tracks == 1 { "1 mono track".to_string() } else { format!("{} mono tracks", self.tracks) };
        let rate = self.rate.map(|rate| format!(", {} kHz", num(rate / 1000.0))).unwrap_or_default();
        format!("The folder \"{}\" with its group \"{}\", and {tracks} to copy{rate}.", self.folder, self.group)
    }
}

/// Whether `text` will do as a seed, and what it holds; or the sentence that says why not. A seed is
/// only taken when a take's archive can actually be made from it.
pub fn check_seed(text: &str) -> Result<SeedSummary, String> {
    let (_, body) = split_bom(text);
    let doc = parse(body)?;
    let seed = Seed::read(&doc, body)?;
    let plan = Plan::make(&seed)?;
    let rate = seed.rate().unwrap_or(48_000.0);
    let trial = TakeFile { channel: "Check".into(), path: PathBuf::from(r"C:\Gazelle\Check.wav"), frames: rate as u64, rate, format: SampleFormat::Int24, data_offset: 44 };
    assemble(&seed, &plan, std::slice::from_ref(&trial))?;
    Ok(SeedSummary { folder: seed.folder_name.clone(), group: seed.group_name.clone(), tracks: seed.templates.len(), rate: seed.rate() })
}

/// **A take's archive**, made from the seed's text and the take's files, checked.
pub fn build(seed_text: &str, files: &[TakeFile]) -> Result<String, String> {
    if files.is_empty() {
        return Err("the take has no files to put in it".into());
    }
    let (bom, body) = split_bom(seed_text);
    let doc = parse(body)?;
    let seed = Seed::read(&doc, body)?;
    let plan = Plan::make(&seed)?;
    let out = assemble(&seed, &plan, files)?;
    Ok(format!("{bom}{out}"))
}

/// What a finished archive holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveFacts {
    /// Tracks the `track` list names by reference, besides the ones written in it.
    pub listed: usize,
    /// Objects, each with its own ID.
    pub objects: usize,
    /// References to them.
    pub references: usize,
}

/// Read an archive back: it is XML, its root is `tracklist2`, no two objects share an ID, and every
/// reference, `obj` or `item` in the `track` list, finds its object.
pub fn check_archive(xml: &str) -> Result<ArchiveFacts, String> {
    let (_, body) = split_bom(xml);
    let doc = parse(body)?;
    let root = doc.root_element();
    if root.tag_name().name() != "tracklist2" {
        return Err(format!("its root is {}, not tracklist2", root.tag_name().name()));
    }
    let mut ids = HashSet::new();
    for node in doc.descendants().filter(|n| is_def(*n)) {
        let id = node.attribute("ID").unwrap_or_default();
        if !ids.insert(id) {
            return Err(format!("two objects have the ID {id}"));
        }
    }
    let mut references = 0;
    for node in doc.descendants().filter(|n| is_ref(*n)) {
        let id = node.attribute("ID").unwrap_or_default();
        if !ids.contains(id) {
            return Err(format!("a reference to {id} finds no object"));
        }
        references += 1;
    }
    let mut listed = 0;
    if let Some(list) = child(root, "list", "track") {
        for item in list.children().filter(|n| n.has_tag_name("item")) {
            let Some(id) = item.attribute("value") else { continue };
            if !ids.contains(id) {
                return Err(format!("the track list names {id}, which is not there"));
            }
            listed += 1;
        }
    }
    Ok(ArchiveFacts { listed, objects: ids.len(), references })
}

// -------------------------------------------------------------------------------------------------
// Reading the seed.
// -------------------------------------------------------------------------------------------------

fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

fn parse(text: &str) -> Result<Document<'_>, String> {
    let options = ParsingOptions { allow_dtd: false, ..ParsingOptions::default() };
    Document::parse_with_options(text, options).map_err(|e| format!("it is not XML that Gazelle can read ({e})"))
}

/// An object: `obj` with a class and an ID.
fn is_def(node: Node) -> bool {
    node.has_tag_name("obj") && node.has_attribute("class") && node.has_attribute("ID")
}

/// A reference to one: `obj` with an ID and no class.
fn is_ref(node: Node) -> bool {
    node.has_tag_name("obj") && !node.has_attribute("class") && node.has_attribute("ID")
}

fn class<'a>(node: Option<Node<'a, '_>>) -> Option<&'a str> {
    node.and_then(|n| n.attribute("class"))
}

fn name_of<'a>(node: Option<Node<'a, '_>>) -> Option<&'a str> {
    node.and_then(|n| n.attribute("name"))
}

/// The child element `<tag name="name">`.
fn child<'a, 'i>(node: Node<'a, 'i>, tag: &str, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|n| n.has_tag_name(tag) && n.attribute("name") == Some(name))
}

/// The `value` of the child element `<tag name="name">`.
fn value_of<'a>(node: Node<'a, '_>, tag: &str, name: &str) -> Option<&'a str> {
    child(node, tag, name).and_then(|n| n.attribute("value"))
}

/// The whitespace just before `node`, to put before anything added beside it.
fn indent_before<'i>(text: &'i str, node: Node) -> &'i str {
    let start = node.range().start;
    let before = &text[..start];
    let trimmed = before.trim_end_matches([' ', '\t', '\r', '\n']);
    &text[trimmed.len()..start]
}

/// The seed, read.
struct Seed<'a, 'i> {
    text: &'i str,
    root: Node<'a, 'i>,
    track_list: Node<'a, 'i>,
    folder: Node<'a, 'i>,
    folder_name: String,
    group_name: String,
    /// Every audio track in the folder, all of which are replaced.
    audio: Vec<Node<'a, 'i>>,
    /// The mono ones with a recording on them, in order, which the copies are made from.
    templates: Vec<Node<'a, 'i>>,
    defs: HashMap<&'a str, Node<'a, 'i>>,
}

impl<'a, 'i> Seed<'a, 'i> {
    fn read(doc: &'a Document<'i>, text: &'i str) -> Result<Seed<'a, 'i>, String> {
        let root = doc.root_element();
        if root.tag_name().name() != "tracklist2" {
            return Err("it is not a Cubase track archive: export one from Cubase with File > Export > Selected Tracks".into());
        }
        let track_list = child(root, "list", "track").ok_or("the track archive holds no tracks")?;
        let mut defs = HashMap::new();
        for node in doc.descendants().filter(|n| is_def(*n)) {
            let id = node.attribute("ID").unwrap_or_default();
            if defs.insert(id, node).is_some() {
                return Err(format!("the track archive has two objects with the ID {id}, which Cubase never writes"));
            }
        }
        let resolve = |node: Node<'a, 'i>| -> Option<Node<'a, 'i>> {
            if is_def(node) {
                Some(node)
            } else if is_ref(node) {
                defs.get(node.attribute("ID").unwrap_or_default()).copied()
            } else {
                None
            }
        };

        let folders: Vec<Node> = track_list.children().filter(|n| class(Some(*n)) == Some("MFolderTrack")).collect();
        if folders.is_empty() {
            return Err("the track archive has no folder track: select the folder itself in Cubase, with its tracks in it, and export that".into());
        }
        let mut first_problem = None;
        for folder in folders {
            let Some(tracks) = child(folder, "obj", "Node").and_then(|node| child(node, "list", "Tracks")) else { continue };
            let audio: Vec<Node> = tracks.children().filter(|n| class(Some(*n)) == Some("MAudioTrackEvent")).collect();
            let mut templates = Vec::new();
            for track in &audio {
                match template_problem(*track, &resolve) {
                    None => templates.push(*track),
                    Some(why) => {
                        first_problem.get_or_insert_with(|| format!("its track \"{}\" {why}", track_name(*track)));
                    }
                }
            }
            if templates.is_empty() {
                continue;
            }
            let folder_name = child(folder, "obj", "Node").and_then(|node| value_of(node, "string", "Name")).unwrap_or("Folder").to_string();
            let group = child(folder, "obj", "GroupTrackEvent").and_then(resolve).filter(|g| class(Some(*g)) == Some("MDeviceTrackEvent"));
            let Some(group) = group else {
                return Err(format!("the folder \"{folder_name}\" has no group channel of its own: make it a folder with group in Cubase and export it again"));
            };
            if group.parent_element() != Some(tracks) {
                return Err(format!("the folder \"{folder_name}\"'s group channel is not inside it"));
            }
            let group_name = child(group, "obj", "Node").and_then(|node| value_of(node, "string", "Name")).unwrap_or("Group").to_string();
            return Ok(Seed { text, root, track_list, folder, folder_name, group_name, audio, templates, defs });
        }
        Err(match first_problem {
            Some(why) => format!("the folder has no mono audio track with a recording on it to copy: {why}"),
            None => "the folder has no audio tracks: record a few seconds on a mono track in it, then export the folder".into(),
        })
    }

    /// The project's rate, as `PArrangeSetup` says it.
    fn rate(&self) -> Option<f64> {
        let setup = self.root.children().find(|n| class(Some(*n)) == Some("PArrangeSetup"))?;
        value_of(setup, "float", "SampleRate")?.parse().ok()
    }

    /// The project's length in seconds, when `PArrangeSetup` says it in seconds.
    fn seconds(&self) -> Option<f64> {
        let setup = self.root.children().find(|n| class(Some(*n)) == Some("PArrangeSetup"))?;
        let length = child(setup, "member", "Length")?;
        let domain = child(length, "member", "Domain")?;
        (value_of(domain, "int", "Type") == Some("1") && value_of(domain, "float", "Period").and_then(|p| p.parse::<f64>().ok()) == Some(1.0)).then_some(())?;
        value_of(length, "float", "Time")?.parse().ok()
    }
}

fn track_name(track: Node) -> String {
    child(track, "obj", "Node").and_then(|node| value_of(node, "string", "Name")).unwrap_or("?").to_string()
}

/// Why a seed audio track cannot be copied for a take's file, or nothing when it can: it is a mono
/// channel with one recording on it, of one mono file.
fn template_problem<'a, 'i>(track: Node<'a, 'i>, resolve: &dyn Fn(Node<'a, 'i>) -> Option<Node<'a, 'i>>) -> Option<&'static str> {
    let Some(device) = child(track, "obj", "Track Device").filter(|d| class(Some(*d)) == Some("MAudioTrack")) else { return Some("has no audio channel") };
    let arrangement: Option<Vec<&str>> = child(device, "member", "DeviceAttributes")
        .and_then(|attrs| child(attrs, "member", "OwnInputBus"))
        .and_then(|bus| child(bus, "member", "Input Arrangement"))
        .and_then(|arr| child(arr, "list", "Type"))
        .map(|list| list.children().filter(|n| n.has_tag_name("item")).filter_map(|n| n.attribute("value")).collect());
    if arrangement.as_deref() != Some(&["0"][..]) {
        return Some("is not mono");
    }
    let Some(event) = child(track, "obj", "Node").and_then(|node| child(node, "list", "Events")).and_then(|events| events.children().find(|n| class(Some(*n)) == Some("MAudioEvent"))) else {
        return Some("has no recording on it");
    };
    let Some(clip) = child(event, "obj", "AudioClip").and_then(resolve).filter(|c| class(Some(*c)) == Some("PAudioClip")) else { return Some("has a recording with no clip") };
    if child(clip, "obj", "Path").and_then(resolve).filter(|p| class(Some(*p)) == Some("FNPath")).is_none() {
        return Some("has a recording with no file");
    }
    let Some(cluster) = child(clip, "obj", "Cluster").filter(|c| class(Some(*c)) == Some("AudioCluster")) else { return Some("has a recording with no audio") };
    let streams: Vec<Node> = child(cluster, "list", "Substreams").map(|list| list.children().filter(|n| n.is_element()).collect()).unwrap_or_default();
    let segments = child(cluster, "list", "Segments").map(|list| list.children().filter(|n| n.has_tag_name("item")).count()).unwrap_or(0);
    let [stream] = streams[..] else { return Some("has a recording made of more than one file") };
    let Some(file) = resolve(stream).filter(|f| class(Some(*f)) == Some("AudioFile")) else { return Some("has a recording with no audio file") };
    if value_of(file, "int", "Channels") != Some("1") || segments != 1 {
        return Some("has a recording that is not one mono file");
    }
    None
}

/// What is kept of the seed as it is, and what is left out.
struct Plan<'a> {
    /// Left out: the folder's audio tracks and anything else in the track list but the folder.
    dropped: HashSet<NodeId>,
    /// Objects written in the output as they are, by ID: a reference to one stays as it is.
    kept: HashSet<&'a str>,
    /// Objects at the top level used only by what is left out.
    drop_top: HashSet<NodeId>,
}

impl<'a> Plan<'a> {
    fn make<'i>(seed: &Seed<'a, 'i>) -> Result<Plan<'a>, String> {
        let mut dropped: HashSet<NodeId> = seed.audio.iter().map(|n| n.id()).collect();
        dropped.extend(seed.track_list.children().filter(|n| n.is_element() && *n != seed.folder).map(|n| n.id()));
        let referenced: HashSet<&str> = seed.root.descendants().filter(|n| is_ref(*n)).filter_map(|n| n.attribute("ID")).collect();
        let tops: Vec<Node> = seed.root.children().filter(|n| is_def(*n)).collect();
        let mut kept_top: HashSet<NodeId> = HashSet::new();
        let mut queue: Vec<Node> = Vec::new();
        for top in seed.root.children().filter(|n| n.is_element()) {
            if is_def(top) && referenced.contains(top.attribute("ID").unwrap_or_default()) {
                continue;
            }
            // The track list, and whatever nothing refers to (the routing, the setup).
            kept_top.insert(top.id());
            queue.push(top);
        }
        let mut kept = HashSet::new();
        while let Some(start) = queue.pop() {
            let mut stack = vec![start];
            while let Some(node) = stack.pop() {
                if dropped.contains(&node.id()) {
                    continue;
                }
                if is_def(node) {
                    kept.insert(node.attribute("ID").unwrap_or_default());
                }
                if is_ref(node) {
                    let id = node.attribute("ID").unwrap_or_default();
                    let Some(target) = seed.defs.get(id) else { return Err(format!("the track archive refers to an object {id} that is not in it")) };
                    let top = top_of(seed.root, *target);
                    if top.is_some_and(|top| top != seed.track_list && !kept_top.contains(&top.id())) {
                        let top = top.unwrap_or(*target);
                        kept_top.insert(top.id());
                        queue.push(top);
                    } else if target.ancestors().any(|a| dropped.contains(&a.id())) {
                        return Err("the folder refers to something inside one of its audio tracks, which Gazelle cannot leave out".into());
                    }
                }
                stack.extend(node.children().filter(|n| n.is_element()));
            }
        }
        let drop_top = tops.iter().filter(|n| !kept_top.contains(&n.id())).map(|n| n.id()).collect();
        Ok(Plan { dropped, kept, drop_top })
    }
}

/// The child of the root that `node` is in, or is.
fn top_of<'a, 'i>(root: Node<'a, 'i>, node: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    node.ancestors().find(|a| a.parent_element() == Some(root))
}

// -------------------------------------------------------------------------------------------------
// Writing: a walk that copies the seed's text and changes only what it is told to.
// -------------------------------------------------------------------------------------------------

/// What to do with one element.
#[derive(Default)]
struct Visit {
    /// Leave it out, with the whitespace before it.
    drop: bool,
    /// Put this in its place.
    replace: Option<String>,
    /// New values for its attributes, unescaped.
    attrs: Vec<(&'static str, String)>,
    /// Put this after its last child, before its end tag.
    append: String,
}

trait Policy<'a, 'i: 'a> {
    fn text(&self) -> &'i str;
    fn visit(&mut self, node: Node<'a, 'i>) -> Visit;
}

/// The element's text as the policy has it, or nothing when it is left out.
fn walk<'a, 'i: 'a, P: Policy<'a, 'i>>(policy: &mut P, node: Node<'a, 'i>) -> Option<String> {
    let visit = policy.visit(node);
    if visit.drop {
        return None;
    }
    if let Some(replacement) = visit.replace {
        return Some(replacement);
    }
    let text = policy.text();
    let range = node.range();
    let mut edits: Vec<(std::ops::Range<usize>, String)> =
        visit.attrs.iter().filter_map(|(name, value)| node.attributes().find(|a| a.name() == *name).map(|a| (a.range_value(), escape(value)))).collect();
    edits.sort_by_key(|(at, _)| at.start);
    let mut out = String::with_capacity(range.len());
    let mut at = range.start;
    for (span, value) in edits {
        out.push_str(&text[at..span.start]);
        out.push_str(&value);
        at = span.end;
    }
    for kid in node.children().filter(|n| n.is_element()) {
        let span = kid.range();
        let gap = &text[at..span.start];
        match walk(policy, kid) {
            Some(piece) => {
                out.push_str(gap);
                out.push_str(&piece);
            }
            None if gap.trim().is_empty() => {}
            None => out.push_str(gap),
        }
        at = span.end;
    }
    out.push_str(&visit.append);
    out.push_str(&text[at..range.end]);
    Some(out)
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// A number as Cubase writes a whole one, or Rust's shortest exact form of any other.
fn num(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// What must be new in each copy: object IDs, runtime IDs, bus UIDs and names.
struct Fresh {
    next_id: u64,
    next_runtime: i64,
    next_bus: i64,
    /// For each plug-in class in `IDString` ("<class>-<n>"), the next number.
    next_instance: HashMap<String, u64>,
    buses: HashSet<i64>,
    bus_names: HashSet<String>,
    channel_ids: HashSet<String>,
    instances: HashSet<String>,
    guids: u64,
}

/// "<32 hex digits>-<n>", a plug-in instance's name.
fn instance(value: &str) -> Option<(&str, u64)> {
    let (class, n) = value.split_once('-')?;
    (class.len() == 32 && class.bytes().all(|b| b.is_ascii_hexdigit())).then_some(())?;
    Some((class, n.parse().ok()?))
}

impl Fresh {
    fn new(seed: &Seed, plan: &Plan) -> Fresh {
        let mut fresh = Fresh {
            next_id: 0,
            next_runtime: 0,
            next_bus: 0,
            next_instance: HashMap::new(),
            buses: HashSet::new(),
            bus_names: HashSet::new(),
            channel_ids: HashSet::new(),
            instances: HashSet::new(),
            guids: 0,
        };
        let mut max_id = 0u64;
        for node in seed.root.descendants().filter(|n| n.is_element()) {
            if let Some(id) = node.attribute("ID").and_then(|id| id.parse::<u64>().ok()) {
                max_id = max_id.max(id);
            }
            let name = node.attribute("name");
            let value = node.attribute("value");
            match (name, value) {
                (Some("RuntimeID"), Some(v)) => fresh.next_runtime = fresh.next_runtime.max(v.parse::<i64>().unwrap_or(0) + 1),
                (Some("Bus UID"), Some(v)) => fresh.next_bus = fresh.next_bus.max(v.parse::<i64>().unwrap_or(0) + 1),
                (Some("Value"), Some(v)) if matches!(name_of(node.parent_element()), Some("InputBusValue" | "OutputBusValue")) => {
                    fresh.next_bus = fresh.next_bus.max(v.parse::<i64>().unwrap_or(0) + 1)
                }
                (Some("IDString"), Some(v)) => {
                    if let Some((class, n)) = instance(v) {
                        let next = fresh.next_instance.entry(class.to_string()).or_insert(0);
                        *next = (*next).max(n + 1);
                    }
                }
                _ => {}
            }
        }
        // Pointer-like, as Cubase's own are: a multiple of 16, past every ID in the seed.
        fresh.next_id = (max_id + 16) & !15;
        // What the part kept as it is already uses.
        for node in seed.root.descendants().filter(|n| n.is_element()) {
            if node.ancestors().any(|a| plan.dropped.contains(&a.id()) || plan.drop_top.contains(&a.id())) {
                continue;
            }
            fresh.note_used(node);
        }
        fresh
    }

    fn note_used(&mut self, node: Node) {
        let parent = node.parent_element();
        let value = node.attribute("value").unwrap_or_default();
        match node.attribute("name") {
            Some("Bus UID") if name_of(parent) == Some("OwnInputBus") => {
                self.buses.insert(value.parse().unwrap_or(0));
            }
            Some("Name") if name_of(parent) == Some("OwnInputBus") => {
                self.bus_names.insert(value.to_string());
            }
            Some("IDString") => {
                if instance(value).is_some() {
                    self.instances.insert(value.to_string());
                } else if name_of(parent) == Some("DeviceAttributes") {
                    self.channel_ids.insert(value.to_string());
                }
            }
            _ => {}
        }
    }

    fn id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 16;
        id.to_string()
    }

    fn runtime(&mut self) -> String {
        let id = self.next_runtime;
        self.next_runtime += 1;
        id.to_string()
    }

    fn bus(&mut self, uid: &str) -> String {
        let uid: i64 = uid.parse().unwrap_or(0);
        if self.buses.insert(uid) {
            return uid.to_string();
        }
        let next = self.next_bus;
        self.next_bus += 1;
        self.buses.insert(next);
        next.to_string()
    }

    fn instance(&mut self, value: &str) -> String {
        if self.instances.insert(value.to_string()) {
            return value.to_string();
        }
        let Some((class, _)) = instance(value) else { return value.to_string() };
        let next = self.next_instance.entry(class.to_string()).or_insert(0);
        let name = format!("{class}-{next}");
        *next += 1;
        self.instances.insert(name.clone());
        name
    }

    /// A 128-bit clip UID, as 32 hex digits, that no other clip anywhere has.
    fn guid(&mut self) -> String {
        self.guids += 1;
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let half = |salt: u8| {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(nanos);
            hasher.write_u64(self.guids);
            hasher.write_u8(salt);
            hasher.finish()
        };
        format!("{:016X}{:016X}", half(1), half(2))
    }
}

/// The name, made unique among `used` the way Cubase does it: "Audio 51", "Audio 51 2".
fn unique(used: &mut HashSet<String>, name: &str) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{name} {n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// One copy of a seed audio track for one file.
struct Copy<'p, 'a, 'i> {
    seed: &'p Seed<'a, 'i>,
    plan: &'p Plan<'a>,
    fresh: &'p mut Fresh,
    file: &'p TakeFile,
    /// What the clip and its event are called: the file's name without ".wav".
    clip: String,
    /// Old ID to new, for every object this copy writes.
    ids: HashMap<&'a str, String>,
    /// Top-level objects to write for this copy, in the order they were first needed.
    pending: VecDeque<Node<'a, 'i>>,
    /// An object written where a reference to it was: the reference's `name`.
    renamed: HashMap<NodeId, &'a str>,
    /// Objects this copy has written, so that one is never written twice.
    written: HashSet<NodeId>,
    /// Buses the copy is routed from and to.
    buses: &'p mut HashSet<String>,
    scale: Option<f64>,
    problem: Option<String>,
}

impl<'p, 'a, 'i> Copy<'p, 'a, 'i> {
    /// New IDs for every object in `node`.
    fn claim(&mut self, node: Node<'a, 'i>) {
        for def in node.descendants().filter(|n| is_def(*n)) {
            let id = def.attribute("ID").unwrap_or_default();
            if !self.ids.contains_key(id) {
                let new = self.fresh.id();
                self.ids.insert(id, new);
            }
        }
    }

    fn fail(&mut self, why: String) {
        self.problem.get_or_insert(why);
    }
}

impl<'p, 'a, 'i: 'a> Policy<'a, 'i> for Copy<'p, 'a, 'i> {
    fn text(&self) -> &'i str {
        self.seed.text
    }

    fn visit(&mut self, node: Node<'a, 'i>) -> Visit {
        let mut visit = Visit::default();
        let tag = node.tag_name().name();
        let name = node.attribute("name");
        let value = node.attribute("value").unwrap_or_default();
        let parent = node.parent_element();
        let grand = parent.and_then(|p| p.parent_element());
        let great = grand.and_then(|g| g.parent_element());
        let set = |visit: &mut Visit, to: String| visit.attrs.push(("value", to));
        let file = self.file;

        if is_def(node) {
            let id = node.attribute("ID").unwrap_or_default();
            let Some(new) = self.ids.get(id).cloned() else {
                self.fail(format!("object {id} was reached without being claimed"));
                return visit;
            };
            let slot = self.renamed.remove(&node.id()).or(name);
            if !self.written.insert(node.id()) {
                // Written once already: from here on it is referred to.
                let slot = slot.map(|s| format!(" name=\"{}\"", escape(s))).unwrap_or_default();
                visit.replace = Some(format!("<obj{slot} ID=\"{new}\"/>"));
                return visit;
            }
            visit.attrs.push(("ID", new));
            if let Some(slot) = slot.filter(|s| Some(*s) != name) {
                visit.attrs.push(("name", slot.to_string()));
            }
            // A track keeps one event: the first.
            if class(Some(node)) == Some("MAudioEvent") && node.prev_sibling_element().is_some_and(|p| class(Some(p)) == Some("MAudioEvent")) {
                visit.drop = true;
            }
            return visit;
        }
        if is_ref(node) {
            let id = node.attribute("ID").unwrap_or_default();
            if let Some(new) = self.ids.get(id) {
                visit.attrs.push(("ID", new.clone()));
            } else if !self.plan.kept.contains(id) {
                let Some(target) = self.seed.defs.get(id).copied() else {
                    self.fail(format!("a reference to {id} finds no object"));
                    return visit;
                };
                if target.parent_element() == Some(self.seed.root) {
                    // An object at the top level: this copy writes its own, there.
                    self.claim(target);
                    self.pending.push_back(target);
                    visit.attrs.push(("ID", self.ids.get(id).cloned().unwrap_or_default()));
                } else {
                    // Written inside something this copy does not write (another seed track, or
                    // another track's clip): the copy writes its own here instead.
                    self.claim(target);
                        if let Some(slot) = name {
                            self.renamed.insert(target.id(), slot);
                        }
                    visit.replace = walk(self, target);
                }
            }
            return visit;
        }

        let pclass = class(parent);
        let pname = name_of(parent);
        match (tag, name) {
            ("int", Some("uniqueID")) if parent.is_some_and(is_def) => {
                let owner = parent.and_then(|p| p.attribute("ID")).unwrap_or_default();
                if let Some(new) = self.ids.get(owner).cloned() {
                    set(&mut visit, new);
                }
            }
            ("int", Some("RuntimeID")) => set(&mut visit, self.fresh.runtime()),
            ("string", Some("Name")) if pclass == Some("MListNode") && pname == Some("Node") && class(grand) == Some("MAudioTrackEvent") => set(&mut visit, file.channel.clone()),
            ("string", Some("String")) if pname == Some("Name") && name_of(grand) == Some("DeviceAttributes") && class(great) == Some("MAudioTrack") => set(&mut visit, file.channel.clone()),
            (_, Some("Start" | "Offset" | "SnapPoint")) if pclass == Some("MAudioEvent") => set(&mut visit, "0".into()),
            (_, Some("Length")) if pclass == Some("MAudioEvent") => set(&mut visit, file.frames.to_string()),
            (_, Some("Description")) if pclass == Some("MAudioEvent") => set(&mut visit, self.clip.clone()),
            ("string", Some("Name")) if pclass == Some("PAudioClip") => set(&mut visit, self.clip.clone()),
            ("float", Some("Origin Time")) if pclass == Some("PAudioClip") => set(&mut visit, "0".into()),
            ("float", Some("Period")) if pname == Some("Domain") && matches!(class(grand), Some("PAudioClip" | "VolumeCurveDataNode")) => set(&mut visit, num(1.0 / file.rate)),
            ("float", Some("Offset")) if pclass == Some("PGridDefinition") => set(&mut visit, "0".into()),
            ("int", Some("SyncPoint")) => set(&mut visit, "0".into()),
            ("item", _) if pname == Some("UID") && class(grand) == Some("PAudioClip") => set(&mut visit, self.fresh.guid()),
            ("string", Some("Name")) if pclass == Some("FNPath") => {
                set(&mut visit, file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }
            ("string", Some("Path")) if pclass == Some("FNPath") => set(&mut visit, folder_of(&file.path)),
            (_, Some("FrameCount")) if pclass == Some("AudioFile") => set(&mut visit, file.frames.to_string()),
            (_, Some("Sample Size")) if pclass == Some("AudioFile") => set(&mut visit, file.format.bits().to_string()),
            (_, Some("Frame Size")) if pclass == Some("AudioFile") => set(&mut visit, file.format.bytes().to_string()),
            (_, Some("Channels")) if pclass == Some("AudioFile") => set(&mut visit, "1".into()),
            (_, Some("Rate")) if pclass == Some("AudioFile") => set(&mut visit, num(file.rate)),
            (_, Some("DataOffset")) if pclass == Some("AudioFile") => set(&mut visit, file.data_offset.to_string()),
            (_, Some("Offset" | "Start")) if parent.is_some_and(|p| p.has_tag_name("item")) && name_of(grand) == Some("Segments") => set(&mut visit, "0".into()),
            (_, Some("Length")) if parent.is_some_and(|p| p.has_tag_name("item")) && name_of(grand) == Some("Segments") => set(&mut visit, file.frames.to_string()),
            ("int", Some("Bus UID")) if pname == Some("OwnInputBus") => set(&mut visit, self.fresh.bus(value)),
            ("string", Some("Name")) if pname == Some("OwnInputBus") => set(&mut visit, unique(&mut self.fresh.bus_names, value)),
            ("string", Some("IDString")) if instance(value).is_some() => set(&mut visit, self.fresh.instance(value)),
            ("string", Some("IDString")) if pname == Some("DeviceAttributes") && class(grand) == Some("MAudioTrack") => set(&mut visit, unique(&mut self.fresh.channel_ids, value)),
            ("int", Some("Value")) if matches!(pname, Some("InputBusValue" | "OutputBusValue")) => {
                self.buses.insert(value.to_string());
            }
            ("float", Some("Length")) if pclass.is_some_and(|c| c.ends_with("TrackEvent")) => {
                if let (Some(scale), Ok(length)) = (self.scale, value.parse::<f64>()) {
                    set(&mut visit, num(length * scale));
                }
            }
            _ => {}
        }
        visit
    }
}

/// The folder a file is in, as `FNPath` holds it: absolute, with a trailing backslash.
fn folder_of(path: &Path) -> String {
    // Windows spells a folder with backslashes, and so does Cubase, whatever the path was given with.
    let folder = path.parent().map(|p| p.display().to_string().replace('/', "\\")).unwrap_or_default();
    if folder.ends_with('\\') {
        folder
    } else {
        format!("{folder}\\")
    }
}

/// The whole archive: the seed as it is, with the copies in place of its audio tracks.
struct Whole<'p, 'a, 'i> {
    seed: &'p Seed<'a, 'i>,
    plan: &'p Plan<'a>,
    first: NodeId,
    /// Each copy, and the IDs the track list names them by.
    tracks: Vec<String>,
    track_ids: Vec<String>,
    /// The copies' top-level objects.
    tops: Vec<String>,
    buses: &'p HashSet<String>,
    file: &'p TakeFile,
    scale: Option<f64>,
}

impl<'p, 'a, 'i: 'a> Policy<'a, 'i> for Whole<'p, 'a, 'i> {
    fn text(&self) -> &'i str {
        self.seed.text
    }

    fn visit(&mut self, node: Node<'a, 'i>) -> Visit {
        let mut visit = Visit::default();
        let text = self.seed.text;
        if node.id() == self.first {
            let gap = indent_before(text, node);
            visit.replace = Some(self.tracks.join(gap));
            return visit;
        }
        if self.plan.dropped.contains(&node.id()) || self.plan.drop_top.contains(&node.id()) {
            visit.drop = true;
            return visit;
        }
        if node == self.seed.root {
            let gap = self.seed.root.children().find(|n| is_def(*n)).map(|n| indent_before(text, n)).unwrap_or("\n   ");
            visit.append = self.tops.iter().map(|top| format!("{gap}{top}")).collect();
            return visit;
        }
        if node == self.seed.track_list {
            let gap = indent_before(text, self.seed.folder);
            visit.append = self.track_ids.iter().map(|id| format!("{gap}<item value=\"{id}\"/>")).collect();
            return visit;
        }
        let parent = node.parent_element();
        let grand = parent.and_then(|p| p.parent_element());
        let name = node.attribute("name");
        let set = |visit: &mut Visit, to: String| visit.attrs.push(("value", to));
        // The routing lists only the buses the archive's tracks still use.
        if node.has_tag_name("item") && name_of(parent) == Some("Bus") && class(grand.and_then(|g| g.parent_element())) == Some("ExternalRouting") {
            if value_of(node, "int", "Bus UID").is_some_and(|uid| !self.buses.contains(uid)) {
                visit.drop = true;
            }
            return visit;
        }
        match (name, class(parent)) {
            (Some("SampleRate"), Some("PArrangeSetup")) => set(&mut visit, num(self.file.rate)),
            (Some("SampleSize"), Some("PArrangeSetup")) => set(&mut visit, self.file.format.bits().to_string()),
            (Some("SampleFormatSize"), Some("PArrangeSetup")) => set(&mut visit, self.file.format.bytes().to_string()),
            (Some("Time"), _) if name_of(parent) == Some("Length") && class(grand) == Some("PArrangeSetup") => self.scaled(node, &mut visit),
            (Some("Length"), Some(owner)) if node.has_tag_name("float") && (owner == "MFolderTrack" || owner.ends_with("TrackEvent")) => self.scaled(node, &mut visit),
            _ => {}
        }
        visit
    }
}

impl Whole<'_, '_, '_> {
    fn scaled(&self, node: Node, visit: &mut Visit) {
        if let (Some(scale), Some(length)) = (self.scale, node.attribute("value").and_then(|v| v.parse::<f64>().ok())) {
            visit.attrs.push(("value", num(length * scale)));
        }
    }
}

/// Everything, put together and read back.
fn assemble(seed: &Seed, plan: &Plan, files: &[TakeFile]) -> Result<String, String> {
    let Some(first) = seed.audio.first() else { return Err("the folder has no audio tracks".into()) };
    let longest = files.iter().map(|f| f.frames as f64 / f.rate.max(1.0)).fold(0.0, f64::max);
    // Past the end of the seed's project, the archive's project and its tracks are made longer by the
    // same factor, so every length stays in step whatever its unit (seconds, or ticks at its tempo).
    let scale = seed.seconds().filter(|&seconds| seconds > 0.0 && longest > seconds).map(|seconds| (longest + 1.0) / seconds);
    let mut fresh = Fresh::new(seed, plan);
    let mut buses: HashSet<String> = HashSet::new();
    for node in seed.root.descendants().filter(|n| n.attribute("name") == Some("Value") && matches!(name_of(n.parent_element()), Some("InputBusValue" | "OutputBusValue"))) {
        if !node.ancestors().any(|a| plan.dropped.contains(&a.id())) {
            buses.insert(node.attribute("value").unwrap_or_default().to_string());
        }
    }
    let mut tracks = Vec::with_capacity(files.len());
    let mut track_ids = Vec::with_capacity(files.len());
    let mut tops = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let template = seed.templates[index.min(seed.templates.len() - 1)];
        let clip = file.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| file.channel.clone());
        let mut copy = Copy {
            seed,
            plan,
            fresh: &mut fresh,
            file,
            clip,
            ids: HashMap::new(),
            pending: VecDeque::new(),
            renamed: HashMap::new(),
            written: HashSet::new(),
            buses: &mut buses,
            scale,
            problem: None,
        };
        copy.claim(template);
        let track = walk(&mut copy, template).unwrap_or_default();
        while let Some(top) = copy.pending.pop_front() {
            if let Some(written) = walk(&mut copy, top) {
                tops.push(written);
            }
        }
        if let Some(why) = copy.problem {
            return Err(format!("the seed's track \"{}\" could not be copied: {why}", track_name(template)));
        }
        track_ids.push(copy.ids.get(template.attribute("ID").unwrap_or_default()).cloned().unwrap_or_default());
        tracks.push(track);
    }
    let mut whole = Whole { seed, plan, first: first.id(), tracks, track_ids, tops, buses: &buses, file: &files[0], scale };
    let root = seed.root.range();
    let body = walk(&mut whole, seed.root).unwrap_or_default();
    let out = format!("{}{body}{}", &seed.text[..root.start], &seed.text[root.end..]);
    check_archive(&out).map_err(|why| format!("the archive Gazelle made from the seed does not hold together ({why})"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! The seed here is a real Cubase 15.0.20 export of a "folder with group" named Recorded, cut
    //! down to its group and two of its mono tracks, with the recordings' folder replaced by
    //! `C:\Takes\Seed\`.

    use super::*;

    const SEED: &str = include_str!("../testdata/cubase-seed.xml");

    fn file(channel: &str, name: &str, frames: u64) -> TakeFile {
        TakeFile { channel: channel.into(), path: PathBuf::from(format!(r"C:\Takes\2026-09-30\{name}.wav")), frames, rate: 96_000.0, format: SampleFormat::Int24, data_offset: 700 }
    }

    fn three() -> Vec<TakeFile> {
        vec![
            file("Kick (Quadro 1)", "2026-09-30 T001 Kick (Quadro 1)", 480_000),
            file("Snare & Top", "2026-09-30 T001 Snare & Top", 480_000),
            file("Vocal", "2026-09-30 T001 Vocal", 480_000),
        ]
    }

    /// The elements of `xml` with this tag and `name`, as (their `value`, the `class` of their parent).
    fn values(doc: &Document, tag: &str, name: &str) -> Vec<(String, Option<String>)> {
        doc.descendants()
            .filter(|n| n.has_tag_name(tag) && n.attribute("name") == Some(name))
            .map(|n| (n.attribute("value").unwrap_or_default().to_string(), n.parent_element().and_then(|p| p.attribute("class")).map(str::to_string)))
            .collect()
    }

    fn folder_tracks<'a, 'i>(doc: &'a Document<'i>) -> Vec<Node<'a, 'i>> {
        let folder = doc.descendants().find(|n| class(Some(*n)) == Some("MFolderTrack")).unwrap();
        let tracks = child(child(folder, "obj", "Node").unwrap(), "list", "Tracks").unwrap();
        tracks.children().filter(|n| n.is_element()).collect()
    }

    #[test]
    fn the_seed_is_a_folder_with_its_group_and_two_mono_tracks() {
        let summary = check_seed(SEED).unwrap();
        assert_eq!(summary, SeedSummary { folder: "Recorded".into(), group: "Recorded".into(), tracks: 2, rate: Some(96_000.0) });
        assert_eq!(summary.words(), "The folder \"Recorded\" with its group \"Recorded\", and 2 mono tracks to copy, 96 kHz.");
        let facts = check_archive(SEED).unwrap();
        assert_eq!(facts.listed, 2, "the seed itself holds together");
    }

    #[test]
    fn every_object_is_new_and_every_reference_finds_one() {
        let out = build(SEED, &three()).unwrap();
        let facts = check_archive(&out).unwrap();
        assert_eq!(facts.listed, 3, "the track list names the three new tracks");
        let seed = Document::parse(SEED).unwrap();
        let doc = Document::parse(&out).unwrap();
        let ids = |doc: &Document| -> HashSet<String> { doc.descendants().filter(|n| is_def(*n)).map(|n| n.attribute("ID").unwrap().to_string()).collect() };
        let (before, after) = (ids(&seed), ids(&doc));
        // The tracks' own objects are all new; what is shared keeps its ID.
        for track in folder_tracks(&doc).iter().filter(|n| class(Some(**n)) == Some("MAudioTrackEvent")) {
            for def in track.descendants().filter(|n| is_def(*n)) {
                assert!(!before.contains(def.attribute("ID").unwrap()), "{:?} kept a seed ID", def.attribute("class"));
            }
        }
        for kept in ["584420176", "551688480", "554207040", "551683552", "1381031584"] {
            assert!(after.contains(kept), "{kept}: the tempo, the signature, the folder, its group and the setup keep their IDs");
        }
        let runtime: Vec<_> = values(&doc, "int", "RuntimeID").into_iter().map(|(v, _)| v).collect();
        assert_eq!(runtime.iter().collect::<HashSet<_>>().len(), runtime.len(), "no two runtime IDs alike");
        let unique_ids: Vec<_> = values(&doc, "int", "uniqueID").into_iter().map(|(v, _)| v).collect();
        assert_eq!(unique_ids.iter().collect::<HashSet<_>>().len(), unique_ids.len());
        let uids: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("list") && n.attribute("name") == Some("UID")).flat_map(|l| l.children().filter_map(|i| i.attribute("value"))).collect();
        assert_eq!(uids.len(), 3);
        assert_eq!(uids.iter().collect::<HashSet<_>>().len(), 3, "each clip its own UID");
        assert!(uids.iter().all(|u| u.len() == 32 && u.bytes().all(|b| b.is_ascii_hexdigit())));
    }

    #[test]
    fn the_tracks_are_the_channels_routed_to_the_group() {
        let out = build(SEED, &three()).unwrap();
        let doc = Document::parse(&out).unwrap();
        let tracks = folder_tracks(&doc);
        assert_eq!(tracks.len(), 4, "the group, then one track per file");
        assert_eq!(class(Some(tracks[0])), Some("MDeviceTrackEvent"));
        let names: Vec<String> = tracks[1..].iter().map(|t| track_name(*t)).collect();
        assert_eq!(names, ["Kick (Quadro 1)", "Snare & Top", "Vocal"]);
        assert!(out.contains("value=\"Snare &amp; Top\""), "escaped as XML wants");
        let channels: Vec<_> = values(&doc, "string", "String").into_iter().map(|(v, _)| v).filter(|v| !v.is_empty()).collect();
        assert!(["Kick (Quadro 1)", "Snare & Top", "Vocal"].iter().all(|n| channels.iter().any(|c| c == n)), "the mixer channels are named too: {channels:?}");
        // Every track is routed to the group's own bus, which the routing still lists.
        let group_bus = value_of(child(child(child(tracks[0], "obj", "Track Device").unwrap(), "member", "DeviceAttributes").unwrap(), "member", "OwnInputBus").unwrap(), "int", "Bus UID").unwrap();
        for track in &tracks[1..] {
            let attrs = child(child(*track, "obj", "Track Device").unwrap(), "member", "DeviceAttributes").unwrap();
            assert_eq!(value_of(child(attrs, "member", "OutputBusValue").unwrap(), "int", "Value"), Some(group_bus));
        }
        let routed: Vec<_> = values(&doc, "int", "Bus UID").into_iter().map(|(v, _)| v).collect();
        let own: Vec<_> = tracks.iter().map(|t| value_of(child(child(child(*t, "obj", "Track Device").unwrap(), "member", "DeviceAttributes").unwrap(), "member", "OwnInputBus").unwrap(), "int", "Bus UID").unwrap()).collect();
        assert_eq!(own.iter().collect::<HashSet<_>>().len(), 4, "each channel its own bus: {own:?}");
        assert!(routed.iter().any(|v| v == group_bus));
        let channel_ids: Vec<_> = tracks[1..]
            .iter()
            .map(|t| value_of(child(child(*t, "obj", "Track Device").unwrap(), "member", "DeviceAttributes").unwrap(), "string", "IDString").unwrap())
            .collect();
        assert_eq!(channel_ids, ["Audio 51 2", "Audio 52 2", "Audio 52 2 2"], "the third is the second again, told apart as Cubase would");
    }

    #[test]
    fn each_clip_is_its_file_where_it_is_from_the_project_start() {
        let mut files = three();
        files[2].frames = 123_456;
        let out = build(SEED, &files).unwrap();
        let doc = Document::parse(&out).unwrap();
        let paths: Vec<_> = doc.descendants().filter(|n| class(Some(*n)) == Some("FNPath")).map(|p| (value_of(p, "string", "Name").unwrap(), value_of(p, "string", "Path").unwrap())).collect();
        assert_eq!(
            paths,
            [
                ("2026-09-30 T001 Kick (Quadro 1).wav", r"C:\Takes\2026-09-30\"),
                ("2026-09-30 T001 Snare & Top.wav", r"C:\Takes\2026-09-30\"),
                ("2026-09-30 T001 Vocal.wav", r"C:\Takes\2026-09-30\")
            ]
        );
        assert!(!out.contains(r"C:\Takes\Seed\"), "nothing of the seed's own recordings is left");
        let events: Vec<Node> = doc.descendants().filter(|n| class(Some(*n)) == Some("MAudioEvent")).collect();
        assert_eq!(events.len(), 3);
        for (event, file) in events.iter().zip(&files) {
            assert_eq!(value_of(*event, "float", "Start"), Some("0"));
            assert_eq!(value_of(*event, "float", "Offset"), Some("0"));
            assert_eq!(value_of(*event, "float", "Length"), Some(file.frames.to_string().as_str()));
            assert_eq!(value_of(*event, "string", "Description"), file.path.file_stem().and_then(|s| s.to_str()));
        }
        let audio: Vec<Node> = doc.descendants().filter(|n| class(Some(*n)) == Some("AudioFile")).collect();
        for (one, file) in audio.iter().zip(&files) {
            assert_eq!(value_of(*one, "int", "FrameCount"), Some(file.frames.to_string().as_str()));
            assert_eq!(value_of(*one, "float", "Rate"), Some("96000"));
            assert_eq!(value_of(*one, "int", "Sample Size"), Some("24"));
            assert_eq!(value_of(*one, "int", "DataOffset"), Some("700"));
        }
        let segments: Vec<_> = values(&doc, "int", "Length").into_iter().filter(|(_, c)| c.is_none()).map(|(v, _)| v).collect();
        assert_eq!(segments, ["480000", "480000", "123456"], "each segment the whole file");
        assert!(values(&doc, "float", "Origin Time").iter().all(|(v, _)| v == "0"));
        assert!(values(&doc, "int", "SyncPoint").iter().all(|(v, _)| v == "0"));
    }

    #[test]
    fn what_gazelle_does_not_change_is_copied_byte_for_byte() {
        let out = build(SEED, &three()).unwrap();
        let seed = Document::parse(SEED).unwrap();
        // The group channel, the tempo and signature tracks, and the head of the file, whole.
        let group = seed.descendants().find(|n| class(Some(*n)) == Some("MDeviceTrackEvent")).unwrap();
        for node in [group, seed.descendants().find(|n| class(Some(*n)) == Some("MTempoTrackEvent")).unwrap(), seed.descendants().find(|n| class(Some(*n)) == Some("MSignatureTrackEvent")).unwrap()] {
            assert!(out.contains(&SEED[node.range()]), "{:?} is not there as it was", node.attribute("class"));
        }
        assert!(out.starts_with(&SEED[..SEED.find("<obj class=\"MFolderTrack\"").unwrap()]));
        // A track's inserts and EQ, which Gazelle knows nothing of, come through as they were.
        let track = seed.descendants().find(|n| class(Some(*n)) == Some("MAudioTrack")).unwrap();
        let eq = child(child(track, "member", "DeviceAttributes").unwrap(), "member", "EQ").unwrap();
        let eq = &SEED[eq.range()];
        assert_eq!(out.matches(eq).count(), SEED.matches(eq).count() - 2 + 3, "every copy has the seed track's EQ, and the group keeps its own");
        let bins = seed.descendants().filter(|n| n.has_tag_name("bin") && n.ancestors().any(|a| a == track)).count();
        assert!(bins > 0);
    }

    #[test]
    fn the_seeds_own_tracks_and_their_objects_are_left_out() {
        let out = build(SEED, &three()[..1]).unwrap();
        let doc = Document::parse(&out).unwrap();
        assert_eq!(folder_tracks(&doc).len(), 2, "the group and the one new track");
        assert_eq!(doc.descendants().filter(|n| class(Some(*n)) == Some("PAudioClip")).count(), 1);
        assert_eq!(doc.descendants().filter(|n| class(Some(*n)) == Some("MAutomationTrack")).count(), 2, "the group's and the new track's");
        let routing: Vec<_> = doc.descendants().filter(|n| class(Some(*n)) == Some("ExternalRouting")).flat_map(|r| r.descendants().filter(|n| n.attribute("name") == Some("Bus UID"))).map(|n| n.attribute("value").unwrap().to_string()).collect();
        assert_eq!(routing, ["37", "98"], "the first track's input and the group; the second's input is no longer used");
        assert!(!out.contains("Audio 02"), "nothing of the second seed track");
    }

    #[test]
    fn an_object_the_second_seed_track_shares_with_the_first_is_written_where_it_is_used() {
        // The second seed track's clip refers to a stretch preset written inside the first's.
        let files = three();
        let out = build(SEED, &files[..2]).unwrap();
        let doc = Document::parse(&out).unwrap();
        let presets: Vec<Node> = doc.descendants().filter(|n| class(Some(*n)) == Some("ElastiquePreset")).collect();
        assert_eq!(presets.len(), 2, "each copy has its own");
        assert!(presets.iter().all(|p| p.attribute("name") == Some("StretchPreset")));
        check_archive(&out).unwrap();
    }

    #[test]
    fn a_take_longer_than_the_seeds_project_makes_it_longer() {
        let seed = Document::parse(SEED).unwrap();
        let seconds: f64 = seed.descendants().find(|n| n.attribute("name") == Some("Time")).unwrap().attribute("value").unwrap().parse().unwrap();
        let mut files = three();
        files[0].frames = ((seconds + 99.0) * 96_000.0) as u64;
        let out = build(SEED, &files).unwrap();
        let doc = Document::parse(&out).unwrap();
        let time: f64 = doc.descendants().find(|n| n.attribute("name") == Some("Time")).unwrap().attribute("value").unwrap().parse().unwrap();
        assert!((time - (seconds + 100.0)).abs() < 1e-3, "{time}");
        let folder: f64 = value_of(doc.descendants().find(|n| class(Some(*n)) == Some("MFolderTrack")).unwrap(), "float", "Length").unwrap().parse().unwrap();
        assert!((folder - time).abs() < 1e-6, "the folder's length is in seconds too");
        let short = build(SEED, &three()).unwrap();
        assert!(short.contains(&format!("value=\"{}\"", seed.descendants().find(|n| n.attribute("name") == Some("Time")).unwrap().attribute("value").unwrap())), "a shorter take leaves it");
    }

    #[test]
    fn line_endings_and_the_rate_and_format_follow_the_seed_and_the_take() {
        let crlf = SEED.replace('\n', "\r\n");
        let mut files = three();
        for f in &mut files {
            f.rate = 48_000.0;
            f.format = SampleFormat::Float32;
        }
        let out = build(&crlf, &files).unwrap();
        assert!(!out.replace("\r\n", "").contains('\n'), "every line ends as the seed's do");
        let doc = Document::parse(&out).unwrap();
        let setup = doc.descendants().find(|n| class(Some(*n)) == Some("PArrangeSetup")).unwrap();
        assert_eq!(value_of(setup, "float", "SampleRate"), Some("48000"));
        assert_eq!(value_of(setup, "int", "SampleSize"), Some("32"));
        assert_eq!(value_of(setup, "int", "SampleFormatSize"), Some("4"));
        let periods: Vec<_> = values(&doc, "float", "Period").into_iter().filter(|(_, c)| c.is_none()).map(|(v, _)| v).collect();
        assert!(periods.iter().filter(|p| p.as_str() != "1").all(|p| p.parse::<f64>().unwrap() == 1.0 / 48_000.0), "{periods:?}");
    }

    #[test]
    fn a_seed_that_will_not_do_says_why() {
        let problem = |text: &str| check_seed(text).unwrap_err();
        assert!(problem("not xml").contains("not XML"));
        assert!(problem("<?xml version=\"1.0\"?><other/>").contains("not a Cubase track archive"));
        assert!(problem("<tracklist2><list name=\"track\" type=\"obj\"/></tracklist2>").contains("no folder track"));
        // Both tracks' own buses made stereo: nothing mono to copy.
        let mut text = SEED.to_string();
        let seed = Document::parse(SEED).unwrap();
        let mut spans: Vec<_> = seed
            .descendants()
            .filter(|n| n.attribute("name") == Some("OwnInputBus") && class(n.parent_element().and_then(|p| p.parent_element())) == Some("MAudioTrack"))
            .map(|bus| {
                let types = child(child(bus, "member", "Input Arrangement").unwrap(), "list", "Type").unwrap();
                types.children().find(|n| n.is_element()).unwrap().attribute_node("value").unwrap().range_value()
            })
            .collect();
        assert_eq!(spans.len(), 2);
        spans.sort_by_key(|s| std::cmp::Reverse(s.start));
        for span in spans {
            text.replace_range(span, "1");
        }
        assert!(problem(&text).contains("is not mono"), "{}", problem(&text));
        // A folder without its own group.
        let plain = SEED.replace("<obj name=\"GroupTrackEvent\"", "<obj name=\"NotTheGroup\"");
        assert!(problem(&plain).contains("no group channel"), "{}", problem(&plain));
    }

    #[test]
    fn the_check_catches_what_does_not_hold_together() {
        assert!(check_archive("<tracklist2><obj class=\"A\" ID=\"1\"/><obj class=\"B\" ID=\"1\"/></tracklist2>").unwrap_err().contains("two objects"));
        assert!(check_archive("<tracklist2><obj name=\"X\" ID=\"2\"/></tracklist2>").unwrap_err().contains("finds no object"));
        assert!(check_archive("<tracklist2><list name=\"track\" type=\"obj\"><item value=\"3\"/></list></tracklist2>").unwrap_err().contains("names 3"));
        assert!(build(SEED, &[]).is_err());
    }
}
