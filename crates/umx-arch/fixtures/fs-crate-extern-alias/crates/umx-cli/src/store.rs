pub fn load(p: &str) -> Vec<u8> {
    crate::host::fs::read(p).unwrap_or_default()
}
