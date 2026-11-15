mod only_tests;

// std::process::Command is not used here.
pub fn relaunch() {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new("/bin/true").exec();
}

pub fn label() -> &'static str {
    "std::process::Command"
}

mod inline_tests {
    #![cfg(test)]
    use std::process::Command;

    #[test]
    fn runs() {
        let _ = Command::new("/bin/true").status();
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    #[test]
    fn runs() {
        let _ = Command::new("/bin/true").status();
    }
}
