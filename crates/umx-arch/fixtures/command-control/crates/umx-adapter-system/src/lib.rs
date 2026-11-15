use std::process::Command;

pub fn run() {
    let _ = Command::new("/bin/true").status();
}
