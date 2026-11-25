//! Source roots in evidence (req-47, ws006 S1): `root:<name>/<path>` names a folder the machine config sets, and
//! `root:repo/<path>` the git top level holding the graph folder. Security ruling ws006 S1: every write re-checks
//! the root through the SourceReader port (canonical, inside the roots) and that the path stays inside it. The
//! pointer is stored as written.
//!
//! Source hashes (usage 220): a write that adds a pointer or a locator hashes its source when it is a local file,
//! through the same port and the same confinement (a path is under the graph folder or its `root:`), and stamps
//! `hash_state`. A URL or `umx:` address is `remote`, never fetched. One check hashes each file once, however many
//! pointers cite it. Only the digest is kept, never the bytes.

use super::problems;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::RefCell;
use umx_core::config::REPO_ROOT;
use umx_core::evidence::{climb, root_source};
use umx_core::locator::{Locator, LocatorKind};
use umx_core::ports::{GraphDir, SourceError, SourceReader};
use umx_core::source_hash::{HashState, SourceDigest, Transcript, pointer_state, stamp_pointer};
use umx_core::{Problem, ValidName, Value};

/// Where named source roots are, from the machine config only (a repo config cannot add one). Not a port: the
/// config service implements it over its own ports, and compose wires it in.
pub trait SourceRoots: Send + Sync {
    /// The folder `source_roots.<name>` names, as written; None when it is not set or not valid.
    fn root_folder(&self, name: &str) -> Option<GraphDir>;
    /// The git top level holding `dir`, looked for only inside the roots; None when there is none.
    fn repo_top(&self, dir: &GraphDir) -> Option<GraphDir>;
}

/// What an evidence write checks `root:` sources with.
#[derive(Clone)]
pub(crate) struct SourceCheck {
    pub(crate) reader: Arc<dyn SourceReader>,
    pub(crate) roots: Option<Arc<dyn SourceRoots>>,
    /// The graph roots, named when a source root is outside them.
    pub(crate) graph_roots: Vec<GraphDir>,
    /// The graph written, named in a warning's next.
    pub(crate) graph: ValidName,
    /// Each file read by this write, by canonical root, path and transcript format.
    pub(crate) read: Rc<RefCell<Reads>>,
}

type Read = Result<SourceDigest, SourceError>;
/// What one check read, by canonical root, path and transcript format.
pub(crate) type Reads = BTreeMap<(String, String, Option<u8>, Option<u64>), Read>;

/// Where a local source is, for the port: its canonical root and the path under it.
pub(crate) enum Place {
    Local(GraphDir, String),
    Remote,
    /// Its `root:` is not usable on this machine (unknown, or not a folder): nothing was read.
    RootUnavailable,
    /// A `..` path that climbs out of the graph's git repo: never read.
    Outside,
}

/// Why a `..` path has no place in the graph's repo.
enum Climb {
    /// The graph is not in a git repo inside the roots (or it is not usable here): `..` is not allowed.
    NoRepo,
    /// It climbs past the repo's top.
    Outside,
}

/// A source that names something on the network or in a graph: never read.
fn remote(source: &str) -> bool {
    let lower = |n: usize| source.get(..n).map(str::to_ascii_lowercase);
    lower(4).as_deref() == Some("umx:")
        || lower(7).as_deref() == Some("http://")
        || lower(8).as_deref() == Some("https://")
}

impl SourceCheck {
    /// Refuses the first `root:` source whose root is unknown or unusable, or whose path leads out of its root.
    /// `dir` is the graph's folder. Other sources were checked when they were parsed.
    pub(crate) fn check(&self, dir: &GraphDir, pointers: &[Value]) -> Result<(), Problem> {
        for p in pointers {
            let Some(source) = p.get("source").and_then(Value::as_str) else {
                continue;
            };
            // Usage 223: a relative path that climbs stays inside the graph's repo, symlinks resolved.
            if let Some((ups, rest)) = climb(source).filter(|(ups, _)| *ups > 0) {
                let (root, path) = self
                    .climb(dir, ups, rest)
                    .map_err(|_| problems::evidence_outside_roots())?;
                self.reader
                    .check_path(&root, &path)
                    .map_err(|_| problems::evidence_outside_roots())?;
                continue;
            }
            let Some((name, path)) = root_source(source) else {
                continue;
            };
            let root = self.root(name, dir)?;
            self.reader.check_path(&root, path).map_err(|e| match e {
                SourceError::OutsideRoots => problems::evidence_outside_roots(),
                SourceError::NotFolder
                | SourceError::Missing
                | SourceError::NotFile
                | SourceError::TooLarge
                | SourceError::Io(_) => problems::root_unavailable(name, problems::ROOT_UNREADABLE),
            })?;
        }
        Ok(())
    }

