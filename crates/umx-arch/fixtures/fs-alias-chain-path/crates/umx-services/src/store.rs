use std as s;
use s::os::unix as u;

pub fn link(a: &str, b: &str) {
    let _ = u::fs::symlink(a, b);
}
