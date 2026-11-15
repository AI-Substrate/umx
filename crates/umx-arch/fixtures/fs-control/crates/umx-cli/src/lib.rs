mod batch;
mod paths;

// std uses that never reach fs, grouped and renamed (R251 B1 control).
use std as base;
use std::os::unix::{process as unix_process};
use std::{io::Read, path::Path as P};
// Rooted at the std alias and never reaching fs (F117 control).
use base::{collections::HashMap as Map};
// Globs of std modules that are not on the way to fs (R258 control).
use std::collections::*;
use base::os::unix::process::*;

pub fn stdin_len() -> usize {
    let mut s = String::new();
    let _ = base::io::stdin().read_to_string(&mut s);
    let _ = P::new("x");
    let _ = Map::<u8, u8>::new();
    let _ = BTreeMap::<u8, u8>::new();
    // A global path through the std alias that never reaches fs (R258 control).
    let _ = ::base::io::stderr();
    let _ = unix_process::ExitStatusExt::from_raw;
    s.len()
}

// std::fs is not used here outside open_tty.
fn open_tty() -> Option<std::fs::File> {
    std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()
}

pub fn label() -> &'static str {
    "std::fs::File::open"
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn writes() {
        fs::write("/tmp/umx-fixture", b"x").unwrap();
        let _ = super::open_tty();
    }
}
