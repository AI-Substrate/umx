//! SourceReader port (c-port-source-reader; req-47, workshop 010): the files evidence and resources cite. S1 of
//! workstream 006 needs only confinement: a source root must canonicalise to an existing folder inside the roots,
//! and a path under it must not lead out (security ruling ws006 S1). The read methods land with S4 on this trait.
//! Nothing here reads a file's bytes.

use crate::ports::GraphDir;
use alloc::string::{String, ToString};
use core::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// The folder, or the path under a root, is outside the allowed roots once symlinks are resolved, or the path
    /// uses `..` or is absolute. Nothing was read.
    OutsideRoots,
    /// The root does not exist or is not a folder.
    NotFolder,
    /// The operating system refused; the message names the operation, never file contents.
    Io(String),
}

impl SourceError {
    pub fn io(message: impl ToString) -> SourceError {
        SourceError::Io(message.to_string())
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::OutsideRoots => f.write_str("it is outside the allowed roots"),
            SourceError::NotFolder => f.write_str("it is not an existing folder"),
            SourceError::Io(m) => f.write_str(m),
        }
    }
}

pub trait SourceReader: Send + Sync {
    /// Which adapter this is (`fs.source-reader`, `fake.source-reader`), as in the adapter matrix (d-6, E-1).
    fn adapter_id(&self) -> &'static str;
    /// The canonical form of `folder` (symlinks resolved): an existing folder inside the roots, else `NotFolder`
    /// or `OutsideRoots`. Called when a root is configured and again at every use.
    fn canonical_root(&self, folder: &GraphDir) -> Result<GraphDir, SourceError>;
    /// Whether `path` (relative, `/`-separated) under the canonical `root` stays inside it: every existing part is
    /// resolved, and a symlink that leads out of `root` (or dangles) is `OutsideRoots`, never followed. A missing
    /// tail is fine: nothing is read.
    fn check_path(&self, root: &GraphDir, path: &str) -> Result<(), SourceError>;
}
