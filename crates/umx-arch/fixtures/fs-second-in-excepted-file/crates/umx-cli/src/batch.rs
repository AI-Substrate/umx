pub fn read(file: &str) -> Vec<u8> {
    use std::fs::OpenOptions;
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(mut f) = OpenOptions::new().read(true).open(file) {
        let _ = f.read_to_end(&mut bytes);
    }
    bytes
}

pub fn keep(bytes: &[u8]) {
    let _ = std::fs::write("/tmp/umx-batch-copy", bytes);
}
