use std::fs;

pub fn load(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_default()
}
