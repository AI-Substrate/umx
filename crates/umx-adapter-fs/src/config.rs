//! fs.config-files and fs.secrets (req-72): umx's config files and its secrets file, read and written only after
//! the same trust checks: a plain file (not a symlink), owned by this user, not writable by group or others (the
//! secrets file: not readable by them either).

use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use umx_core::config::{FILE_NAME, REPO_DIR};
use umx_core::graphfile::GRAPHFILE;
use umx_core::ports::config_files::{
    GRAPHFILE_CAP, REFUSED_GRAPHFILE_TOO_LARGE, REFUSED_NOT_FILE, REFUSED_NOT_REPO_TOP,
    REFUSED_OWNER, REFUSED_READ_ONLY, REFUSED_SYMLINK, REFUSED_TOO_LARGE, REFUSED_WRITABLE,
    SMALL_FILE_CAP,
};
use umx_core::ports::{
    ConfigFile, ConfigFiles, ConfigFilesError, GraphDir, Secret, SecretSource, Secrets,
    SecretsIssue, Swapped,
};

use crate::{Body, dir_of, read_capped, resolve};

/// One of umx's small files, read through [`read_capped`] with [`SMALL_FILE_CAP`] (F45) and trusted on the open
/// handle (F49): a symlink, something not a plain file, a file not owned by this user, writable by group or others
/// or with a `deny` bit set, or an over-cap file is refused with its reason, never read whole. The file judged is
/// the file read, so a swap or chmod after any earlier look cannot pass. Ok(None) if absent.
///
/// If the open itself is denied (a 0022 config, a 0044 secrets file: the owner cannot read it), a look at the path
/// names the refusal and its repair instead of a bare permission error (F51). That look only ever refuses; nothing
/// is read on its say-so. A denial after the open (fstat or read) stays an io error.
pub(crate) fn read_small(
    path: &Path,
    deny: u32,
    deny_reason: &'static str,
) -> std::io::Result<Result<Option<Vec<u8>>, &'static str>> {
    let vet = |meta: &fs::Metadata| judge(meta, deny, deny_reason);
    let denied = || trust(path, deny, deny_reason).ok().flatten();
    Ok(
        match read_capped(path, SMALL_FILE_CAP, vet, denied, || {})?.0 {
            Body::Absent => Ok(None),
            Body::Bytes(bytes) => Ok(Some(bytes)),
            Body::Symlink => Err(REFUSED_SYMLINK),
            Body::NotRegular => Err(REFUSED_NOT_FILE),
            Body::TooLarge(_) => Err(REFUSED_TOO_LARGE),
            Body::Refused(reason) => Err(reason),
        },
    )
}

/// The owner and mode rules for a regular file's metadata: owned by this user, not writable by group or others,
/// and no `deny` bit set.
fn judge(meta: &fs::Metadata, deny: u32, deny_reason: &'static str) -> Option<&'static str> {
    if meta.uid() != current_uid() {
        Some(REFUSED_OWNER)
    } else if meta.mode() & 0o022 != 0 {
        Some(REFUSED_WRITABLE)
    } else if meta.mode() & deny != 0 {
        Some(deny_reason)
    } else {
        None
    }
}

/// The current user's id. One of this crate's two libc wrappers, with `available_bytes` (rules.toml [owned] libc).
pub(crate) fn current_uid() -> u32 {
    // SAFETY: getuid takes no arguments, cannot fail and reads no memory of ours.
    unsafe { libc::getuid() }
}

pub(crate) const REFUSED_READABLE: &str = "group or others can read it (run chmod 600 on it)";

pub(crate) fn graph_dir(path: &Path) -> GraphDir {
    path.to_str()
        .and_then(|p| GraphDir::parse(p).ok())
        .unwrap_or_else(|| GraphDir::parse("/").expect("root"))
}

/// Ok(None) if absent; the reason if it exists but is not trusted. `deny` holds the mode bits that must be clear.
/// A look by path, for writes; reads are judged on their open handle by [`read_small`].
pub(crate) fn trust(
    path: &Path,
    deny: u32,
    deny_reason: &'static str,
) -> std::io::Result<Option<&'static str>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    Ok(if meta.file_type().is_symlink() {
        Some(REFUSED_SYMLINK)
    } else if !meta.is_file() {
        Some(REFUSED_NOT_FILE)
    } else {
        judge(&meta, deny, deny_reason)
    })
}

