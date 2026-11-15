use std::fs::File as Handle;

pub fn open(p: &str) -> bool {
    Handle::open(p).is_ok()
}
