use std::os::unix::*;
use self::fs as ufs;

pub fn link(a: &str, b: &str) {
    let _ = ufs::symlink(a, b);
}
