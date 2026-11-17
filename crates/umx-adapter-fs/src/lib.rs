//! File system adapters (blueprint 001, c-adapter-fs). fs.graph-files, fs.config-files, fs.secrets and
//! fs.local-state now; later the source reader and change feed.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use umx_core::codec::GraphFile;
use umx_core::ports::{
    Change, Committed, CrashPoint, EntryKind, FileRef, FileStamp, FilesError, FindLimits, Found,
    FoundGraph, GraphDir, GraphFiles, GraphStamps, ModelKey, RELOCATE_INSIDE_ITSELF,
    RELOCATE_PARENT_MISSING, RELOCATE_TARGET_EXISTS, Written,
};
use umx_core::scan::FOUND_GRAPH_JSON_CAP;
use umx_core::vectors::{ShardFile, ShardKind};

mod config;
mod local_state;
mod source_reader;
pub use config::{FsConfigFiles, FsSecrets};
pub use local_state::FsLocalState;
pub use source_reader::FsSourceReader;

/// Bytes free for this user on the file system holding `path` (statvfs: available blocks times the fragment size).
/// `umx models fetch` checks it against a model's size before downloading (architect, ws004 s14).
pub fn available_bytes(path: &Path) -> std::io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "a path with a NUL byte")
    })?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c` is a valid NUL-terminated path and `stat` is written by statvfs before it is read.
    let rc = unsafe { libc::statvfs(c.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: statvfs returned 0, so it filled `stat`.
    let stat = unsafe { stat.assume_init() };
    // The field types differ by platform (f_bavail is u32 on macOS, u64 on Linux), so a cast is a no-op on some.
    #[allow(clippy::unnecessary_cast)]
    let (blocks, size) = (stat.f_bavail as u64, stat.f_frsize as u64);
    Ok(blocks.saturating_mul(size))
}

/// What the root of an installed skill is, for `umx doctor` (F111): `Ok(Some(target))` when it is a link (the
/// target as the link holds it, empty if it cannot be read; never followed), `Ok(None)` when it is anything else,
/// and Err when nothing is there or it cannot be looked at.
pub fn skill_root(dir: &Path) -> std::io::Result<Option<std::path::PathBuf>> {
    let meta = fs::symlink_metadata(dir)?;
    Ok(meta
        .file_type()
        .is_symlink()
        .then(|| fs::read_link(dir).unwrap_or_default()))
}

/// A file of an installed skill for `umx doctor` (F94, architect ruling 2026-11-12): its bytes when it is a regular
/// file of at most `cap` bytes, opened with O_NOFOLLOW and O_NONBLOCK and checked on the handle, so a file swapped
/// for a link or a FIFO after the walk saw it is refused rather than followed or waited on. None otherwise.
pub fn skill_file(path: &Path, cap: u64) -> Option<Vec<u8>> {
    match read_capped(path, cap, |_| None, || None, || {}) {
        Ok((Body::Bytes(bytes), _)) => Some(bytes),
        _ => None,
    }
}

/// The entries of a folder of an installed skill for `umx doctor` (F94): each name and whether it is a folder,
/// from a folder opened with O_NOFOLLOW and O_DIRECTORY that is still the same folder (device and inode) once
/// listed. Names starting with `.` (such as .DS_Store) are left out, as the skill's fingerprint skips them. Err when
/// the folder is a link or not a folder, changes while listed, holds anything else but plain folders and regular
/// files (a link, FIFO, socket or device), or holds more than `budget` entries: the listing stops at the first one
/// over (R230 B1), so a huge folder is never enumerated or kept in full.
pub fn skill_folder(dir: &Path, budget: usize) -> std::io::Result<Vec<(String, bool)>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let refused = |what: &str| std::io::Error::new(ErrorKind::InvalidData, what.to_owned());
    let handle = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK)
        .open(dir)?;
    let held = handle.metadata()?;
    if !held.is_dir() {
        return Err(refused("not a folder"));
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| refused("a name that is not UTF-8"))?;
        if name.starts_with('.') {
            continue;
        }
        // DirEntry::file_type does not follow links.
        let kind = entry.file_type()?;
        if !(kind.is_dir() || kind.is_file()) {
            return Err(refused("a link or special file"));
        }
        if entries.len() == budget {
            return Err(refused("more entries than the skill may hold"));
        }
        entries.push((name, kind.is_dir()));
    }
    let now = fs::symlink_metadata(dir)?;
    if (now.dev(), now.ino()) != (held.dev(), held.ino()) || now.file_type().is_symlink() {
        return Err(refused("the folder changed while it was listed"));
    }
    Ok(entries)
}

/// A folder's entries for a graph walk (ws002 s17): each UTF-8 name with what it is, never followed (a symlink is
/// `Other`), from a folder opened with O_NOFOLLOW and O_DIRECTORY that is still the same folder (device and inode)
/// once listed. Err when the folder is a link or not a folder, or changes while listed.
fn list_folder(dir: &Path) -> std::io::Result<Vec<(String, EntryKind)>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let handle = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK)
        .open(dir)?;
    let held = handle.metadata()?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // DirEntry::file_type does not follow links.
        let kind = entry.file_type()?;
        let kind = if kind.is_dir() {
            EntryKind::Folder
        } else if kind.is_file() && name == MARKER {
            EntryKind::Pending
        } else if kind.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        entries.push((name, kind));
    }
    let now = fs::symlink_metadata(dir)?;
    if (now.dev(), now.ino()) != (held.dev(), held.ino()) || now.file_type().is_symlink() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "the folder changed while it was listed",
        ));
    }
    Ok(entries)
}

/// A found folder's `graph.json`, as a walk reads it: never through a link, and only up to the cap.
fn found_graph_json(folder: &Path) -> Option<Vec<u8>> {
    let path = folder.join(GraphFile::Graph.file_name());
    match read_capped(&path, FOUND_GRAPH_JSON_CAP, |_| None, || None, || {}) {
        Ok((Body::Bytes(bytes), _)) => Some(bytes),
        _ => None,
    }
}

/// The one lock for every writer in the models folder (`umx models fetch`, the F32 widener): opens or creates
/// `<folder>/.models.lock` (0600, never through a symlink) and takes an exclusive lock on it, waiting for a holder.
/// Dropping the returned file unlocks (architect ruling 2026-11-10, ws004 s14 and s16).
pub fn models_folder_lock(folder: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(folder.join(".models.lock"))?;
    file.lock()?;
    Ok(file)
}

/// fs.graph-files: graph files on disk, confined to the given roots (req-47).
///
/// A write goes to a temp file in the same folder, is synced, then renamed over the target, so a reader or a crash
/// never sees a half-written file; then the target's folder is synced (ws004 s29: two fsyncs for one file). A set
/// of several changes also syncs the folders before the renames and adds a marker between the temp files and the renames:
/// once the marker and the folder are synced the set stands, and if the process dies before the renames finish,
/// the first call on that folder in the next process finishes them (or reports `RecoveryConflict` if a target
/// changed since). Temp files and the marker start with `.umx-tmp-`, so read and list never show them and the
/// graph folder's `.gitignore` line covers them.
///
/// Confinement resolves symlinks: the roots and the graph folder (or its nearest existing ancestor) are
/// canonicalized before the check, all I/O uses the canonical folder, and a graph file that is itself a symlink is
/// refused on read. A symlink swapped in between the check and the I/O is not caught (the daemon's user owns these
/// folders). The marker names files only by their bare names, and recovery accepts nothing else. It compares the
/// targets' bytes as read from disk, never an in-memory image, which can be stale after a pull.
pub struct FsGraphFiles {
    roots: Vec<GraphDir>,
    counter: AtomicU64,
    /// Folders recovered in this process; recovery runs before the first call on each. Also the one boundary
    /// every call runs under: a set holds it from its recovery to its end, so a set queued behind one that left
    /// its marker (NotSynced) recovers that marker first and never writes over it (review R54-1), and two sets
    /// never race for the one marker name.
    recovered: Mutex<BTreeSet<PathBuf>>,
    /// Folders whose names this instance has made durable ([`names_durable`]): once per folder per daemon.
    durable: Durable,
    /// Test hooks, compiled only into this crate's tests: release builds cannot crash a set on purpose.
    #[cfg(test)]
    crash: Mutex<Option<CrashPoint>>,
    #[cfg(test)]
    fail_temp: Mutex<Option<GraphFile>>,
    #[cfg(test)]
    bytes_read: AtomicU64,
    /// The next relocate answers as a rename across file systems would.
    #[cfg(test)]
    cross_device: Mutex<bool>,
    /// Run on the next relocate's target after its checks and before its rename: another writer's folder.
    #[cfg(test)]
    before_relocate: Mutex<Option<fn(&Path)>>,
}

/// Temp files and the marker start with this, so they never collide with a graph file name.
const TEMP_PREFIX: &str = ".umx-tmp-";
/// A set's marker: one JSON line per change, written after the temp files and before the renames.
const MARKER: &str = ".umx-tmp-set.json";

/// The marker's hash, named on its first line. Recovery compares hashes only under this name; a marker naming
/// another (written by a later or earlier build) is a conflict, never a comparison of the wrong numbers.
const HASH: &str = "fnv1a64-words-v1";

/// FNV-1a 64 over little-endian u64 words (the last one zero-padded), then the length: detects a file changed out
/// of band since a crash, at about an eighth of byte-wise FNV-1a's cost. Not security. Written here because the
/// std hasher's algorithm may change between Rust releases, and a marker must read the same after an upgrade.
fn hash(bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let (words, rest) = bytes.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(PRIME);
    }
    if !rest.is_empty() {
        let mut w = [0u8; 8];
        w[..rest.len()].copy_from_slice(rest);
        h = (h ^ u64::from_le_bytes(w)).wrapping_mul(PRIME);
    }
    (h ^ bytes.len() as u64).wrapping_mul(PRIME)
}

/// A file's size and hash, or absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stamp {
    Absent,
    File { size: u64, hash: u64 },
}

impl Stamp {
    fn of(bytes: Option<&[u8]>) -> Stamp {
        bytes.map_or(Stamp::Absent, |b| Stamp::File {
            size: b.len() as u64,
            hash: hash(b),
        })
    }

    fn to_json(self) -> Value {
        match self {
            Stamp::Absent => json!("absent"),
            Stamp::File { size, hash } => json!({ "size": size, "hash": format!("{hash:016x}") }),
        }
    }

    fn from_json(v: &Value) -> Option<Stamp> {
        if v.as_str() == Some("absent") {
            return Some(Stamp::Absent);
        }
        let hash = v["hash"].as_str().filter(|h| h.len() == 16)?;
        Some(Stamp::File {
            size: v["size"].as_u64()?,
            hash: u64::from_str_radix(hash, 16).ok()?,
        })
    }
}

/// One marker line: replace `target` with `temp` (or remove it), expecting `old` before and `new` after.
struct Line {
    target: FileRef,
    temp: Option<String>,
    old: Stamp,
    new: Stamp,
}

impl Line {
    fn to_json(&self) -> String {
        json!({
            "target": self.target.path(),
            "temp": self.temp.as_deref().unwrap_or("remove"),
            "old": self.old.to_json(),
            "new": self.new.to_json(),
        })
        .to_string()
    }

    /// Accepts only a path [`FileRef::path`] writes as the target (a graph file name, or a checked shard path) and
    /// a bare `.umx-tmp-` name (or `remove`) as the temp, which lives beside its target, so a marker written by
    /// anyone else cannot name a path outside the folder.
    fn from_json(text: &str) -> Option<Line> {
        let v: Value = text.parse().ok()?;
        let target = FileRef::parse(v["target"].as_str()?)?;
        let temp = match v["temp"].as_str()? {
            "remove" if target != FileRef::Graph(GraphFile::GitIgnore) => None,
            t if is_temp_name(t) => Some(t.to_owned()),
            _ => return None,
        };
        let (old, new) = (Stamp::from_json(&v["old"])?, Stamp::from_json(&v["new"])?);
        // A removal leaves the target absent; a rename leaves it holding the temp's bytes.
        (temp.is_some() != (new == Stamp::Absent)).then_some(Line {
            target,
            temp,
            old,
            new,
        })
    }
}

/// A bare temp file name: the prefix, then letters, digits, `.`, `-` and `_` only (no `/`, so never another folder).
fn is_temp_name(name: &str) -> bool {
    name.len() <= 128
        && name != MARKER
        && name.strip_prefix(TEMP_PREFIX).is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        })
}

/// What a capped read of one path found ([`read_capped`]).
pub(crate) enum Body {
    Absent,
    /// A symlink: never followed, since it could lead out of the roots.
    Symlink,
    /// A folder, a FIFO or anything else that is not a regular file.
    NotRegular,
    /// Over the cap: this many bytes, and none of them read.
    TooLarge(u64),
    /// The open file failed the caller's trust check (owner or mode, F49), with its reason; none of it read.
    Refused(&'static str),
    Bytes(Vec<u8>),
}

/// The one way this crate reads a file it did not just write (graph files, and umx's config, secrets and state
/// files, F45). The open never follows a symlink and never waits (a FIFO); the size comes from the open handle, so
/// the file checked is the file read; and the read stops one byte past the cap, so a file that grew after the
/// check is refused too. An over-cap body is never held. `vet` judges the open file's metadata (owner and mode for
/// umx's own files, F49), so a file swapped or chmodded after any earlier look is judged as it is read. `denied`
/// runs only when the open itself is refused permission, and may name a refusal for it (F51); a later error from
/// the same handle never reaches it. `after_check` runs between the size check and the read (tests use it to grow the file). Returns the body and
/// the bytes taken from disk.
pub(crate) fn read_capped(
    path: &Path,
    cap: u64,
    vet: impl FnOnce(&fs::Metadata) -> Option<&'static str>,
    denied: impl FnOnce() -> Option<&'static str>,
    after_check: impl FnOnce(),
) -> std::io::Result<(Body, u64)> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let opened = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    let handle = match opened {
        Ok(h) => h,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok((Body::Absent, 0));
        }
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok((Body::Symlink, 0)),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            return match denied() {
                Some(reason) => Ok((Body::Refused(reason), 0)),
                None => Err(e),
            };
        }
        Err(e) => return Err(e),
    };
    #[cfg(test)]
    if let Some(folder) = path.parent() {
        tests::run_hook(&tests::AFTER_OPEN, folder);
    }
    let meta = handle.metadata()?;
    if !meta.is_file() {
        return Ok((Body::NotRegular, 0));
    }
    if let Some(reason) = vet(&meta) {
        return Ok((Body::Refused(reason), 0));
    }
    if meta.len() > cap {
        return Ok((Body::TooLarge(meta.len()), 0));
    }
    after_check();
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&handle).take(cap + 1).read_to_end(&mut bytes)?;
    let read = bytes.len() as u64;
    if read > cap {
        let now = handle.metadata().map_or(0, |m| m.len());
        return Ok((Body::TooLarge(now.max(read)), read));
    }
    Ok((Body::Bytes(bytes), read))
}

/// The most bytes a set's marker may hold: one line per change, about 250 bytes each, so some 4000 changes.
const MARKER_CAP: u64 = 1 << 20;

/// One change takes [`FsGraphFiles::write_one`]; tests can send it the general way to measure the difference.
fn fast_path() -> bool {
    #[cfg(test)]
    if tests::planted(tests::Plant::GeneralPath) {
        return false;
    }
    true
}

pub(crate) fn sync_folder(folder: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if tests::planted(tests::Plant::FoldersUnsynced) {
        return Ok(());
    }
    #[cfg(test)]
    if tests::fault(&tests::Fault::Sync(folder.to_path_buf())) {
        return Err(std::io::Error::other("planted folder sync failure"));
    }
    fs::File::open(folder)?.sync_all()?;
    #[cfg(test)]
    tests::note(tests::Event::DirSynced(folder.to_path_buf()));
    Ok(())
}

/// Syncs a temp file's bytes before it is renamed into place: without it a crash could leave the new name on
/// bytes that never reached the disk.
pub(crate) fn sync_temp(file: &fs::File, _path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if tests::planted(tests::Plant::TempUnsynced) {
        return Ok(());
    }
    file.sync_all()?;
    #[cfg(test)]
    tests::note(tests::Event::FileSynced(_path.to_path_buf()));
    Ok(())
}

pub(crate) fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)?;
    #[cfg(test)]
    tests::note(tests::Event::Renamed(from.to_path_buf(), to.to_path_buf()));
    Ok(())
}

/// Creates `path` and any missing folder above it (each with `mode`, before the umask), one at a time from the
/// top. It syncs nothing: [`names_durable`] makes their names durable before a write says Durable.
pub(crate) fn make_dirs(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut missing = Vec::new();
    let mut at = Some(path);
    while let Some(p) = at {
        match fs::symlink_metadata(p) {
            Err(e) if e.kind() == ErrorKind::NotFound => missing.push(p.to_path_buf()),
            _ => break,
        }
        at = p.parent();
    }
    for dir in missing.into_iter().rev() {
        if let Err(e) = fs::DirBuilder::new().mode(mode).create(&dir)
            && e.kind() != ErrorKind::AlreadyExists
        {
            return Err(e);
        }
        #[cfg(test)]
        tests::note(tests::Event::Made(dir));
    }
    Ok(())
}

/// Folders whose names this process has synced into the folders above them, with the inode synced (a folder
/// removed and made again is a new one).
pub(crate) type Durable = Mutex<BTreeMap<PathBuf, u64>>;

/// Makes the name of each folder from `from` up to `top` durable in the folder above it, syncing that folder the
/// first time this process (`durable`) meets each one, so the topmost sync is `top`'s parent and never above.
/// Existence is never proof: a folder made by a write that failed, by an earlier process, or by anyone else is
/// not in the set until a sync for it succeeds, so a failed sync is tried again by the next write (R1-F1).
pub(crate) fn names_durable(durable: &Durable, from: &Path, top: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    #[cfg(test)]
    if tests::planted(tests::Plant::NamesForgotten) {
        return Ok(());
    }
    let mut synced = durable.lock().unwrap_or_else(|e| e.into_inner());
    for dir in from.ancestors() {
        let Some(above) = dir.parent() else { break };
        let ino = fs::symlink_metadata(dir)?.ino();
        if synced.get(dir) != Some(&ino) {
            sync_folder(above)?;
            synced.insert(dir.to_path_buf(), ino);
        }
        if dir == top || !dir.starts_with(top) {
            break;
        }
    }
    Ok(())
}

fn remove_if_there(path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    tests::note(tests::Event::Removed(path.to_path_buf()));
    match fs::remove_file(path) {
        Err(e) if !matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => Err(e),
        _ => Ok(()),
    }
}

/// True if `path` exists and is a symlink.
fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// The folder a file lives in inside the canonical graph folder, or `Err` if a part of the way is a symlink (it
/// could lead out of the roots). Shard folders' parts come from a checked `ModelKey`, so none is `..`.
fn parent_of(folder: &Path, file: &FileRef) -> Result<PathBuf, ()> {
    match file {
        FileRef::Graph(_) | FileRef::Receipts => Ok(folder.to_path_buf()),
        FileRef::Shard(s) => space_folder(folder, s.kind(), s.key()),
        FileRef::SimilarMeta(key) => space_folder(folder, ShardKind::Similar, key),
    }
}

/// `embeddings/<model@dims>` or `similar/<model@dims>` inside the graph folder, refusing a symlink on the way.
fn space_folder(folder: &Path, kind: ShardKind, key: &ModelKey) -> Result<PathBuf, ()> {
    let mut out = folder.to_path_buf();
    for part in kind.folder(key).split('/') {
        out.push(part);
        if is_symlink(&out) {
            return Err(());
        }
    }
    Ok(out)
}

/// The vector space folders under `embeddings/` and `similar/` (not symlinks), for recovery's temp sweep.
fn space_folders(folder: &Path) -> Vec<PathBuf> {
    [ShardKind::Vectors, ShardKind::Similar]
        .into_iter()
        .flat_map(|kind| {
            let top = folder.join(kind.top());
            if is_symlink(&top) {
                return Vec::new();
            }
            fs::read_dir(&top)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .map(|e| e.path())
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect()
}

fn io_error_at(op: &str, file: &FileRef, e: std::io::Error) -> FilesError {
    FilesError::io(format!("{op} {file}: {}", e.kind()))
}

impl FsGraphFiles {
    /// Roots are canonicalized here; a root that cannot be resolved is kept as given.
    pub fn new(roots: Vec<GraphDir>) -> FsGraphFiles {
        let roots = roots
            .into_iter()
            .map(|r| {
                resolve(Path::new(r.as_str()))
                    .ok()
                    .and_then(dir_of)
                    .unwrap_or(r)
            })
            .collect();
        FsGraphFiles {
            roots,
            counter: AtomicU64::new(0),
            recovered: Mutex::default(),
            durable: Mutex::default(),
            #[cfg(test)]
            crash: Mutex::new(None),
            #[cfg(test)]
            fail_temp: Mutex::new(None),
            #[cfg(test)]
            bytes_read: AtomicU64::new(0),
            #[cfg(test)]
            cross_device: Mutex::new(false),
            #[cfg(test)]
            before_relocate: Mutex::new(None),
        }
    }

    /// The local vector cache for secured graphs (req-46): one root, `root` itself, trusted like the rest of
    /// `$UMX_HOME`. It is created 0700 if missing; an existing one must be a real folder (not a symlink), owned by
    /// this user, with no group or other bits. If it is not, the instance has no roots, so it refuses every call
    /// (`OutsideRoots`) and no secured vector is written anywhere; the reason is returned.
    pub fn vector_cache(root: &Path) -> (FsGraphFiles, Option<&'static str>) {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        if fs::symlink_metadata(root).is_err() {
            let _ = fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(root);
        }
        let refused = match fs::symlink_metadata(root) {
            Err(_) => Some("the vector cache folder cannot be created"),
            Ok(m) if m.file_type().is_symlink() => Some("the vector cache folder is a symlink"),
            Ok(m) if !m.is_dir() => Some("the vector cache path is not a folder"),
            Ok(m) if m.uid() != config::current_uid() => {
                Some("the vector cache folder belongs to another user")
            }
            Ok(m) if m.mode() & 0o077 != 0 => {
                Some("the vector cache folder is open to other users (chmod 700 it)")
            }
            Ok(_) => None,
        };
        let roots = match (refused, root.to_str().and_then(|r| GraphDir::parse(r).ok())) {
            (None, Some(dir)) => vec![dir],
            _ => Vec::new(),
        };
        (FsGraphFiles::new(roots), refused.or(roots_missing(root)))
    }

    /// Tests only: stops the next `write_set` at `point`, as if the process died there; nothing is cleaned up.
    #[cfg(test)]
    pub fn crash_at_for_tests(&self, point: Option<CrashPoint>) {
        *self.crash.lock().unwrap() = point;
    }

    /// Tests only: makes writing this file's temp fail, as on a full disk.
    #[cfg(test)]
    pub fn fail_temp_for_tests(&self, file: Option<GraphFile>) {
        *self.fail_temp.lock().unwrap() = file;
    }

    /// Tests only: a new instance over the same roots, as after a restart.
    #[cfg(test)]
    pub fn restart_for_tests(&self) -> FsGraphFiles {
        FsGraphFiles::new(self.roots.clone())
    }

    /// Reads one file under its cap (security), mapping what [`FsGraphFiles::read_body_stamped`] found
    /// to the port's errors, with the stamp of the handle the bytes came from.
    fn read_capped(
        &self,
        dir: &GraphDir,
        folder: &Path,
        file: &FileRef,
    ) -> Result<(Option<Vec<u8>>, FileStamp), FilesError> {
        let cap = file.cap();
        match self.read_body_stamped(&folder.join(file.path()), cap) {
            Ok((Body::Absent, _)) => Ok((None, FileStamp::ABSENT)),
            Ok((Body::Bytes(bytes), stamp)) => Ok((Some(bytes), stamp)),
            Ok((Body::Symlink, _)) => Err(FilesError::OutsideRoots(dir.clone())),
            Ok((Body::NotRegular, _)) => {
                Err(FilesError::io(format!("read {file}: not a regular file")))
            }
            Ok((Body::Refused(reason), _)) => Err(FilesError::io(format!("read {file}: {reason}"))),
            Ok((Body::TooLarge(size), _)) => Err(FilesError::TooLarge {
                file: file.clone(),
                size,
                cap,
            }),
            Err(e) => Err(io_error_at("read", file, e)),
        }
    }

    /// The one way this adapter reads a graph file's bytes: reads, recovery's stamps, the marker and a set's old
    /// stamps (review F1), through [`read_capped`].
    fn read_body(&self, path: &Path, cap: u64) -> std::io::Result<Body> {
        self.read_body_stamped(path, cap).map(|(body, _)| body)
    }

    /// [`FsGraphFiles::read_body`] with the F144 stamp of the open handle, taken by fstat before the body is read
    /// (the metadata the size check used): a write that lands during or after the read leaves the file unlike
    /// the stamp, so the next check sees it. Absent unless the body is `Bytes`.
    fn read_body_stamped(&self, path: &Path, cap: u64) -> std::io::Result<(Body, FileStamp)> {
        let mut stamp = FileStamp::ABSENT;
        let (body, _read) = read_capped(
            path,
            cap,
            |meta| {
                stamp = stamp_of(meta);
                None
            },
            || None,
            || {
                #[cfg(test)]
                if let Some(folder) = path.parent() {
                    tests::run_hook(&tests::AFTER_SIZE_CHECK, folder);
                }
            },
        )?;
        #[cfg(test)]
        self.bytes_read.fetch_add(_read, Ordering::Relaxed);
        Ok((body, stamp))
    }

    /// Tests only: the bytes every read so far took from disk (gf_c26: an over-cap body is never read).
    #[cfg(test)]
    pub fn bytes_read_for_tests(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn crash_here(&self, point: CrashPoint) -> bool {
        let mut armed = self.crash.lock().unwrap();
        let hit = *armed == Some(point);
        if hit {
            *armed = None;
        }
        hit
    }

    #[cfg(not(test))]
    fn crash_here(&self, _point: CrashPoint) -> bool {
        false
    }

    #[cfg(test)]
    fn temp_refused(&self, file: GraphFile) -> bool {
        *self.fail_temp.lock().unwrap() == Some(file)
    }

    #[cfg(not(test))]
    fn temp_refused(&self, _file: GraphFile) -> bool {
        false
    }

    /// The canonical folder for `dir`, if it is inside a root.
    fn check(&self, dir: &GraphDir) -> Result<PathBuf, FilesError> {
        let canonical =
            resolve(Path::new(dir.as_str())).map_err(|e| io_error("resolve", None, e))?;
        match dir_of(canonical.clone()) {
            Some(c) if self.roots.iter().any(|r| c.is_within(r)) => Ok(canonical),
            _ => Err(FilesError::OutsideRoots(dir.clone())),
        }
    }

    /// Runs once per folder per process, before anything else touches it. A marker means a crash after a set's
    /// point of no return: finish its renames, unless a target changed since (then change nothing and report
    /// the conflict, on this call and every later one). Temp files without a marker are from a crash before the
    /// point of no return: remove them, the old state stands.
    fn recover(&self, recovered: &mut BTreeSet<PathBuf>, folder: &Path) -> Result<(), FilesError> {
        if recovered.contains(folder) {
            return Ok(());
        }
        let err = |e| io_error("recover", None, e);
        let mut leftovers: Vec<String> = match fs::read_dir(folder) {
            Ok(entries) => entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| n.starts_with(TEMP_PREFIX))
                .collect(),
            // No folder, nothing to recover; a folder this process makes later starts clean.
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                recovered.insert(folder.to_path_buf());
                return Ok(());
            }
            Err(e) => return Err(err(e)),
        };
        if leftovers.iter().any(|n| n == MARKER) {
            let unreadable = || FilesError::io("recover folder: the marker is not one umx wrote");
            let text = match self
                .read_body(&folder.join(MARKER), MARKER_CAP)
                .map_err(err)?
            {
                Body::Bytes(bytes) => String::from_utf8(bytes).map_err(|_| unreadable())?,
                _ => return Err(unreadable()),
            };
            let mut rows = text.lines();
            let named = rows
                .next()
                .and_then(|h| h.parse::<Value>().ok())
                .ok_or_else(unreadable)?;
            let lines: Vec<Line> = rows
                .map(Line::from_json)
                .collect::<Option<_>>()
                .ok_or_else(unreadable)?;
            if lines.is_empty() {
                return Err(unreadable());
            }
            if named["hash"] != HASH {
                return Err(FilesError::RecoveryConflict(
                    lines.iter().map(|l| l.target.clone()).collect(),
                ));
            }
            let mut todo = Vec::new();
            let mut conflicts = Vec::new();
            let mut parents = BTreeSet::new();
            for line in &lines {
                // A temp holds its target's new bytes, so both have the target's cap. Anything over it, or not a
                // regular file, matches no stamp umx wrote: a conflict, found without reading the body.
                let cap = line.target.cap();
                let stamp = |path: &Path| -> Result<Option<Stamp>, FilesError> {
                    Ok(match self.read_body(path, cap).map_err(err)? {
                        Body::Absent => Some(Stamp::Absent),
                        Body::Bytes(b) => Some(Stamp::of(Some(&b))),
                        Body::Symlink | Body::NotRegular | Body::TooLarge(_) | Body::Refused(_) => {
                            None
                        }
                    })
                };
                // A shard folder swapped for a symlink since the crash: never follow it.
                let Ok(parent) = parent_of(folder, &line.target) else {
                    conflicts.push(line.target.clone());
                    continue;
                };
                let now = stamp(&folder.join(line.target.path()))?;
                if now == Some(line.new) {
                    continue;
                }
                let temp_ok = match &line.temp {
                    Some(t) => stamp(&parent.join(t))? == Some(line.new),
                    None => true,
                };
                if now == Some(line.old) && temp_ok {
                    todo.push((line, parent));
                } else {
                    conflicts.push(line.target.clone());
                }
            }
            if !conflicts.is_empty() {
                return Err(FilesError::RecoveryConflict(conflicts));
            }
            for (line, parent) in todo {
                let target = folder.join(line.target.path());
                match &line.temp {
                    Some(t) => fs::rename(parent.join(t), target),
                    None => remove_if_there(&target),
                }
                .map_err(err)?;
                parents.insert(parent);
            }
            for parent in &parents {
                sync_folder(parent).map_err(err)?;
            }
            sync_folder(folder).map_err(err)?;
            fs::remove_file(folder.join(MARKER)).map_err(err)?;
            leftovers.retain(|n| n != MARKER);
        }
        // Orphans of a crash before a point of no return, here and in the shard folders. One that will not go (a
        // folder by that name) is left; it is never read as the graph.
        for name in &leftovers {
            let _ = fs::remove_file(folder.join(name));
        }
        if !leftovers.is_empty() {
            sync_folder(folder).map_err(err)?;
        }
        for space in space_folders(folder) {
            let temps: Vec<PathBuf> = fs::read_dir(&space)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
                        .map(|e| e.path())
                        .collect()
                })
                .unwrap_or_default();
            for temp in &temps {
                let _ = fs::remove_file(temp);
            }
            if !temps.is_empty() {
                sync_folder(&space).map_err(err)?;
            }
        }
        recovered.insert(folder.to_path_buf());
        Ok(())
    }

    /// Writes and syncs a new temp file named for `what`; returns its name and its still-open handle, which a set
    /// keeps for a content file so it can stamp the file it renamed by fstat, never by path.
    fn temp(&self, folder: &Path, what: &str, bytes: &[u8]) -> std::io::Result<(String, fs::File)> {
        loop {
            let n = self.counter.fetch_add(1, Ordering::Relaxed);
            let name = format!("{TEMP_PREFIX}{n}-{what}");
            let path = folder.join(&name);
            let mut f = match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                other => other?,
            };
            #[cfg(test)]
            if tests::fault(&tests::Fault::Temp) {
                let _ = fs::remove_file(&path);
                return Err(std::io::Error::from(ErrorKind::StorageFull));
            }
            let written = f.write_all(bytes).and_then(|()| sync_temp(&f, &path));
            if let Err(e) = written {
                let _ = fs::remove_file(&path);
                return Err(e);
            }
            return Ok((name, f));
        }
    }

    /// The three content files' stamps by path now (lstat: a link is stamped as itself, never followed).
    fn stamps_in(folder: &Path) -> Result<GraphStamps, FilesError> {
        let mut out = GraphStamps::ABSENT;
        for file in GraphStamps::FILES {
            match fs::symlink_metadata(folder.join(file.file_name())) {
                Ok(m) => out.set(file, stamp_of(&m)),
                Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
                Err(e) => return Err(io_error("stat", Some(file), e)),
            }
        }
        Ok(out)
    }

    /// A check before the point of no return (F144): the content files as they are now, or `Changed` naming each
    /// that differs from `expect`. A set of several runs it twice: before its marker is written (after the old
    /// targets are read for it) and after, just before the first rename; one change just before its rename.
    /// `also` files (ws003 s09) are stamped in the same pass, by path, and named after the content files.
    fn stamps_before_commit(
        folder: &Path,
        expect: Option<&GraphStamps>,
        also: &[(FileRef, FileStamp)],
    ) -> Result<GraphStamps, FilesError> {
        #[cfg(test)]
        tests::run_hook(&tests::BEFORE_STAMP_CHECK, folder);
        let now = FsGraphFiles::stamps_in(folder)?;
        let mut changed: Vec<FileRef> = expect
            .map(|e| e.changed(&now))
            .unwrap_or_default()
            .into_iter()
            .map(FileRef::from)
            .collect();
        for (file, stamp) in also {
            let path = parent_of(folder, file)
                .map_err(|()| FilesError::io(format!("stat {file}: a symlink on the way")))?
                .join(file.path().rsplit('/').next().unwrap_or_default());
            let found = match fs::symlink_metadata(&path) {
                Ok(m) => stamp_of(&m),
                Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                    FileStamp::ABSENT
                }
                Err(e) => return Err(io_error_at("stat", file, e)),
            };
            if found != *stamp {
                changed.push(file.clone());
            }
        }
        match changed.is_empty() {
            true => Ok(now),
            false => Err(FilesError::Changed(changed)),
        }
    }

    /// The stamps after a set: each content file it wrote by fstat on its temp's handle after the rename, each it
    /// removed absent, the rest as `before` found them. A handle that cannot be stat'ed stamps absent, which a later
    /// check sees as changed (fail closed).
    fn stamps_after(
        before: GraphStamps,
        changes: &[Change<'_>],
        handles: &[Option<fs::File>],
    ) -> GraphStamps {
        let mut out = before;
        for (c, handle) in changes.iter().zip(handles) {
            let FileRef::Graph(file) = c.file else {
                continue;
            };
            let stamp = match (c.bytes, handle) {
                (None, _) => FileStamp::ABSENT,
                (Some(_), Some(h)) => h.metadata().map_or(FileStamp::ABSENT, |m| stamp_of(&m)),
                (Some(_), None) => FileStamp::ABSENT,
            };
            out.set(file, stamp);
        }
        out
    }

    /// One change (ws004 s29): its temp file synced, renamed over the target (or the target removed), then the
    /// target's own folder synced, then the name of each folder from there up to the graph folder synced into the
    /// folder above it, once per folder per instance ([`names_durable`]; the graph folder's parent is the highest). One rename is atomic, so a crash
    /// at any point leaves the old file or the new one: no marker, and no folder sync before the rename (a temp
    /// file's name need not outlive a crash; recovery removes a leftover one). Two fsyncs for a file in an existing
    /// folder, where a set of several pays five or more. With `expect`, the content files are stamped just before the
    /// rename ([`FsGraphFiles::stamps_before_commit`]).
    fn write_one(
        &self,
        recovered: &mut BTreeSet<PathBuf>,
        dir: &GraphDir,
        folder: &Path,
        c: &Change<'_>,
        (expect, also): (Option<&GraphStamps>, &[(FileRef, FileStamp)]),
    ) -> Result<Committed, FilesError> {
        let at = |e| io_error_at("write", &c.file, e);
        let outside = |()| FilesError::OutsideRoots(dir.clone());
        make_dirs(folder, 0o777).map_err(at)?;
        // Shards live under `embeddings/<model@dims>/`: made here, never through a symlink (checked again after).
        let parent = parent_of(folder, &c.file).map_err(outside)?;
        if parent != folder {
            make_dirs(&parent, 0o777).map_err(at)?;
            parent_of(folder, &c.file).map_err(outside)?;
        }
        let made = match c.bytes {
            None => None,
            Some(bytes) => {
                let name = c.file.path();
                let what = name.rsplit('/').next().unwrap_or(&name);
                let refused = matches!(&c.file, FileRef::Graph(f) if self.temp_refused(*f));
                let temp = match refused {
                    true => Err(std::io::Error::from(ErrorKind::StorageFull)),
                    false => self.temp(&parent, what, bytes),
                };
                Some(temp.map_err(at)?)
            }
        };
        let (temp, handle) = match made {
            Some((name, handle)) => (Some(name), Some(handle)),
            None => (None, None),
        };
        for point in [CrashPoint::AfterTemps, CrashPoint::AfterMarker] {
            if self.crash_here(point) {
                recovered.remove(folder);
                return Err(FilesError::io("crash"));
            }
        }
        #[cfg(test)]
        tests::run_hook(&tests::AFTER_MARKER, folder);
        let before = match FsGraphFiles::stamps_before_commit(folder, expect, also) {
            Ok(before) => before,
            Err(e) => {
                if let Some(name) = &temp {
                    let _ = fs::remove_file(parent.join(name));
                }
                return Err(e);
            }
        };
        let target = folder.join(c.file.path());
        let done = match &temp {
            Some(name) => rename(&parent.join(name), &target),
            None => remove_if_there(&target),
        };
        if let Err(e) = done {
            if let Some(name) = &temp {
                let _ = fs::remove_file(parent.join(name));
            }
            return Err(at(e));
        }
        if self.crash_here(CrashPoint::AfterFirstRename) {
            recovered.remove(folder);
            return Err(FilesError::io("crash"));
        }
        let stamps = FsGraphFiles::stamps_after(before, core::slice::from_ref(c), &[handle]);
        let written = match sync_folder(&parent)
            .and_then(|()| names_durable(&self.durable, &parent, folder))
        {
            Ok(()) => Written::Durable,
            Err(e) => Written::NotSynced(io_error("sync", None, e)),
        };
        Ok(Committed { written, stamps })
    }

    /// Runs under the boundary (`recovered` is its guard), after recovery.
    fn write_set_in(
        &self,
        recovered: &mut BTreeSet<PathBuf>,
        dir: &GraphDir,
        folder: &Path,
        changes: &[Change<'_>],
        (expect, also): (Option<&GraphStamps>, &[(FileRef, FileStamp)]),
    ) -> Result<Committed, FilesError> {
        if let [one] = changes
            && fast_path()
        {
            return self.write_one(recovered, dir, folder, one, (expect, also));
        }
        let first = &changes[0].file;
        fs::create_dir_all(folder).map_err(|e| io_error_at("write", first, e))?;
        // Each file's own folder: shards live under `embeddings/<model@dims>/`. Made here, and never through a
        // symlink, so nothing is written outside the roots.
        let mut parents: Vec<PathBuf> = Vec::new();
        for c in changes {
            let parent =
                parent_of(folder, &c.file).map_err(|()| FilesError::OutsideRoots(dir.clone()))?;
            if parent != folder {
                fs::create_dir_all(&parent).map_err(|e| io_error_at("write", &c.file, e))?;
                parent_of(folder, &c.file).map_err(|()| FilesError::OutsideRoots(dir.clone()))?;
            }
            parents.push(parent);
        }
        let touched: BTreeSet<&PathBuf> =
            parents.iter().filter(|p| p.as_path() != folder).collect();
        let cleanup = |names: &[Option<String>]| {
            for (name, parent) in names.iter().zip(&parents) {
                if let Some(name) = name {
                    let _ = fs::remove_file(parent.join(name));
                }
            }
        };
        // 1. Temp files, each synced, each beside its target so its rename is atomic. A content file's handle stays
        // open until its rename, to stamp it after (fstat; never by path). Only the three: a set of many shards
        // must not hold a handle per shard.
        let mut temps: Vec<Option<String>> = Vec::new();
        let mut handles: Vec<Option<fs::File>> = Vec::new();
        for (c, parent) in changes.iter().zip(&parents) {
            let Some(bytes) = c.bytes else {
                temps.push(None);
                handles.push(None);
                continue;
            };
            let failing = match &c.file {
                FileRef::Graph(f) => self.temp_refused(*f),
                FileRef::Shard(_) | FileRef::SimilarMeta(_) | FileRef::Receipts => false,
            };
            let name = c.file.path();
            let what = name.rsplit('/').next().unwrap_or(&name);
            let made = if failing {
                Err(std::io::Error::from(ErrorKind::StorageFull))
            } else {
                self.temp(parent, what, bytes)
            };
            match made {
                Ok((name, handle)) => {
                    temps.push(Some(name));
                    let stamped =
                        matches!(&c.file, FileRef::Graph(f) if *f != GraphFile::GitIgnore);
                    handles.push(stamped.then_some(handle));
                }
                Err(e) => {
                    cleanup(&temps);
                    return Err(io_error_at("write", &c.file, e));
                }
            }
        }
        // The temp files' folder entries, and any folder made above, are on disk before the point of no return.
        let synced = touched
            .iter()
            .try_for_each(|p| sync_folder(p))
            .and_then(|()| match touched.is_empty() {
                true => Ok(()),
                // Each touched space folder's top (`embeddings/`, `similar/`), then the graph folder.
                false => touched
                    .iter()
                    .filter_map(|p| p.parent())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .try_for_each(sync_folder)
                    .and_then(|()| sync_folder(folder)),
            });
        if let Err(e) = synced {
            cleanup(&temps);
            return Err(io_error_at("write", first, e));
        }
        if self.crash_here(CrashPoint::AfterTemps) {
            // A crashed set left its files as they are: the next call on the folder recovers them (gf_c23).
            recovered.remove(folder);
            return Err(FilesError::io("crash"));
        }
        #[cfg(test)]
        tests::run_hook(&tests::BEFORE_OLD_STAMPS, folder);
        // 2. The marker: written with the old targets' stamps as a temp file and synced, then the first F144 check,
        // then published (renamed into place and the folder synced): the point of no return, but for the second
        // check below, which takes the marker back. One change needs no marker, since one rename is atomic.
        let marked = changes.len() > 1;
        if marked {
            let mut text = json!({ "hash": HASH }).to_string();
            text.push('\n');
            for (c, temp) in changes.iter().zip(&temps) {
                let target = folder.join(c.file.path());
                let old = match self.read_body(&target, c.file.cap()) {
                    Ok(Body::Absent) => Stamp::Absent,
                    Ok(Body::Bytes(bytes)) => Stamp::of(Some(&bytes)),
                    // The file being replaced is over its cap (put there by hand or a pull): refused unread, as a
                    // read of it would be.
                    Ok(Body::TooLarge(size)) => {
                        cleanup(&temps);
                        return Err(FilesError::TooLarge {
                            file: c.file.clone(),
                            size,
                            cap: c.file.cap(),
                        });
                    }
                    Ok(Body::Symlink | Body::NotRegular | Body::Refused(_)) => {
                        cleanup(&temps);
                        return Err(FilesError::io(format!(
                            "write {}: not a regular file",
                            c.file
                        )));
                    }
                    Err(e) => {
                        cleanup(&temps);
                        return Err(io_error_at("write", &c.file, e));
                    }
                };
                let line = Line {
                    target: c.file.clone(),
                    temp: temp.clone(),
                    old,
                    new: Stamp::of(c.bytes),
                };
                text.push_str(&line.to_json());
                text.push('\n');
            }
            // Recovery reads the marker under its cap, so a set must never write a bigger one.
            if text.len() as u64 > MARKER_CAP {
                cleanup(&temps);
                return Err(FilesError::Invalid(
                    "a set with more changes than its marker can hold",
                ));
            }
            let marker_temp = match self.temp(folder, "set.json", text.as_bytes()) {
                Ok((name, _)) => folder.join(name),
                Err(e) => {
                    cleanup(&temps);
                    return Err(io_error_at("write", first, e));
                }
            };
            // The first F144 check, between the marker's old stamps and its publication (review R1): a change that
            // landed before the old stamps were taken is refused here, and one that lands after differs from them,
            // so a recovery after a crash calls it a conflict and never rolls forward over it.
            if let Err(e) = FsGraphFiles::stamps_before_commit(folder, expect, also) {
                let _ = fs::remove_file(&marker_temp);
                cleanup(&temps);
                return Err(e);
            }
            let published = rename(&marker_temp, &folder.join(MARKER))
                .inspect_err(|_| {
                    let _ = fs::remove_file(&marker_temp);
                })
                .and_then(|()| sync_folder(folder));
            if let Err(e) = published {
                cleanup(&temps);
                let _ = fs::remove_file(folder.join(MARKER));
                return Err(io_error_at("write", first, e));
            }
        }
        if self.crash_here(CrashPoint::AfterMarker) {
            // A crashed set left its files as they are: the next call on the folder recovers them (gf_c23).
            recovered.remove(folder);
            return Err(FilesError::io("crash"));
        }
        #[cfg(test)]
        tests::run_hook(&tests::AFTER_MARKER, folder);
        // The second F144 check, as late as the set allows: every temp and the marker are synced and the old
        // targets read, nothing is renamed yet. A refusal takes the marker back first, so no recovery finishes this
        // set; a crash before it goes leaves recovery to compare the targets with the marker's old stamps (a
        // conflict, never a write over a change).
        let before = match FsGraphFiles::stamps_before_commit(folder, expect, also) {
            Ok(before) => before,
            Err(e) => {
                if marked {
                    let _ = fs::remove_file(folder.join(MARKER)).and_then(|()| sync_folder(folder));
                }
                cleanup(&temps);
                return Err(e);
            }
        };
        // 3. Renames and removes, in the order given. From here the set stands, so a failure is NotSynced.
        for (i, ((c, temp), parent)) in changes.iter().zip(&temps).zip(&parents).enumerate() {
            let target = folder.join(c.file.path());
            let done = match temp {
                Some(name) => rename(&parent.join(name), &target),
                None => remove_if_there(&target),
            };
            if let Err(e) = done {
                let e = io_error_at("write", &c.file, e);
                if !marked {
                    cleanup(&temps);
                    return Err(e);
                }
                #[cfg(test)]
                tests::run_hook(&tests::AFTER_FAILED_RENAME, folder);
                // The next call must recover again: this set left its marker for it to finish. The stamps of the
                // files not renamed yet will not match once recovery renames them (a rename moves ctime): the next
                // checked write refuses, which is the safe side.
                recovered.remove(folder);
                return Ok(Committed {
                    written: Written::NotSynced(e),
                    stamps: FsGraphFiles::stamps_after(before, changes, &handles),
                });
            }
            if i == 0 && self.crash_here(CrashPoint::AfterFirstRename) {
                // A crashed set left its files as they are: the next call on the folder recovers them (gf_c23).
                recovered.remove(folder);
                return Err(FilesError::io("crash"));
            }
        }
        // 4. Sync the renames in every folder they touched, drop the marker, sync again.
        let finished = touched
            .iter()
            .try_for_each(|p| sync_folder(p))
            .and_then(|()| sync_folder(folder))
            .and_then(|()| {
                if marked {
                    fs::remove_file(folder.join(MARKER))?;
                    sync_folder(folder)?;
                }
                Ok(())
            });
        let stamps = FsGraphFiles::stamps_after(before, changes, &handles);
        let written = match finished {
            Ok(()) => Written::Durable,
            Err(e) => {
                if marked {
                    recovered.remove(folder);
                }
                Written::NotSynced(io_error("sync", None, e))
            }
        };
        Ok(Committed { written, stamps })
    }
}

fn roots_missing(root: &Path) -> Option<&'static str> {
    root.to_str()
        .and_then(|r| GraphDir::parse(r).ok())
        .is_none()
        .then_some("the vector cache path is not an absolute UTF-8 path")
}

/// Canonicalizes the nearest existing ancestor of `path` and appends the rest. `path` comes from a GraphDir, so the
/// rest has no `.` or `..` parts.
pub(crate) fn resolve(path: &Path) -> std::io::Result<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        match fs::canonicalize(&existing) {
            Ok(mut out) => {
                out.extend(rest.iter().rev());
                return Ok(out);
            }
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
                    return Err(e);
                };
                rest.push(name);
                existing.pop();
            }
            Err(e) => return Err(e),
        }
    }
}

pub(crate) fn dir_of(path: PathBuf) -> Option<GraphDir> {
    GraphDir::parse(path.to_str()?).ok()
}

fn io_error(op: &str, file: Option<GraphFile>, e: std::io::Error) -> FilesError {
    let name = file.map_or("folder", GraphFile::file_name);
    FilesError::io(format!("{op} {name}: {}", e.kind()))
}

/// rename(2) that never replaces what is at `to`, in one call, so a folder made there after the checks is refused,
/// never replaced (review R24): `renamex_np(RENAME_EXCL)` on macOS, `renameat2(RENAME_NOREPLACE)` on Linux.
/// Ok(false) when this system or file system has no such rename.
fn rename_exclusive(from: &Path, to: &Path) -> std::io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    // SAFETY: both are NUL-terminated paths that live past the call.
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    // SAFETY: as above; AT_FDCWD makes both paths relative to the working folder, as rename(2) does.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let rc: i32 = {
        let _ = (&from, &to);
        return Ok(false);
    };
    if rc == 0 {
        return Ok(true);
    }
    let e = std::io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::EINVAL | libc::ENOTSUP | libc::ENOSYS) => Ok(false),
        _ => Err(e),
    }
}

/// The F144 stamp of a file's metadata: (device, inode, size, mtime ns, ctime ns), the Unix parts. A rename or a
/// touch moves ctime, so a file replaced by one with the same bytes and times still differs.
fn stamp_of(meta: &fs::Metadata) -> FileStamp {
    use std::os::unix::fs::MetadataExt;
    let ns = |secs: i64, nsec: i64| {
        (secs as u64)
            .wrapping_mul(1_000_000_000)
            .wrapping_add(nsec as u64)
    };
    FileStamp::from_parts([
        meta.dev(),
        meta.ino(),
        meta.size(),
        ns(meta.mtime(), meta.mtime_nsec()),
        ns(meta.ctime(), meta.ctime_nsec()),
    ])
}

impl GraphFiles for FsGraphFiles {
    fn adapter_id(&self) -> &'static str {
        "fs.graph-files"
    }

    fn relocate(&self, from: &GraphDir, to: &GraphDir) -> Result<(), FilesError> {
        let source = self.check(from)?;
        if to.is_within(from) {
            return Err(FilesError::Invalid(RELOCATE_INSIDE_ITSELF));
        }
        match fs::symlink_metadata(from.as_str()) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(FilesError::OutsideRoots(from.clone()));
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(FilesError::io("relocate: the graph folder is not a folder")),
            Err(e) => return Err(io_error("relocate", None, e)),
        }
        let target = self.check(to)?;
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, &source)?;
        // Refused up front for a clear answer; the rename below is what keeps it true (review R24).
        if fs::symlink_metadata(to.as_str()).is_ok() || fs::symlink_metadata(&target).is_ok() {
            return Err(FilesError::Invalid(RELOCATE_TARGET_EXISTS));
        }
        if !target.parent().is_some_and(Path::is_dir) {
            return Err(FilesError::Invalid(RELOCATE_PARENT_MISSING));
        }
        #[cfg(test)]
        if std::mem::take(&mut *self.cross_device.lock().unwrap()) {
            return Err(FilesError::CrossDevice);
        }
        #[cfg(test)]
        if let Some(made) = self.before_relocate.lock().unwrap().take() {
            made(&target);
        }
        let renamed = match rename_exclusive(&source, &target) {
            Ok(true) => Ok(()),
            // A file system with no exclusive rename: the checks above are all there is.
            Ok(false) => fs::rename(&source, &target),
            Err(e) => Err(e),
        };
        match renamed {
            Ok(()) => {
                recovered.remove(&source);
                recovered.insert(target);
                Ok(())
            }
            Err(e)
                if e.kind() == ErrorKind::AlreadyExists
                    || e.raw_os_error() == Some(libc::ENOTEMPTY) =>
            {
                Err(FilesError::Invalid(RELOCATE_TARGET_EXISTS))
            }
            Err(e) if e.kind() == ErrorKind::CrossesDevices => Err(FilesError::CrossDevice),
            Err(e) => Err(io_error("relocate", None, e)),
        }
    }

    fn read_ref(&self, dir: &GraphDir, file: &FileRef) -> Result<Option<Vec<u8>>, FilesError> {
        let folder = self.check(dir)?;
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, &folder)?;
        parent_of(&folder, file).map_err(|()| FilesError::OutsideRoots(dir.clone()))?;
        self.read_capped(dir, &folder, file).map(|(bytes, _)| bytes)
    }

    fn read_ref_stamped(
        &self,
        dir: &GraphDir,
        file: &FileRef,
    ) -> Result<(Option<Vec<u8>>, FileStamp), FilesError> {
        let folder = self.check(dir)?;
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, &folder)?;
        parent_of(&folder, file).map_err(|()| FilesError::OutsideRoots(dir.clone()))?;
        self.read_capped(dir, &folder, file)
    }

    fn stamps(&self, dir: &GraphDir) -> Result<GraphStamps, FilesError> {
        let folder = self.check(dir)?;
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, &folder)?;
        FsGraphFiles::stamps_in(&folder)
    }

    fn write_set_checked_also(
        &self,
        dir: &GraphDir,
        changes: &[Change<'_>],
        expect: Option<&GraphStamps>,
        also: &[(FileRef, FileStamp)],
    ) -> Result<Committed, FilesError> {
        let folder = self.check(dir)?;
        Change::validate(changes)?;
        #[cfg(test)]
        tests::run_hook(&tests::BEFORE_BOUNDARY, &folder);
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, &folder)?;
        #[cfg(test)]
        tests::run_hook(&tests::AFTER_RECOVER, &folder);
        self.write_set_in(&mut recovered, dir, &folder, changes, (expect, also))
    }

    fn list(&self, dir: &GraphDir) -> Result<Vec<GraphFile>, FilesError> {
        let folder = self.check(dir)?;
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, &folder)?;
        let mut out = Vec::new();
        for file in GraphFile::ALL {
            // A symlinked graph file is listed (so `graph new` will not write over it) but refused on read.
            match fs::symlink_metadata(folder.join(file.file_name())) {
                Ok(m) if m.is_file() || m.file_type().is_symlink() => out.push(file),
                Ok(_) => {}
                Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
                Err(e) => return Err(io_error("list", Some(file), e)),
            }
        }
        Ok(out)
    }

    fn resolve(&self, dir: &GraphDir) -> Result<GraphDir, FilesError> {
        let folder = self.check(dir)?;
        dir_of(folder).ok_or_else(|| FilesError::io("resolve folder: not a UTF-8 path"))
    }

    fn list_shards(
        &self,
        dir: &GraphDir,
        key: &ModelKey,
        kind: ShardKind,
    ) -> Result<Vec<ShardFile>, FilesError> {
        let folder = self.check(dir)?;
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, &folder)?;
        let space =
            space_folder(&folder, kind, key).map_err(|()| FilesError::OutsideRoots(dir.clone()))?;
        let entries = match fs::read_dir(&space) {
            Ok(entries) => entries,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok(Vec::new());
            }
            Err(e) => return Err(FilesError::io(format!("list shards: {}", e.kind()))),
        };
        let mut out: Vec<ShardFile> = entries
            .flatten()
            .filter_map(|e| ShardFile::from_file_name_in(kind, key, e.file_name().to_str()?))
            .collect();
        out.sort();
        Ok(out)
    }

    fn list_vector_spaces(&self, dir: &GraphDir) -> Result<Vec<ModelKey>, FilesError> {
        let folder = self.check(dir)?;
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, &folder)?;
        let embeddings = folder.join("embeddings");
        if is_symlink(&embeddings) {
            return Err(FilesError::OutsideRoots(dir.clone()));
        }
        let entries = match fs::read_dir(&embeddings) {
            Ok(entries) => entries,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok(Vec::new());
            }
            Err(e) => return Err(FilesError::io(format!("list vector spaces: {}", e.kind()))),
        };
        let mut out: Vec<ModelKey> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| ModelKey::parse(e.file_name().to_str()?).ok())
            .collect();
        out.sort();
        Ok(out)
    }

    fn find_graphs(&self, root: &GraphDir, limits: FindLimits) -> Result<Found, FilesError> {
        let top = self.check(root)?;
        let at = |rel: &str| {
            if rel.is_empty() {
                top.clone()
            } else {
                top.join(rel)
            }
        };
        let started = std::time::Instant::now();
        Ok(umx_core::scan::walk_for_graphs(
            limits,
            |rel| list_folder(&at(rel)).ok(),
            |rel| found_graph_json(&at(rel)),
            || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ))
    }

    /// No recovery: list and read only, as the walk does (R289-3).
    fn peek_graph(&self, dir: &GraphDir) -> Result<Option<FoundGraph>, FilesError> {
        let folder = self.check(dir)?;
        let entries = match list_folder(&folder) {
            Ok(entries) => entries,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok(None);
            }
            Err(e) => return Err(io_error("peek", None, e)),
        };
        Ok(umx_core::scan::graph_in(".".into(), &entries, || {
            found_graph_json(&folder)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    use umx_core::setup::GraphFilesSetup;

    #[test]
    fn the_models_lock_is_exclusive_until_dropped() {
        let dir = std::env::temp_dir().join(format!("umx-models-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let held = models_folder_lock(&dir).unwrap();
        let try_other = {
            let dir = dir.clone();
            move || {
                let other = std::fs::File::open(dir.join(".models.lock")).unwrap();
                other.try_lock().map(|()| other)
            }
        };
        let busy = std::thread::spawn(try_other.clone()).join().unwrap();
        assert!(
            matches!(busy, Err(std::fs::TryLockError::WouldBlock)),
            "{busy:?}"
        );
        drop(held);
        assert!(std::thread::spawn(try_other).join().unwrap().is_ok());
        let link = dir.join("link");
        std::fs::create_dir_all(&link).unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere"), link.join(".models.lock")).unwrap();
        assert!(
            models_folder_lock(&link).is_err(),
            "a symlinked lock file is refused"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn available_bytes_reads_a_temp_folder() {
        let dir = std::env::temp_dir();
        assert!(available_bytes(&dir).unwrap() > 0);
        assert!(available_bytes(&dir.join("umx-no-such-folder")).is_err());
    }

    /// What a write did to the disk, in order, as seen on this thread: the input to [`after_power_cut`].
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum Event {
        Made(PathBuf),
        FileSynced(PathBuf),
        DirSynced(PathBuf),
        Renamed(PathBuf, PathBuf),
        Removed(PathBuf),
    }

    /// A broken build a test plants on its own thread, to prove its check fires.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Plant {
        TempUnsynced,
        FoldersUnsynced,
        /// One change goes the general way (the path before s29), for the before/after bench.
        GeneralPath,
        /// `names_durable` does nothing: a folder's name is taken as durable because the folder exists (R1).
        NamesForgotten,
    }

    /// A one-shot failure a test arms on its own thread: the next temp file write, or the next sync of a folder.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum Fault {
        Temp,
        Sync(PathBuf),
    }

    thread_local! {
        static LOG: std::cell::RefCell<Option<Vec<Event>>> = const { std::cell::RefCell::new(None) };
        static PLANTS: std::cell::RefCell<Vec<Plant>> = const { std::cell::RefCell::new(Vec::new()) };
        static FAULTS: std::cell::RefCell<Vec<Fault>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    pub(crate) fn arm(fault: Fault) {
        FAULTS.with_borrow_mut(|f| f.push(fault));
    }

    /// True once for each armed `fault`, which is then spent.
    pub(crate) fn fault(fault: &Fault) -> bool {
        FAULTS.with_borrow_mut(|f| match f.iter().position(|x| x == fault) {
            Some(i) => {
                f.remove(i);
                true
            }
            None => false,
        })
    }

    pub(crate) fn note(event: Event) {
        LOG.with_borrow_mut(|log| {
            if let Some(log) = log {
                log.push(event);
            }
        });
    }

    pub(crate) fn planted(plant: Plant) -> bool {
        PLANTS.with_borrow(|p| p.contains(&plant))
    }

    /// Runs `f` on this thread with `plants` in force; returns its result and what it did to the disk.
    /// Nests: an inner call's events are also added to the outer call's history.
    pub(crate) fn record<T>(plants: &[Plant], f: impl FnOnce() -> T) -> (T, Vec<Event>) {
        let outer_plants = PLANTS.replace(plants.to_vec());
        let outer = LOG.replace(Some(Vec::new()));
        let out = f();
        PLANTS.set(outer_plants);
        let events = LOG.take().unwrap_or_default();
        match outer {
            Some(mut log) => {
                log.extend(events.iter().cloned());
                LOG.set(Some(log));
            }
            None => FAULTS.set(Vec::new()),
        }
        (out, events)
    }

    fn fsyncs(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, Event::FileSynced(_) | Event::DirSynced(_)))
            .count()
    }

    /// What a file can hold after a power cut.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub(crate) enum After {
        Old,
        New,
        /// The new name on bytes that never reached the disk (read back empty).
        Torn,
    }

    /// What `target` can hold after a power cut once `events` happened, by the worst a POSIX file system may do: a
    /// rename or remove stands only once its folder is synced after it, a made folder only once the folder above
    /// it is, and renamed bytes are whole only if their temp file was synced before the rename.
    pub(crate) fn after_power_cut(events: &[Event], target: &Path) -> BTreeSet<After> {
        let hit =
            |e: &Event| matches!(e, Event::Renamed(_, to) | Event::Removed(to) if to == target);
        let Some(i) = events.iter().rposition(hit) else {
            return [After::Old].into();
        };
        let new = match &events[i] {
            Event::Renamed(from, _) if !events[..i].contains(&Event::FileSynced(from.clone())) => {
                After::Torn
            }
            _ => After::New,
        };
        let synced_after =
            |dir: &Path, from: usize| events[from..].contains(&Event::DirSynced(dir.to_path_buf()));
        let folders_stand = events.iter().enumerate().all(|(j, e)| match e {
            Event::Made(d) if target.starts_with(d) => synced_after(d.parent().unwrap(), j),
            _ => true,
        });
        if synced_after(target.parent().unwrap(), i) && folders_stand {
            [new].into()
        } else {
            [After::Old, new].into()
        }
    }

    /// The s29 guarantee over a recorded write: a power cut after any prefix of it leaves `target` old or new,
    /// and once the write said Durable, new. Err names the first breach.
    fn old_or_new_then_durable(
        events: &[Event],
        target: &Path,
        durable: bool,
    ) -> Result<(), String> {
        for k in 0..=events.len() {
            let after = after_power_cut(&events[..k], target);
            if after.contains(&After::Torn) {
                return Err(format!("torn after {:?}", &events[..k]));
            }
        }
        let end = after_power_cut(events, target);
        if durable && end != [After::New].into() {
            return Err(format!("said Durable but could be {end:?}: {events:?}"));
        }
        Ok(())
    }

    /// One-change writes s29 covers: a graph file, a shard and a similar_to file, each into an existing folder
    /// and a fresh one, and a shard removal. Returns, per case, the check's result and the fsync count.
    fn one_change_cases(plants: &[Plant]) -> Vec<(String, Result<(), String>, usize)> {
        let (files, root, _temp) = graph_files();
        let key = ModelKey::new("m", 4).unwrap();
        let shard = FileRef::Shard(ShardFile::new(key.clone(), "b7").unwrap());
        let meta = FileRef::SimilarMeta(key);
        let mut out = Vec::new();
        for (name, graph, file, bytes, seed) in [
            (
                "graph file, fresh graph",
                "a",
                FileRef::from(GraphFile::Nodes),
                Some(&b"n1\n"[..]),
                false,
            ),
            (
                "graph file, existing",
                "b",
                FileRef::from(GraphFile::Nodes),
                Some(b"n1\n"),
                true,
            ),
            (
                "shard, fresh graph",
                "c",
                shard.clone(),
                Some(b"v1\n"),
                false,
            ),
            ("shard, existing", "d", shard.clone(), Some(b"v1\n"), true),
            (
                "similar meta, fresh graph",
                "e",
                meta.clone(),
                Some(b"s1\n"),
                false,
            ),
            (
                "similar meta, existing",
                "f",
                meta.clone(),
                Some(b"s1\n"),
                true,
            ),
            ("shard removed", "h", shard.clone(), None, true),
        ] {
            let g = root.join(graph).unwrap();
            if seed {
                let old = [Change {
                    file: file.clone(),
                    bytes: Some(b"old\n"),
                }];
                files.write_set(&g, &old).unwrap();
            }
            let set = [Change {
                file: file.clone(),
                bytes,
            }];
            let (written, events) = record(plants, || files.write_set(&g, &set));
            let target = files.check(&g).unwrap().join(file.path());
            let durable = matches!(written, Ok(Written::Durable));
            let checked = match written {
                Ok(_) => old_or_new_then_durable(&events, &target, durable),
                Err(e) => Err(format!("write failed: {e:?}")),
            };
            out.push((name.to_owned(), checked, fsyncs(&events)));
        }
        out
    }

    #[test]
    fn one_change_writes_are_old_or_new_at_every_point_and_durable_after_two_fsyncs() {
        let fast = one_change_cases(&[]);
        let general = one_change_cases(&[Plant::GeneralPath]);
        let counts: Vec<(&str, usize, usize)> = fast
            .iter()
            .zip(&general)
            .map(|((name, checked, n), (_, before, m))| {
                assert_eq!(checked, &Ok(()), "{name}");
                assert_eq!(before, &Ok(()), "{name} (general path)");
                (name.as_str(), *n, *m)
            })
            .collect();
        // (fsyncs now, before s29). Existing folder: the temp file and the file's own folder. A fresh graph also
        // syncs the folder above each folder made, the graph folder's included, which the general path never did.
        let want = [
            ("graph file, fresh graph", 3, 2),
            ("graph file, existing", 2, 2),
            ("shard, fresh graph", 5, 6),
            ("shard, existing", 2, 6),
            ("similar meta, fresh graph", 5, 6),
            ("similar meta, existing", 2, 6),
            ("shard removed", 1, 5),
        ];
        assert_eq!(counts, want);
    }

    /// The plants prove the check above can fail: a temp file renamed unsynced can come back torn, and a write
    /// that says Durable without syncing its folders may still roll back.
    #[test]
    fn the_one_change_check_fires_on_a_skipped_temp_or_folder_sync() {
        for plant in [Plant::TempUnsynced, Plant::FoldersUnsynced] {
            for (name, checked, _) in one_change_cases(&[plant]) {
                // A removal has no temp file to skip.
                if plant == Plant::TempUnsynced && name == "shard removed" {
                    assert_eq!(checked, Ok(()));
                    continue;
                }
                assert!(checked.is_err(), "{plant:?} {name}");
            }
        }
    }

    /// Crash-point witness (ws004 s29): a one-shard write stopped after its rename and before its folder sync,
    /// then each state a power cut can leave on disk, reopened. Err if a reopened read is neither old nor new.
    fn reopen_after_a_crash_after_the_rename(plants: &[Plant]) -> Result<(), String> {
        let (files, root, _temp) = graph_files();
        let g = root.join("g").unwrap();
        let shard = FileRef::Shard(ShardFile::new(ModelKey::new("m", 4).unwrap(), "b7").unwrap());
        let (old, new) = (&b"old\n"[..], &b"new\n"[..]);
        files
            .write_set(
                &g,
                &[Change {
                    file: shard.clone(),
                    bytes: Some(old),
                }],
            )
            .unwrap();
        files.crash_at_for_tests(Some(CrashPoint::AfterFirstRename));
        let set = [Change {
            file: shard.clone(),
            bytes: Some(new),
        }];
        let (crashed, events) = record(plants, || files.write_set(&g, &set));
        assert!(crashed.is_err(), "{crashed:?}");
        let target = files.check(&g).unwrap().join(shard.path());
        for state in after_power_cut(&events, &target) {
            let on_disk: &[u8] = match state {
                After::Old => old,
                After::New => new,
                After::Torn => b"",
            };
            fs::write(&target, on_disk).unwrap();
            let read = files.restart_for_tests().read_ref(&g, &shard).unwrap();
            if read.as_deref() != Some(old) && read.as_deref() != Some(new) {
                return Err(format!("{state:?} reopened as {read:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn a_crash_after_the_rename_reopens_old_or_new_and_fires_without_the_temp_sync() {
        assert_eq!(reopen_after_a_crash_after_the_rename(&[]), Ok(()));
        assert!(reopen_after_a_crash_after_the_rename(&[Plant::TempUnsynced]).is_err());
    }

    /// LocalState and the config files write through `replace_owner_only`, never `write_set`: the same guarantee
    /// (old or new at every point, durable after), including the first write that makes its folders.
    #[test]
    fn owner_only_replaces_are_old_or_new_and_durable_with_their_made_folders() {
        let temp = temp_root();
        for (k, (plants, ok)) in [
            (&[][..], true),
            (&[Plant::TempUnsynced][..], false),
            (&[Plant::FoldersUnsynced][..], false),
            (&[Plant::NamesForgotten][..], false),
        ]
        .into_iter()
        .enumerate()
        {
            // A path of its own per run: the folders-made set lives as long as the process.
            let top = temp.0.join(format!("state{k}"));
            let folder = top.join("new");
            for n in 0..2 {
                let path = folder.join("file.json");
                let (written, events) = record(plants, || {
                    config::replace_owner_only(&folder, &top, &path, b"{}", n).unwrap()
                });
                let checked = old_or_new_then_durable(&events, &path, written.is_none());
                // A later write by a planted run can pass: the first one is the one that made the folders.
                if n == 0 || ok {
                    assert_eq!(checked.is_ok(), ok, "{plants:?} write {n}: {checked:?}");
                }
                if plants.is_empty() {
                    // First: the temp, the folder, and the names of the two folders made (synced into the folders
                    // above them, top's parent the highest). Then: two.
                    assert_eq!(fsyncs(&events), if n == 0 { 4 } else { 2 }, "{events:?}");
                }
            }
        }
    }

    /// R1: a first write that fails after making its folders (its temp write, or the sync that makes a made
    /// folder durable) and then a retry, recorded as one history, so folders made by the failed call count. The
    /// retry may say Durable only once every folder either call made is synced into the folder above it. Runs a
    /// shard write into a fresh graph and an owner-only replace into fresh folders; Err names the first breach.
    fn failed_then_retried(plants: &[Plant]) -> Vec<(String, Result<(), String>)> {
        let mut out = Vec::new();
        let shard = FileRef::Shard(ShardFile::new(ModelKey::new("m", 4).unwrap(), "b7").unwrap());
        for (n, fails) in ["temp", "sync of embeddings/", "sync of the graph folder"]
            .into_iter()
            .enumerate()
        {
            let (files, root, temp) = graph_files();
            let g = root.join(&format!("g{n}")).unwrap();
            let folder = temp.0.join(format!("g{n}"));
            let target = folder.join(shard.path());
            arm(match fails {
                "temp" => Fault::Temp,
                "sync of embeddings/" => Fault::Sync(folder.join("embeddings")),
                _ => Fault::Sync(folder.clone()),
            });
            let set = [Change {
                file: shard.clone(),
                bytes: Some(b"v1\n"),
            }];
            let ((first, retry), events) = record(plants, || {
                (files.write_set(&g, &set), files.write_set(&g, &set))
            });
            let checked = match (first, retry) {
                (Ok(Written::Durable), _) if plants.is_empty() => {
                    Err(format!("the first write did not fail: {fails}"))
                }
                (_, Ok(w)) => {
                    old_or_new_then_durable(&events, &target, matches!(w, Written::Durable))
                }
                (_, Err(e)) => Err(format!("the retry failed: {e:?}")),
            };
            out.push((format!("shard, {fails} fails"), checked));
        }
        for fails in ["temp", "sync of the made folder's name"] {
            let temp = temp_root();
            let top = temp.0.join("state");
            let folder = top.join("new");
            let path = folder.join("file.json");
            arm(match fails {
                "temp" => Fault::Temp,
                _ => Fault::Sync(temp.0.join("state")),
            });
            let ((first, retry), events) = record(plants, || {
                (
                    config::replace_owner_only(&folder, &top, &path, b"{}", 0),
                    config::replace_owner_only(&folder, &top, &path, b"{}", 1),
                )
            });
            let checked = match (first, retry) {
                (Ok(None), _) if plants.is_empty() => {
                    Err(format!("the first write did not fail: {fails}"))
                }
                (_, Ok(not_synced)) => {
                    old_or_new_then_durable(&events, &path, not_synced.is_none())
                }
                (_, Err(e)) => Err(format!("the retry failed: {e:?}")),
            };
            out.push((format!("owner-only, {fails} fails"), checked));
        }
        out
    }

    #[test]
    fn a_retry_after_a_failed_first_write_syncs_every_folder_the_failed_one_made() {
        for (name, checked) in failed_then_retried(&[]) {
            assert_eq!(checked, Ok(()), "{name}");
        }
        // The plant: folders synced only by a write that succeeds, and existence taken as proof (R1's bug).
        for (name, checked) in failed_then_retried(&[Plant::NamesForgotten]) {
            assert!(checked.is_err(), "{name}: the check did not fire");
        }
    }

    /// R1-F1 (graph-storage-expert): a first process whose write leaves the graph folder's name unsynced (the
    /// sync of the graph folder's parent fails, or the process dies after making its folders), then a new process,
    /// whose set of synced folders starts empty, writes again. Recorded as one history, its Durable must come after
    /// the graph folder's name is synced; a third write is back to two fsyncs. Err names the first breach.
    fn a_new_process_after_a_failed_one(plants: &[Plant]) -> Vec<(String, Result<(), String>)> {
        let shard = FileRef::Shard(ShardFile::new(ModelKey::new("m", 4).unwrap(), "b7").unwrap());
        let set = [Change {
            file: shard.clone(),
            bytes: Some(b"v1\n"),
        }];
        let mut out = Vec::new();
        for first in [
            "graph folder's parent sync fails",
            "crash after the folders are made",
        ] {
            let (files, root, temp) = graph_files();
            let g = root.join("g").unwrap();
            let target = temp.0.join("g").join(shard.path());
            match first {
                "crash after the folders are made" => {
                    files.crash_at_for_tests(Some(CrashPoint::AfterTemps))
                }
                _ => arm(Fault::Sync(temp.0.clone())),
            }
            let ((before, after, steady), events) = record(plants, || {
                let before = files.write_set(&g, &set);
                let next = files.restart_for_tests();
                let after = next.write_set(&g, &set);
                let (_, steady) = record(plants, || next.write_set(&g, &set));
                (before, after, steady)
            });
            let checked = match (before, after) {
                (Ok(Written::Durable), _) if plants.is_empty() => {
                    Err("the first process did not fail".into())
                }
                (_, Ok(w)) => {
                    let named = events
                        .iter()
                        .any(|e| e == &Event::DirSynced(temp.0.clone()));
                    old_or_new_then_durable(&events, &target, matches!(w, Written::Durable))
                        .and_then(|()| {
                            named
                                .then_some(())
                                .ok_or("the graph folder's name was never synced".into())
                        })
                        .and_then(|()| match fsyncs(&steady) {
                            2 => Ok(()),
                            n => Err(format!("a steady write did {n} fsyncs")),
                        })
                }
                (_, Err(e)) => Err(format!("the new process failed: {e:?}")),
            };
            out.push((first.to_owned(), checked));
        }
        out
    }

    #[test]
    fn a_new_process_syncs_the_graph_folder_a_failed_process_left_unsynced() {
        for (name, checked) in a_new_process_after_a_failed_one(&[]) {
            assert_eq!(checked, Ok(()), "{name}");
        }
        for (name, checked) in a_new_process_after_a_failed_one(&[Plant::NamesForgotten]) {
            assert!(checked.is_err(), "{name}: the check did not fire");
        }
    }

    /// Hold bench (ws004 s29, graph-storage-expert): one shard write at a time into an existing space folder, the
    /// s29 fast path against the general path, interleaved. Prints fsyncs and p50/p95 ms per write. Ignored: it is
    /// a timing run; run it alone with `--ignored --exact tests::hold_bench_one_shard_write --nocapture`.
    #[test]
    #[ignore]
    fn hold_bench_one_shard_write() {
        let (files, root, _temp) = graph_files();
        let g = root.join("g").unwrap();
        let shard = FileRef::Shard(ShardFile::new(ModelKey::new("m", 256).unwrap(), "b7").unwrap());
        let bytes = vec![b'v'; 32 * 1024];
        let set = [Change {
            file: shard.clone(),
            bytes: Some(&bytes),
        }];
        files.write_set(&g, &set).unwrap();
        let runs = 200;
        let mut times: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
        let mut counts = [0usize; 2];
        for i in 0..runs * 2 {
            let which = i % 2;
            let plants: &[Plant] = if which == 0 {
                &[]
            } else {
                &[Plant::GeneralPath]
            };
            let t = std::time::Instant::now();
            let (written, events) = record(plants, || files.write_set(&g, &set));
            times[which].push(t.elapsed().as_secs_f64() * 1000.0);
            assert!(matches!(written, Ok(Written::Durable)), "{written:?}");
            counts[which] = fsyncs(&events);
        }
        for (which, name) in [(0, "s29 fast path"), (1, "general path (before)")] {
            let v = &mut times[which];
            v.sort_by(f64::total_cmp);
            let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
            println!(
                "{{\"path\":\"{name}\",\"bytes\":{},\"writes\":{runs},\"fsyncs\":{},\"p50_ms\":{:.2},\"p95_ms\":{:.2},\"max_ms\":{:.2}}}",
                bytes.len(),
                counts[which],
                at(0.5),
                at(0.95),
                at(1.0)
            );
        }
    }

    /// A test's pause point inside `write_set`, for one folder only, so tests running in parallel never take
    /// each other's hooks.
    /// One callback per folder, so tests in different temp folders can arm the same hook in parallel.
    type Hook = Mutex<Vec<(PathBuf, Box<dyn FnOnce() + Send>)>>;
    pub(super) static BEFORE_BOUNDARY: Hook = Mutex::new(Vec::new());
    pub(super) static AFTER_RECOVER: Hook = Mutex::new(Vec::new());
    pub(super) static AFTER_MARKER: Hook = Mutex::new(Vec::new());
    pub(super) static AFTER_FAILED_RENAME: Hook = Mutex::new(Vec::new());
    pub(super) static AFTER_SIZE_CHECK: Hook = Mutex::new(Vec::new());
    pub(super) static AFTER_OPEN: Hook = Mutex::new(Vec::new());
    pub(super) static BEFORE_STAMP_CHECK: Hook = Mutex::new(Vec::new());
    pub(super) static BEFORE_OLD_STAMPS: Hook = Mutex::new(Vec::new());

    pub(super) fn run_hook(hook: &Hook, folder: &Path) {
        let callback = {
            let mut armed = hook.lock().unwrap();
            armed
                .iter()
                .position(|(f, _)| f == folder)
                .map(|i| armed.remove(i).1)
        };
        if let Some(callback) = callback {
            callback();
        }
    }

    pub(super) fn set_hook(hook: &Hook, folder: &Path, callback: impl FnOnce() + Send + 'static) {
        let mut armed = hook.lock().unwrap();
        armed.retain(|(f, _)| f != folder);
        armed.push((folder.to_path_buf(), Box::new(callback)));
    }

    /// The reviewer's witness for R54-1 (round 1). Set A commits (its marker is synced) and then fails a
    /// rename, so it returns NotSynced and leaves its marker for the next call to finish. Set B, queued behind A,
    /// must run that recovery before writing its own marker, or A's committed node change is lost.
    #[test]
    fn reviewer_queued_write_must_recover_a_prior_committed_set() {
        use std::os::unix::fs::PermissionsExt;
        let (files, root, _temp) = graph_files();
        let files = Arc::new(files);
        let g = root.join("g").unwrap();
        let folder = files.check(&g).unwrap();
        files.write(&g, GraphFile::Graph, b"g0\n").unwrap();
        files.write(&g, GraphFile::Nodes, b"n0\n").unwrap();
        files.write(&g, GraphFile::Edges, b"e0\n").unwrap();

        // A pauses after its marker is synced. When released it takes write permission off the folder, so its first
        // rename fails (NotSynced, marker kept), and puts it back before it returns. Both happen in A's own thread,
        // so nothing else can touch the folder in between.
        let (a_at_marker, a_paused) = mpsc::channel();
        let (release_a, a_released) = mpsc::channel::<()>();
        let a_folder = folder.clone();
        set_hook(&AFTER_MARKER, &folder, move || {
            a_at_marker.send(()).unwrap();
            a_released.recv().unwrap();
            fs::set_permissions(&a_folder, fs::Permissions::from_mode(0o500)).unwrap();
        });
        let (marker_left, marker_seen) = mpsc::channel();
        let a_folder = folder.clone();
        set_hook(&AFTER_FAILED_RENAME, &folder, move || {
            fs::set_permissions(&a_folder, fs::Permissions::from_mode(0o700)).unwrap();
            marker_left.send(a_folder.join(MARKER).exists()).unwrap();
        });
        let a = std::thread::spawn({
            let (files, g) = (files.clone(), g.clone());
            move || {
                files.write_set(
                    &g,
                    &[
                        Change {
                            file: GraphFile::Graph.into(),
                            bytes: Some(b"g1\n"),
                        },
                        Change {
                            file: GraphFile::Nodes.into(),
                            bytes: Some(b"n1\n"),
                        },
                    ],
                )
            }
        });
        a_paused.recv().unwrap();

        // B enters while A is paused. Without one boundary around recovery and the set, B passes the cached
        // recovery at once; with it, B waits for A.
        let (b_entered, b_entering) = mpsc::channel();
        set_hook(&BEFORE_BOUNDARY, &folder, move || {
            b_entered.send(()).unwrap()
        });
        let (b_past_recovery, b_recovered) = mpsc::channel();
        let (release_b, b_released) = mpsc::channel::<()>();
        set_hook(&AFTER_RECOVER, &folder, move || {
            b_past_recovery.send(()).unwrap();
            b_released.recv().unwrap();
        });
        let b = std::thread::spawn({
            let (files, g) = (files.clone(), g.clone());
            move || {
                files.write_set(
                    &g,
                    &[
                        Change {
                            file: GraphFile::Graph.into(),
                            bytes: Some(b"g2\n"),
                        },
                        Change {
                            file: GraphFile::Edges.into(),
                            bytes: Some(b"e2\n"),
                        },
                    ],
                )
            }
        });
        b_entering.recv().unwrap();
        let b_ran_ahead = b_recovered.recv_timeout(Duration::from_secs(2)).is_ok();

        // A's rename fails: its set stands and its marker stays for the next call to finish.
        release_a.send(()).unwrap();
        let a_result = a.join().unwrap();
        assert!(
            matches!(a_result, Ok(Written::NotSynced(_))),
            "{a_result:?}"
        );
        assert!(marker_seen.recv().unwrap(), "A left its marker");

        release_b.send(()).unwrap();
        assert_eq!(b.join().unwrap().unwrap(), Written::Durable);
        assert_eq!(
            files.read(&g, GraphFile::Nodes).unwrap().as_deref(),
            Some(&b"n1\n"[..]),
            "B must not discard A's committed node change (B ran ahead of A: {b_ran_ahead})"
        );
        assert_eq!(
            files.read(&g, GraphFile::Graph).unwrap().as_deref(),
            Some(&b"g2\n"[..])
        );
        assert_eq!(
            files.read(&g, GraphFile::Edges).unwrap().as_deref(),
            Some(&b"e2\n"[..])
        );
        assert!(!b_ran_ahead, "B passed recovery while A held the folder");
    }

    /// A fresh temp folder, removed on drop.
    pub struct TempRoot(PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_root() -> TempRoot {
        static N: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().canonicalize().unwrap();
        let path = base.join(format!(
            "umx-fs-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempRoot(path)
    }

    impl umx_contracts::graph_files::SetHarness for FsGraphFiles {
        fn crash_at(&self, point: Option<CrashPoint>) {
            self.crash_at_for_tests(point);
        }
        fn fail_temp_of(&self, file: Option<GraphFile>) {
            self.fail_temp_for_tests(file);
        }
        fn restart(&self) -> FsGraphFiles {
            self.restart_for_tests()
        }
        fn tamper(&self, dir: &GraphDir, file: impl Into<FileRef>, bytes: &[u8]) {
            fs::write(self.check(dir).unwrap().join(file.into().path()), bytes).unwrap();
        }
        fn raw(&self, dir: &GraphDir, file: impl Into<FileRef>) -> Option<Vec<u8>> {
            fs::read(self.check(dir).unwrap().join(file.into().path())).ok()
        }
        /// Temp files and the marker in the folder and in its shard folders (named by their relative path).
        fn bytes_read(&self) -> u64 {
            self.bytes_read_for_tests()
        }
        fn touch(&self, dir: &GraphDir, file: GraphFile) {
            let path = self.check(dir).unwrap().join(file.file_name());
            let long_ago = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(long_ago)
                .unwrap();
        }
        fn swap_for_link(&self, dir: &GraphDir, file: GraphFile) {
            let folder = self.check(dir).unwrap();
            let (path, real) = (
                folder.join(file.file_name()),
                folder.join(format!("{}.real", file.file_name())),
            );
            fs::rename(&path, &real).unwrap();
            std::os::unix::fs::symlink(&real, &path).unwrap();
        }
        fn change_during_next_read(&self, dir: &GraphDir, file: GraphFile, bytes: &[u8]) {
            let folder = self.check(dir).unwrap();
            let (path, bytes) = (folder.join(file.file_name()), bytes.to_vec());
            // After the open and the size check, before the body is read: the bytes read are the new ones, in place.
            set_hook(&AFTER_SIZE_CHECK, &folder, move || {
                fs::write(path, bytes).unwrap()
            });
        }
        fn change_after_next_marker(&self, dir: &GraphDir, file: impl Into<FileRef>, bytes: &[u8]) {
            let folder = self.check(dir).unwrap();
            let (path, bytes) = (folder.join(file.into().path()), bytes.to_vec());
            set_hook(&AFTER_MARKER, &folder, move || {
                fs::write(path, bytes).unwrap()
            });
        }
        fn change_before_next_marker(
            &self,
            dir: &GraphDir,
            file: impl Into<FileRef>,
            bytes: &[u8],
        ) {
            let folder = self.check(dir).unwrap();
            let (path, bytes) = (folder.join(file.into().path()), bytes.to_vec());
            set_hook(&BEFORE_OLD_STAMPS, &folder, move || {
                fs::write(path, bytes).unwrap()
            });
        }
        fn leftovers(&self, dir: &GraphDir) -> Vec<String> {
            let folder = self.check(dir).unwrap();
            let temps_in = |path: &Path, prefix: &str| -> Vec<String> {
                fs::read_dir(path)
                    .map(|entries| {
                        entries
                            .map(|e| e.unwrap().file_name().into_string().unwrap())
                            .filter(|n| n.starts_with(TEMP_PREFIX))
                            .map(|n| format!("{prefix}{n}"))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let mut names = temps_in(&folder, "");
            for space in space_folders(&folder) {
                let rel = space.strip_prefix(&folder).unwrap().display().to_string();
                names.extend(temps_in(&space, &format!("{rel}/")));
            }
            names.sort();
            names
        }
    }

    fn graph_files() -> (FsGraphFiles, GraphDir, TempRoot) {
        let temp = temp_root();
        let root = GraphDir::parse(temp.0.to_str().unwrap()).unwrap();
        (FsGraphFiles::new(vec![root.clone()]), root, temp)
    }

    impl umx_contracts::graph_files::FindHarness for FsGraphFiles {
        fn make_graph(&self, dir: &GraphDir, bytes: &[u8]) {
            fs::create_dir_all(dir.as_str()).unwrap();
            fs::write(Path::new(dir.as_str()).join("graph.json"), bytes).unwrap();
        }
        fn make_folder(&self, dir: &GraphDir) {
            fs::create_dir_all(dir.as_str()).unwrap();
        }
        fn make_repo(&self, dir: &GraphDir) {
            fs::create_dir_all(Path::new(dir.as_str()).join(".git")).unwrap();
        }
        fn make_link(&self, link: &GraphDir, target: &GraphDir) {
            std::os::unix::fs::symlink(target.as_str(), link.as_str()).unwrap();
        }
        fn make_pending(&self, dir: &GraphDir) {
            let at = Path::new(dir.as_str());
            fs::write(at.join(format!("{TEMP_PREFIX}nodes")), b"{}\n").unwrap();
            fs::write(at.join(MARKER), b"{}\n").unwrap();
        }
        fn make_unlistable(&self, dir: &GraphDir, on: bool) {
            use std::os::unix::fs::PermissionsExt;
            let mode = if on { 0o000 } else { 0o755 };
            fs::set_permissions(dir.as_str(), fs::Permissions::from_mode(mode)).unwrap();
        }
        fn cross_device_next_relocate(&self) {
            *self.cross_device.lock().unwrap() = true;
        }
        fn leftovers(&self, dir: &GraphDir) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(dir.as_str())
                .unwrap()
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| n.starts_with(TEMP_PREFIX))
                .collect();
            names.sort();
            names
        }
    }

    umx_contracts::graph_files_find_contract!(
        graph_files;
        gf_f1_finds_graphs_sorted_and_never_inside_one(),
        gf_f2_never_follows_a_symlinked_folder(),
        gf_f3_skips_dot_and_listed_folders(),
        gf_f4_stops_at_a_cap_with_truncated(),
        gf_f5_reports_a_nested_repo_and_does_not_enter_it(),
        gf_f6_a_root_outside_the_roots_is_refused(),
        gf_f7_a_graph_json_over_its_cap_is_found_unread(),
        gf_f8_peek_and_find_report_a_pending_write_and_never_recover_it(),
        gf_f9_a_creation_cut_short_is_a_pending_graph_with_no_json(),
        gf_f10_a_folder_the_walk_cannot_list_is_counted_and_peek_refuses(),
        gf_f11_relocate_moves_the_folder_whole(),
        gf_f12_relocate_refuses_a_target_that_exists(),
        gf_f13_relocate_never_leaves_the_roots(),
        gf_f14_relocate_refuses_a_symlinked_folder(),
        gf_f15_relocate_refuses_a_missing_parent(),
        gf_f16_relocate_across_file_systems_moves_nothing(),
        gf_f17_relocate_carries_everything_inside(),
        gf_f18_relocate_refuses_a_file_target_and_a_folder_inside_itself(),
    );

    /// A linked git worktree inside the walked folder holds `.git` as a file: it is a nested repo all the same,
    /// reported and not entered (gate).
    #[test]
    fn a_nested_linked_worktree_is_a_nested_repo() {
        let (port, root, _temp) = graph_files();
        let wt = Path::new(root.as_str()).join("wt");
        fs::create_dir_all(wt.join("g")).unwrap();
        fs::write(wt.join(".git"), b"gitdir: /elsewhere/.git/worktrees/wt\n").unwrap();
        fs::write(wt.join("g/graph.json"), b"{}").unwrap();
        let found = port.find_graphs(&root, FindLimits::default()).unwrap();
        assert_eq!(found.nested_repos, ["wt"], "{found:?}");
        assert!(found.graphs.is_empty(), "{found:?}");
    }

    /// A walk out of time stops with truncated, before listing anything (the fake's time never passes).
    #[test]
    fn a_walk_out_of_time_is_truncated() {
        let (port, root, _temp) = graph_files();
        fs::create_dir_all(Path::new(root.as_str()).join("g")).unwrap();
        fs::write(Path::new(root.as_str()).join("g/graph.json"), b"{}").unwrap();
        let none = FindLimits {
            time_ms: 0,
            ..FindLimits::default()
        };
        let found = port.find_graphs(&root, none).unwrap();
        assert!(found.graphs.is_empty() && found.truncated, "{found:?}");
        assert_eq!(
            port.find_graphs(&root, FindLimits::default())
                .unwrap()
                .graphs
                .len(),
            1
        );
    }

    umx_contracts::graph_files_contract!(
        graph_files;
        gf_c1_write_then_read_is_identical(),
        gf_c2_read_missing_is_none(),
        gf_c3_overwrite_replaces_the_whole_file(),
        gf_c4_list_is_in_graph_file_order(),
        gf_c5_list_of_missing_dir_is_empty(),
        gf_c6_dirs_are_separate(),
        gf_c7_outside_roots_is_refused(),
        gf_c8_empty_file_round_trips(),
        gf_c9_write_creates_missing_dirs(),
        gf_c10_read_returns_a_copy(),
        gf_c11_resolve_is_stable_and_confined(),
        gf_c13_an_error_means_nothing_changed(),
        gf_c14_set_applies_every_change(),
        gf_c15_set_error_changes_nothing(),
        gf_c16_crash_before_commit_keeps_the_old_state(),
        gf_c17_crash_after_commit_rolls_forward(),
        gf_c18_crash_mid_renames_rolls_forward(),
        gf_c19_changed_target_is_a_conflict(),
        gf_c20_set_never_leaves_the_roots(),
        gf_c21_invalid_set_is_refused(),
        gf_c22_one_file_write_is_a_set(),
        gf_c23_a_set_after_an_unfinished_set_finishes_it_first(),
        gf_c24_concurrent_sets_serialize(),
        gf_c25_resolve_never_recovers(),
        gf_c26_an_over_cap_file_is_refused_unread(),
        gf_c27_a_change_over_cap_is_refused_unwritten(),
        gf_c28_recovery_and_old_stamps_never_read_an_over_cap_body(),
        gf_c29_similar_files_round_trip_and_list_by_kind(),
        gf_c30_a_set_returns_the_stamps_a_fresh_look_sees(),
        gf_c31_a_change_since_the_stamps_refuses_the_set(),
        gf_c32_read_stamped_stamps_the_bytes_it_read(),
        gf_c33_a_change_after_the_marker_refuses_the_set(),
        gf_c34_receipts_round_trip_capped_and_are_no_graph_file(),
        gf_c35_a_set_with_receipts_is_all_or_nothing_across_crashes(),
        gf_c36_a_receipts_only_set_writes_and_keeps_the_content_stamps(),
        gf_c37_a_changed_also_file_refuses_the_set(),
    );

    /// graph.json and nodes.jsonl with old bytes, then a two-file set crashed after its marker; returns the folder.
    fn crashed_set(files: &FsGraphFiles, g: &GraphDir) -> PathBuf {
        files.write(g, GraphFile::Graph, b"g0\n").unwrap();
        files.write(g, GraphFile::Nodes, b"n0\n").unwrap();
        files.crash_at_for_tests(Some(CrashPoint::AfterMarker));
        let set = [
            Change {
                file: GraphFile::Graph.into(),
                bytes: Some(b"g1\n"),
            },
            Change {
                file: GraphFile::Nodes.into(),
                bytes: Some(b"n1\n"),
            },
        ];
        assert!(files.write_set(g, &set).is_err());
        files.check(g).unwrap()
    }

    #[test]
    fn a_marker_that_names_anything_but_a_graph_file_and_a_bare_temp_name_is_refused() {
        let (files, root, temp) = graph_files();
        let g = root.join("g").unwrap();
        let folder = crashed_set(&files, &g);
        fs::write(temp.0.join("outside"), b"keep\n").unwrap();
        let stamp = Stamp::of(Some(b"keep\n")).to_json();
        for (target, tmp) in [
            ("../outside", ".umx-tmp-1-graph.json"),
            ("graph.json", "../outside"),
            ("graph.json", ".umx-tmp-1/../../outside"),
            ("/etc/hosts", ".umx-tmp-1-graph.json"),
            (".gitignore", "remove"),
            ("graph.json", ".umx-tmp-set.json"),
        ] {
            let line = json!({ "target": target, "temp": tmp, "old": stamp, "new": stamp });
            let head = json!({ "hash": HASH });
            fs::write(folder.join(MARKER), format!("{head}\n{line}\n")).unwrap();
            let after = files.restart_for_tests();
            let got = after.read(&g, GraphFile::Graph);
            assert!(
                matches!(&got, Err(FilesError::Io(m)) if m.contains("not one umx wrote")),
                "{target} {tmp}: {got:?}"
            );
        }
        assert_eq!(fs::read(temp.0.join("outside")).unwrap(), b"keep\n");
        assert_eq!(fs::read(folder.join("graph.json")).unwrap(), b"g0\n");
    }

    #[test]
    fn a_target_changed_to_the_same_size_is_still_a_conflict() {
        let (files, root, _temp) = graph_files();
        let g = root.join("g").unwrap();
        let folder = crashed_set(&files, &g);
        // Same size as "g0\n", different bytes: only the hash can tell.
        fs::write(folder.join("graph.json"), b"gX\n").unwrap();
        let after = files.restart_for_tests();
        assert_eq!(
            after.read(&g, GraphFile::Nodes),
            Err(FilesError::RecoveryConflict(vec![GraphFile::Graph.into()]))
        );
        // Put back as it was, the set finishes.
        fs::write(folder.join("graph.json"), b"g0\n").unwrap();
        assert_eq!(
            after.read(&g, GraphFile::Graph).unwrap().as_deref(),
            Some(&b"g1\n"[..])
        );
    }

    #[test]
    fn a_temp_file_swapped_for_a_symlink_is_not_renamed_into_place() {
        let (files, root, temp) = graph_files();
        let g = root.join("g").unwrap();
        let folder = crashed_set(&files, &g);
        fs::write(temp.0.join("secret"), b"g1\n").unwrap();
        let marker = fs::read_to_string(folder.join(MARKER)).unwrap();
        let first = Line::from_json(marker.lines().nth(1).unwrap()).unwrap();
        let swapped = folder.join(first.temp.unwrap());
        fs::remove_file(&swapped).unwrap();
        std::os::unix::fs::symlink(temp.0.join("secret"), &swapped).unwrap();
        let after = files.restart_for_tests();
        assert_eq!(
            after.read(&g, GraphFile::Nodes),
            Err(FilesError::RecoveryConflict(vec![GraphFile::Graph.into()]))
        );
        assert!(
            !fs::symlink_metadata(folder.join("graph.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_refused_set_creates_nothing_outside_the_roots() {
        let (inside, outside) = (temp_root(), temp_root());
        let files = FsGraphFiles::new(vec![root_of(&inside)]);
        let g = root_of(&outside).join("g").unwrap();
        let set = [
            Change {
                file: GraphFile::Graph.into(),
                bytes: Some(b"{}"),
            },
            Change {
                file: GraphFile::Nodes.into(),
                bytes: Some(b""),
            },
        ];
        assert!(matches!(
            files.write_set(&g, &set),
            Err(FilesError::OutsideRoots(_))
        ));
        assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
    }

    #[test]
    fn the_marker_is_json_lines_with_sizes_and_hashes() {
        let (files, root, _temp) = graph_files();
        let g = root.join("g").unwrap();
        let folder = crashed_set(&files, &g);
        let marker = fs::read_to_string(folder.join(MARKER)).unwrap();
        let lines: Vec<Value> = marker.lines().map(|l| l.parse().unwrap()).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], json!({ "hash": "fnv1a64-words-v1" }));
        assert_eq!(lines[1]["target"], "graph.json");
        assert!(lines[1]["temp"].as_str().unwrap().starts_with(TEMP_PREFIX));
        assert_eq!(lines[1]["old"]["size"], 3);
        assert_eq!(lines[1]["new"]["hash"], "32cf1518d71f70df");
    }

    #[test]
    fn the_marker_hash_is_pinned() {
        // Values computed outside Rust (Python, same definition). A change here strands every marker on disk.
        assert_eq!(hash(b""), 0xaf63_bd4c_8601_b7df);
        assert_eq!(hash(b"umx write set\n"), 0x95a8_6e19_af28_f813);
        assert_eq!(hash(b"g0\n"), 0x3635_1518_da02_99df);
    }

    #[test]
    fn a_marker_from_another_hash_is_a_conflict_not_a_comparison() {
        let (files, root, _temp) = graph_files();
        let g = root.join("g").unwrap();
        let folder = crashed_set(&files, &g);
        let marker = fs::read_to_string(folder.join(MARKER)).unwrap();
        let other = marker.replacen(HASH, "fnv1a64-words-v2", 1);
        fs::write(folder.join(MARKER), other).unwrap();
        let after = files.restart_for_tests();
        assert_eq!(
            after.read(&g, GraphFile::Graph),
            Err(FilesError::RecoveryConflict(vec![
                GraphFile::Graph.into(),
                GraphFile::Nodes.into()
            ]))
        );
        assert_eq!(fs::read(folder.join("graph.json")).unwrap(), b"g0\n");
    }

    // Adapter-only: crash atomicity. A temp file left by a crash mid-write is never read or listed as the graph,
    // and a finished write leaves no temp file behind.
    #[test]
    fn crash_leftovers_are_never_the_graph_and_writes_leave_no_temp_files() {
        let (files, root, temp) = graph_files();
        let g = root.join("g").unwrap();
        files.write(&g, GraphFile::Nodes, b"old\n").unwrap();
        let folder = temp.0.join("g");
        fs::write(
            folder.join(format!("{TEMP_PREFIX}nodes.jsonl-999-0")),
            b"half a li",
        )
        .unwrap();
        assert_eq!(
            files.read(&g, GraphFile::Nodes).unwrap().as_deref(),
            Some(&b"old\n"[..])
        );
        assert_eq!(files.list(&g).unwrap(), vec![GraphFile::Nodes]);

        fs::remove_file(folder.join(format!("{TEMP_PREFIX}nodes.jsonl-999-0"))).unwrap();
        for i in 0..20 {
            files
                .write(&g, GraphFile::Nodes, format!("v{i}\n").as_bytes())
                .unwrap();
        }
        let names: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("nodes.jsonl")]);
    }

    fn root_of(temp: &TempRoot) -> GraphDir {
        GraphDir::parse(temp.0.to_str().unwrap()).unwrap()
    }

    #[test]
    fn the_vector_cache_root_is_private_or_refuses_everything() {
        use std::os::unix::fs::PermissionsExt;
        let temp = temp_root();
        let root = temp.0.join("vectors");
        let (cache, refused) = FsGraphFiles::vector_cache(&root);
        assert_eq!(refused, None);
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let g = root_of(&temp).join("vectors/g").unwrap();
        cache.write(&g, GraphFile::Nodes, b"x").unwrap();
        // Open to others, or a symlink: refused, and every call is refused.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let (open, refused) = FsGraphFiles::vector_cache(&root);
        assert!(refused.is_some());
        assert!(matches!(
            open.read(&g, GraphFile::Nodes),
            Err(FilesError::OutsideRoots(_))
        ));
        let link = temp.0.join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let (linked, refused) = FsGraphFiles::vector_cache(&link);
        assert_eq!(refused, Some("the vector cache folder is a symlink"));
        assert!(
            linked
                .write(
                    &root_of(&temp).join("link/g").unwrap(),
                    GraphFile::Nodes,
                    b"y"
                )
                .is_err()
        );
    }

    #[test]
    fn a_shard_folder_symlinked_out_of_the_roots_is_never_followed() {
        let (inside, outside) = (temp_root(), temp_root());
        let files = FsGraphFiles::new(vec![root_of(&inside)]);
        let key = ModelKey::new("m", 4).unwrap();
        let shard = FileRef::Shard(ShardFile::new(key.clone(), "b7").unwrap());
        let set = |bytes: &'static [u8]| {
            [Change {
                file: shard.clone(),
                bytes: Some(bytes),
            }]
        };
        // A plain shard lands in its own folder and leaves no temp file there.
        let g = root_of(&inside).join("g").unwrap();
        files.write_set(&g, &set(b"line\n")).unwrap();
        assert_eq!(
            fs::read(inside.0.join("g/embeddings/m@4/b7.tsv")).unwrap(),
            b"line\n"
        );
        assert_eq!(
            umx_contracts::graph_files::SetHarness::leftovers(&files, &g),
            Vec::<String>::new()
        );
        // embeddings/, or one vector space's folder, swapped for a symlink that leads out of the roots.
        for (graph, link, target) in [
            ("h", "h/embeddings", outside.0.clone()),
            ("k", "k/embeddings/m@4", outside.0.clone()),
        ] {
            fs::create_dir_all(inside.0.join(link).parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&target, inside.0.join(link)).unwrap();
            let h = root_of(&inside).join(graph).unwrap();
            let refused = |r: Result<(), FilesError>| matches!(r, Err(FilesError::OutsideRoots(_)));
            assert!(
                refused(files.write_set(&h, &set(b"x")).map(|_| ())),
                "{link}"
            );
            assert!(refused(files.read_ref(&h, &shard).map(|_| ())), "{link}");
            assert!(
                refused(files.list_shards(&h, &key, ShardKind::Vectors).map(|_| ())),
                "{link}"
            );
        }
        assert_eq!(
            fs::read_dir(&outside.0).unwrap().count(),
            0,
            "nothing written outside"
        );
    }

    /// ws006 S23 (review R24): a folder made at the target after relocate's checks and before its rename is refused,
    /// never replaced: the source stays, the other writer's folder stays empty.
    #[test]
    fn a_target_made_after_the_checks_is_never_replaced() {
        let temp = temp_root();
        let files = FsGraphFiles::new(vec![root_of(&temp)]);
        let (a, b) = (
            root_of(&temp).join("a").unwrap(),
            root_of(&temp).join("b").unwrap(),
        );
        files.write(&a, GraphFile::Graph, b"{}").unwrap();
        *files.before_relocate.lock().unwrap() = Some(|target| fs::create_dir(target).unwrap());
        assert_eq!(
            files.relocate(&a, &b),
            Err(FilesError::Invalid(RELOCATE_TARGET_EXISTS))
        );
        assert!(Path::new(a.as_str()).join("graph.json").exists());
        assert_eq!(fs::read_dir(b.as_str()).unwrap().count(), 0);
    }

    /// A file that passes the size check on its open handle and then grows past the cap before the read is still
    /// refused: the read stops one byte past the cap, and the size reported is the file's size by then.
    #[test]
    fn a_file_that_grows_past_its_cap_after_the_check_is_refused() {
        let temp = temp_root();
        let files = FsGraphFiles::new(vec![root_of(&temp)]);
        let g = root_of(&temp).join("g").unwrap();
        let cap = FileRef::from(GraphFile::Graph).cap() as usize;
        files.write(&g, GraphFile::Graph, &vec![b' '; cap]).unwrap();
        let path = files.check(&g).unwrap().join("graph.json");
        let grow = path.clone();
        set_hook(&AFTER_SIZE_CHECK, &files.check(&g).unwrap(), move || {
            let mut f = fs::OpenOptions::new().append(true).open(grow).unwrap();
            f.write_all(&[b' '; 10]).unwrap();
        });
        let before = files.bytes_read_for_tests();
        assert_eq!(
            files.read(&g, GraphFile::Graph),
            Err(FilesError::TooLarge {
                file: GraphFile::Graph.into(),
                size: cap as u64 + 10,
                cap: cap as u64,
            })
        );
        assert_eq!(
            files.bytes_read_for_tests() - before,
            cap as u64 + 1,
            "the read stops one byte past the cap"
        );
        assert_eq!(fs::metadata(&path).unwrap().len(), cap as u64 + 10);
    }

    /// nodes.jsonl past 4 GiB (sparse, so the test writes nothing) is refused from its size alone, without reading
    /// it; and a FIFO in a graph file's place is refused at once instead of blocking the daemon.
    #[test]
    fn a_huge_nodes_file_is_refused_unread_and_a_fifo_never_blocks() {
        let temp = temp_root();
        let files = FsGraphFiles::new(vec![root_of(&temp)]);
        let g = root_of(&temp).join("g").unwrap();
        files.write(&g, GraphFile::Graph, b"{}").unwrap();
        let folder = files.check(&g).unwrap();
        let cap = u64::from(u32::MAX);
        fs::File::create(folder.join("nodes.jsonl"))
            .unwrap()
            .set_len(cap + 1)
            .unwrap();
        assert_eq!(
            files.read(&g, GraphFile::Nodes),
            Err(FilesError::TooLarge {
                file: GraphFile::Nodes.into(),
                size: cap + 1,
                cap,
            })
        );
        assert_eq!(files.bytes_read_for_tests(), 0);
        let made = std::process::Command::new("mkfifo")
            .arg(folder.join("edges.jsonl"))
            .status()
            .unwrap();
        assert!(made.success());
        assert!(
            matches!(files.read(&g, GraphFile::Edges), Err(FilesError::Io(m)) if m.contains("not a regular file"))
        );
    }

    #[test]
    fn a_folder_symlinked_outside_the_roots_is_refused() {
        let (inside, outside) = (temp_root(), temp_root());
        let files = FsGraphFiles::new(vec![root_of(&inside)]);
        std::os::unix::fs::symlink(&outside.0, inside.0.join("link")).unwrap();
        let through = root_of(&inside).join("link/g").unwrap();
        let refused = |r: Result<(), FilesError>| matches!(r, Err(FilesError::OutsideRoots(_)));
        assert!(refused(
            files.write(&through, GraphFile::Graph, b"{}").map(|_| ())
        ));
        assert!(refused(files.read(&through, GraphFile::Graph).map(|_| ())));
        assert!(refused(files.list(&through).map(|_| ())));
        assert!(!outside.0.join("g").exists());
    }

    #[test]
    fn a_graph_file_symlinked_outside_the_roots_is_refused_on_read() {
        let (inside, outside) = (temp_root(), temp_root());
        let files = FsGraphFiles::new(vec![root_of(&inside)]);
        let g = root_of(&inside).join("g").unwrap();
        files.write(&g, GraphFile::Graph, b"{}").unwrap();
        fs::write(outside.0.join("secret"), b"not yours\n").unwrap();
        std::os::unix::fs::symlink(outside.0.join("secret"), inside.0.join("g/nodes.jsonl"))
            .unwrap();
        assert!(matches!(
            files.read(&g, GraphFile::Nodes),
            Err(FilesError::OutsideRoots(_))
        ));
        assert_eq!(
            files.list(&g).unwrap(),
            vec![GraphFile::Graph, GraphFile::Nodes]
        );
    }

    #[test]
    fn a_root_and_a_path_that_resolve_to_the_same_place_match() {
        // A root given through a symlink and a path given by its real location (and the other way round) are
        // the same place, like /var and /private/var on macOS.
        let temp = temp_root();
        let real = temp.0.join("real");
        fs::create_dir(&real).unwrap();
        let link = temp.0.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (real, link) = (real.to_str().unwrap(), link.to_str().unwrap());
        for (root, path) in [(link, real), (real, link)] {
            let files = FsGraphFiles::new(vec![GraphDir::parse(root).unwrap()]);
            let g = GraphDir::parse(path).unwrap().join("g").unwrap();
            files.write(&g, GraphFile::Graph, b"{}").unwrap();
            assert_eq!(
                files.read(&g, GraphFile::Graph).unwrap().as_deref(),
                Some(&b"{}"[..])
            );
        }
    }

    #[test]
    fn every_spelling_of_one_folder_resolves_to_one_identity() {
        let temp = temp_root();
        fs::create_dir(temp.0.join("real")).unwrap();
        std::os::unix::fs::symlink(temp.0.join("real"), temp.0.join("link")).unwrap();
        let files = FsGraphFiles::new(vec![root_of(&temp)]);
        let via_real = root_of(&temp).join("real/g").unwrap();
        let via_link = root_of(&temp).join("link/g").unwrap();
        assert_eq!(
            files.resolve(&via_real).unwrap(),
            files.resolve(&via_link).unwrap()
        );
    }

    #[test]
    fn a_failed_folder_sync_after_the_rename_is_not_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let (files, root, temp) = graph_files();
        let g = root.join("g").unwrap();
        files.write(&g, GraphFile::Nodes, b"old\n").unwrap();
        let folder = temp.0.join("g");
        // Write and search but no read: the temp file and the rename work, opening the folder to sync it fails.
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o300)).unwrap();
        let result = files.write(&g, GraphFile::Nodes, b"new\n");
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(result, Ok(Written::NotSynced(FilesError::Io(_)))),
            "{result:?}"
        );
        assert_eq!(
            files.read(&g, GraphFile::Nodes).unwrap().as_deref(),
            Some(&b"new\n"[..]),
            "the new bytes are in place"
        );
    }

    #[test]
    fn a_failed_write_keeps_the_old_file_and_removes_its_temp_file() {
        let (files, root, temp) = graph_files();
        let g = root.join("g").unwrap();
        files.write(&g, GraphFile::Graph, b"{}\n").unwrap();
        // A folder where the target file should be makes the rename fail.
        fs::create_dir(temp.0.join("g").join("edges.jsonl")).unwrap();
        assert!(matches!(
            files.write(&g, GraphFile::Edges, b"x\n"),
            Err(FilesError::Io(_))
        ));
        let names: Vec<_> = fs::read_dir(temp.0.join("g"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert_eq!(
            files.read(&g, GraphFile::Graph).unwrap().as_deref(),
            Some(&b"{}\n"[..])
        );
    }

    #[test]
    fn skill_reads_never_follow_a_link_or_wait_on_a_fifo() {
        // F94 plants: a link or FIFO where the walk saw a file or folder is refused, never followed or waited on.
        let base = std::env::temp_dir().join(format!("umx-skill-reads-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (skill, outside) = (base.join("skill"), base.join("outside"));
        fs::create_dir_all(skill.join("references")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(skill.join("SKILL.md"), b"skill").unwrap();
        fs::write(skill.join(".DS_Store"), b"x").unwrap();
        fs::write(outside.join("secret.md"), b"outside").unwrap();
        let mut listed = skill_folder(&skill, 512).unwrap();
        listed.sort();
        assert_eq!(
            listed,
            [("SKILL.md".into(), false), ("references".into(), true)]
        );
        assert_eq!(
            skill_file(&skill.join("SKILL.md"), 5).as_deref(),
            Some(&b"skill"[..])
        );
        assert_eq!(skill_file(&skill.join("SKILL.md"), 4), None, "over the cap");

        // A link at the folder itself, a link inside it, and a link where a file was.
        std::os::unix::fs::symlink(&outside, base.join("linked")).unwrap();
        assert!(skill_folder(&base.join("linked"), 512).is_err());
        std::os::unix::fs::symlink(
            outside.join("secret.md"),
            skill.join("references/swapped.md"),
        )
        .unwrap();
        assert!(skill_folder(&skill.join("references"), 512).is_err());
        assert_eq!(
            skill_file(&skill.join("references/swapped.md"), 1 << 20),
            None
        );
        fs::remove_file(skill.join("references/swapped.md")).unwrap();

        // A FIFO in the folder, and a FIFO read as a file: refused at once, never blocking.
        let fifo = skill.join("references/pipe.md");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(skill_folder(&skill.join("references"), 512).is_err());
        let started = std::time::Instant::now();
        assert_eq!(skill_file(&fifo, 1 << 20), None);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_skill_folder_over_its_entry_budget_is_refused_while_listed() {
        // R230 B1: the budget holds inside the listing; dot entries do not count.
        let dir = std::env::temp_dir().join(format!("umx-skill-budget-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            fs::write(dir.join(format!("f{i}.md")), b"x").unwrap();
        }
        fs::write(dir.join(".DS_Store"), b"x").unwrap();
        assert_eq!(skill_folder(&dir, 5).unwrap().len(), 5);
        let over = skill_folder(&dir, 4).unwrap_err();
        assert_eq!(over.kind(), ErrorKind::InvalidData);
        assert!(over.to_string().contains("more entries"), "{over}");
        assert!(skill_folder(&dir, 0).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_skill_root_says_link_or_not_without_following_it() {
        // F111: a link answers with its own target (never followed), a folder with None, nothing there with Err.
        let base = std::env::temp_dir().join(format!("umx-skill-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("skill")).unwrap();
        std::os::unix::fs::symlink("skill", base.join("linked")).unwrap();
        std::os::unix::fs::symlink(base.join("gone"), base.join("dangling")).unwrap();
        assert_eq!(skill_root(&base.join("skill")).unwrap(), None);
        assert_eq!(
            skill_root(&base.join("linked")).unwrap(),
            Some(std::path::PathBuf::from("skill"))
        );
        assert_eq!(
            skill_root(&base.join("dangling")).unwrap(),
            Some(base.join("gone"))
        );
        assert_eq!(
            skill_root(&base.join("missing")).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        fs::remove_dir_all(&base).unwrap();
    }
}
