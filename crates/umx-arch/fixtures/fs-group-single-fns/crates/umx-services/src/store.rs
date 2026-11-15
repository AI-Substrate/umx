use std::fs::{read, write};

pub fn copy(a: &str, b: &str) {
    let _ = write(b, read(a).unwrap_or_default());
}
