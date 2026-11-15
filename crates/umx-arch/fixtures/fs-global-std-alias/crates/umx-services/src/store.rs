use ::std as s;

pub fn load(p: &str) -> Vec<u8> {
    s::fs::read(p).unwrap_or_default()
}
