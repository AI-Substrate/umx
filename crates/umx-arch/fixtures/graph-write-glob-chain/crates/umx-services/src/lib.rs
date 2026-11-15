#![no_std]

use umx_graph::*;

pub fn persist(files: &dyn umx_core::ports::GraphFiles, dir: &umx_core::ports::GraphDir) {
    let _ = files.write_set(dir, &[]);
}