/// fs.config-files: `$UMX_HOME/config.toml` and `<repo>/.umx/config.toml` for repos inside the roots, nothing else.
pub struct FsConfigFiles {
    home: PathBuf,
    roots: Vec<GraphDir>,
    /// flowspace3's config folder, for [`ConfigFile::Flowspace3`]; None: that file reads as missing.
    flowspace3: Option<PathBuf>,
    counter: AtomicU64,
}

impl FsConfigFiles {
    /// Roots are canonicalized here, as in [`crate::FsGraphFiles`].
    pub fn new(home: PathBuf, roots: Vec<GraphDir>) -> FsConfigFiles {
        let roots = roots
            .into_iter()
            .map(|r| {
                resolve(Path::new(r.as_str()))
                    .ok()
                    .and_then(dir_of)
                    .unwrap_or(r)
            })
            .collect();
        FsConfigFiles {
            home,
            roots,
            flowspace3: None,
            counter: AtomicU64::new(0),
        }
    }

    /// Where flowspace3's config folder is: `ConfigFile::Flowspace3` reads `<dir>/config.toml` and nothing else.
    pub fn with_flowspace3(mut self, dir: PathBuf) -> FsConfigFiles {
        self.flowspace3 = Some(dir);
        self
    }

    fn within_roots(&self, dir: &GraphDir) -> bool {
        self.roots.iter().any(|r| dir.is_within(r))
    }

    /// The canonical repo folder, if inside the roots.
    fn repo_dir(&self, repo: &GraphDir) -> Result<PathBuf, ConfigFilesError> {
        let canonical = resolve(Path::new(repo.as_str()))
            .map_err(|e| ConfigFilesError::io(format!("resolve folder: {}", e.kind())))?;
        match dir_of(canonical.clone()) {
            Some(c) if self.within_roots(&c) => Ok(canonical),
            _ => Err(ConfigFilesError::OutsideRoots(repo.clone())),
        }
    }

    fn file_path(&self, file: &ConfigFile) -> Result<PathBuf, ConfigFilesError> {
        match file {
            ConfigFile::Machine => Ok(self.home.join(FILE_NAME)),
            ConfigFile::Flowspace3 => self
                .flowspace3
                .as_ref()
                .map(|d| d.join(FILE_NAME))
                .ok_or_else(|| ConfigFilesError::io("flowspace3's config folder is not known")),
            ConfigFile::Repo(repo) => {
                let dir = self.repo_dir(repo)?.join(REPO_DIR);
                if fs::symlink_metadata(&dir).is_ok_and(|m| !m.is_dir()) {
                    return Err(ConfigFilesError::Refused {
                        file: graph_dir(&dir),
                        reason: REFUSED_SYMLINK,
                    });
                }
                Ok(dir.join(FILE_NAME))
            }
            ConfigFile::Graphfile(repo) => {
                let top = self.repo_dir(repo)?;
                if fs::symlink_metadata(top.join(".git")).is_err() {
                    return Err(ConfigFilesError::Refused {
                        file: graph_dir(&top.join(GRAPHFILE)),
                        reason: REFUSED_NOT_REPO_TOP,
                    });
                }
                Ok(top.join(GRAPHFILE))
            }
        }
    }

    /// The file's bytes, read on its open handle: umx's own files with their owner and mode rules and
    /// [`SMALL_FILE_CAP`]; a Graphfile, shared through git, as a plain file up to [`GRAPHFILE_CAP`] (s17).
    fn read_path(
        &self,
        file: &ConfigFile,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, ConfigFilesError> {
        let read = if matches!(file, ConfigFile::Graphfile(_)) {
            read_capped(path, GRAPHFILE_CAP, |_| None, || None, || {}).map(|(body, _)| match body {
                Body::Absent => Ok(None),
                Body::Bytes(bytes) => Ok(Some(bytes)),
                Body::Symlink => Err(REFUSED_SYMLINK),
                Body::NotRegular => Err(REFUSED_NOT_FILE),
                Body::TooLarge(_) => Err(REFUSED_GRAPHFILE_TOO_LARGE),
                Body::Refused(reason) => Err(reason),
            })
        } else {
            read_small(path, 0, "")
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(FILE_NAME);
        match read {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(reason)) => Err(ConfigFilesError::Refused {
                file: graph_dir(path),
                reason,
            }),
            Err(e) => Err(ConfigFilesError::io(format!("read {name}: {}", e.kind()))),
        }
    }

    /// The path to write `file` at, after the same checks a read makes (a Graphfile: not a symlink, a plain file).
    fn writable(&self, file: &ConfigFile) -> Result<PathBuf, ConfigFilesError> {
        if *file == ConfigFile::Flowspace3 {
            return Err(ConfigFilesError::Refused {
                file: self.path(file),
                reason: REFUSED_READ_ONLY,
            });
        }
        if !matches!(file, ConfigFile::Graphfile(_)) {
            return self.checked(file);
        }
        let path = self.file_path(file)?;
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => Err(REFUSED_SYMLINK),
            Ok(m) if !m.is_file() => Err(REFUSED_NOT_FILE),
            _ => return Ok(path),
        }
        .map_err(|reason| ConfigFilesError::Refused {
            file: graph_dir(&path),
            reason,
        })
    }

