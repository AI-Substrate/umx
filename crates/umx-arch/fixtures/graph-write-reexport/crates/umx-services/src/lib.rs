#![no_std]

use umx_graph::Unchecked;

pub fn persist(files: &dyn umx_core::ports::GraphFiles, dir: &umx_core::ports::GraphDir) {
    let _ = Unchecked::write_set(files, dir, &[]);
}
