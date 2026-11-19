//! Security s3b N1: the cli tests' [`Capture`] files are created new. A module of the cli tests (it uses their
//! `Capture`), kept in its own file.

use super::*;

/// Security s3b N1: a capture file is created new. A link (dangling or not) or a file already at its path is refused,
/// never followed or truncated; a new one is the user's alone.
#[test]
fn capture_files_are_created_new_never_through_a_link() {
    let dir = std::env::temp_dir().join(format!("umx-t-{}-capture-new", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let target = dir.join("target");
    fs::write(&target, "kept").unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let dangling = dir.join("dangling");
    std::os::unix::fs::symlink(dir.join("none"), &dangling).unwrap();
    for path in [&link, &dangling, &target] {
        let err = Capture::create(path).map(drop).unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::AlreadyExists,
            "{}",
            path.display()
        );
    }
    assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
    assert!(!dir.join("none").exists());
    let fresh = dir.join("fresh");
    drop(Capture::create(&fresh).unwrap());
    assert_eq!(
        fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::remove_dir_all(&dir).unwrap();
}
