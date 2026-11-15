#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use umx_core::ports::{GraphDir, GraphFiles, GraphStamps};

pub trait LocalState {
    fn write(&self, bytes: &[u8]) -> Result<(), ()>;
}

pub trait ConfigFiles {
    fn write(&self, file: &str, bytes: &[u8]) -> Result<(), ()>;
}

/// A byte sink with io::Write's one-argument write.
pub trait Sink {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, ()>;
}

/// The config service holds its ConfigFiles as `files` too: its write is not a graph write.
pub struct ConfigService {
    files: Box<dyn ConfigFiles>,
}

impl ConfigService {
    pub fn set(&self, file: &str) -> Result<(), ()> {
        self.files.write(file, &Vec::new())
    }
}

// The checked path; GraphFilesSetup and setup:: in a comment, and "umx_core::setup" in a string, are words, not code.
pub fn persist(files: &dyn GraphFiles, dir: &GraphDir, stamps: &GraphStamps) {
    let _ = "umx_core::setup::GraphFilesSetup";
    let _ = files.write_set_checked(dir, &[], Some(stamps));
}

pub fn record(state: &dyn LocalState, bytes: &[u8]) -> Result<(), ()> {
    state.write(bytes)
}

pub fn log(out: &mut dyn Sink) {
    let _ = out.write(b"line\n");
}

#[cfg(test)]
mod tests {
    use umx_core::setup::GraphFilesSetup;

    fn seed(files: &dyn umx_core::ports::GraphFiles, dir: &umx_core::ports::GraphDir) {
        files.write_set(dir, &[]).unwrap();
    }
}