    /// Where a relative path with `ups` leading `..` parts lands (usage 223): under the canonical top of the git repo
    /// holding the graph folder (as `root:repo` finds it), as a path with no `..`, or why it has no place there.
    fn climb(&self, dir: &GraphDir, ups: usize, rest: &str) -> Result<(GraphDir, String), Climb> {
        let top = self
            .roots
            .as_deref()
            .and_then(|r| r.repo_top(dir))
            .ok_or(Climb::NoRepo)?;
        let top = self
            .reader
            .canonical_root(&top)
            .map_err(|_| Climb::NoRepo)?;
        let here = self.reader.canonical_root(dir).map_err(|_| Climb::NoRepo)?;
        let inner = match here.as_str().strip_prefix(top.as_str()) {
            Some(inner) if inner.is_empty() || inner.starts_with('/') => inner,
            _ => return Err(Climb::NoRepo),
        };
        let mut parts: Vec<&str> = inner.split('/').filter(|p| !p.is_empty()).collect();
        if ups > parts.len() {
            return Err(Climb::Outside);
        }
        parts.truncate(parts.len() - ups);
        parts.extend(rest.split('/').filter(|p| !p.is_empty() && *p != "."));
        if parts.is_empty() {
            return Err(Climb::Outside);
        }
        Ok((top, parts.join("/")))
    }

    /// The canonical folder of a root, checked inside the roots now.
    fn root(&self, name: &str, dir: &GraphDir) -> Result<GraphDir, Problem> {
        let roots = self.roots.as_deref();
        if name == REPO_ROOT {
            let no_repo = || problems::root_unavailable(name, problems::ROOT_NO_REPO);
            let top = roots.and_then(|r| r.repo_top(dir)).ok_or_else(no_repo)?;
            return self.reader.canonical_root(&top).map_err(|_| no_repo());
        }
        let folder = roots
            .and_then(|r| r.root_folder(name))
            .ok_or_else(|| problems::root_unknown(name))?;
        self.reader.canonical_root(&folder).map_err(|e| match e {
            SourceError::OutsideRoots => problems::path_outside_roots(&folder, &self.graph_roots),
            SourceError::NotFolder
            | SourceError::Missing
            | SourceError::NotFile
            | SourceError::TooLarge => problems::root_unavailable(name, problems::ROOT_NOT_FOLDER),
            SourceError::Io(_) => problems::root_unavailable(name, problems::ROOT_UNREADABLE),
        })
    }
}

impl SourceCheck {
    /// Where `source` is, for the graph in `dir`: `root:<name>/<path>` under its root; a path relative to the graph
    /// folder, or an absolute one inside it, under the graph folder (as evidence and locators resolve them).
    pub(crate) fn place(&self, dir: &GraphDir, source: &str) -> Place {
        if remote(source) {
            return Place::Remote;
        }
        if let Some((name, path)) = root_source(source) {
            return match self.root(name, dir) {
                Ok(root) => Place::Local(root, path.to_string()),
                Err(_) => Place::RootUnavailable,
            };
        }
        // A hand-edited file may hold what a write refuses: an absolute path never climbs (as on write).
        if source.starts_with('/') && source.split('/').any(|p| p == "..") {
            return Place::Outside;
        }
        if let Some((ups, rest)) = climb(source).filter(|(ups, _)| *ups > 0) {
            return match self.climb(dir, ups, rest) {
                Ok((root, path)) => Place::Local(root, path),
                Err(Climb::NoRepo) => Place::RootUnavailable,
                Err(Climb::Outside) => Place::Outside,
            };
        }
        let path = match source.strip_prefix(dir.as_str()) {
            Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/'),
            _ => source,
        };
        match self.reader.canonical_root(dir) {
            Ok(root) => Place::Local(root, path.to_string()),
            Err(_) => Place::RootUnavailable,
        }
    }

    /// Reads `source` (once per check and `prefix`, however often it is asked) and says what it found; None when its
    /// root is not usable on this machine, so nothing was read. `prefix` also hashes the first that many bytes.
    pub(crate) fn read(
        &self,
        dir: &GraphDir,
        source: &str,
        prefix: Option<u64>,
    ) -> Option<(HashState, Option<SourceDigest>)> {
        let (root, path) = match self.place(dir, source) {
            Place::Local(root, path) => (root, path),
            Place::Remote => return Some((HashState::Remote, None)),
            Place::Outside => return Some((HashState::Outside, None)),
            Place::RootUnavailable => return None,
        };
        let transcript = Transcript::of(source);
        let key = (
            root.as_str().to_string(),
            path.clone(),
            transcript.map(|t| t as u8),
            prefix,
        );
        let read = self.read.borrow().get(&key).cloned();
        let read = match read {
            Some(r) => r,
            None => {
                let r = self.reader.digest(&root, &path, transcript, prefix);
                self.read.borrow_mut().insert(key, r.clone());
                r
            }
        };
        Some(match read {
            Ok(d) => (HashState::Hashed, Some(d)),
            Err(SourceError::Missing) => (HashState::Missing, None),
            Err(SourceError::OutsideRoots) => (HashState::Outside, None),
            Err(SourceError::TooLarge) => (HashState::TooLarge, None),
            Err(SourceError::NotFile | SourceError::NotFolder | SourceError::Io(_)) => {
                (HashState::Unreadable, None)
            }
        })
    }

