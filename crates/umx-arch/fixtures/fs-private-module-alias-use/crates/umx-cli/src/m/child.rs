use crate::m::s::fs as f;

pub fn load(p: &str) -> Vec<u8> {
    f::read(p).unwrap_or_default()
}
