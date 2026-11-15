extern crate std as host;

pub fn load(p: &str) -> Vec<u8> {
    ::host::fs::read(p).unwrap_or_default()
}
