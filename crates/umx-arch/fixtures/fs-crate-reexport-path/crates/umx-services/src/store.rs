pub fn load(p: &str) -> Vec<u8> {
    crate::s::fs::read(p).unwrap_or_default()
}
