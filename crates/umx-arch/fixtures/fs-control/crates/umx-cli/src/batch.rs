pub fn read(file: &str) -> Vec<u8> {
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut bytes = Vec::new();
    if let Ok(mut f) = OpenOptions::new().read(true).custom_flags(0).open(file) {
        let _ = f.read_to_end(&mut bytes);
    }
    bytes
}
