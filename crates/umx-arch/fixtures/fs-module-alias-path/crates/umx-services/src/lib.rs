#![no_std]

mod store;

pub fn load(p: &str) -> Vec<u8> {
    store::s::fs::read(p).unwrap_or_default()
}
