//! fs.local-state (ws004 s06 A2): `$UMX_HOME/state/graph-policies.json`, trusted as the machine config is: a plain
//! file owned by this user and open to no one else (0600), in a plain folder owned by this user and open to no one
//! else (0700). Anything else is refused, never read or written; the graph store then counts every graph as secured.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use umx_core::local_state::{STATE_DIR, STATE_FILE};
use umx_core::ports::local_state::{REFUSED_FOLDER, REFUSED_MISSING};
use umx_core::ports::{GraphDir, LocalState, LocalStateError, StateWritten};

use crate::config::{
    REFUSED_READABLE, current_uid, graph_dir, read_small, replace_owner_only, trust,
};

pub struct FsLocalState {
    folder: PathBuf,
    file: PathBuf,
    name: &'static str,
    counter: AtomicU64,
    absent_ok: bool,
}

impl FsLocalState {
    /// The policy record under `home` (`$UMX_HOME`).
    pub fn new(home: PathBuf) -> FsLocalState {
        FsLocalState::named(home, STATE_DIR, STATE_FILE)
    }

    /// Another state file under the same rules, in its own owner-only folder under `home` (the secret allows, ws004
    /// s08): never in the policy record's folder, whose existence without the record is refused.
    pub fn named(home: PathBuf, dir: &'static str, name: &'static str) -> FsLocalState {
        let folder = home.join(dir);
        FsLocalState {
            file: folder.join(name),
            folder,
            name,
            counter: AtomicU64::new(0),
            absent_ok: false,
        }
    }

    /// An independent state file whose absence is an empty store, even when its trusted folder exists.
    pub fn with_absent_ok(mut self, absent_ok: bool) -> Self {
        self.absent_ok = absent_ok;
        self
    }

    fn refused(&self, reason: &'static str) -> LocalStateError {
        LocalStateError::Refused {
            file: graph_dir(&self.file),
            reason,
        }
    }

    fn io(&self, e: std::io::Error) -> LocalStateError {
        LocalStateError::io(format!("check {}: {}", self.name, e.kind()))
    }

    /// Ok(true) if the folder is trusted, Ok(false) if it is absent.
    fn folder_checked(&self) -> Result<bool, LocalStateError> {
        match fs::symlink_metadata(&self.folder) {
            Ok(m) => {
                let trusted =
                    m.file_type().is_dir() && m.uid() == current_uid() && m.mode() & 0o077 == 0;
                if trusted {
                    Ok(true)
                } else {
                    Err(self.refused(REFUSED_FOLDER))
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::NotFound) => Ok(false),
            Err(e) => Err(self.io(e)),
        }
    }

    /// Ok if the folder is absent or trusted and the file is absent or trusted (by path: for writes).
    fn checked(&self) -> Result<(), LocalStateError> {
        if !self.folder_checked()? {
            return Ok(());
        }
        match trust(&self.file, 0o077, REFUSED_READABLE).map_err(|e| self.io(e))? {
            None => Ok(()),
            Some(reason) => Err(self.refused(reason)),
        }
    }
}

impl LocalState for FsLocalState {
    fn adapter_id(&self) -> &'static str {
        "fs.local-state"
    }

    fn path(&self) -> GraphDir {
        graph_dir(&self.file)
    }

