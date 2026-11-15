//! The unchecked writes of a [`GraphFiles`] port, for tests, setup and tools (F144, architect). They write with no
//! expected stamps, so a loaded graph's write through them could put an old copy over files changed on disk.
//! Outside [`crate::ports`] (it is no port, and `use ports::*` never brings it in) and re-exported nowhere;
//! umx-arch's r-checked-graph-writes keeps umx-services' shipped code from naming it: the services reach graph
//! folders only through [`GraphFiles::write_set_checked`].

use crate::codec::GraphFile;
use crate::ports::{Change, FilesError, GraphDir, GraphFiles, Written};

/// [`GraphFiles::write_set_checked`] with no check, for every port.
pub trait GraphFilesSetup: GraphFiles {
    /// A set with no expected stamps.
    fn write_set(&self, dir: &GraphDir, changes: &[Change<'_>]) -> Result<Written, FilesError> {
        self.write_set_checked(dir, changes, None)
            .map(|c| c.written)
    }

    /// One file: a one-change set with no check (gf_c22).
    fn write(&self, dir: &GraphDir, file: GraphFile, bytes: &[u8]) -> Result<Written, FilesError> {
        self.write_set(
            dir,
            &[Change {
                file: file.into(),
                bytes: Some(bytes),
            }],
        )
    }
}

impl<T: GraphFiles + ?Sized> GraphFilesSetup for T {}