    /// A write, or with `expected` a swap: the file is compared with it after the temp file is synced, right before
    /// the rename.
    fn swap_or_write(
        &self,
        file: &ConfigFile,
        expected: Option<Option<&[u8]>>,
        bytes: &[u8],
    ) -> Result<Swapped, ConfigFilesError> {
        let path = self.writable(file)?;
        let folder = path
            .parent()
            .expect("a config file has a folder")
            .to_path_buf();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(FILE_NAME);
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let mut refused = None;
        let still = || {
            let Some(expected) = expected else {
                return Ok(true);
            };
            match self.read_path(file, &path) {
                Ok(now) => Ok(now.as_deref() == expected),
                Err(e) => {
                    refused = Some(e);
                    Ok(false)
                }
            }
        };
        let done = replace_file(&folder, &folder, &path, bytes, n, mode_of(file), still);
        if let Some(e) = refused {
            return Err(e);
        }
        match done {
            Ok((true, None)) => Ok(Swapped::Done),
            Ok((false, _)) => Ok(Swapped::Changed),
            Ok((true, Some(e))) | Err(e) => {
                Err(ConfigFilesError::io(format!("write {name}: {}", e.kind())))
            }
        }
    }

    fn checked(&self, file: &ConfigFile) -> Result<PathBuf, ConfigFilesError> {
        let path = self.file_path(file)?;
        match trust(&path, 0, "") {
            Ok(None) => Ok(path),
            Ok(Some(reason)) => Err(ConfigFilesError::Refused {
                file: graph_dir(&path),
                reason,
            }),
            Err(e) => Err(ConfigFilesError::io(format!(
                "check {FILE_NAME}: {}",
                e.kind()
            ))),
        }
    }
}

/// Replaces `path` (in `folder`) with `bytes`, owner-only: [`replace_file`] with mode 0600 and no check. The one
/// atomic write for umx's own files.
pub(crate) fn replace_owner_only(
    folder: &Path,
    top: &Path,
    path: &Path,
    bytes: &[u8],
    n: u64,
) -> std::io::Result<Option<std::io::Error>> {
    replace_file(folder, top, path, bytes, n, 0o600, || Ok(true)).map(|(_, synced)| synced)
}

