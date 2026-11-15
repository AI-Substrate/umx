#![no_std]

// A crate umx-services ships with hands the unchecked trait on under another name (F144).
pub use umx_core::setup::GraphFilesSetup as Unchecked;
