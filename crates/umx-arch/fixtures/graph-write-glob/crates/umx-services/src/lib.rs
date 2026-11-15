#![no_std]

use umx_core::ports::GraphFiles;
use umx_core::setup::*;

// A glob brings the unchecked trait into scope without naming it: `files.write_set(` then compiles (F144).
pub fn persist(files: &dyn GraphFiles, dir: &umx_core::ports::GraphDir) {
    let _ = files.write_set(dir, &[]);
}
