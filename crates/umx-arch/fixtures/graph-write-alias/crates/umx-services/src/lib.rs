#![no_std]

use umx_core::ports::GraphFiles;
use umx_core::setup::GraphFilesSetup as W;

// The unchecked wrapper under another name: a loaded graph's write that skips the stamps (F144).
pub fn persist(files: &dyn GraphFiles, dir: &umx_core::ports::GraphDir) {
    let _ = W::write_set(files, dir, &[]);
}
