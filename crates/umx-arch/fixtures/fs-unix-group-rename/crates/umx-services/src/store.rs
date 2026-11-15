use std::os::unix::{fs as unix_files};

pub fn link(a: &str, b: &str) {
    let _ = unix_files::symlink(a, b);
}
