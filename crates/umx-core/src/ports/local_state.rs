//! LocalState port (ws004 s06 A2): this machine's record of each graph's last-confirmed embedding policy, as bytes.
//! One file outside every graph folder and repository (`$UMX_HOME/state/graph-policies.json`), so a pull or an edit
//! of graph.json cannot change it. The adapter reads it only when it can trust it, as the machine config, and
//! replaces it whole and owner-only. The format is [`crate::local_state`].

use crate::ports::GraphDir;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalStateError {
    /// The file or its folder is a symlink, not owned by this user, or open to group or others. `reason` says how
    /// to fix it. Nothing was read or written.
    Refused {
        file: GraphDir,
        reason: &'static str,
    },
    /// The operating system refused; the message names the operation, never file contents.
    Io(String),
}

impl fmt::Display for LocalStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocalStateError::Refused { file, reason } => write!(f, "{file} is refused: {reason}"),
            LocalStateError::Io(m) => f.write_str(m),
        }
    }
}

impl LocalStateError {
    pub fn io(message: impl ToString) -> LocalStateError {
        LocalStateError::Io(message.to_string())
    }
}

/// How a write ended. `NotSynced`: the new file is in place but the folder sync after the rename failed, so it may
/// not survive a power cut; it is a warning, never undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateWritten {
    Durable,
    NotSynced(String),
}

/// Why the state folder is refused, shared by the adapter and the fake.
pub const REFUSED_FOLDER: &str =
    "its folder is not a plain folder owned by you with mode 700 (run chmod 700 on it)";
/// The file is gone but its folder is there: deleted, not never written. Refused, so removing it cannot lower a
/// policy; the folder is only created by the first write of the file.
pub const REFUSED_MISSING: &str =
    "it is missing but its folder exists (restore it or remove the empty folder)";

pub trait LocalState: Send + Sync {
    /// Which adapter this is (`fs.local-state`, `fake.local-state`), as in the adapter matrix (d-6, E-1).
    fn adapter_id(&self) -> &'static str;
    /// Where the file is (for messages).
    fn path(&self) -> GraphDir;
    /// The file's bytes, or None if neither it nor its folder exists. A missing file in an existing folder is
    /// refused ([`REFUSED_MISSING`]).
    fn read(&self) -> Result<Option<Vec<u8>>, LocalStateError>;
    /// Replaces the whole file atomically, owner-only (0600) in an owner-only folder (0700) it creates.
    fn write(&self, bytes: &[u8]) -> Result<StateWritten, LocalStateError>;
}
