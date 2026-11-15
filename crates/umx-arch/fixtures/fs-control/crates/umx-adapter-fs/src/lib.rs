use std::fs;

pub fn read(path: &str) -> std::io::Result<Vec<u8>> {
    fs::read(path)
}
