#![no_std]

pub fn io() {
    let _ = std::fs::read("x");
    let _ = std::time::SystemTime::now();
    let _ = std::env::args();
    std::process::exit(0);
}
