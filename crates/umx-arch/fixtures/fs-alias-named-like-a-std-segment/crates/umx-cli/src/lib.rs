use std as os;
use std::os::unix as u;

pub fn link(a: &str, b: &str) {
    let _ = os::io::stderr();
    let _ = u::fs::symlink(a, b);
}
