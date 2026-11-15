use std::*;

pub fn exists(p: &str) -> bool {
    let meta = fs::metadata;
    meta(p).is_ok()
}