/// Replaces `path` (in `folder`) with `bytes`: a `mode` temp file (`n` keeps temp names apart), fsync, then
/// `still` (false: the temp goes, nothing is replaced, and Ok((false, None)) comes back), rename over the old file,
/// then fsync the folder, which is created 0700 if missing; then the name of each folder from `folder` up to `top`
/// is made durable in the folder above it, once per folder per process (`names_durable`). `top` is the highest
/// folder this write may create. Err: nothing was replaced. Ok((true, Some(e))): the new file is in place but a
/// folder sync failed.
pub(crate) fn replace_file(
    folder: &Path,
    top: &Path,
    path: &Path,
    bytes: &[u8],
    n: u64,
    mode: u32,
    still: impl FnOnce() -> std::io::Result<bool>,
) -> std::io::Result<(bool, Option<std::io::Error>)> {
    static DURABLE: crate::Durable = std::sync::Mutex::new(BTreeMap::new());
    let name = path.file_name().and_then(|f| f.to_str()).unwrap_or("file");
    let temp = folder.join(format!(".umx-tmp-{name}-{}-{n}", std::process::id()));
    let result = (|| {
        crate::make_dirs(folder, 0o700)?;
        #[cfg(test)]
        if crate::tests::fault(&crate::tests::Fault::Temp) {
            return Err(std::io::Error::from(ErrorKind::StorageFull));
        }
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temp)?;
        f.write_all(bytes)?;
        crate::sync_temp(&f, &temp)?;
        if !still()? {
            return Ok(false);
        }
        crate::rename(&temp, path).map(|()| true)
    })();
    match result {
        Ok(true) => Ok((
            true,
            crate::sync_folder(folder)
                .and_then(|()| crate::names_durable(&DURABLE, folder, top))
                .err(),
        )),
        Ok(false) => {
            let _ = fs::remove_file(&temp);
            Ok((false, None))
        }
        Err(e) => {
            let _ = fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// The mode a file is written with: 0644 for a Graphfile, which git shares; 0600 for umx's own files.
fn mode_of(file: &ConfigFile) -> u32 {
    if matches!(file, ConfigFile::Graphfile(_)) {
        0o644
    } else {
        0o600
    }
}

impl ConfigFiles for FsConfigFiles {
    fn adapter_id(&self) -> &'static str {
        "fs.config-files"
    }

    fn path(&self, file: &ConfigFile) -> GraphDir {
        match file {
            ConfigFile::Machine => graph_dir(&self.home.join(FILE_NAME)),
            ConfigFile::Flowspace3 => graph_dir(
                &self
                    .flowspace3
                    .as_ref()
                    .map_or_else(|| self.home.join("flowspace3"), Clone::clone)
                    .join(FILE_NAME),
            ),
            ConfigFile::Repo(repo) => {
                graph_dir(&Path::new(repo.as_str()).join(REPO_DIR).join(FILE_NAME))
            }
            ConfigFile::Graphfile(repo) => graph_dir(&Path::new(repo.as_str()).join(GRAPHFILE)),
        }
    }

    fn read(&self, file: &ConfigFile) -> Result<Option<Vec<u8>>, ConfigFilesError> {
        let path = self.file_path(file)?;
        self.read_path(file, &path)
    }

    fn write(&self, file: &ConfigFile, bytes: &[u8]) -> Result<(), ConfigFilesError> {
        self.swap_or_write(file, None, bytes).map(|_| ())
    }

    fn swap(
        &self,
        file: &ConfigFile,
        expected: Option<&[u8]>,
        bytes: &[u8],
    ) -> Result<Swapped, ConfigFilesError> {
        self.swap_or_write(file, Some(expected), bytes)
    }

    fn repo_root(&self, dir: &GraphDir) -> Result<Option<GraphDir>, ConfigFilesError> {
        let Ok(start) = resolve(Path::new(dir.as_str())) else {
            return Ok(None);
        };
        let mut at = Some(start.as_path());
        while let Some(folder) = at {
            let Some(d) = dir_of(folder.to_path_buf()) else {
                return Ok(None);
            };
            if !self.within_roots(&d) {
                return Ok(None);
            }
            if fs::symlink_metadata(folder.join(".git")).is_ok() {
                return Ok(Some(d));
            }
            at = folder.parent();
        }
        Ok(None)
    }
}

/// fs.secrets: env first (a snapshot compose takes once, d-9), then `$UMX_HOME/secrets.env`, re-read on each lookup.
/// No Debug: it holds secret values.
pub struct FsSecrets {
    env: BTreeMap<String, String>,
    file: PathBuf,
}

/// A line of the secrets file: `NAME=value`, nothing else.
fn secret_line(line: &str) -> Option<(&str, &str)> {
    // A CRLF file or trailing spaces must not end up in the key.
    let (name, value) = line.trim_end().split_once('=')?;
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some('A'..='Z' | '_'))
        && name.len() <= 64
        && chars.all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '_'));
    ok.then_some((name, value))
}

enum FileState {
    Absent,
    Refused(&'static str),
    Lines(String),
}

impl FsSecrets {
    pub fn new(env: BTreeMap<String, String>, file: PathBuf) -> FsSecrets {
        FsSecrets { env, file }
    }

    fn state(&self) -> FileState {
        match read_small(&self.file, 0o077, REFUSED_READABLE) {
            Ok(Ok(Some(bytes))) => match String::from_utf8(bytes) {
                Ok(text) => FileState::Lines(text),
                Err(_) => FileState::Refused("it is not UTF-8"),
            },
            Ok(Ok(None)) => FileState::Absent,
            Ok(Err(reason)) => FileState::Refused(reason),
            Err(_) => FileState::Refused("it cannot be read"),
        }
    }
}

impl Secrets for FsSecrets {
    fn adapter_id(&self) -> &'static str {
        "fs.secrets"
    }

