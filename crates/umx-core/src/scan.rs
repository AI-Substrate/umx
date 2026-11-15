//! The walk behind `GraphFiles::find_graphs` (ws002 s17, D66), for `umx graph scan`: pure, so every adapter walks
//! with the same rules (what is skipped, the caps, nested repositories) and only supplies the I/O (architect, s17).

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::GraphFile;

/// The bounds of a [`crate::ports::GraphFiles::find_graphs`] walk (security, s17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindLimits {
    /// How many folders down from the root the walk looks; the root is 0.
    pub depth: usize,
    /// At most this many folders listed.
    pub folders: usize,
    /// At most this long, in milliseconds.
    pub time_ms: u64,
}

impl FindLimits {
    pub const DEPTH: usize = 8;
    pub const MAX_DEPTH: usize = 32;
    pub const FOLDERS: usize = 100_000;
    pub const TIME_MS: u64 = 10_000;

    /// The default bounds, `depth` folders down (at most [`Self::MAX_DEPTH`]).
    pub fn with_depth(depth: usize) -> FindLimits {
        FindLimits {
            depth: depth.min(Self::MAX_DEPTH),
            ..FindLimits::default()
        }
    }
}

impl Default for FindLimits {
    fn default() -> FindLimits {
        FindLimits {
            depth: Self::DEPTH,
            folders: Self::FOLDERS,
            time_ms: Self::TIME_MS,
        }
    }
}

/// A walk reads a found folder's `graph.json` up to this many bytes (security, s17).
pub const FOUND_GRAPH_JSON_CAP: u64 = 64 * 1024;

/// Folders a walk never enters, besides every folder whose name starts with `.` (security, s17).
pub const SKIPPED_FOLDERS: &[&str] = &[".git", ".umx-tmp", "node_modules", "target"];

/// Whether a walk passes over a folder named `name`.
pub fn skipped_folder(name: &str) -> bool {
    name.starts_with('.') || SKIPPED_FOLDERS.contains(&name)
}

/// One graph folder a walk found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoundGraph {
    /// Relative to the walk's root, `/` between parts; `.` for the root itself.
    pub path: String,
    /// Its `graph.json`, or None when it could not be read whole (a link, too large, unreadable) or is not there
    /// yet (a creation cut short: only the marker and temp files, F176).
    pub graph_json: Option<Vec<u8>>,
    /// The folder holds a write set's marker ([`EntryKind::Pending`]): a write was cut short and only a load
    /// recovers it, so `graph_json` may be the old or the new one. Nothing that looks at the folder repairs it
    /// (graph-storage-expert, s17).
    pub pending: bool,
}

/// What [`crate::ports::GraphFiles::find_graphs`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Sorted by path.
    pub graphs: Vec<FoundGraph>,
    /// Folders under the root that are git repository tops of their own (they hold `.git`), not entered; sorted.
    pub nested_repos: Vec<String>,
    /// A limit stopped the walk (or left a folder below the depth unseen): there may be more.
    pub truncated: bool,
    /// Folders the walk reached but could not list (no permission, or changed while listed), passed over: what
    /// they hold is not known (F177).
    pub unlisted: usize,
}

/// What a folder's entry is, as a walk sees it: never followed, so a symlink is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    File,
    Other,
    /// A write set's marker: the adapter's sign that a set was cut short and recovery has not run.
    Pending,
}

/// The graph in a folder with these `entries`, as a walk and [`crate::ports::GraphFiles::peek_graph`] both report
/// it, or None when the folder holds neither a `graph.json` file nor a write set's marker. A marker alone is a
/// graph whose creation was cut short before `graph.json` was renamed in: pending, with no `graph.json` (F176).
/// `graph_json` reads it (capped, no link, no recovery), and is called only when the file is there.
pub fn graph_in(
    path: String,
    entries: &[(String, EntryKind)],
    graph_json: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<FoundGraph> {
    let has = |k: EntryKind, name: Option<&str>| {
        entries
            .iter()
            .any(|(n, kind)| *kind == k && name.is_none_or(|w| n == w))
    };
    let json = has(EntryKind::File, Some(GraphFile::Graph.file_name()));
    let pending = has(EntryKind::Pending, None);
    (json || pending).then(|| FoundGraph {
        path,
        graph_json: if json { graph_json() } else { None },
        pending,
    })
}

/// The walk behind every adapter's [`crate::ports::GraphFiles::find_graphs`], breadth first from the root. `list(rel)` gives the
/// entries of the folder at `rel` (`""` for the root) or None when it cannot be listed (it is passed over and
/// counted in [`Found::unlisted`]);
/// `graph_json(rel)` reads that folder's `graph.json` as [`FoundGraph::graph_json`]; `elapsed_ms()` is the time
/// since the walk began.
pub fn walk_for_graphs(
    limits: FindLimits,
    mut list: impl FnMut(&str) -> Option<Vec<(String, EntryKind)>>,
    mut graph_json: impl FnMut(&str) -> Option<Vec<u8>>,
    elapsed_ms: impl Fn() -> u64,
) -> Found {
    let mut found = Found::default();
    let mut queue = alloc::collections::VecDeque::from([(String::new(), 0usize)]);
    let mut listed = 0usize;
    while let Some((rel, depth)) = queue.pop_front() {
        if listed >= limits.folders || elapsed_ms() >= limits.time_ms {
            found.truncated = true;
            break;
        }
        listed += 1;
        let Some(entries) = list(&rel) else {
            found.unlisted += 1;
            continue;
        };
        if depth > 0 && entries.iter().any(|(n, _)| n == ".git") {
            found.nested_repos.push(rel);
            continue;
        }
        let path = if rel.is_empty() {
            ".".into()
        } else {
            rel.clone()
        };
        if let Some(graph) = graph_in(path, &entries, || graph_json(&rel)) {
            found.graphs.push(graph);
            continue;
        }
        let mut folders: Vec<&String> = entries
            .iter()
            .filter(|(n, k)| *k == EntryKind::Folder && !skipped_folder(n))
            .map(|(n, _)| n)
            .collect();
        folders.sort();
        for name in folders {
            if depth + 1 > limits.depth {
                found.truncated = true;
                break;
            }
            let child = if rel.is_empty() {
                name.clone()
            } else {
                alloc::format!("{rel}/{name}")
            };
            queue.push_back((child, depth + 1));
        }
    }
    found.graphs.sort_by(|a, b| a.path.cmp(&b.path));
    found.nested_repos.sort();
    found
}
