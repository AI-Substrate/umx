use std as s;
use s::{fs as files};

pub fn load(p: &str) -> Vec<u8> {
    files::read(p).unwrap_or_default()
}
