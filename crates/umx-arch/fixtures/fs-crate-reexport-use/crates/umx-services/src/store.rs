use crate::u::{fs as ufs};

pub fn link(a: &str, b: &str) {
    let _ = ufs::symlink(a, b);
}
