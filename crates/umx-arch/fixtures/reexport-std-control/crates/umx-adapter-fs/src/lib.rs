use std::fs;
pub(self) use std::os as o;
pub use serde_json::Value;
pub use std::fmt::Display;
pub use std::io::{Read, Write};
pub use std::collections::HashMap as Map;
pub use core::fmt;
pub(crate) use core::cmp::Ordering;
// A private fs-module alias used only privately, and a non-fs std alias re-exported (R302-B1 control).
use std::fs as disk;
use std::io as sio;
pub use sio::ErrorKind as Kind;

pub fn read(path: &str) -> std::io::Result<Vec<u8>> {
    let _ = o::raw::c_int::default();
    let _ = disk::metadata(path);
    fs::read(path)
}
