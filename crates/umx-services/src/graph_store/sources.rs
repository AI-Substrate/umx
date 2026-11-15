//! Source roots in evidence (req-47, ws006 S1): `root:<name>/<path>` names a folder the machine config sets, and
//! `root:repo/<path>` the git top level holding the graph folder. Security ruling ws006 S1: every write re-checks
//! the root through the SourceReader port (canonical, inside the roots) and that the path stays inside it. The
//! pointer is stored as written; nothing is read.

use super::problems;
use alloc::sync::Arc;
use alloc::vec::Vec;
use umx_core::config::REPO_ROOT;
use umx_core::evidence::root_source;
use umx_core::ports::{GraphDir, SourceError, SourceReader};
use umx_core::{Problem, Value};

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
}

impl SourceCheck {
    /// Refuses the first `root:` source whose root is unknown or unusable, or whose path leads out of its root.
    /// `dir` is the graph's folder. Other sources were checked when they were parsed.
    pub(crate) fn check(&self, dir: &GraphDir, pointers: &[Value]) -> Result<(), Problem> {
        for p in pointers {
            let Some((name, path)) = p
                .get("source")
                .and_then(Value::as_str)
                .and_then(root_source)
            else {
                continue;
            };
            let root = self.root(name, dir)?;
            self.reader.check_path(&root, path).map_err(|e| match e {
                SourceError::OutsideRoots => problems::evidence_outside_roots(),
                SourceError::NotFolder | SourceError::Io(_) => {
                    problems::root_unavailable(name, problems::ROOT_UNREADABLE)
                }
            })?;
        }
        Ok(())
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
            SourceError::NotFolder => problems::root_unavailable(name, problems::ROOT_NOT_FOLDER),
            SourceError::Io(_) => problems::root_unavailable(name, problems::ROOT_UNREADABLE),
        })
    }
}
