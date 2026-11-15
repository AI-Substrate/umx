#![no_std]

extern /* not the literal text */ crate
    std as host;

pub fn read(path: &str) {
    let _ = host::fs::read(path);
}
