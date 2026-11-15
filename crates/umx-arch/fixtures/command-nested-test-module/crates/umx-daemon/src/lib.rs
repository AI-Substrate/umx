pub fn shipped_launch() {
    let _ = std::process::Command::new("/bin/true").status();
}

mod tests {
    #![cfg(test)]

    #[test]
    fn smoke() {}
}
