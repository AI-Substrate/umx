use std::process::{Command, Stdio};

pub fn relaunch() {
    let _ = std::process::Command::new("/bin/true").stdin(Stdio::null()).status();
}

pub fn open_browser(url: &str) {
    let _ = Command::new("/usr/bin/open").arg(url).status();
}
