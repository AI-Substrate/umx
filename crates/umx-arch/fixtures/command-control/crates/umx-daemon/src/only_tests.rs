#![cfg(test)]
// A whole file that is test-only: its Command is allowed.
use std::process::Command;

#[test]
fn runs() {
    let _ = Command::new("/bin/true").status();
}
