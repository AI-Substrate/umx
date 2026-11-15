use std::fs::File;

#[test]
fn reads() {
    let _ = File::open("/dev/null");
}
