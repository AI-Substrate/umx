#![no_std]

use umx_core::ports::GraphFiles;

// A fully qualified call needs no use at all (F144).
pub fn persist(files: &dyn GraphFiles, dir: &umx_core::ports::GraphDir) {
    let _ = umx_core::setup::GraphFilesSetup::write_set(files, dir, &[]);
}