    fn lookup(&self, name: &str) -> Option<(SecretSource, Secret)> {
        if let Some(v) = self.env.get(name).filter(|v| !v.is_empty()) {
            return Some((SecretSource::Env, Secret::new(v.clone())));
        }
        let FileState::Lines(text) = self.state() else {
            return None;
        };
        text.lines()
            .filter_map(secret_line)
            .find(|(n, v)| *n == name && !v.is_empty())
            .map(|(_, v)| (SecretSource::SecretsFile, Secret::new(v.to_string())))
    }

    fn issues(&self) -> Vec<SecretsIssue> {
        let file = graph_dir(&self.file);
        match self.state() {
            FileState::Absent => Vec::new(),
            FileState::Refused(reason) => vec![SecretsIssue::Refused { file, reason }],
            FileState::Lines(text) => text
                .lines()
                .enumerate()
                .filter(|(_, l)| {
                    !l.trim().is_empty() && !l.starts_with('#') && secret_line(l).is_none()
                })
                .map(|(i, _)| SecretsIssue::BadLine {
                    file: file.clone(),
                    line: i as u64 + 1,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub struct TempRoot(pub PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_root() -> TempRoot {
        static N: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap();
        let path = base.join(format!(
            "umx-cfg-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempRoot(path)
    }

    /// A port over `<temp>/home` with `<temp>/work` as the only root; `<temp>/work/repo` is a git repository.
    fn config_files() -> (FsConfigFiles, GraphDir, TempRoot) {
        let temp = temp_root();
        let work = temp.0.join("work");
        fs::create_dir_all(work.join("repo/.git")).unwrap();
        let root = GraphDir::parse(work.to_str().unwrap()).unwrap();
        (
            FsConfigFiles::new(temp.0.join("home"), vec![root.clone()]),
            root,
            temp,
        )
    }

    umx_contracts::config_files_contract!(
        config_files;
        cf_c1_write_then_read_is_identical(),
        cf_c2_read_missing_is_none(),
        cf_c3_machine_and_repo_are_separate(),
        cf_c4_repo_outside_roots_is_refused(),
        cf_c5_repo_root_finds_git_within_roots(),
        cf_c6_path_names_the_file(),
        cf_c7_an_oversized_file_is_refused(),
    );

    /// A linked git worktree's top holds `.git` as a file (`gitdir: ...`), not a folder: it is a repo top all the
    /// same, found from a folder inside it, and its Graphfile is read and written there (gate).
    #[test]
    fn a_linked_worktree_whose_git_is_a_file_is_a_repo_top() {
        let (port, root, _temp) = config_files();
        let linked = Path::new(root.as_str()).join("linked");
        fs::create_dir_all(linked.join("crates/umx")).unwrap();
        fs::write(
            linked.join(".git"),
            "gitdir: /elsewhere/.git/worktrees/linked\n",
        )
        .unwrap();
        let top = GraphDir::parse(linked.to_str().unwrap()).unwrap();
        let inner = GraphDir::parse(linked.join("crates/umx").to_str().unwrap()).unwrap();
        assert_eq!(port.repo_root(&inner).unwrap(), Some(top.clone()));
        let file = ConfigFile::Graphfile(top);
        port.write(&file, b"graphfile = 1\n").unwrap();
        assert_eq!(
            port.read(&file).unwrap().as_deref(),
            Some(&b"graphfile = 1\n"[..])
        );
    }

    fn flowspace3(fs3: umx_contracts::config_files::Fs3) -> (FsConfigFiles, TempRoot) {
        use umx_contracts::config_files::Fs3;
        let (port, _root, temp) = config_files();
        let dir = temp.0.join("fs3home/flowspace3");
        fs::create_dir_all(&dir).unwrap();
        match fs3 {
            Fs3::Missing => {}
            Fs3::File(bytes) => fs::write(dir.join(FILE_NAME), bytes).unwrap(),
            Fs3::Mode(bytes, mode) => {
                fs::write(dir.join(FILE_NAME), bytes).unwrap();
                fs::set_permissions(dir.join(FILE_NAME), fs::Permissions::from_mode(mode)).unwrap();
            }
            Fs3::Symlink(bytes) => {
                fs::write(temp.0.join("elsewhere.toml"), bytes).unwrap();
                std::os::unix::fs::symlink(temp.0.join("elsewhere.toml"), dir.join(FILE_NAME))
                    .unwrap();
            }
        }
        (port.with_flowspace3(dir), temp)
    }

    fn graphfile(gf: umx_contracts::config_files::Gf) -> (FsConfigFiles, GraphDir, TempRoot) {
        use umx_contracts::config_files::Gf;
        let (port, root, temp) = config_files();
        fs::create_dir_all(temp.0.join("work/other")).unwrap();
        if let Gf::Symlink(bytes) = gf {
            fs::write(temp.0.join("elsewhere"), bytes).unwrap();
            std::os::unix::fs::symlink(
                temp.0.join("elsewhere"),
                temp.0.join("work/repo/Graphfile"),
            )
            .unwrap();
        }
        (port, root, temp)
    }

    umx_contracts::config_files_graphfile_contract!(
        graphfile;
        cf_g1_a_graphfile_round_trips_byte_for_byte(),
        cf_g2_a_missing_graphfile_reads_none(),
        cf_g3_a_symlinked_graphfile_is_refused(),
        cf_g4_a_graphfile_outside_the_roots_is_refused(),
        cf_g5_an_oversized_graphfile_is_refused(),
        cf_g6_swap_writes_only_over_what_was_expected(),
        cf_g7_a_graphfile_away_from_a_repo_top_is_refused(),
        cf_g8_the_graphfile_is_at_the_repo_top(),
    );

    /// Each file's mode (architect, s17): umx's own files owner-only, the Graphfile, which git shares, 0644; a write
    /// and a swap alike.
    #[test]
    fn each_file_is_written_with_its_mode() {
        let (port, root, temp) = config_files();
        let repo = root.join("repo").unwrap();
        let mode = |p: PathBuf| fs::metadata(p).unwrap().mode() & 0o777;
        port.write(&ConfigFile::Machine, b"m\n").unwrap();
        port.write(&ConfigFile::Repo(repo.clone()), b"r\n").unwrap();
        port.swap(&ConfigFile::Graphfile(repo.clone()), None, b"g\n")
            .unwrap();
        assert_eq!(mode(temp.0.join("home/config.toml")), 0o600);
        assert_eq!(mode(temp.0.join("work/repo/.umx/config.toml")), 0o600);
        assert_eq!(mode(temp.0.join("work/repo/Graphfile")), 0o644);
        port.write(&ConfigFile::Graphfile(repo), b"g2\n").unwrap();
        assert_eq!(mode(temp.0.join("work/repo/Graphfile")), 0o644);
    }

    umx_contracts::config_files_flowspace3_contract!(
        flowspace3;
        cf_f1_flowspace3_reads_and_is_never_written(),
        cf_f2_a_symlinked_flowspace3_file_is_refused(),
        cf_f3_flowspace3_path_names_its_config_toml(),
        cf_f4_a_flowspace3_file_others_can_write_is_refused(),
    );

    fn secrets() -> (FsSecrets, TempRoot) {
        let temp = temp_root();
        let file = temp.0.join("secrets.env");
        let env = [("FROM_ENV".to_string(), "env-value".to_string())].into();
        (FsSecrets::new(env, file), temp)
    }

    fn write_secrets(port: &FsSecrets, text: &str) {
        fs::write(&port.file, text).unwrap();
        fs::set_permissions(&port.file, fs::Permissions::from_mode(0o600)).unwrap();
    }

    umx_contracts::secrets_contract!(
        secrets, write_secrets;
        se_c1_env_beats_the_file(),
        se_c2_missing_is_none(),
        se_c3_bad_lines_are_named_by_number_only(),
        se_c4_secrets_never_format(),
        se_c5_line_ends_are_not_part_of_the_value(),
        se_c6_an_oversized_file_is_ignored_and_reported(),
    );

    /// Planted (F45): a 4 GiB sparse file, owner-only and otherwise trusted. A whole read would take seconds and
    /// gigabytes; the capped read refuses it from the size on the open handle, reading nothing.
    fn huge(path: &Path) {
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).unwrap();
        }
        let f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        f.set_len(4 << 30).unwrap();
    }

    #[test]
    fn a_huge_repo_config_or_secrets_file_is_refused_unread() {
        let (port, root, _temp) = config_files();
        let repo = root.join("repo").unwrap();
        huge(&Path::new(repo.as_str()).join(REPO_DIR).join(FILE_NAME));
        let started = std::time::Instant::now();
        match port.read(&ConfigFile::Repo(repo)) {
            Err(ConfigFilesError::Refused { reason, .. }) => assert_eq!(reason, REFUSED_TOO_LARGE),
            other => panic!("{:?}", other.map(|b| b.map(|b| b.len()))),
        }
        let (secrets, _s) = secrets();
        huge(&secrets.file);
        assert!(secrets.lookup("ANY").is_none());
        assert!(matches!(
            secrets.issues().as_slice(),
            [SecretsIssue::Refused {
                reason: REFUSED_TOO_LARGE,
                ..
            }]
        ));
        // Generous: a whole read of 8 GiB is far slower; this only guards against reading at all.
        assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    }

    #[test]
    fn untrusted_config_files_are_refused_for_read_and_write() {
        let (port, _root, temp) = config_files();
        let home = temp.0.join("home");
        fs::create_dir_all(&home).unwrap();
        let file = home.join(FILE_NAME);
        fs::write(&file, b"roots = [\"/\"]\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
        fn refused<T: std::fmt::Debug>(r: Result<T, ConfigFilesError>) -> &'static str {
            match r {
                Err(ConfigFilesError::Refused { reason, .. }) => reason,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(refused(port.read(&ConfigFile::Machine)), REFUSED_WRITABLE);
        assert_eq!(
            refused(port.write(&ConfigFile::Machine, b"")),
            REFUSED_WRITABLE
        );
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            port.read(&ConfigFile::Machine).unwrap().is_some(),
            "0644 is fine"
        );
        fs::remove_file(&file).unwrap();
        let elsewhere = temp.0.join("elsewhere.toml");
        fs::write(&elsewhere, b"").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
        assert_eq!(refused(port.read(&ConfigFile::Machine)), REFUSED_SYMLINK);
        fs::remove_file(&file).unwrap();
        // A writable file is replaced owner-only.
        port.write(&ConfigFile::Machine, b"a = 1\n").unwrap();
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
        // A repo's .umx that is a symlink is refused.
        let repo = GraphDir::parse(temp.0.join("work/repo").to_str().unwrap()).unwrap();
        std::os::unix::fs::symlink(&home, temp.0.join("work/repo/.umx")).unwrap();
        assert_eq!(refused(port.read(&ConfigFile::Repo(repo))), REFUSED_SYMLINK);
    }

    /// F49: umx's small files are trusted on the open handle. [`plant`] swaps the path between the open and the trust
    /// judgment. An untrusted file opened then swapped for a trusted one is still refused; a trusted file opened then
    /// swapped for an untrusted one is read, and its own bytes come back. Judging by path gets both wrong. One test
    /// per file and direction, so a regression names each one.
    ///
    /// Writes `OPENED=opened` (mode `first`) at `path` and a file of mode `then` beside it, renamed over `path`
    /// right after the read opens it.
    fn plant(path: &Path, first: u32, then: u32) {
        let folder = path.parent().unwrap();
        fs::create_dir_all(folder).unwrap();
        fs::write(path, OPENED).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(first)).unwrap();
        let swap = folder.join("swap");
        fs::write(&swap, b"SWAPPED=swapped\n").unwrap();
        fs::set_permissions(&swap, fs::Permissions::from_mode(then)).unwrap();
        let target = path.to_path_buf();
        crate::tests::set_hook(&crate::tests::AFTER_OPEN, folder, move || {
            fs::rename(swap, target).unwrap()
        });
    }

    const OPENED: &[u8] = b"OPENED=opened\n";

    fn machine_config() -> (FsConfigFiles, PathBuf, TempRoot) {
        let (config, _root, temp) = config_files();
        let file = temp.0.join("home").join(FILE_NAME);
        (config, file, temp)
    }

    fn state() -> (crate::local_state::FsLocalState, PathBuf, TempRoot) {
        use umx_core::local_state::{STATE_DIR, STATE_FILE};
        use umx_core::ports::LocalState;
        let temp = temp_root();
        let state = crate::local_state::FsLocalState::new(temp.0.clone());
        state.write(b"{}\n").unwrap();
        let file = temp.0.join(STATE_DIR).join(STATE_FILE);
        (state, file, temp)
    }

    fn state_refused(r: Result<Option<Vec<u8>>, umx_core::ports::LocalStateError>) -> &'static str {
        match r {
            Err(umx_core::ports::LocalStateError::Refused { reason, .. }) => reason,
            other => panic!("state: not refused: {other:?}"),
        }
    }

    fn config_refused(r: Result<Option<Vec<u8>>, ConfigFilesError>) -> &'static str {
        match r {
            Err(ConfigFilesError::Refused { reason, .. }) => reason,
            other => panic!("config: not refused: {other:?}"),
        }
    }

    #[test]
    fn config_opened_untrusted_then_swapped_is_refused() {
        let (config, file, _temp) = machine_config();
        plant(&file, 0o666, 0o600);
        assert_eq!(
            config_refused(config.read(&ConfigFile::Machine)),
            REFUSED_WRITABLE
        );
    }

    #[test]
    fn config_opened_trusted_then_swapped_reads_its_own_bytes() {
        let (config, file, _temp) = machine_config();
        plant(&file, 0o600, 0o666);
        assert_eq!(
            config.read(&ConfigFile::Machine).unwrap().as_deref(),
            Some(OPENED)
        );
    }

    #[test]
    fn secrets_opened_untrusted_then_swapped_is_refused() {
        let (secrets, _temp) = secrets();
        plant(&secrets.file, 0o644, 0o600);
        assert!(secrets.lookup("OPENED").is_none());
    }

    #[test]
    fn secrets_opened_trusted_then_swapped_reads_its_own_bytes() {
        let (secrets, _temp) = secrets();
        plant(&secrets.file, 0o600, 0o644);
        assert_eq!(secrets.lookup("OPENED").unwrap().1.expose(), "opened");
    }

    #[test]
    fn state_opened_untrusted_then_swapped_is_refused() {
        use umx_core::ports::LocalState;
        let (state, file, _temp) = state();
        plant(&file, 0o644, 0o600);
        assert_eq!(state_refused(state.read()), REFUSED_READABLE);
    }

    #[test]
    fn state_opened_trusted_then_swapped_reads_its_own_bytes() {
        use umx_core::ports::LocalState;
        let (state, file, _temp) = state();
        plant(&file, 0o600, 0o644);
        assert_eq!(state.read().unwrap().as_deref(), Some(OPENED));
    }

    /// F51 (review R108-F1): when the owner cannot open the file, the refusal and its chmod repair still come
    /// back, not a bare permission error (which startup reported as write_failed). A file whose path shows nothing
    /// to refuse (0000) stays an io error: the path look never invents a reason, and never allows a read.
    #[test]
    fn a_denied_open_still_names_the_refusal() {
        use umx_core::ports::LocalState;
        let (config, root, temp) = config_files();
        let repo = ConfigFile::Repo(root.join("repo").unwrap());
        let files = [
            temp.0.join("home").join(FILE_NAME),
            temp.0.join("work/repo").join(REPO_DIR).join(FILE_NAME),
        ];
        for (file, which) in files.iter().zip([ConfigFile::Machine, repo]) {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, b"a = 1\n").unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o022)).unwrap();
            assert_eq!(
                config_refused(config.read(&which)),
                REFUSED_WRITABLE,
                "{which:?}"
            );
            fs::set_permissions(file, fs::Permissions::from_mode(0o000)).unwrap();
            assert!(
                matches!(config.read(&which), Err(ConfigFilesError::Io(_))),
                "{which:?}"
            );
        }

        let (secrets, _s) = secrets();
        write_secrets(&secrets, "KEY=value\n");
        fs::set_permissions(&secrets.file, fs::Permissions::from_mode(0o044)).unwrap();
        assert!(secrets.lookup("KEY").is_none());
        assert_eq!(
            secrets.issues(),
            [SecretsIssue::Refused {
                file: graph_dir(&secrets.file),
                reason: REFUSED_READABLE
            }]
        );

        let (state, file, _t) = state();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o044)).unwrap();
        assert_eq!(state_refused(state.read()), REFUSED_READABLE);
    }

    #[test]
    fn an_open_secrets_file_is_ignored() {
        let (port, _temp) = secrets();
        write_secrets(&port, "KEY=sentinel-value\n");
        assert!(port.lookup("KEY").is_some());
        fs::set_permissions(&port.file, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(port.lookup("KEY").is_none());
        assert_eq!(
            port.issues(),
            [SecretsIssue::Refused {
                file: graph_dir(&port.file),
                reason: REFUSED_READABLE
            }]
        );
    }

    #[test]
    fn secrets_lines_are_plain_name_equals_value() {
        let (port, _temp) = secrets();
        write_secrets(
            &port,
            "# comment\n\nA=$HOME\nexport B=x\nC = y\nD=multi\n  line\n",
        );
        assert_eq!(
            port.lookup("A").unwrap().1.expose(),
            "$HOME",
            "no expansion"
        );
        assert!(port.lookup("B").is_none());
        assert!(port.lookup("C").is_none());
        let lines: Vec<u64> = port
            .issues()
            .into_iter()
            .map(|i| match i {
                SecretsIssue::BadLine { line, .. } => line,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(lines, [4, 5, 7]);
    }
}