    /// Stamps each new pointer's hash keys from what its source holds now.
    pub(crate) fn hash(&self, dir: &GraphDir, pointers: &mut [Value]) {
        for p in pointers {
            let Some(source) = p.get("source").and_then(Value::as_str).map(String::from) else {
                continue;
            };
            let (state, digest) = self
                .read(dir, &source, None)
                .unwrap_or((HashState::Unreadable, None));
            stamp_pointer(p, state, digest.as_ref(), false);
        }
    }

    /// Sets a new file locator's hash, length and seq_max from what its source holds now; a URL is `remote`, and a
    /// folder is not hashed.
    pub(crate) fn hash_locator(&self, dir: &GraphDir, locator: &mut Locator) {
        let read = self.read_locator(dir, locator);
        stamp_locator(
            locator,
            read.unwrap_or((HashState::Unreadable, None)),
            false,
        );
    }

    /// A refresh of a stored locator: false, and the locator untouched, when its `root:` is not usable here.
    pub(crate) fn refresh_locator(&self, dir: &GraphDir, locator: &mut Locator) -> bool {
        match self.read_locator(dir, locator) {
            Some(read) => {
                stamp_locator(locator, read, true);
                true
            }
            None => false,
        }
    }

    fn read_locator(
        &self,
        dir: &GraphDir,
        locator: &Locator,
    ) -> Option<(HashState, Option<SourceDigest>)> {
        match locator.kind {
            LocatorKind::Folder => Some((HashState::Remote, None)),
            LocatorKind::Url => Some((HashState::Remote, None)),
            LocatorKind::File => self.read(dir, &locator.source, None),
        }
    }

    /// One warning for the new pointers whose local file could not be hashed: the write went ahead.
    pub(crate) fn warnings(&self, pointers: &[Value]) -> Vec<Problem> {
        let states: Vec<HashState> = pointers
            .iter()
            .filter_map(pointer_state)
            .filter(|s| !matches!(s, HashState::Hashed | HashState::Remote))
            .collect();
        match states.first() {
            Some(first) => alloc::vec![problems::source_not_hashed(
                &self.graph,
                states.len(),
                *first
            )],
            None => Vec::new(),
        }
    }

    /// The warning for a locator whose local file could not be hashed.
    pub(crate) fn locator_warnings(&self, locator: &Locator) -> Vec<Problem> {
        match locator.hash_state.as_deref().and_then(HashState::parse) {
            Some(s) if !matches!(s, HashState::Hashed | HashState::Remote) => {
                alloc::vec![problems::source_not_hashed(&self.graph, 1, s)]
            }
            _ => Vec::new(),
        }
    }
}

/// Sets a locator's hash keys from a read; a folder has none. `accepted` as in [`stamp_pointer`].
fn stamp_locator(
    locator: &mut Locator,
    (state, digest): (HashState, Option<SourceDigest>),
    accepted: bool,
) {
    let folder = locator.kind == LocatorKind::Folder;
    locator.hash = digest.as_ref().map(|d| d.hash.clone());
    locator.length = digest.as_ref().map(|d| d.length);
    locator.seq_max = digest.as_ref().and_then(|d| d.seq_max);
    locator.hash_state = (!folder).then(|| state.as_str().to_string());
    locator.hash_accepted =
        (accepted && !folder && !matches!(state, HashState::Hashed | HashState::Remote))
            .then_some(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Repo;

    impl SourceRoots for Repo {
        fn root_folder(&self, _: &str) -> Option<GraphDir> {
            None
        }
        fn repo_top(&self, _: &GraphDir) -> Option<GraphDir> {
            GraphDir::parse("/work/repo").ok()
        }
    }

    /// Security note on #74: a graph file edited by hand to hold `/../x` (refused on write) is refused on read too.
    #[test]
    fn an_absolute_path_with_dotdot_is_outside_on_read() {
        let at = |p: &str| GraphDir::parse(p).unwrap();
        let reader = umx_fakes::FakeSourceReader::new(at("/work"));
        reader.add_folder(at("/work/repo/graphs/g"));
        let check = SourceCheck {
            reader: Arc::new(reader),
            roots: Some(Arc::new(Repo)),
            graph_roots: Vec::new(),
            graph: ValidName::alias("demo").unwrap(),
            read: Rc::default(),
        };
        let dir = at("/work/repo/graphs/g");
        for source in ["/../x.md", "/work/repo/graphs/g/../../x.md"] {
            assert!(
                matches!(check.place(&dir, source), Place::Outside),
                "{source}"
            );
        }
        assert!(matches!(
            check.place(&dir, "../../x.md"),
            Place::Local(_, ref p) if p == "x.md"
        ));
    }
}
