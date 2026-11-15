pub fn load(p: &str) -> Vec<u8> {
    super::s::fs::read(p).unwrap_or_default()
}
