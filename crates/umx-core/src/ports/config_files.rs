//! ConfigFiles port (req-72): read and write umx's two config files as bytes, and find the repository a folder is
//! in. The adapter may touch exactly `$UMX_HOME/config.toml` and `<repo>/.umx/config.toml` for a repo inside the
//! graph roots, and refuses a file it cannot trust. It may also read (never write) flowspace3's `config.toml`, for
//! `umx config import` (ws004 s27); it can name no other file of that tool, so its secrets.env is out of reach.
//! Since ws002 s17 (D66) it also reads and writes a repo's `Graphfile`, at the repo's top only.

use crate::ports::GraphDir;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Which config file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigFile {
    Machine,
    /// The repository's root folder (the one holding `.git`).
    Repo(GraphDir),
    /// flowspace3's `config.toml`: read only, with the same checks as the others; a write is refused.
    Flowspace3,
    /// A repository's `Graphfile` (ws002 s17, D66): the repo's top folder (the one holding `.git`; a folder that does
    /// not is refused). A file shared through git, so not the owner and mode rules of umx's own files: a plain file
    /// (never a symlink), read up to [`GRAPHFILE_CAP`], written 0644 (architect, s17).
    Graphfile(GraphDir),
}

/// What [`ConfigFiles::swap`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swapped {
    /// The file held what the caller expected and now holds the new bytes.
    Done,
    /// The file no longer held what the caller expected: nothing was written.
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigFilesError {
    /// The file is a symlink, not a plain file, not owned by this user, or writable by group or others. `reason`
    /// says how to fix it. Nothing was read or written.
    Refused {
        file: GraphDir,
        reason: &'static str,
    },
    /// The repo folder is outside the roots. Nothing was read or written.
    OutsideRoots(GraphDir),
    /// The operating system refused; the message names the operation, never file contents.
    Io(String),
}

impl fmt::Display for ConfigFilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigFilesError::Refused { file, reason } => write!(f, "{file} is refused: {reason}"),
            ConfigFilesError::OutsideRoots(dir) => write!(f, "{dir} is outside the allowed roots"),
            ConfigFilesError::Io(m) => f.write_str(m),
        }
    }
}

impl ConfigFilesError {
    pub fn io(message: impl ToString) -> ConfigFilesError {
        ConfigFilesError::Io(message.to_string())
    }
}

/// Reasons a file is refused, shared by the adapter and the fake.
pub const REFUSED_SYMLINK: &str = "it is a symlink (replace it with a plain file)";
pub const REFUSED_NOT_FILE: &str = "it is not a plain file";
pub const REFUSED_OWNER: &str = "it is not owned by you (chown it to yourself)";
pub const REFUSED_READ_ONLY: &str = "it is another tool's config, which umx only reads";
pub const REFUSED_WRITABLE: &str = "group or others can write it (run chmod 600 on it)";
/// umx's own small files (config.toml, secrets.env, graph-policies.json) are read only up to this many bytes
/// (F45): a repo's `.umx/config.toml` arrives by git pull, so its size is not under the user's control.
pub const SMALL_FILE_CAP: u64 = 64 * 1024;
pub const REFUSED_TOO_LARGE: &str = "it is over 64 KiB, too big for a umx config file (check what wrote it, then shorten or remove it)";
/// A Graphfile is read and written up to this many bytes (graph-storage-expert, s17).
pub const GRAPHFILE_CAP: u64 = crate::graphfile::MAX_BYTES as u64;
pub const REFUSED_GRAPHFILE_TOO_LARGE: &str =
    "it is over 1 MiB, too big for a Graphfile (check what wrote it, then shorten it)";
pub const REFUSED_NOT_REPO_TOP: &str =
    "its folder is not a git repository's top (no .git there), the only place a Graphfile is read";

pub trait ConfigFiles: Send + Sync {
    /// Which adapter this is (`fs.config-files`, `fake.config-files`), as in the adapter matrix (d-6, E-1).
    fn adapter_id(&self) -> &'static str;
    /// Where the file is (for messages and `config show`).
    fn path(&self, file: &ConfigFile) -> GraphDir;
    /// The file's bytes, or None if it does not exist.
    fn read(&self, file: &ConfigFile) -> Result<Option<Vec<u8>>, ConfigFilesError>;
    /// Replaces the whole file atomically, owner-only (0600; a Graphfile 0644), creating `.umx/` for a repo file.
    fn write(&self, file: &ConfigFile, bytes: &[u8]) -> Result<(), ConfigFilesError>;
    /// [`ConfigFiles::write`], but only if the file still holds `expected` (None: it must not exist), compared right
    /// before the rename: Ok(Changed) and nothing written otherwise. It narrows the race with another writer but does
    /// not close it, as a re-read then a rename is not one atomic compare on POSIX; enough for a committed file, with
    /// the caller retrying once and then warning (security and architect, s17). Allowed for every file but
    /// flowspace3's, which is refused as for `write`.
    fn swap(
        &self,
        file: &ConfigFile,
        expected: Option<&[u8]>,
        bytes: &[u8],
    ) -> Result<Swapped, ConfigFilesError>;
    /// The nearest folder at or above `dir` that holds `.git`, walking up only while inside the roots; None if
    /// there is none.
    fn repo_root(&self, dir: &GraphDir) -> Result<Option<GraphDir>, ConfigFilesError>;
}