    fn read(&self) -> Result<Option<Vec<u8>>, LocalStateError> {
        let folder = self.folder_checked()?;
        match read_small(&self.file, 0o077, REFUSED_READABLE) {
            Ok(Ok(Some(bytes))) => Ok(Some(bytes)),
            Ok(Ok(None)) if folder && !self.absent_ok => Err(self.refused(REFUSED_MISSING)),
            Ok(Ok(None)) => Ok(None),
            Ok(Err(reason)) => Err(self.refused(reason)),
            Err(e) => Err(LocalStateError::io(format!(
                "read {}: {}",
                self.name,
                e.kind()
            ))),
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<StateWritten, LocalStateError> {
        self.checked()?;
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        // The state folder sits in UMX_HOME, which this write may also make.
        let home = self.folder.parent().unwrap_or(&self.folder);
        match replace_owner_only(&self.folder, home, &self.file, bytes, n) {
            Ok(None) => Ok(StateWritten::Durable),
            Ok(Some(e)) => Ok(StateWritten::NotSynced(format!(
                "sync {STATE_DIR}: {}",
                e.kind()
            ))),
            Err(e) => Err(LocalStateError::io(format!(
                "write {}: {}",
                self.name,
                e.kind()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub struct TempHome(pub PathBuf);

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_home() -> TempHome {
        static N: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap();
        let path = base.join(format!(
            "umx-state-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempHome(path)
    }

    fn local_state() -> (FsLocalState, TempHome) {
        let home = temp_home();
        (FsLocalState::new(home.0.clone()), home)
    }

    /// Opens the file to group and others, which the trust check refuses.
    fn spoil(port: &FsLocalState) {
        fs::set_permissions(&port.file, fs::Permissions::from_mode(0o644)).unwrap();
    }

    fn remove(port: &FsLocalState) {
        fs::remove_file(&port.file).unwrap();
    }

    umx_contracts::local_state_contract!(
        local_state, spoil, remove;
        ls_c1_write_then_read_is_identical(),
        ls_c2_read_missing_is_none(),
        ls_c3_an_untrusted_file_is_neither_read_nor_written(),
        ls_c4_path_names_the_state_file(),
        ls_c5_a_removed_file_is_refused_not_empty(),
        ls_c6_an_oversized_file_is_refused(),
    );

    #[test]
    fn a_huge_state_file_is_refused_unread() {
        // Planted (F45): a 4 GiB sparse, owner-only state file is refused from its size, not read.
        let (port, _home) = local_state();
        port.write(b"{}\n").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&port.file)
            .unwrap()
            .set_len(4 << 30)
            .unwrap();
        match port.read() {
            Err(LocalStateError::Refused { reason, .. }) => {
                assert_eq!(reason, umx_core::ports::config_files::REFUSED_TOO_LARGE)
            }
            other => panic!("{:?}", other.map(|b| b.map(|b| b.len()))),
        }
    }

    fn mode(path: &std::path::Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o777
    }

    #[test]
    fn the_file_is_written_owner_only_in_an_owner_only_folder() {
        let (port, _home) = local_state();
        assert_eq!(port.write(b"{}\n").unwrap(), StateWritten::Durable);
        assert_eq!((mode(&port.folder), mode(&port.file)), (0o700, 0o600));
        let names: Vec<_> = fs::read_dir(&port.folder)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [STATE_FILE], "no temp file left behind");
    }

    #[test]
    fn a_symlink_or_an_open_folder_is_refused_and_left_alone() {
        let (port, home) = local_state();
        port.write(b"one\n").unwrap();
        // An open folder: refused for read and write; the file keeps its bytes.
        fs::set_permissions(&port.folder, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            port.read(),
            Err(LocalStateError::Refused {
                reason: REFUSED_FOLDER,
                ..
            })
        ));
        assert!(matches!(
            port.write(b"two\n"),
            Err(LocalStateError::Refused { .. })
        ));
        fs::set_permissions(&port.folder, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(port.read().unwrap().as_deref(), Some(&b"one\n"[..]));
        // A symlink in place of the file, even to a file this user owns at 0600.
        let target = home.0.join("elsewhere.json");
        fs::write(&target, b"other\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(&port.file).unwrap();
        std::os::unix::fs::symlink(&target, &port.file).unwrap();
        assert!(matches!(port.read(), Err(LocalStateError::Refused { .. })));
        assert!(matches!(
            port.write(b"three\n"),
            Err(LocalStateError::Refused { .. })
        ));
        assert_eq!(
            fs::read(&target).unwrap(),
            b"other\n",
            "the link's target is untouched"
        );
        // A symlink in place of the folder.
        let (port, home) = local_state();
        let real = home.0.join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&real, &port.folder).unwrap();
        assert!(matches!(
            port.read(),
            Err(LocalStateError::Refused {
                reason: REFUSED_FOLDER,
                ..
            })
        ));
    }

    #[test]
    fn scopes_allow_an_absent_file_only_in_the_explicit_instance() {
        let home = temp_home();
        fs::set_permissions(&home.0, fs::Permissions::from_mode(0o700)).unwrap();
        let state = FsLocalState::named(home.0.clone(), "", umx_core::scopes::SCOPES_FILE);
        assert!(matches!(
            state.read(),
            Err(LocalStateError::Refused {
                reason: REFUSED_MISSING,
                ..
            })
        ));
        let state = state.with_absent_ok(true);
        assert_eq!(state.read().unwrap(), None);
        state.write(b"umx_scopes = 1\n[scopes]\n").unwrap();
        assert_eq!(mode(&state.file), 0o600);
        state.write(b"replacement").unwrap();
        assert_eq!(state.read().unwrap().unwrap(), b"replacement");
        fs::remove_file(&state.file).unwrap();
        assert_eq!(state.read().unwrap(), None);
        let target = home.0.join("target");
        fs::write(&target, b"unchanged").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, &state.file).unwrap();
        assert!(state.read().is_err());
        assert!(state.write(b"changed").is_err());
        assert_eq!(fs::read(target).unwrap(), b"unchanged");
    }
}

#[cfg(test)]
mod allows_cap {
    use super::*;
    use umx_core::secret_allows::{ALLOWS_DIR, ALLOWS_FILE, MAX_ALLOWS_BYTES};

    /// R120-2: a secret-allows file at its own cap still reads through the small-file reader.
    #[test]
    fn an_allows_file_at_its_cap_still_reads() {
        let home = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("umx-allows-cap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let state = FsLocalState::named(home.clone(), ALLOWS_DIR, ALLOWS_FILE);
        let bytes = vec![b' '; MAX_ALLOWS_BYTES];
        state.write(&bytes).unwrap();
        assert_eq!(
            state.read().unwrap().map(|b| b.len()),
            Some(MAX_ALLOWS_BYTES)
        );
        let _ = fs::remove_dir_all(&home);
    }
}
