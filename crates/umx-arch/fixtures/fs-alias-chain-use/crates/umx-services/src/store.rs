use o::unix::{fs as f};
use s::os as o;
use std as s;

pub fn link(a: &str, b: &str) {
    let _ = f::symlink(a, b);
}
