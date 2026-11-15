use std as s;
use s::os as o;
use o::unix as u;

pub fn link(a: &str, b: &str) {
    let _ = u::fs::symlink(a, b);
}
