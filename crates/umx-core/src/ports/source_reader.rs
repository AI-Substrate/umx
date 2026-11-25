//! SourceReader port (c-port-source-reader; req-47, workshop 010): the files evidence and resources cite. S1 of
//! workstream 006 needs only confinement: a source root must canonicalise to an existing folder inside the roots,
//! and a path under it must not lead out (security ruling ws006 S1). [`SourceReader::digest`] reads one file, as
//! a stream, to hash it when a reference is added and when `umx check --sources` looks again (usage 220): only the
//! digest leaves the port, never the bytes.

use crate::ports::GraphDir;
use crate::source_hash::{SourceDigest, Transcript};
use alloc::string::{String, ToString};
use core::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// The folder, or the path under a root, is outside the allowed roots once symlinks are resolved, or the path
    /// uses `..` or is absolute. Nothing was read.
    OutsideRoots,
    /// The root does not exist or is not a folder.
    NotFolder,
    /// [`SourceReader::digest`]: nothing is at the path.
    Missing,
    /// [`SourceReader::digest`]: the path is a folder or another thing that is not a file.
    NotFile,
    /// [`SourceReader::digest`]: the file is over [`crate::source_hash::SOURCE_MAX_BYTES`]; nothing past it was read.
    TooLarge,
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
            SourceError::Missing => f.write_str("no file is there"),
            SourceError::NotFile => f.write_str("it is not a file"),
            SourceError::TooLarge => f.write_str("it is over the size umx reads"),
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
    /// The digest of the file at `path` under the canonical `root`, read as a stream and never kept: confined as
    /// [`check_path`](SourceReader::check_path) confines (`OutsideRoots`, nothing read), then `Missing` when
    /// nothing is there, `NotFile` for a folder, `TooLarge` past [`crate::source_hash::SOURCE_MAX_BYTES`] (never
    /// streamed past it), `Io` when the system refuses.
    fn digest(
        &self,
        root: &GraphDir,
        path: &str,
        transcript: Option<Transcript>,
    ) -> Result<SourceDigest, SourceError>;
}
