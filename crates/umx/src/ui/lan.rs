use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use umx_core::ports::UiHost;
use umx_daemon::Config;
use umx_protocol::{Envelope, Issue, json};

const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn generate(host: &dyn UiHost) -> Result<String, Issue> {
    let mut letters = String::new();
    while letters.len() < 12 {
        let code = host.code().map_err(|e| super::issue(&e))?;
        // The last base64 character contains padding bits. The first 42 are uniform six-bit draws.
        for byte in code.bytes().take(42) {
            let Some(n) = BASE64.iter().position(|b| *b == byte) else {
                return Err(super::failed("the random source returned an invalid code"));
            };
            if n < ALPHABET.len() * 2 {
                letters.push(ALPHABET[n % ALPHABET.len()] as char);
                if letters.len() == 12 {
                    break;
                }
            }
        }
    }
    Ok(format!(
        "{}-{}-{}\n",
        &letters[..4],
        &letters[4..8],
        &letters[8..]
    ))
}

fn read(path: &Path, uid: Option<u32>) -> Result<Option<String>, Issue> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(super::failed(&format!(
                "cannot inspect the LAN password: {e}"
            )));
        }
    };
    if !meta.is_file()
        || meta.permissions().mode() & 0o777 != 0o600
        || uid.is_some_and(|u| meta.uid() != u)
    {
        return Err(super::failed(
            "the LAN password must be a regular file owned by you with mode 0600",
        ));
    }
    let file = File::open(path)
        .map_err(|e| super::failed(&format!("cannot open the LAN password: {e}")))?;
    let opened = file
        .metadata()
        .map_err(|e| super::failed(&format!("cannot inspect the LAN password: {e}")))?;
    if opened.dev() != meta.dev() || opened.ino() != meta.ino() {
        return Err(super::failed(
            "the LAN password file changed while opening it",
        ));
    }
    let mut text = String::new();
    file.take(64)
        .read_to_string(&mut text)
        .map_err(|e| super::failed(&format!("cannot read the LAN password: {e}")))?;
    if text.len() != 15
        || !text.bytes().enumerate().all(|(i, b)| match i {
            4 | 9 => b == b'-',
            14 => b == b'\n',
            _ => ALPHABET.contains(&b),
        })
    {
        return Err(super::failed("the LAN password file is invalid"));
    }
    Ok(Some(text))
}

pub(super) fn ensure(config: &Config, host: &dyn UiHost, rotate: bool) -> Result<String, Issue> {
    let path = config.home.join("ui-lan-password");
    let previous = read(&path, config.uid)?;
    if !rotate && let Some(password) = previous {
        return Ok(password);
    }
    fs::create_dir_all(&config.home)
        .map_err(|e| super::failed(&format!("cannot create UMX_HOME: {e}")))?;
    let home = fs::metadata(&config.home)
        .map_err(|e| super::failed(&format!("cannot inspect UMX_HOME: {e}")))?;
    if home.permissions().mode() & 0o022 != 0 || config.uid.is_some_and(|u| home.uid() != u) {
        return Err(super::failed(
            "UMX_HOME must belong to you and not be writable by others",
        ));
    }
    let password = generate(host)?;
    let nonce = host.code().map_err(|e| super::issue(&e))?;
    let temporary = config.home.join(format!(".ui-lan-password-{nonce}"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| super::failed(&format!("cannot create the LAN password: {e}")))?;
    let result = (|| {
        file.write_all(password.as_bytes())?;
        file.sync_all()?;
        if rotate {
            fs::rename(&temporary, &path)?;
            // The view already refuses sessions from before the new file (bin/lan.mjs ties them to it); this drops
            // their hashes too.
            match fs::remove_file(config.home.join("ui-lan-sessions")) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        } else {
            // Publish without replacing a password another launcher may have just created.
            match fs::hard_link(&temporary, &path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Ok::<(), std::io::Error>(())
    })();
    let cleanup = fs::remove_file(&temporary);
    result.map_err(|e| super::failed(&format!("cannot save the LAN password: {e}")))?;
    if let Err(e) = cleanup
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(super::failed(&format!(
            "cannot remove the temporary LAN password: {e}"
        )));
    }
    read(&path, config.uid)?.ok_or_else(|| super::failed("the LAN password file disappeared"))
}

pub fn password(config: &Config, rotate: bool) -> Envelope {
    let host = umx_compose::ui_host(crate::mode(), config);
    match ensure(config, host.as_ref(), rotate) {
        Ok(password) => Envelope::ok(
            "ui lan-password",
            json!({
                "password": password.trim_end(),
                "path": config.home.join("ui-lan-password").display().to_string(),
                "rotated": rotate,
            }),
        ),
        Err(issue) => Envelope::fail("ui lan-password", issue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use umx_fakes::FakeUiHost;

    #[test]
    fn password_is_private_reused_rotated_and_symlinks_are_refused() {
        let root = super::super::tests::temp();
        let config = Config::at(root.join("home"), Some(true));
        let host = FakeUiHost::default();
        let first = ensure(&config, &host, false).unwrap();
        let path = config.home.join("ui-lan-password");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(ensure(&config, &host, false).unwrap(), first);
        let sessions = config.home.join("ui-lan-sessions");
        fs::write(&sessions, "{}").unwrap();
        assert_eq!(ensure(&config, &host, false).unwrap(), first);
        assert!(sessions.exists());
        assert_ne!(ensure(&config, &host, true).unwrap(), first);
        assert!(!sessions.exists());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(ensure(&config, &host, false).is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), &path).unwrap();
        assert!(ensure(&config, &host, false).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
