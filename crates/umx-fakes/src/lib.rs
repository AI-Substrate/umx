//! Strict in-memory fakes, one per port (blueprint 001, c-fakes). They pass the same contract suites as the real
//! adapters. Wired only in stub mode and in tests.

mod agent_identity;
pub use agent_identity::FakeAgentIdentity;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use umx_core::Id;
use umx_core::codec::GraphFile;
use umx_core::ports::{
    Change, Clock, Committed, ConfigFile, ConfigFiles, ConfigFilesError, CrashPoint, EmbedError,
    Embedded, Embedder, FileRef, FileStamp, FilesError, GraphDir, GraphFiles, GraphStamps,
    IdGenerator, LocalState, LocalStateError, Locality, ModelKey, Secret, SecretSource, Secrets,
    SecretsIssue, ServiceError, ServiceManager, ServiceSpec, ServiceState, SourceError,
    SourceReader, StateWritten, TextKind, Written, config_files, unit_safe,
};
use umx_core::ports::{
    FetchError, FetchProgress, FileFetched, ModelFetcher, ModelSource, model_fetcher,
};
use umx_core::ports::{
    UiCodeUse, UiDetached, UiEnd, UiError, UiHost, UiLaunch, UiProcess, UiServed, ui_host,
};
use umx_core::vectors::{ShardFile, ShardKind};

/// 2026-11-09T00:00:00Z, the fakes' default "now".
pub const FAKE_EPOCH_MS: u64 = 1_794_182_400_000;

/// fake.graph-files: a map of (folder, file name) to bytes under one root. A set goes through the same steps as
/// fs.graph-files (temp files, a marker, renames, cleanup) with the same crash points and the same recovery, so an
/// ordering bug shows here too (anti-pattern ap-29). The "disk" is shared with every `restart_for_tests` copy.
pub struct FakeGraphFiles {
    root: GraphDir,
    disk: Arc<Mutex<Disk>>,
    recovered: Mutex<BTreeSet<GraphDir>>,
    crash: Mutex<Option<CrashPoint>>,
    fail_writes: AtomicBool,
    fail_file: Mutex<Option<GraphFile>>,
    fail_sync: AtomicBool,
    shard_writes: AtomicUsize,
    similar_writes: AtomicUsize,
    graph_writes: AtomicUsize,
    bytes_read: AtomicU64,
    /// A change a test lands between the next `read_stamped` of that file and its return (F144 same-open clause).
    mid_read: Mutex<Option<(GraphDir, GraphFile, Vec<u8>)>>,
    /// A change a test lands after the marker of the next set in the first folder, before its renames: in the
    /// second folder's file (F144 late-check clause; the cache's post-check when the folders differ).
    after_marker: Mutex<Option<LateChange>>,
    /// A change a test lands in the next set in the first folder once its temps are written, before it takes the
    /// old stamps for its marker (F144: a change then must never be recorded as old).
    before_old: Mutex<Option<LateChange>>,
    /// The next relocate answers as a rename across file systems would.
    cross_device: AtomicBool,
}

/// The set's folder, then the folder, file and bytes of the change that lands after its marker.
type LateChange = (GraphDir, GraphDir, FileRef, Vec<u8>);

const TEMP_PREFIX: &str = ".umx-tmp-";
const MARKER: &str = ".umx-tmp-set.json";

#[derive(Default)]
struct Disk {
    files: BTreeMap<(GraphDir, String), Vec<u8>>,
    /// Each file's generation, bumped by every put (a write, a rename, a tamper) and by a touch: the fake's
    /// [`FileStamp`] (F144).
    gens: BTreeMap<(GraphDir, String), u64>,
    next_gen: u64,
    /// Each folder's synced marker: one line per change.
    markers: BTreeMap<GraphDir, Vec<MarkerLine>>,
    temps: u64,
    /// What a graph walk sees besides the files (ws002 s17): plain folders, folders holding `.git`, and symlinks.
    folders: BTreeSet<GraphDir>,
    repos: BTreeSet<GraphDir>,
    links: BTreeSet<GraphDir>,
    /// Folders a walk cannot list (F177).
    unlistable: BTreeSet<GraphDir>,
}

/// What the fs marker records, kept as bytes: the fake compares whole files where fs compares size and hash.
#[derive(Clone)]
struct MarkerLine {
    target: FileRef,
    temp: Option<String>,
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
}

impl Disk {
    fn get(&self, dir: &GraphDir, name: &str) -> Option<&Vec<u8>> {
        self.files.get(&(dir.clone(), name.to_owned()))
    }

    fn take(&mut self, dir: &GraphDir, name: &str) -> Option<Vec<u8>> {
        self.gens.remove(&(dir.clone(), name.to_owned()));
        self.files.remove(&(dir.clone(), name.to_owned()))
    }

    fn put(&mut self, dir: &GraphDir, name: &str, bytes: Vec<u8>) {
        self.touch(dir, name);
        self.files.insert((dir.clone(), name.to_owned()), bytes);
    }

    /// A new generation for the file, its bytes kept.
    fn touch(&mut self, dir: &GraphDir, name: &str) {
        self.next_gen += 1;
        self.gens
            .insert((dir.clone(), name.to_owned()), self.next_gen);
    }

    fn stamp(&self, dir: &GraphDir, file: &FileRef) -> FileStamp {
        self.gens
            .get(&(dir.clone(), file.path()))
            .map_or(FileStamp::ABSENT, |g| {
                FileStamp::from_parts([*g, 0, 0, 0, 0])
            })
    }

    fn stamps(&self, dir: &GraphDir) -> GraphStamps {
        let mut out = GraphStamps::ABSENT;
        for file in GraphStamps::FILES {
            out.set(file, self.stamp(dir, &file.into()));
        }
        out
    }

    fn temp_names(&self, dir: &GraphDir) -> Vec<String> {
        self.files
            .keys()
            .filter(|(d, n)| d == dir && n.starts_with(TEMP_PREFIX))
            .map(|(_, n)| n.clone())
            .collect()
    }

    /// Applies one marker line: rename its temp over the target, or remove the target.
    fn apply(&mut self, dir: &GraphDir, line: &MarkerLine) {
        let target = line.target.path();
        match &line.temp {
            Some(temp) => {
                let bytes = self.take(dir, temp).unwrap_or_default();
                self.put(dir, &target, bytes);
            }
            None => {
                self.take(dir, &target);
            }
        }
    }
}

/// `dir/name` as text.
fn alloc_join(dir: &GraphDir, name: &str) -> String {
    format!("{}/{name}", dir.as_str())
}

impl FakeGraphFiles {
    pub fn new(root: GraphDir) -> FakeGraphFiles {
        FakeGraphFiles::over(root, Arc::default())
    }

    /// For a graph walk (ws002 s17): `dir` is a folder, even if empty.
    pub fn add_folder_for_tests(&self, dir: GraphDir) {
        self.disk.lock().unwrap().folders.insert(dir);
    }

    /// For a graph walk: `dir` holds `.git`, a repository's top.
    pub fn add_repo_for_tests(&self, dir: GraphDir) {
        self.disk.lock().unwrap().repos.insert(dir);
    }

    /// For a graph walk: `link` is a symlink (to anything); a walk never goes through it.
    pub fn add_link_for_tests(&self, link: GraphDir) {
        self.disk.lock().unwrap().links.insert(link);
    }

    /// Tests only: `dir` cannot be listed (`on`), as a folder without read permission, or can be again.
    pub fn cross_device_next_relocate_for_tests(&self) {
        self.cross_device.store(true, Ordering::SeqCst);
    }

    pub fn unlistable_for_tests(&self, dir: GraphDir, on: bool) {
        let mut disk = self.disk.lock().unwrap();
        if on {
            disk.unlistable.insert(dir);
        } else {
            disk.unlistable.remove(&dir);
        }
    }

    fn over(root: GraphDir, disk: Arc<Mutex<Disk>>) -> FakeGraphFiles {
        FakeGraphFiles {
            root,
            disk,
            recovered: Mutex::default(),
            crash: Mutex::new(None),
            fail_writes: AtomicBool::new(false),
            fail_file: Mutex::new(None),
            fail_sync: AtomicBool::new(false),
            shard_writes: AtomicUsize::new(0),
            similar_writes: AtomicUsize::new(0),
            graph_writes: AtomicUsize::new(0),
            bytes_read: AtomicU64::new(0),
            mid_read: Mutex::new(None),
            after_marker: Mutex::new(None),
            before_old: Mutex::new(None),
            cross_device: AtomicBool::new(false),
        }
    }

    /// Vector shard changes that landed, so tests can prove a change wrote exactly one shard, or none.
    pub fn shard_writes(&self) -> usize {
        self.shard_writes.load(Ordering::SeqCst)
    }

    /// similar_to file changes that landed (shards and `_meta.tsv`), counted apart from vector shards.
    pub fn similar_writes(&self) -> usize {
        self.similar_writes.load(Ordering::SeqCst)
    }

    /// Graph file changes that landed (graph.json, nodes.jsonl, edges.jsonl, one per file per set), so tests can
    /// prove a change wrote its files once.
    pub fn graph_writes(&self) -> usize {
        self.graph_writes.load(Ordering::SeqCst)
    }

    /// Makes every write fail with an I/O error (and change nothing) until switched off.
    pub fn fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::SeqCst);
    }

    /// Makes writes of one file fail (and change nothing) until set to None. A set with that file fails whole,
    /// before its point of no return.
    pub fn fail_writes_to(&self, file: Option<GraphFile>) {
        *self.fail_file.lock().unwrap() = file;
    }

    /// Makes every write land but report `Written::NotSynced`, as when the folder sync after a rename fails.
    pub fn fail_sync(&self, fail: bool) {
        self.fail_sync.store(fail, Ordering::SeqCst);
    }

    /// Tests only: stops the next `write_set` at `point`, as if the process died there.
    pub fn crash_at_for_tests(&self, point: Option<CrashPoint>) {
        *self.crash.lock().unwrap() = point;
    }

    /// Tests only: a new fake over the same files, as after a restart.
    pub fn restart_for_tests(&self) -> FakeGraphFiles {
        FakeGraphFiles::over(self.root.clone(), Arc::clone(&self.disk))
    }

    /// Tests only: writes a file behind the port's back (a pull, a checkout). Its generation moves on.
    pub fn tamper_for_tests(&self, dir: &GraphDir, file: impl Into<FileRef>, bytes: &[u8]) {
        self.disk
            .lock()
            .unwrap()
            .put(dir, &file.into().path(), bytes.to_vec());
    }

    /// Tests only: a touch, the bytes kept and the generation moved on.
    pub fn touch_for_tests(&self, dir: &GraphDir, file: GraphFile) {
        let mut disk = self.disk.lock().unwrap();
        if disk.get(dir, file.file_name()).is_some() {
            disk.touch(dir, file.file_name());
        }
    }

    /// Tests only: lands `bytes` in `file` after the next `read_stamped` of it has taken its bytes and before it
    /// returns, as a writer racing the read would.
    pub fn change_during_next_read_for_tests(&self, dir: &GraphDir, file: GraphFile, bytes: &[u8]) {
        *self.mid_read.lock().unwrap() = Some((dir.clone(), file, bytes.to_vec()));
    }

    /// Tests only: once the next set in `set_in` has its temps and marker in place, before its first rename, lands
    /// `bytes` in `file` of `dir` (F144: the check runs after the marker; `dir` may be another folder on this disk,
    /// such as a graph whose vectors the set caches).
    pub fn change_after_next_marker_for_tests(
        &self,
        set_in: &GraphDir,
        dir: &GraphDir,
        file: impl Into<FileRef>,
        bytes: &[u8],
    ) {
        *self.after_marker.lock().unwrap() =
            Some((set_in.clone(), dir.clone(), file.into(), bytes.to_vec()));
    }

    /// Tests only: lands `bytes` in `file` once the next set in `dir` has its temps written, before it takes the old
    /// stamps for its marker (F144: such a change must be refused, never recorded as old).
    pub fn change_before_next_marker_for_tests(
        &self,
        dir: &GraphDir,
        file: impl Into<FileRef>,
        bytes: &[u8],
    ) {
        *self.before_old.lock().unwrap() =
            Some((dir.clone(), dir.clone(), file.into(), bytes.to_vec()));
    }

    /// Tests only: a file's bytes with no recovery run.
    pub fn raw_for_tests(&self, dir: &GraphDir, file: impl Into<FileRef>) -> Option<Vec<u8>> {
        self.disk
            .lock()
            .unwrap()
            .get(dir, &file.into().path())
            .cloned()
    }

    /// Tests only: every byte a read has copied out so far.
    pub fn bytes_read_for_tests(&self) -> u64 {
        self.bytes_read.load(Ordering::SeqCst)
    }

    /// Tests only: a write set of graph.json, nodes and edges on `dir` (a creation when `dir` has none yet) cut short
    /// after its marker (CrashPoint::AfterMarker), so its temp
    /// files and marker are left for the next call that recovers.
    pub fn cut_short_for_tests(&self, dir: &GraphDir) {
        self.crash_at_for_tests(Some(CrashPoint::AfterMarker));
        let changes = [
            Change {
                file: GraphFile::Graph.into(),
                bytes: Some(b"{}\n"),
            },
            Change {
                file: GraphFile::Nodes.into(),
                bytes: Some(b"{}\n"),
            },
            Change {
                file: GraphFile::Edges.into(),
                bytes: Some(b"{}\n"),
            },
        ];
        let _ = self.write_set_checked(dir, &changes, None);
        self.crash_at_for_tests(None);
    }

    /// Tests only: the temp file and marker names in the folder, sorted.
    pub fn leftovers_for_tests(&self, dir: &GraphDir) -> Vec<String> {
        let disk = self.disk.lock().unwrap();
        let mut names = disk.temp_names(dir);
        if disk.markers.contains_key(dir) {
            names.push(MARKER.to_owned());
        }
        names.sort();
        names
    }

    fn check(&self, dir: &GraphDir) -> Result<(), FilesError> {
        if dir.is_within(&self.root) {
            Ok(())
        } else {
            Err(FilesError::OutsideRoots(dir.clone()))
        }
    }

    fn crash_here(&self, point: CrashPoint) -> bool {
        let mut armed = self.crash.lock().unwrap();
        let hit = *armed == Some(point);
        if hit {
            *armed = None;
        }
        hit
    }

    /// The fs adapter's recovery, once per folder per instance, before anything else touches the folder.
    fn recover(
        &self,
        recovered: &mut BTreeSet<GraphDir>,
        dir: &GraphDir,
    ) -> Result<(), FilesError> {
        if recovered.contains(dir) {
            return Ok(());
        }
        let mut disk = self.disk.lock().unwrap();
        if let Some(lines) = disk.markers.get(dir).cloned() {
            let mut todo = Vec::new();
            let mut conflicts = Vec::new();
            for line in &lines {
                // Like fs.graph-files: a body over the target's cap matches no stamp, so it is a conflict and is
                // never read; every body compared counts as read.
                let cap = line.target.cap();
                let body = |name: &str| match disk.get(dir, name) {
                    Some(b) if b.len() as u64 > cap => Err(()),
                    Some(b) => {
                        self.bytes_read.fetch_add(b.len() as u64, Ordering::SeqCst);
                        Ok(Some(b))
                    }
                    None => Ok(None),
                };
                let Ok(now) = body(&line.target.path()) else {
                    conflicts.push(line.target.clone());
                    continue;
                };
                if now == line.new.as_ref() {
                    continue;
                }
                let temp_ok = match &line.temp {
                    Some(t) => body(t).is_ok_and(|b| b == line.new.as_ref()),
                    None => true,
                };
                if now == line.old.as_ref() && temp_ok {
                    todo.push(line.clone());
                } else {
                    conflicts.push(line.target.clone());
                }
            }
            if !conflicts.is_empty() {
                return Err(FilesError::RecoveryConflict(conflicts));
            }
            for line in &todo {
                disk.apply(dir, line);
            }
            disk.markers.remove(dir);
        }
        for temp in disk.temp_names(dir) {
            disk.take(dir, &temp);
        }
        recovered.insert(dir.clone());
        Ok(())
    }
}

impl FakeGraphFiles {
    /// The read half of `read_ref`, under the disk lock the caller holds.
    fn read_in(
        &self,
        disk: &Disk,
        dir: &GraphDir,
        file: &FileRef,
    ) -> Result<Option<Vec<u8>>, FilesError> {
        let Some(bytes) = disk.get(dir, &file.path()) else {
            return Ok(None);
        };
        // Like fs.graph-files: the size first, and an over-cap body is never copied out.
        let (size, cap) = (bytes.len() as u64, file.cap());
        if size > cap {
            return Err(FilesError::TooLarge {
                file: file.clone(),
                size,
                cap,
            });
        }
        self.bytes_read.fetch_add(size, Ordering::SeqCst);
        Ok(Some(bytes.clone()))
    }
}

impl GraphFiles for FakeGraphFiles {
    fn adapter_id(&self) -> &'static str {
        "fake.graph-files"
    }

    fn relocate(&self, from: &GraphDir, to: &GraphDir) -> Result<(), FilesError> {
        use umx_core::ports::{
            RELOCATE_INSIDE_ITSELF, RELOCATE_PARENT_MISSING, RELOCATE_TARGET_EXISTS,
        };
        self.check(from)?;
        self.check(to)?;
        if to.is_within(from) {
            return Err(FilesError::Invalid(RELOCATE_INSIDE_ITSELF));
        }
        let mut recovered = self.recovered.lock().unwrap();
        if self
            .disk
            .lock()
            .unwrap()
            .links
            .iter()
            .any(|l| from.is_within(l))
        {
            return Err(FilesError::OutsideRoots(from.clone()));
        }
        self.recover(&mut recovered, from)?;
        let mut disk = self.disk.lock().unwrap();
        let folder = |disk: &Disk, dir: &GraphDir| {
            *dir == self.root
                || disk.folders.contains(dir)
                || disk.repos.contains(dir)
                || disk.links.contains(dir)
                || disk.files.keys().any(|(d, _)| d.is_within(dir))
                || disk.markers.keys().any(|d| d.is_within(dir))
        };
        // A file at the path is there too (review R26): `<dir>/<name>` is a key of `files`.
        let file = |disk: &Disk, path: &GraphDir| {
            path.as_str()
                .rsplit_once('/')
                .and_then(|(d, name)| {
                    GraphDir::parse(if d.is_empty() { "/" } else { d })
                        .ok()
                        .map(|d| (d, name))
                })
                .is_some_and(|(d, name)| disk.get(&d, name).is_some())
        };
        if !folder(&disk, from) || file(&disk, from) {
            return Err(FilesError::io("relocate: the graph folder is not there"));
        }
        if folder(&disk, to) || file(&disk, to) {
            return Err(FilesError::Invalid(RELOCATE_TARGET_EXISTS));
        }
        let exists = folder;
        // `/b`'s parent is `/` (review R26).
        let parent = to
            .as_str()
            .rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p });
        let parent = parent.and_then(|p| GraphDir::parse(p).ok());
        if !parent.is_some_and(|p| exists(&disk, &p)) {
            return Err(FilesError::Invalid(RELOCATE_PARENT_MISSING));
        }
        if self.cross_device.swap(false, Ordering::SeqCst) {
            return Err(FilesError::CrossDevice);
        }
        // Everything at or under `from` moves, generations kept: a rename keeps each file as it was.
        let moved = |d: &GraphDir| {
            GraphDir::parse(&format!(
                "{}{}",
                to.as_str(),
                &d.as_str()[from.as_str().len()..]
            ))
            .unwrap()
        };
        let disk = &mut *disk;
        let keys: Vec<(GraphDir, String)> = disk
            .files
            .keys()
            .filter(|(d, _)| d.is_within(from))
            .cloned()
            .collect();
        for (d, name) in keys {
            let bytes = disk.files.remove(&(d.clone(), name.clone())).unwrap();
            let generation = disk.gens.remove(&(d.clone(), name.clone()));
            let to_dir = moved(&d);
            disk.files.insert((to_dir.clone(), name.clone()), bytes);
            if let Some(g) = generation {
                disk.gens.insert((to_dir, name), g);
            }
        }
        // Folders, symlinks and each cut-short write's marker inside move too, as a whole-folder rename moves them
        // (review R25).
        for set in [
            &mut disk.folders,
            &mut disk.repos,
            &mut disk.links,
            &mut disk.unlistable,
        ] {
            let inside: Vec<GraphDir> = set.iter().filter(|d| d.is_within(from)).cloned().collect();
            for d in inside {
                set.remove(&d);
                set.insert(moved(&d));
            }
        }
        let marked: Vec<GraphDir> = disk
            .markers
            .keys()
            .filter(|d| d.is_within(from))
            .cloned()
            .collect();
        for d in marked {
            let lines = disk.markers.remove(&d).expect("listed above");
            disk.markers.insert(moved(&d), lines);
        }
        recovered.remove(from);
        recovered.insert(to.clone());
        Ok(())
    }

    fn read_ref(&self, dir: &GraphDir, file: &FileRef) -> Result<Option<Vec<u8>>, FilesError> {
        self.check(dir)?;
        self.recover(&mut self.recovered.lock().unwrap(), dir)?;
        self.read_in(&self.disk.lock().unwrap(), dir, file)
    }

    fn read_ref_stamped(
        &self,
        dir: &GraphDir,
        file: &FileRef,
    ) -> Result<(Option<Vec<u8>>, FileStamp), FilesError> {
        self.check(dir)?;
        self.recover(&mut self.recovered.lock().unwrap(), dir)?;
        // The bytes and the generation under one lock, as fs takes both from one open handle: the stamp is the
        // bytes' own, whatever lands after.
        let read = {
            let disk = self.disk.lock().unwrap();
            self.read_in(&disk, dir, file)
                .map(|bytes| (bytes, disk.stamp(dir, file)))
        };
        let racing = self
            .mid_read
            .lock()
            .unwrap()
            .take_if(|(d, f, _)| d == dir && FileRef::Graph(*f) == *file);
        if let Some((d, f, bytes)) = racing {
            self.tamper_for_tests(&d, f, &bytes);
        }
        read
    }

    fn stamps(&self, dir: &GraphDir) -> Result<GraphStamps, FilesError> {
        self.check(dir)?;
        self.recover(&mut self.recovered.lock().unwrap(), dir)?;
        Ok(self.disk.lock().unwrap().stamps(dir))
    }

    fn write_set_checked_also(
        &self,
        dir: &GraphDir,
        changes: &[Change<'_>],
        expect: Option<&GraphStamps>,
        also: &[(FileRef, FileStamp)],
    ) -> Result<Committed, FilesError> {
        // Every file the set checks whose stamp differs now: the content files first, then the `also` ones.
        let changed_in = |disk: &Disk| -> Vec<FileRef> {
            let mut out: Vec<FileRef> = expect
                .map(|e| e.changed(&disk.stamps(dir)))
                .unwrap_or_default()
                .into_iter()
                .map(FileRef::from)
                .collect();
            out.extend(
                also.iter()
                    .filter(|(f, stamp)| disk.stamp(dir, f) != *stamp)
                    .map(|(f, _)| f.clone()),
            );
            out
        };
        self.check(dir)?;
        Change::validate(changes)?;
        // Like fs.graph-files, the set runs under the recovery boundary from its recovery to its end.
        let mut recovered = self.recovered.lock().unwrap();
        self.recover(&mut recovered, dir)?;
        let mut disk = self.disk.lock().unwrap();
        // Like a file system, a folder cannot be made inside a file.
        let blocked = disk.files.keys().any(|(d, name)| {
            d.join(name)
                .is_ok_and(|in_the_way| dir.is_within(&in_the_way))
        });
        if blocked {
            return Err(FilesError::io("fake: a file is in the way of the folder"));
        }
        // Like fs.graph-files, a set of several changes stamps each old file for its marker, under the file's cap.
        if changes.len() > 1 {
            for c in changes {
                if let Some(old) = disk.get(dir, &c.file.path()) {
                    let (size, cap) = (old.len() as u64, c.file.cap());
                    if size > cap {
                        return Err(FilesError::TooLarge {
                            file: c.file.clone(),
                            size,
                            cap,
                        });
                    }
                    self.bytes_read.fetch_add(size, Ordering::SeqCst);
                }
            }
        }
        // 1. Temp files.
        let mut lines = Vec::new();
        for c in changes {
            let refused = self.fail_writes.load(Ordering::SeqCst)
                || self
                    .fail_file
                    .lock()
                    .unwrap()
                    .is_some_and(|f| c.file == FileRef::Graph(f));
            if refused && c.bytes.is_some() {
                for line in &lines {
                    if let MarkerLine { temp: Some(t), .. } = line {
                        disk.take(dir, t);
                    }
                }
                return Err(FilesError::io(format!("fake: write {} refused", c.file)));
            }
            let temp = c.bytes.map(|bytes| {
                disk.temps += 1;
                let name = format!(
                    "{TEMP_PREFIX}{}-{}",
                    disk.temps,
                    c.file.path().replace('/', "_")
                );
                disk.put(dir, &name, bytes.to_vec());
                name
            });
            lines.push(MarkerLine {
                target: c.file.clone(),
                temp,
                old: None,
                new: c.bytes.map(<[u8]>::to_vec),
            });
        }
        if self.crash_here(CrashPoint::AfterTemps) {
            // A crashed set left its files as they are: the next call on the folder recovers them (gf_c23).
            recovered.remove(dir);
            return Err(FilesError::io("crash"));
        }
        let early = self
            .before_old
            .lock()
            .unwrap()
            .take_if(|(set_in, ..)| set_in == dir);
        if let Some((_, there, file, bytes)) = early {
            disk.put(&there, &file.path(), bytes);
        }
        // Like fs.graph-files, the old targets are taken for the marker once the temps are in place.
        for line in &mut lines {
            line.old = disk.get(dir, &line.target.path()).cloned();
        }
        // The first F144 check, as fs.graph-files makes it for a set of several: once the old targets are taken and
        // before the marker, so a change made while the temps were written is never recorded as old for a
        // recovery to roll forward over.
        if lines.len() > 1 {
            let changed = changed_in(&disk);
            if !changed.is_empty() {
                for line in &lines {
                    if let MarkerLine { temp: Some(t), .. } = line {
                        disk.take(dir, t);
                    }
                }
                return Err(FilesError::Changed(changed));
            }
        }
        // 2. The marker; once it is in place the set stands, but for the second F144 check below. One change needs
        // none: one rename is atomic.
        let marked = lines.len() > 1;
        if marked {
            disk.markers.insert(dir.clone(), lines.clone());
        }
        if self.crash_here(CrashPoint::AfterMarker) {
            // A crashed set left its files as they are: the next call on the folder recovers them (gf_c23).
            recovered.remove(dir);
            return Err(FilesError::io("crash"));
        }
        let late = self
            .after_marker
            .lock()
            .unwrap()
            .take_if(|(set_in, ..)| set_in == dir);
        if let Some((_, there, file, bytes)) = late {
            disk.put(&there, &file.path(), bytes);
        }
        // The second F144 check, as fs.graph-files makes it, as late as the set allows: every temp and the marker are in
        // place, nothing is renamed yet. A refusal takes the marker back and drops the temps.
        {
            let changed = changed_in(&disk);
            if !changed.is_empty() {
                disk.markers.remove(dir);
                for line in &lines {
                    if let MarkerLine { temp: Some(t), .. } = line {
                        disk.take(dir, t);
                    }
                }
                return Err(FilesError::Changed(changed));
            }
        }
        // 3. Renames and removes, in the order given.
        for (i, line) in lines.iter().enumerate() {
            disk.apply(dir, line);
            match &line.target {
                FileRef::Shard(s) if s.kind() == ShardKind::Vectors => {
                    self.shard_writes.fetch_add(1, Ordering::SeqCst);
                }
                FileRef::Shard(_) | FileRef::SimilarMeta(_) => {
                    self.similar_writes.fetch_add(1, Ordering::SeqCst);
                }
                FileRef::Graph(_) | FileRef::Receipts => {
                    self.graph_writes.fetch_add(1, Ordering::SeqCst);
                }
            }
            if i == 0 && self.crash_here(CrashPoint::AfterFirstRename) {
                // A crashed set left its files as they are: the next call on the folder recovers them (gf_c23).
                recovered.remove(dir);
                return Err(FilesError::io("crash"));
            }
        }
        // The stamps after the renames, taken under the lock that made them (fs takes them from the temps' handles).
        let stamps = disk.stamps(dir);
        // 4. Cleanup. A failed sync leaves the marker for the next call to finish.
        if self.fail_sync.load(Ordering::SeqCst) {
            if marked {
                recovered.remove(dir);
            }
            return Ok(Committed {
                written: Written::NotSynced(FilesError::io("fake: sync refused")),
                stamps,
            });
        }
        disk.markers.remove(dir);
        Ok(Committed {
            written: Written::Durable,
            stamps,
        })
    }

    fn list(&self, dir: &GraphDir) -> Result<Vec<GraphFile>, FilesError> {
        self.check(dir)?;
        self.recover(&mut self.recovered.lock().unwrap(), dir)?;
        let disk = self.disk.lock().unwrap();
        Ok(GraphFile::ALL
            .into_iter()
            .filter(|f| disk.get(dir, f.file_name()).is_some())
            .collect())
    }

    /// A map has no aliases: a folder is its own identity.
    fn resolve(&self, dir: &GraphDir) -> Result<GraphDir, FilesError> {
        self.check(dir)?;
        Ok(dir.clone())
    }

    fn list_shards(
        &self,
        dir: &GraphDir,
        key: &ModelKey,
        kind: ShardKind,
    ) -> Result<Vec<ShardFile>, FilesError> {
        self.check(dir)?;
        self.recover(&mut self.recovered.lock().unwrap(), dir)?;
        let disk = self.disk.lock().unwrap();
        Ok(disk
            .files
            .keys()
            .filter(|(d, _)| d == dir)
            .filter_map(|(_, name)| match FileRef::parse(name)? {
                FileRef::Shard(s) if s.key() == key && s.kind() == kind => Some(s),
                _ => None,
            })
            .collect())
    }

    fn list_vector_spaces(&self, dir: &GraphDir) -> Result<Vec<ModelKey>, FilesError> {
        self.check(dir)?;
        self.recover(&mut self.recovered.lock().unwrap(), dir)?;
        let disk = self.disk.lock().unwrap();
        let keys: BTreeSet<ModelKey> = disk
            .files
            .keys()
            .filter(|(d, _)| d == dir)
            .filter_map(|(_, name)| match FileRef::parse(name)? {
                FileRef::Shard(s) if s.kind() == ShardKind::Vectors => Some(s.key().clone()),
                _ => None,
            })
            .collect();
        Ok(keys.into_iter().collect())
    }

    /// The walk every adapter makes ([`umx_core::scan::walk_for_graphs`]) over this fake's disk: a
    /// folder holds its files, the folders under it (from every file, folder, repo and link added below it), `.git`
    /// when it is a repo, and its links as `Other`; nothing under a link is seen. Time does not pass.
    fn find_graphs(
        &self,
        root: &GraphDir,
        limits: umx_core::ports::FindLimits,
    ) -> Result<umx_core::ports::Found, FilesError> {
        use umx_core::ports::EntryKind;
        self.check(root)?;
        let disk = self.disk.lock().unwrap();
        let at = |rel: &str| {
            if rel.is_empty() {
                Some(root.clone())
            } else {
                root.join(rel).ok()
            }
        };
        let under_link = |d: &GraphDir| disk.links.iter().any(|l| d.is_within(l));
        let list = |rel: &str| {
            let dir = at(rel)?;
            if under_link(&dir) || disk.unlistable.contains(&dir) {
                return None;
            }
            let mut entries: BTreeMap<String, EntryKind> = BTreeMap::new();
            let child = |d: &GraphDir| -> Option<String> {
                let rest = d.as_str().strip_prefix(dir.as_str())?.strip_prefix('/')?;
                rest.split('/').next().map(str::to_owned)
            };
            for (d, name) in disk.files.keys() {
                if *d == dir {
                    entries.insert(name.clone(), EntryKind::File);
                } else if let Some(c) = child(d) {
                    entries.insert(c, EntryKind::Folder);
                }
            }
            for d in disk.folders.iter().chain(&disk.repos) {
                if let Some(c) = child(d) {
                    entries.insert(c, EntryKind::Folder);
                }
            }
            if disk.repos.contains(&dir) {
                entries.insert(".git".into(), EntryKind::Folder);
            }
            if disk.markers.contains_key(&dir) {
                entries.insert(MARKER.into(), EntryKind::Pending);
            }
            for l in &disk.links {
                if let Some(c) = child(l)
                    && l.as_str() == alloc_join(&dir, &c)
                {
                    entries.insert(c, EntryKind::Other);
                }
            }
            Some(entries.into_iter().collect())
        };
        let graph_json = |rel: &str| {
            let bytes = disk.get(&at(rel)?, GraphFile::Graph.file_name())?;
            (bytes.len() as u64 <= umx_core::scan::FOUND_GRAPH_JSON_CAP).then(|| bytes.clone())
        };
        Ok(umx_core::scan::walk_for_graphs(
            limits,
            list,
            graph_json,
            || 0,
        ))
    }

    /// No recovery: the files and the marker as they are (R289-3).
    fn peek_graph(
        &self,
        dir: &GraphDir,
    ) -> Result<Option<umx_core::ports::FoundGraph>, FilesError> {
        use umx_core::ports::EntryKind;
        self.check(dir)?;
        let disk = self.disk.lock().unwrap();
        if disk.links.iter().any(|l| dir.is_within(l)) {
            return Err(FilesError::io("peek folder: a symlink"));
        }
        if disk.unlistable.contains(dir) {
            return Err(FilesError::io("peek folder: PermissionDenied"));
        }
        let graph = disk.get(dir, GraphFile::Graph.file_name());
        let mut entries = Vec::new();
        if graph.is_some() {
            entries.push((GraphFile::Graph.file_name().to_owned(), EntryKind::File));
        }
        if disk.markers.contains_key(dir) {
            entries.push((MARKER.to_owned(), EntryKind::Pending));
        }
        Ok(umx_core::scan::graph_in(".".into(), &entries, || {
            graph
                .filter(|b| b.len() as u64 <= umx_core::scan::FOUND_GRAPH_JSON_CAP)
                .cloned()
        }))
    }
}

/// fake.clock: settable; starts at `FAKE_EPOCH_MS`.
pub struct FakeClock(AtomicU64);

impl FakeClock {
    pub fn new(ms: u64) -> FakeClock {
        FakeClock(AtomicU64::new(ms))
    }

    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Default for FakeClock {
    fn default() -> FakeClock {
        FakeClock::new(FAKE_EPOCH_MS)
    }
}

impl Clock for FakeClock {
    fn adapter_id(&self) -> &'static str {
        "fake.clock"
    }

    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// fake.id-generator: sequential ids with `FAKE_EPOCH_MS` in the time bits, so they look like real ones.
pub struct FakeIds(AtomicU64);

impl FakeIds {
    pub fn new() -> FakeIds {
        FakeIds(AtomicU64::new(1))
    }
}

impl Default for FakeIds {
    fn default() -> FakeIds {
        FakeIds::new()
    }
}

impl IdGenerator for FakeIds {
    fn adapter_id(&self) -> &'static str {
        "fake.id-generator"
    }

    fn next_id(&self) -> Id {
        let n = self.0.fetch_add(1, Ordering::SeqCst);
        Id(((FAKE_EPOCH_MS as u128) << 80) | n as u128)
    }
}

/// fake.config-files: the machine file and repo files in memory. Repos are the folders given to [`Self::add_repo`],
/// inside the one root.
pub struct FakeConfigFiles {
    home: GraphDir,
    root: GraphDir,
    repos: Mutex<BTreeSet<GraphDir>>,
    files: Mutex<BTreeMap<ConfigFile, Vec<u8>>>,
    refused: Mutex<BTreeMap<ConfigFile, &'static str>>,
}

impl FakeConfigFiles {
    pub fn new(home: GraphDir, root: GraphDir) -> FakeConfigFiles {
        FakeConfigFiles {
            home,
            root,
            repos: Mutex::default(),
            files: Mutex::default(),
            refused: Mutex::default(),
        }
    }

    /// Makes `dir` a repository (as if it held `.git`).
    pub fn add_repo(&self, dir: GraphDir) {
        self.repos.lock().unwrap().insert(dir);
    }

    /// Puts `bytes` in `file` without the port's write rules: how a test gives the fake flowspace3's config,
    /// which the port itself never writes.
    pub fn seed(&self, file: ConfigFile, bytes: &[u8]) {
        self.files.lock().unwrap().insert(file, bytes.to_vec());
    }

    /// Makes `file` untrusted (as if it were a symlink or open to others) until set to None.
    pub fn refuse(&self, file: ConfigFile, reason: Option<&'static str>) {
        let mut refused = self.refused.lock().unwrap();
        match reason {
            Some(r) => refused.insert(file, r),
            None => refused.remove(&file),
        };
    }

    fn check(&self, file: &ConfigFile) -> Result<(), ConfigFilesError> {
        if let ConfigFile::Repo(dir) | ConfigFile::Graphfile(dir) = file
            && !dir.is_within(&self.root)
        {
            return Err(ConfigFilesError::OutsideRoots(dir.clone()));
        }
        if let ConfigFile::Graphfile(dir) = file
            && !self.repos.lock().unwrap().contains(dir)
        {
            return Err(ConfigFilesError::Refused {
                file: self.path(file),
                reason: config_files::REFUSED_NOT_REPO_TOP,
            });
        }
        let refused = self.refused.lock().unwrap().get(file).copied();
        match refused {
            Some(reason) => Err(ConfigFilesError::Refused {
                file: self.path(file),
                reason,
            }),
            None => Ok(()),
        }
    }
}

impl ConfigFiles for FakeConfigFiles {
    fn adapter_id(&self) -> &'static str {
        "fake.config-files"
    }

    fn path(&self, file: &ConfigFile) -> GraphDir {
        let path = match file {
            ConfigFile::Machine => self.home.join("config.toml"),
            ConfigFile::Flowspace3 => self.home.join(".config/flowspace3/config.toml"),
            ConfigFile::Repo(dir) => dir.join(".umx/config.toml"),
            ConfigFile::Graphfile(dir) => dir.join(umx_core::graphfile::GRAPHFILE),
        };
        path.expect("a config path")
    }

    fn read(&self, file: &ConfigFile) -> Result<Option<Vec<u8>>, ConfigFilesError> {
        self.check(file)?;
        let bytes = self.files.lock().unwrap().get(file).cloned();
        let (cap, reason) = if matches!(file, ConfigFile::Graphfile(_)) {
            (
                config_files::GRAPHFILE_CAP,
                config_files::REFUSED_GRAPHFILE_TOO_LARGE,
            )
        } else {
            (
                config_files::SMALL_FILE_CAP,
                config_files::REFUSED_TOO_LARGE,
            )
        };
        if bytes.as_ref().is_some_and(|b| b.len() as u64 > cap) {
            return Err(ConfigFilesError::Refused {
                file: self.path(file),
                reason,
            });
        }
        Ok(bytes)
    }

    fn swap(
        &self,
        file: &ConfigFile,
        expected: Option<&[u8]>,
        bytes: &[u8],
    ) -> Result<umx_core::ports::Swapped, ConfigFilesError> {
        self.check(file)?;
        if *file == ConfigFile::Flowspace3 {
            return Err(ConfigFilesError::Refused {
                file: self.path(file),
                reason: config_files::REFUSED_READ_ONLY,
            });
        }
        let mut files = self.files.lock().unwrap();
        if files.get(file).map(Vec::as_slice) != expected {
            return Ok(umx_core::ports::Swapped::Changed);
        }
        files.insert(file.clone(), bytes.to_vec());
        Ok(umx_core::ports::Swapped::Done)
    }

    fn write(&self, file: &ConfigFile, bytes: &[u8]) -> Result<(), ConfigFilesError> {
        self.check(file)?;
        if *file == ConfigFile::Flowspace3 {
            return Err(ConfigFilesError::Refused {
                file: self.path(file),
                reason: config_files::REFUSED_READ_ONLY,
            });
        }
        self.files
            .lock()
            .unwrap()
            .insert(file.clone(), bytes.to_vec());
        Ok(())
    }

    fn repo_root(&self, dir: &GraphDir) -> Result<Option<GraphDir>, ConfigFilesError> {
        let repos = self.repos.lock().unwrap();
        let mut at = Some(dir.clone());
        while let Some(d) = at {
            if !d.is_within(&self.root) {
                return Ok(None);
            }
            if repos.contains(&d) {
                return Ok(Some(d));
            }
            at = d
                .as_str()
                .rsplit_once('/')
                .and_then(|(parent, _)| {
                    GraphDir::parse(if parent.is_empty() { "/" } else { parent }).ok()
                })
                .filter(|p| *p != d);
        }
        Ok(None)
    }
}

/// fake.source-reader: folders, files and symlinks held in memory under one root. Every symlink target is an
/// absolute, already canonical path. `open` makes every folder under the root exist (stub mode, where nothing is on
/// disk).
pub struct FakeSourceReader {
    root: GraphDir,
    open: bool,
    folders: Mutex<BTreeSet<GraphDir>>,
    files: Mutex<BTreeSet<GraphDir>>,
    links: Mutex<BTreeMap<GraphDir, GraphDir>>,
    /// What `digest` reads: a file added without bytes reads as empty.
    contents: Mutex<BTreeMap<GraphDir, Vec<u8>>>,
    digests: AtomicUsize,
}

impl FakeSourceReader {
    pub fn new(root: GraphDir) -> FakeSourceReader {
        let folders = Mutex::new(BTreeSet::from([root.clone()]));
        FakeSourceReader {
            root,
            open: false,
            folders,
            files: Mutex::default(),
            links: Mutex::default(),
            contents: Mutex::default(),
            digests: AtomicUsize::new(0),
        }
    }

    /// Every path under the root is a folder that exists (stub mode), and `digest` reads any path it was not told
    /// about as an empty file.
    pub fn open(root: GraphDir) -> FakeSourceReader {
        FakeSourceReader {
            open: true,
            ..FakeSourceReader::new(root)
        }
    }

    /// Makes `dir` and every folder above it exist.
    pub fn add_folder(&self, dir: GraphDir) {
        let mut folders = self.folders.lock().unwrap();
        let mut at = Some(dir);
        while let Some(d) = at {
            at = parent(&d);
            folders.insert(d);
        }
    }

    pub fn add_file(&self, file: GraphDir) {
        if let Some(p) = parent(&file) {
            self.add_folder(p);
        }
        self.files.lock().unwrap().insert(file);
    }

    /// Adds `file` holding `bytes`, or replaces what it holds.
    pub fn write_file(&self, file: GraphDir, bytes: &[u8]) {
        self.contents
            .lock()
            .unwrap()
            .insert(file.clone(), bytes.to_vec());
        self.add_file(file);
    }

    pub fn remove_file(&self, file: &GraphDir) {
        self.contents.lock().unwrap().remove(file);
        self.files.lock().unwrap().remove(file);
    }

    /// How many files `digest` has read.
    pub fn digests(&self) -> usize {
        self.digests.load(Ordering::SeqCst)
    }

    pub fn add_link(&self, link: GraphDir, target: GraphDir) {
        if let Some(p) = parent(&link) {
            self.add_folder(p);
        }
        self.links.lock().unwrap().insert(link, target);
    }

    fn exists(&self, p: &GraphDir) -> bool {
        (self.open && p.is_within(&self.root))
            || self.folders.lock().unwrap().contains(p)
            || self.files.lock().unwrap().contains(p)
    }

    /// Follows every link along `path`; missing parts are kept as written.
    fn resolve(&self, path: &GraphDir) -> GraphDir {
        let links = self.links.lock().unwrap();
        let mut at = GraphDir::parse("/").expect("root");
        for part in path.as_str().split('/').filter(|p| !p.is_empty()) {
            at = at.join(part).expect("a part");
            let mut hops = 0;
            while let Some(t) = links.get(&at) {
                at = t.clone();
                hops += 1;
                if hops > 8 {
                    break;
                }
            }
        }
        at
    }
}

fn parent(d: &GraphDir) -> Option<GraphDir> {
    d.as_str()
        .rsplit_once('/')
        .and_then(|(p, _)| GraphDir::parse(if p.is_empty() { "/" } else { p }).ok())
        .filter(|p| p != d)
}

impl SourceReader for FakeSourceReader {
    fn adapter_id(&self) -> &'static str {
        "fake.source-reader"
    }

    fn canonical_root(&self, folder: &GraphDir) -> Result<GraphDir, SourceError> {
        let canonical = self.resolve(folder);
        if !canonical.is_within(&self.root) {
            return Err(SourceError::OutsideRoots);
        }
        let is_folder = (self.open && !self.files.lock().unwrap().contains(&canonical))
            || self.folders.lock().unwrap().contains(&canonical);
        if is_folder {
            Ok(canonical)
        } else {
            Err(SourceError::NotFolder)
        }
    }

    fn check_path(&self, root: &GraphDir, path: &str) -> Result<(), SourceError> {
        if path.starts_with('/') || path.split('/').any(|p| p == "..") {
            return Err(SourceError::OutsideRoots);
        }
        let mut at = root.clone();
        for part in path.split('/').filter(|p| !p.is_empty() && *p != ".") {
            at = at.join(part).map_err(|_| SourceError::OutsideRoots)?;
            let link = self.links.lock().unwrap().get(&at).cloned();
            match link {
                Some(target) => {
                    let target = self.resolve(&target);
                    if !self.exists(&target) || !target.is_within(root) {
                        return Err(SourceError::OutsideRoots);
                    }
                    at = target;
                }
                None if !self.exists(&at) => return Ok(()),
                None => {}
            }
        }
        Ok(())
    }

    fn digest(
        &self,
        root: &GraphDir,
        path: &str,
        transcript: Option<umx_core::source_hash::Transcript>,
        prefix: Option<u64>,
    ) -> Result<umx_core::source_hash::SourceDigest, SourceError> {
        self.check_path(root, path)?;
        let mut at = root.clone();
        for part in path.split('/').filter(|p| !p.is_empty() && *p != ".") {
            at = at.join(part).map_err(|_| SourceError::OutsideRoots)?;
        }
        let at = self.resolve(&at);
        if !at.is_within(root) {
            return Err(SourceError::OutsideRoots);
        }
        let bytes = self.contents.lock().unwrap().get(&at).cloned();
        let bytes = match bytes {
            Some(b) => b,
            None if self.files.lock().unwrap().contains(&at) => Vec::new(),
            None if self.folders.lock().unwrap().contains(&at) => {
                return Err(SourceError::NotFile);
            }
            None if self.open => Vec::new(),
            None => return Err(SourceError::Missing),
        };
        let max = umx_core::source_hash::SOURCE_MAX_BYTES;
        let prefix = prefix.filter(|n| *n <= max);
        if prefix.is_none() && bytes.len() as u64 > max {
            return Err(SourceError::TooLarge);
        }
        self.digests.fetch_add(1, Ordering::SeqCst);
        Ok(umx_core::source_hash::digest_read(
            &bytes, transcript, prefix,
        ))
    }
}

/// fake.secrets: an env map and an optional secrets file text, parsed like the real one.
pub struct FakeSecrets {
    env: BTreeMap<String, String>,
    file: Mutex<Option<String>>,
}

impl FakeSecrets {
    pub fn new(env: BTreeMap<String, String>) -> FakeSecrets {
        FakeSecrets {
            env,
            file: Mutex::new(None),
        }
    }

    pub fn write_file(&self, text: &str) {
        *self.file.lock().unwrap() = Some(text.to_owned());
    }
}

fn secret_line(line: &str) -> Option<(&str, &str)> {
    // A CRLF file or trailing spaces must not end up in the key.
    let (name, value) = line.trim_end().split_once('=')?;
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some('A'..='Z' | '_'))
        && name.len() <= 64
        && chars.all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '_'));
    ok.then_some((name, value))
}

impl Secrets for FakeSecrets {
    fn adapter_id(&self) -> &'static str {
        "fake.secrets"
    }

    fn lookup(&self, name: &str) -> Option<(SecretSource, Secret)> {
        if let Some(v) = self.env.get(name).filter(|v| !v.is_empty()) {
            return Some((SecretSource::Env, Secret::new(v.clone())));
        }
        let file = self.file.lock().unwrap();
        file.as_deref()
            .filter(|t| t.len() as u64 <= config_files::SMALL_FILE_CAP)?
            .lines()
            .filter_map(secret_line)
            .find(|(n, v)| *n == name && !v.is_empty())
            .map(|(_, v)| (SecretSource::SecretsFile, Secret::new(v.to_owned())))
    }

    fn issues(&self) -> Vec<SecretsIssue> {
        let file = GraphDir::parse("/fake-home/secrets.env").expect("path");
        let text = self.file.lock().unwrap();
        if text
            .as_deref()
            .is_some_and(|t| t.len() as u64 > config_files::SMALL_FILE_CAP)
        {
            return vec![SecretsIssue::Refused {
                file,
                reason: config_files::REFUSED_TOO_LARGE,
            }];
        }
        text.as_deref()
            .unwrap_or("")
            .lines()
            .enumerate()
            .filter(|(_, l)| {
                !l.trim().is_empty() && !l.starts_with('#') && secret_line(l).is_none()
            })
            .map(|(i, _)| SecretsIssue::BadLine {
                file: file.clone(),
                line: i as u64 + 1,
            })
            .collect()
    }
}

/// fake.local-state: the state file as bytes in memory, with switches for the failures the real one can have: an
/// untrusted file (refused), a failed write, and a write whose folder sync fails.
#[derive(Default)]
pub struct FakeLocalState {
    bytes: Mutex<Option<Vec<u8>>>,
    /// Whether the state folder exists: from the first write (or `set_bytes`) on, as with the real one.
    folder: AtomicBool,
    refused: Mutex<Option<&'static str>>,
    write_error: Mutex<Option<&'static str>>,
    not_synced: AtomicBool,
    writes: AtomicUsize,
    reads: AtomicUsize,
    absent_ok: bool,
}

impl FakeLocalState {
    pub fn new() -> FakeLocalState {
        FakeLocalState::default()
    }

    pub fn with_absent_ok(mut self, absent_ok: bool) -> Self {
        self.absent_ok = absent_ok;
        self
    }

    /// Puts bytes in the file, as a person or another umx would.
    pub fn set_bytes(&self, bytes: &[u8]) {
        self.folder.store(true, Ordering::Relaxed);
        *self.bytes.lock().unwrap() = Some(bytes.to_vec());
    }

    /// Deletes the file and leaves its folder.
    pub fn remove_file(&self) {
        *self.bytes.lock().unwrap() = None;
    }

    /// The file's bytes, whatever its trust.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        self.bytes.lock().unwrap().clone()
    }

    /// Makes the file untrusted (as a symlink or a file open to others) until set to None.
    pub fn refuse(&self, reason: Option<&'static str>) {
        *self.refused.lock().unwrap() = reason;
    }

    /// Makes writes fail with this OS error text until set to None; nothing is written.
    pub fn fail_writes(&self, error: Option<&'static str>) {
        *self.write_error.lock().unwrap() = error;
    }

    /// Writes land but report that the folder sync failed.
    pub fn set_not_synced(&self, on: bool) {
        self.not_synced.store(on, Ordering::Relaxed);
    }

    /// Writes that replaced the file.
    pub fn writes(&self) -> usize {
        self.writes.load(Ordering::Relaxed)
    }

    /// Reads of the file, refused ones included.
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }

    fn check(&self) -> Result<(), LocalStateError> {
        match *self.refused.lock().unwrap() {
            Some(reason) => Err(LocalStateError::Refused {
                file: self.path(),
                reason,
            }),
            None => Ok(()),
        }
    }
}

impl LocalState for FakeLocalState {
    fn adapter_id(&self) -> &'static str {
        "fake.local-state"
    }

    fn path(&self) -> GraphDir {
        GraphDir::parse("/fake-home/state/graph-policies.json").expect("a state path")
    }

    fn read(&self) -> Result<Option<Vec<u8>>, LocalStateError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.check()?;
        match self.bytes() {
            None if self.folder.load(Ordering::Relaxed) && !self.absent_ok => {
                Err(LocalStateError::Refused {
                    file: self.path(),
                    reason: umx_core::ports::local_state::REFUSED_MISSING,
                })
            }
            Some(b) if b.len() as u64 > config_files::SMALL_FILE_CAP => {
                Err(LocalStateError::Refused {
                    file: self.path(),
                    reason: config_files::REFUSED_TOO_LARGE,
                })
            }
            bytes => Ok(bytes),
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<StateWritten, LocalStateError> {
        self.check()?;
        if let Some(e) = *self.write_error.lock().unwrap() {
            return Err(LocalStateError::io(e));
        }
        self.set_bytes(bytes);
        self.writes.fetch_add(1, Ordering::Relaxed);
        Ok(if self.not_synced.load(Ordering::Relaxed) {
            StateWritten::NotSynced("sync state: fake".into())
        } else {
            StateWritten::Durable
        })
    }
}

/// fake.embedder: bag-of-words hashing. Each lower-cased word (a run of letters or digits) is hashed with the
/// model key into one of `dims` slots with a sign, plus one marker token for the text kind; the slots are summed
/// and L2-normalised. Same text, kind and key give the same vector. Texts that share words score a higher cosine,
/// across kinds too (a query finds documents), while a query and a document of the same text still differ, so
/// similarity tests can use it. Words are its tokens: inputs longer than `max_input_tokens` words are cut and reported truncated.
pub struct FakeEmbedder {
    key: ModelKey,
    max_input_tokens: usize,
    max_batch_inputs: usize,
    locality: Locality,
    offline: AtomicBool,
    calls: AtomicUsize,
    texts: AtomicUsize,
}

impl FakeEmbedder {
    pub fn new(key: ModelKey) -> FakeEmbedder {
        FakeEmbedder {
            key,
            max_input_tokens: 512,
            max_batch_inputs: 16,
            locality: Locality::Local,
            offline: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            texts: AtomicUsize::new(0),
        }
    }

    pub fn with_max_input_tokens(mut self, max: usize) -> FakeEmbedder {
        self.max_input_tokens = max;
        self
    }

    /// The most inputs one call takes; a longer call is refused, as a provider would.
    pub fn with_max_batch_inputs(mut self, max: usize) -> FakeEmbedder {
        self.max_batch_inputs = max.max(1);
        self
    }

    /// Pretends to send texts off the machine, for tests of what may use a hosted embedder. Default `Local`.
    pub fn with_locality(mut self, locality: Locality) -> FakeEmbedder {
        self.locality = locality;
        self
    }

    /// While offline, every non-empty batch fails with `Unavailable`, as a provider would with no network.
    pub fn set_offline(&self, offline: bool) {
        self.offline.store(offline, Ordering::SeqCst);
    }

    /// Calls that reached the provider (non-empty batches that passed input checks), so tests can prove that
    /// something embedded nothing.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Texts sent to the provider across all calls.
    pub fn texts_embedded(&self) -> usize {
        self.texts.load(Ordering::SeqCst)
    }

    fn vector(&self, text: &str, kind: TextKind) -> (Vec<f32>, bool) {
        let dims = self.key.dims();
        let key = self.key.to_string();
        let mut v = vec![0f32; dims];
        let mut words = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty());
        let mut add = |token: &[u8]| {
            let h = fnv1a(&[key.as_bytes(), token]);
            let sign = if h >> 63 == 0 { 1.0 } else { -1.0 };
            v[(h % dims as u64) as usize] += sign;
        };
        for word in words.by_ref().take(self.max_input_tokens) {
            add(word.to_lowercase().as_bytes());
        }
        let truncated = words.next().is_some();
        // A NUL never appears in a word, so the marker cannot collide with one.
        add(match kind {
            TextKind::Document => &b"\0document"[..],
            TextKind::Query => &b"\0query"[..],
        });
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            // Every word cancelled out, or the text had no word characters: a fixed unit vector for this text.
            v[(fnv1a(&[text.as_bytes()]) % dims as u64) as usize] = 1.0;
        } else {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        (v, truncated)
    }
}

fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.iter().chain(&[0xff]) {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

impl Embedder for FakeEmbedder {
    fn adapter_id(&self) -> &'static str {
        "fake.embedder"
    }

    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
        if texts.is_empty() {
            return Ok(Embedded::default());
        }
        if let Some(index) = texts.iter().position(|t| t.trim().is_empty()) {
            return Err(EmbedError::EmptyInput { index });
        }
        if self.offline.load(Ordering::SeqCst) {
            return Err(EmbedError::Unavailable {
                reason: "fake embedder is offline".to_string(),
                next: "switch the fake back online".to_string(),
            });
        }
        if texts.len() > self.max_batch_inputs {
            return Err(EmbedError::BadResponse(format!(
                "{} inputs in one call; the fake takes {}",
                texts.len(),
                self.max_batch_inputs
            )));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.texts.fetch_add(texts.len(), Ordering::SeqCst);
        let mut out = Embedded::default();
        for (i, text) in texts.iter().enumerate() {
            let (v, truncated) = self.vector(text, kind);
            out.vectors.push(v);
            if truncated {
                out.truncated.push(i);
            }
        }
        Ok(out)
    }

    fn key(&self) -> ModelKey {
        self.key.clone()
    }

    fn concurrency_ceiling(&self) -> usize {
        4
    }

    fn max_batch_inputs(&self) -> usize {
        self.max_batch_inputs
    }

    fn max_input_tokens(&self) -> usize {
        self.max_input_tokens
    }

    fn locality(&self) -> Locality {
        self.locality
    }

    /// Unavailable while offline, else Ok; never counts as a call and embeds nothing.
    fn ready(&self) -> Result<(), EmbedError> {
        if self.offline.load(Ordering::SeqCst) {
            return Err(EmbedError::Unavailable {
                reason: "fake embedder is offline".to_string(),
                next: "switch the fake back online".to_string(),
            });
        }
        Ok(())
    }

    /// The fake signs in to nothing; posing as hosted, it names a stand-in, as a hosted adapter must (em_c12).
    fn credential(&self) -> Option<umx_core::ports::Credential> {
        (self.locality == Locality::Hosted).then(|| umx_core::ports::Credential {
            kind: "fake",
            from: "fake.embedder".to_string(),
        })
    }
}

/// fake.service-manager: one service slot in memory. Installed means loaded and running as pid 1.
#[derive(Default)]
pub struct FakeServiceManager {
    installed: Mutex<Option<ServiceSpec>>,
    restarts: AtomicUsize,
}

impl FakeServiceManager {
    pub fn new() -> FakeServiceManager {
        FakeServiceManager::default()
    }

    /// How many times `restart` ran, so tests can prove something restarted (or did not).
    pub fn restarts(&self) -> usize {
        self.restarts.load(Ordering::SeqCst)
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<ServiceSpec>> {
        self.installed.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl ServiceManager for FakeServiceManager {
    fn adapter_id(&self) -> &'static str {
        "fake.service-manager"
    }

    fn status(&self) -> Result<ServiceState, ServiceError> {
        Ok(match self.slot().clone() {
            None => ServiceState::NotInstalled,
            Some(spec) => ServiceState::Installed {
                spec: Some(spec),
                loaded: true,
                pid: Some(1),
            },
        })
    }

    fn install(&self, spec: &ServiceSpec) -> Result<(), ServiceError> {
        let dirs = spec.path.iter().flat_map(|p| p.split(':'));
        for path in [&spec.program, &spec.home, &spec.log]
            .into_iter()
            .map(String::as_str)
            .chain(dirs)
        {
            if !unit_safe(path) {
                return Err(ServiceError::BadPath(path.to_owned()));
            }
        }
        let mut slot = self.slot();
        match slot.as_ref() {
            Some(found) if found.program != spec.program || found.home != spec.home => {
                Err(ServiceError::Conflict {
                    found: Some(found.clone()),
                })
            }
            _ => {
                *slot = Some(spec.clone());
                Ok(())
            }
        }
    }

    fn uninstall(&self) -> Result<(), ServiceError> {
        *self.slot() = None;
        Ok(())
    }

    fn restart(&self) -> Result<(), ServiceError> {
        if self.slot().is_none() {
            return Err(ServiceError::Failed(
                "no umx service is installed".to_string(),
            ));
        }
        self.restarts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// fake.ui-host: no process and no browser. A launch's `node` picks how its server behaves: a path ending in
/// `-silent` never gets ready, `-dies` exits 3 first, `-ends` stops by itself; any other path serves on its port (or
/// 4747). Codes differ on every call. [`FakeUiHost::watch`] sees what the last server read on stdin, how many
/// servers started, the URLs opened and whether the last server still runs; dropping the host stops its servers.
/// ws009 M1b2: `detach` runs nothing. It answers what [`FakeUiWatch::detach_says`] set, else by the exe's name as
/// `serve` does (`-silent` times out, `-dies` exits 1, otherwise it says its last argument before the report flag).
/// Processes are what [`FakeUiWatch::add_process`] added; `signal` ends one. `launch_code` arms a code on a live
/// server unless [`FakeUiWatch::tab`] says a tab is connected.
#[derive(Default)]
pub struct FakeUiHost(Arc<Mutex<FakeUi>>);

/// What a [`FakeUiHost`] did, readable after the host is gone.
#[derive(Clone, Default)]
pub struct FakeUiWatch(Arc<Mutex<FakeUi>>);

#[derive(Default)]
struct FakeUi {
    codes_made: u64,
    /// The lines the last server read on its stdin.
    stdin: Vec<String>,
    /// id to (alive, ends by itself).
    servers: BTreeMap<u64, (bool, bool)>,
    last: Option<u64>,
    opened: Vec<String>,
    detach_says: Option<Vec<u8>>,
    detached: Vec<Vec<String>>,
    reported: Vec<(String, Vec<u8>)>,
    processes: BTreeMap<u32, UiProcess>,
    signalled: Vec<u32>,
    tab: bool,
    codes: Vec<String>,
    held_elsewhere: bool,
    endings: Vec<UiEnd>,
}

fn fake_ui(state: &Mutex<FakeUi>) -> std::sync::MutexGuard<'_, FakeUi> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeUiWatch {
    /// The lines the last server read on its stdin.
    pub fn stdin_seen(&self) -> Vec<String> {
        fake_ui(&self.0).stdin.clone()
    }

    /// How many servers were started.
    pub fn served(&self) -> usize {
        fake_ui(&self.0).servers.len()
    }

    /// Whether the last server still runs.
    pub fn alive(&self) -> bool {
        let state = fake_ui(&self.0);
        state
            .last
            .and_then(|id| state.servers.get(&id))
            .is_some_and(|(alive, _)| *alive)
    }

    /// The URLs handed to the browser, in order.
    pub fn opened(&self) -> Vec<String> {
        fake_ui(&self.0).opened.clone()
    }

    /// What every later `detach` reports (an envelope's JSON), whatever it runs; None goes back to the exe's name.
    pub fn detach_says(&self, said: Option<Vec<u8>>) {
        fake_ui(&self.0).detach_says = said;
    }

    /// Each detached child's exe and arguments, the report flag and its path included.
    pub fn detached(&self) -> Vec<Vec<String>> {
        fake_ui(&self.0).detached.clone()
    }

    /// Each `report`: where to, and the line.
    pub fn reported(&self) -> Vec<(String, Vec<u8>)> {
        fake_ui(&self.0).reported.clone()
    }

    /// A process `process` and `signal` know.
    pub fn add_process(&self, pid: u32, process: UiProcess) {
        fake_ui(&self.0).processes.insert(pid, process);
    }

    /// Whether `process` still knows `pid`.
    pub fn has_process(&self, pid: u32) -> bool {
        fake_ui(&self.0).processes.contains_key(&pid)
    }

    /// The pids signalled, in order.
    pub fn signalled(&self) -> Vec<u32> {
        fake_ui(&self.0).signalled.clone()
    }

    /// What each `ending` said, in order.
    pub fn endings(&self) -> Vec<UiEnd> {
        fake_ui(&self.0).endings.clone()
    }

    /// Whether another process holds this UMX_HOME's view, so `own_view` is false.
    pub fn held_elsewhere(&self, held: bool) {
        fake_ui(&self.0).held_elsewhere = held;
    }

    /// Whether a tab of the servers counts as connected.
    pub fn tab(&self, live: bool) {
        fake_ui(&self.0).tab = live;
    }

    /// The launch codes armed, in order.
    pub fn codes(&self) -> Vec<String> {
        fake_ui(&self.0).codes.clone()
    }
}

impl FakeUiHost {
    pub fn new() -> FakeUiHost {
        FakeUiHost::default()
    }

    pub fn watch(&self) -> FakeUiWatch {
        FakeUiWatch(self.0.clone())
    }

    pub fn served(&self) -> usize {
        self.watch().served()
    }

    pub fn alive(&self) -> bool {
        self.watch().alive()
    }

    pub fn opened(&self) -> Vec<String> {
        self.watch().opened()
    }
}

impl Drop for FakeUiHost {
    fn drop(&mut self) {
        for (alive, _) in fake_ui(&self.0).servers.values_mut() {
            *alive = false;
        }
    }
}

impl UiHost for FakeUiHost {
    fn adapter_id(&self) -> &'static str {
        "fake.ui-host"
    }

    fn code(&self) -> Result<String, UiError> {
        let mut state = fake_ui(&self.0);
        state.codes_made += 1;
        let mut bytes = [0u8; ui_host::CODE_BYTES];
        bytes[..8].copy_from_slice(&state.codes_made.to_be_bytes());
        Ok(ui_host::base64url(&bytes))
    }

    fn serve(&self, launch: &UiLaunch) -> Result<UiServed, UiError> {
        if !launch.node.starts_with('/') {
            return Err(UiError::Start(format!("{} not found", launch.node)));
        }
        let mut state = fake_ui(&self.0);
        state.stdin.clear();
        let id = state.servers.len() as u64 + 1;
        state.last = Some(id);
        if launch.node.ends_with("-silent") || launch.node.ends_with("-dies") {
            state.servers.insert(id, (false, false));
            return Err(if launch.node.ends_with("-silent") {
                UiError::TimedOut
            } else {
                UiError::Exited(Some(3))
            });
        }
        state
            .servers
            .insert(id, (true, launch.node.ends_with("-ends")));
        Ok(UiServed {
            id,
            port: launch.port.unwrap_or(4747),
        })
    }

    fn running(&self, server: &UiServed) -> bool {
        let mut state = fake_ui(&self.0);
        match state.servers.get_mut(&server.id) {
            Some((alive, ends)) => {
                if *ends {
                    *alive = false;
                }
                *alive
            }
            None => false,
        }
    }

    fn stop(&self, server: &UiServed) {
        if let Some((alive, _)) = fake_ui(&self.0).servers.get_mut(&server.id) {
            *alive = false;
        }
    }

    fn open(&self, url: &str) -> Result<(), UiError> {
        if !ui_host::url_ok(url) {
            return Err(UiError::Open("not a loopback URL umx may open".to_string()));
        }
        fake_ui(&self.0).opened.push(url.to_string());
        Ok(())
    }

    fn detach(&self, exe: &str, args: &[String], _ready_ms: u64) -> Result<UiDetached, UiError> {
        let mut state = fake_ui(&self.0);
        let mut argv = Vec::from([exe.to_string()]);
        argv.extend(args.iter().cloned());
        argv.push(ui_host::REPORT_FLAG.to_string());
        argv.push("/fake/home/run/ui-report.sock".to_string());
        state.detached.push(argv);
        let pid = 40_000 + state.detached.len() as u32;
        if let Some(said) = state.detach_says.clone() {
            return Ok(UiDetached { pid, said });
        }
        if exe.ends_with("-silent") {
            return Err(UiError::TimedOut);
        }
        if exe.ends_with("-dies") {
            return Err(UiError::Exited(Some(1)));
        }
        let said = args.last().cloned().unwrap_or_default().into_bytes();
        Ok(UiDetached { pid, said })
    }

    fn report(&self, to: &str, line: &[u8]) -> Result<(), UiError> {
        fake_ui(&self.0)
            .reported
            .push((to.to_string(), line.to_vec()));
        Ok(())
    }

    fn process(&self, pid: u32) -> Option<UiProcess> {
        fake_ui(&self.0).processes.get(&pid).cloned()
    }

    fn signal(&self, pid: u32) -> Result<(), UiError> {
        let mut state = fake_ui(&self.0);
        if state.processes.remove(&pid).is_none() {
            return Err(UiError::Start(format!("no process {pid}")));
        }
        state.signalled.push(pid);
        Ok(())
    }

    fn own_view(&self) -> Result<bool, UiError> {
        Ok(!fake_ui(&self.0).held_elsewhere)
    }

    fn ending(&self, server: &UiServed, why: UiEnd) -> Result<(), UiError> {
        let mut state = fake_ui(&self.0);
        if !state
            .servers
            .get(&server.id)
            .is_some_and(|(alive, _)| *alive)
        {
            return Err(UiError::Exited(None));
        }
        state.endings.push(why);
        Ok(())
    }

    fn launch_code(&self, server: &UiServed, code: &str) -> Result<UiCodeUse, UiError> {
        let mut state = fake_ui(&self.0);
        if !state
            .servers
            .get(&server.id)
            .is_some_and(|(alive, _)| *alive)
        {
            return Err(UiError::Exited(None));
        }
        state.stdin.push(format!("code {code}"));
        if state.tab {
            return Ok(UiCodeUse::Tab);
        }
        state.codes.push(code.to_string());
        Ok(UiCodeUse::Armed)
    }
}

/// What the fake model fetcher serves for one repo.
#[derive(Clone, Debug)]
pub enum Served {
    Bytes(Vec<u8>),
    /// A redirect to a host that is not allowed.
    RedirectOff(String),
}

/// fake.model-fetcher: serves files from memory by repo and keeps them by the port's rules on a real folder (names
/// checked first, folders 0700, a present regular file with the pinned sha256 kept, others written to a temp name,
/// checked for size and sha256, then renamed 0600). Counts downloads.
#[derive(Default)]
pub struct FakeModelFetcher {
    served: Mutex<BTreeMap<String, Served>>,
    downloads: AtomicUsize,
}

impl FakeModelFetcher {
    pub fn new() -> FakeModelFetcher {
        FakeModelFetcher::default()
    }

    /// Serves `served` for every file of `repo`.
    pub fn serve(&self, repo: &str, served: Served) {
        self.served.lock().unwrap().insert(repo.to_string(), served);
    }

    /// Files downloaded so far (present files are not).
    pub fn downloads(&self) -> usize {
        self.downloads.load(Ordering::SeqCst)
    }
}

impl ModelFetcher for FakeModelFetcher {
    fn fetch(
        &self,
        source: &ModelSource,
        models: &str,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<Vec<FileFetched>, FetchError> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        use std::path::Path;
        for f in &source.files {
            if !model_fetcher::safe_file_name(&f.path) {
                return Err(FetchError::Path {
                    file: f.path.clone(),
                    reason: "not one plain file name".into(),
                });
            }
        }
        let folder = source.folder(models);
        let write = |file: &str, e: std::io::Error| FetchError::Write {
            file: file.to_string(),
            reason: e.kind().to_string(),
        };
        std::fs::create_dir_all(&folder).map_err(|e| write(&folder, e))?;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| write(&folder, e))?;
        let mut out = Vec::new();
        for f in &source.files {
            let target = Path::new(&folder).join(&f.path);
            let present = std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_file())
                && std::fs::read(&target)
                    .is_ok_and(|b| umx_core::models::sha256_hex(&b) == f.sha256);
            if present {
                out.push(FileFetched {
                    file: f.path.clone(),
                    size: f.size,
                    downloaded: false,
                });
                continue;
            }
            let bytes = match self.served.lock().unwrap().get(&source.repo).cloned() {
                Some(Served::Bytes(b)) => b,
                Some(Served::RedirectOff(host)) => return Err(FetchError::Redirect { host }),
                None => {
                    return Err(FetchError::Status {
                        host: "fake".into(),
                        status: 404,
                    });
                }
            };
            if bytes.len() as u64 != f.size {
                return Err(FetchError::Size {
                    file: f.path.clone(),
                    expected: f.size,
                    got: Some(bytes.len() as u64),
                });
            }
            let got = umx_core::models::sha256_hex(&bytes);
            if got != f.sha256 {
                return Err(FetchError::Hash {
                    file: f.path.clone(),
                    expected: f.sha256.clone(),
                    got,
                });
            }
            let temp = Path::new(&folder).join(format!(".{}.part", f.path));
            let _ = std::fs::remove_file(&temp);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)
                .map_err(|e| write(&f.path, e))?;
            std::io::Write::write_all(&mut file, &bytes).map_err(|e| write(&f.path, e))?;
            std::fs::rename(&temp, &target).map_err(|e| write(&f.path, e))?;
            self.downloads.fetch_add(1, Ordering::SeqCst);
            progress(FetchProgress {
                file: f.path.clone(),
                done: f.size,
                total: f.size,
            });
            out.push(FileFetched {
                file: f.path.clone(),
                size: f.size,
                downloaded: true,
            });
        }
        Ok(out)
    }

    fn adapter_id(&self) -> &'static str {
        "fake.model-fetcher"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use umx_core::setup::GraphFilesSetup;

    /// A fake model fetcher serving the contract's cases over a fresh temp folder.
    struct FakeFetchWorld {
        port: FakeModelFetcher,
        models: String,
    }

    impl FakeFetchWorld {
        fn new() -> FakeFetchWorld {
            use umx_contracts::model_fetcher::Case;
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "umx-fake-fetch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let port = FakeModelFetcher::new();
            for case in [Case::Good, Case::Mismatch, Case::Oversize] {
                port.serve(case.repo(), Served::Bytes(case.bytes()));
            }
            port.serve(
                Case::OffList.repo(),
                Served::RedirectOff("elsewhere.example".into()),
            );
            FakeFetchWorld {
                port,
                models: dir.display().to_string(),
            }
        }
    }

    impl Drop for FakeFetchWorld {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.models);
        }
    }

    impl umx_contracts::model_fetcher::FetchWorld for FakeFetchWorld {
        type Port = FakeModelFetcher;
        fn port(&self) -> &FakeModelFetcher {
            &self.port
        }
        fn models(&self) -> &str {
            &self.models
        }
    }

    umx_contracts::model_fetcher_contract!(
        FakeFetchWorld::new;
        mf_c1_fetch_keeps_each_verified_file_0600_in_a_0700_folder(),
        mf_c2_a_present_verified_file_is_not_downloaded_again(),
        mf_c3_a_hash_mismatch_leaves_nothing(),
        mf_c4_an_oversize_body_is_refused_unwritten(),
        mf_c5_a_redirect_off_the_allow_list_is_refused(),
        mf_c6_a_damaged_file_is_replaced(),
        mf_c7_an_unsafe_file_name_is_refused_before_any_path(),
        mf_c8_a_symlink_at_the_final_name_is_never_trusted(),
    );

    struct FakeUiStage(FakeUiWatch);

    impl umx_contracts::ui_host::UiStage for FakeUiStage {
        fn launch(&self, server: umx_contracts::ui_host::Server, port: Option<u16>) -> UiLaunch {
            use umx_contracts::ui_host::Server;
            let node = match server {
                Server::Ready => "/fake/node",
                Server::Silent => "/fake/node-silent",
                Server::Dies => "/fake/node-dies",
                Server::Ends => "/fake/node-ends",
            };
            UiLaunch {
                node: node.to_string(),
                app_dir: "/fake/apps/ui".to_string(),
                port,
                env: vec![("UMX_HOME".to_string(), "/fake/home".to_string())],
                ready_ms: 1_000,
            }
        }
        fn stdin_seen(&self) -> Vec<String> {
            self.0.stdin_seen()
        }
        fn argv_and_env_seen(&self) -> String {
            "/fake/node /fake/apps/ui/bin/serve.mjs --stdin UMX_HOME=/fake/home".to_string()
        }
        fn alive(&self) -> bool {
            self.0.alive()
        }
        fn opened(&self) -> Vec<String> {
            self.0.opened()
        }
        fn tab(&self, live: bool) {
            self.0.tab(live);
        }
        fn codes_seen(&self) -> Vec<String> {
            self.0.codes()
        }
        fn child(&self, child: umx_contracts::ui_host::Child) -> (String, Vec<String>) {
            use umx_contracts::ui_host::Child;
            match child {
                Child::Reports(line) => ("/fake/umx".to_string(), vec![line.to_string()]),
                Child::Exits => ("/fake/umx-dies".to_string(), Vec::new()),
                Child::Silent => ("/fake/umx-silent".to_string(), Vec::new()),
            }
        }
        fn live_process(&self) -> u32 {
            self.0.add_process(
                4321,
                UiProcess {
                    started: "Thu Nov 12 20:00:00 2026".to_string(),
                    argv: "/fake/umx ui".to_string(),
                },
            );
            4321
        }
        fn ended(&self, pid: u32) -> bool {
            !self.0.has_process(pid)
        }
        fn other_owner(&self, holds: bool) {
            self.0.held_elsewhere(holds);
        }
        fn ending_seen(&self) -> Option<String> {
            self.0.endings().last().map(|w| w.as_str().to_string())
        }
    }

    fn fake_ui_host() -> (FakeUiHost, FakeUiStage) {
        let host = FakeUiHost::new();
        let watch = host.watch();
        (host, FakeUiStage(watch))
    }

    umx_contracts::ui_host_contract!(
        fake_ui_host;
        uh_c1_codes_are_base64url_and_never_repeat(),
        uh_c2_the_server_gets_no_secret_and_the_port_is_reported(),
        uh_c3_stop_or_dropping_the_host_leaves_no_process(),
        uh_c4_a_server_never_ready_times_out_and_leaves_no_process(),
        uh_c5_a_server_that_dies_before_ready_is_an_exit_and_leaves_no_process(),
        uh_c6_a_server_that_ends_is_seen_as_not_running(),
        uh_c7_open_hands_over_only_a_loopback_url(),
        uh_c8_a_launch_code_reaches_stdin_only_and_is_dropped_while_a_tab_is_connected(),
        uh_c9_a_process_is_reported_until_it_is_signalled(),
        uh_c10_a_detached_child_reports_one_line_or_its_end_is_an_error(),
        uh_c11_the_view_is_owned_by_one_host_at_a_time(),
        uh_c12_why_the_view_ends_reaches_the_server(),
    );

    umx_contracts::service_manager_contract!(
        FakeServiceManager::new;
        sm_c1_fresh_is_not_installed(),
        sm_c2_install_is_read_back_and_loaded(),
        sm_c3_installing_the_same_spec_again_is_an_upgrade(),
        sm_c4_another_program_or_home_is_a_conflict_and_changes_nothing(),
        sm_c5_uninstall_leaves_nothing_and_can_repeat(),
        sm_c6_an_unsafe_path_is_refused_and_changes_nothing(),
        sm_c7_restart_needs_an_installed_service(),
    );

    impl umx_contracts::graph_files::SetHarness for FakeGraphFiles {
        fn crash_at(&self, point: Option<CrashPoint>) {
            self.crash_at_for_tests(point);
        }
        fn fail_temp_of(&self, file: Option<GraphFile>) {
            self.fail_writes_to(file);
        }
        fn restart(&self) -> FakeGraphFiles {
            self.restart_for_tests()
        }
        fn tamper(&self, dir: &GraphDir, file: impl Into<FileRef>, bytes: &[u8]) {
            self.tamper_for_tests(dir, file, bytes);
        }
        fn raw(&self, dir: &GraphDir, file: impl Into<FileRef>) -> Option<Vec<u8>> {
            self.raw_for_tests(dir, file)
        }
        fn leftovers(&self, dir: &GraphDir) -> Vec<String> {
            self.leftovers_for_tests(dir)
        }
        fn bytes_read(&self) -> u64 {
            self.bytes_read_for_tests()
        }
        fn touch(&self, dir: &GraphDir, file: GraphFile) {
            self.touch_for_tests(dir, file);
        }
        /// A map has no links: the swap is a replace with the same bytes, which moves the generation as a link would.
        fn swap_for_link(&self, dir: &GraphDir, file: GraphFile) {
            let bytes = self.raw_for_tests(dir, file).unwrap();
            self.tamper_for_tests(dir, file, &bytes);
        }
        fn change_during_next_read(&self, dir: &GraphDir, file: GraphFile, bytes: &[u8]) {
            self.change_during_next_read_for_tests(dir, file, bytes);
        }
        fn change_after_next_marker(&self, dir: &GraphDir, file: impl Into<FileRef>, bytes: &[u8]) {
            self.change_after_next_marker_for_tests(dir, dir, file, bytes);
        }
        fn change_before_next_marker(
            &self,
            dir: &GraphDir,
            file: impl Into<FileRef>,
            bytes: &[u8],
        ) {
            self.change_before_next_marker_for_tests(dir, file, bytes);
        }
    }

    fn graph_files() -> (FakeGraphFiles, GraphDir, ()) {
        let root = GraphDir::parse("/fake-root").unwrap();
        (FakeGraphFiles::new(root.clone()), root, ())
    }

    impl umx_contracts::graph_files::FindHarness for FakeGraphFiles {
        fn make_graph(&self, dir: &GraphDir, bytes: &[u8]) {
            self.tamper_for_tests(dir, GraphFile::Graph, bytes);
        }
        fn make_folder(&self, dir: &GraphDir) {
            self.add_folder_for_tests(dir.clone());
        }
        fn make_repo(&self, dir: &GraphDir) {
            self.add_repo_for_tests(dir.clone());
        }
        fn make_link(&self, link: &GraphDir, _target: &GraphDir) {
            self.add_link_for_tests(link.clone());
        }
        fn make_pending(&self, dir: &GraphDir) {
            self.cut_short_for_tests(dir);
        }
        fn leftovers(&self, dir: &GraphDir) -> Vec<String> {
            self.leftovers_for_tests(dir)
        }
        fn make_unlistable(&self, dir: &GraphDir, on: bool) {
            self.unlistable_for_tests(dir.clone(), on);
        }
        fn cross_device_next_relocate(&self) {
            self.cross_device_next_relocate_for_tests();
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
    umx_contracts::clock_contract!(
        FakeClock::default;
        ck_c1_now_is_a_plausible_epoch_ms(),
        ck_c2_now_never_goes_backwards(),
    );
    umx_contracts::id_generator_contract!(
        FakeIds::new;
        id_c1_ids_are_unique(),
        id_c2_each_id_is_greater_than_the_last(),
        id_c3_threads_share_one_sequence(),
    );

    fn config_files() -> (FakeConfigFiles, GraphDir, ()) {
        let root = GraphDir::parse("/fake-root").unwrap();
        let port = FakeConfigFiles::new(GraphDir::parse("/fake-home").unwrap(), root.clone());
        port.add_repo(root.join("repo").unwrap());
        (port, root, ())
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

    fn flowspace3(fs3: umx_contracts::config_files::Fs3) -> (FakeConfigFiles, ()) {
        use umx_contracts::config_files::Fs3;
        let (port, _root, guard) = config_files();
        match fs3 {
            Fs3::Missing => {}
            Fs3::File(bytes) => port.seed(ConfigFile::Flowspace3, bytes),
            Fs3::Symlink(bytes) => {
                port.seed(ConfigFile::Flowspace3, bytes);
                port.refuse(ConfigFile::Flowspace3, Some(config_files::REFUSED_SYMLINK));
            }
            Fs3::Mode(bytes, _) => {
                port.seed(ConfigFile::Flowspace3, bytes);
                port.refuse(ConfigFile::Flowspace3, Some(config_files::REFUSED_WRITABLE));
            }
        }
        (port, guard)
    }

    fn graphfile(gf: umx_contracts::config_files::Gf) -> (FakeConfigFiles, GraphDir, ()) {
        use umx_contracts::config_files::Gf;
        let (port, root, guard) = config_files();
        let file = ConfigFile::Graphfile(root.join("repo").unwrap());
        if let Gf::Symlink(bytes) = gf {
            port.seed(file.clone(), bytes);
            port.refuse(file, Some(config_files::REFUSED_SYMLINK));
        }
        (port, root, guard)
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

    umx_contracts::config_files_flowspace3_contract!(
        flowspace3;
        cf_f1_flowspace3_reads_and_is_never_written(),
        cf_f2_a_symlinked_flowspace3_file_is_refused(),
        cf_f3_flowspace3_path_names_its_config_toml(),
        cf_f4_a_flowspace3_file_others_can_write_is_refused(),
    );

    fn source_reader() -> (FakeSourceReader, GraphDir, GraphDir, ()) {
        let root = GraphDir::parse("/fake-root").unwrap();
        let outside = GraphDir::parse("/fake-outside").unwrap();
        let at = |r: &GraphDir, p: &str| r.join(p).unwrap();
        let port = FakeSourceReader::new(root.clone());
        port.add_folder(outside.clone());
        port.add_folder(at(&root, "src/sub"));
        port.add_file(at(&root, "src/a.md"));
        port.add_file(at(&root, "file.txt"));
        port.write_file(
            at(&root, "src/t.ndjson"),
            umx_contracts::source_reader::TRANSCRIPT.as_bytes(),
        );
        port.add_link(at(&root, "src/link_in"), at(&root, "src/sub"));
        port.add_link(at(&root, "src/link_out"), outside.clone());
        port.add_link(at(&root, "src/dangling"), at(&outside, "missing"));
        port.add_link(at(&root, "link_root"), outside.clone());
        (port, root, outside, ())
    }

    umx_contracts::source_reader_contract!(
        source_reader;
        sr_c1_a_root_inside_is_canonical(),
        sr_c2_a_root_outside_the_roots_is_refused(),
        sr_c3_a_root_that_is_a_file_or_missing_is_not_a_folder(),
        sr_c4_dotdot_and_absolute_paths_are_refused(),
        sr_c5_a_symlink_out_is_refused(),
        sr_c6_a_missing_tail_is_fine(),
        sr_c7_a_file_is_digested(),
        sr_c8_a_digest_reads_nothing_it_may_not(),
    );

    fn secrets() -> (FakeSecrets, ()) {
        let env = [("FROM_ENV".to_owned(), "env-value".to_owned())].into();
        (FakeSecrets::new(env), ())
    }

    umx_contracts::secrets_contract!(
        secrets, |p: &FakeSecrets, text: &str| p.write_file(text);
        se_c1_env_beats_the_file(),
        se_c2_missing_is_none(),
        se_c3_bad_lines_are_named_by_number_only(),
        se_c4_secrets_never_format(),
        se_c5_line_ends_are_not_part_of_the_value(),
        se_c6_an_oversized_file_is_ignored_and_reported(),
    );

    fn local_state() -> (FakeLocalState, ()) {
        (FakeLocalState::new(), ())
    }

    umx_contracts::local_state_contract!(
        local_state, |p: &FakeLocalState| p.refuse(Some("spoiled")), FakeLocalState::remove_file;
        ls_c1_write_then_read_is_identical(),
        ls_c2_read_missing_is_none(),
        ls_c3_an_untrusted_file_is_neither_read_nor_written(),
        ls_c4_path_names_the_state_file(),
        ls_c5_a_removed_file_is_refused_not_empty(),
        ls_c6_an_oversized_file_is_refused(),
    );

    fn embedder(reach: umx_contracts::embedder::Reach) -> FakeEmbedder {
        let fake =
            FakeEmbedder::new(ModelKey::new("fake-bow", 64).unwrap()).with_max_input_tokens(16);
        fake.set_offline(reach == umx_contracts::embedder::Reach::Offline);
        fake
    }

    umx_contracts::embedder_contract!(
        embedder;
        em_c1_one_vector_per_input_in_order(),
        em_c2_width_is_key_dims(),
        em_c3_vectors_are_unit_length(),
        em_c4_same_text_and_kind_give_the_same_vector(),
        em_c5_empty_batch_is_empty_even_offline(),
        em_c6_distinct_texts_give_distinct_vectors(),
        em_c7_identical_texts_in_one_batch_match(),
        em_c8_blank_input_is_refused_by_index(),
        em_c9_offline_is_unavailable(),
        em_c10_over_cap_input_is_refused_or_reported(),
        em_c11_ready_is_ok_online(),
    em_c12_a_hosted_adapter_names_its_credential_after_ready(),
    em_c13_reported_tokens_are_none_or_count_the_batch(),
    em_c14_a_batch_of_max_batch_inputs_embeds(),
    );

    /// ws006 S23 (review R26): under the root `/`, a folder's parent may be `/` itself, as the stub daemon's is.
    #[test]
    fn a_folder_relocates_to_a_top_level_folder_under_the_root() {
        let files = FakeGraphFiles::new(GraphDir::parse("/").unwrap());
        let (a, b) = (
            GraphDir::parse("/a").unwrap(),
            GraphDir::parse("/b").unwrap(),
        );
        files.write(&a, GraphFile::Graph, b"{}").unwrap();
        files.relocate(&a, &b).unwrap();
        assert_eq!(
            files.read(&b, GraphFile::Graph).unwrap().as_deref(),
            Some(&b"{}"[..])
        );
    }

    #[test]
    fn ready_sends_no_text() {
        for reach in [
            umx_contracts::embedder::Reach::Online,
            umx_contracts::embedder::Reach::Offline,
        ] {
            let fake = embedder(reach);
            umx_contracts::embedder::ready_sends_no_text(&fake, || fake.texts_embedded());
            assert_eq!(fake.calls(), 0);
        }
    }

    fn one(fake: &FakeEmbedder, text: &str, kind: TextKind) -> Vec<f32> {
        fake.embed(&[text.to_string()], kind)
            .unwrap()
            .vectors
            .remove(0)
    }

    #[test]
    fn shared_words_rank_higher() {
        use umx_contracts::embedder::cosine;
        let fake = embedder(umx_contracts::embedder::Reach::Online);
        let q = one(&fake, "graph storage layout", TextKind::Query);
        for kind in [TextKind::Query, TextKind::Document] {
            let near = one(&fake, "The graph storage layout on disk", kind);
            let far = one(&fake, "Quarterly revenue grew", kind);
            assert!(cosine(&q, &near) > cosine(&q, &far) + 0.3, "{kind:?}");
        }
        assert_ne!(
            one(&fake, "graph", TextKind::Query),
            one(&fake, "graph", TextKind::Document)
        );
    }

    #[test]
    fn long_input_is_cut_and_reported() {
        let fake =
            FakeEmbedder::new(ModelKey::new("fake-bow", 64).unwrap()).with_max_input_tokens(3);
        let out = fake
            .embed(
                &[
                    "one two three".to_string(),
                    "one two three four".to_string(),
                ],
                TextKind::Document,
            )
            .unwrap();
        assert_eq!(out.truncated, vec![1]);
        assert_eq!(
            out.vectors[0], out.vectors[1],
            "only the first 3 words count"
        );
    }

    /// Negative control for em_c1: correct for Document, reversed for Query.
    struct QueryReversed(FakeEmbedder);

    impl Embedder for QueryReversed {
        fn adapter_id(&self) -> &'static str {
            "fake.embedder"
        }

        fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError> {
            let mut out = self.0.embed(texts, kind)?;
            if kind == TextKind::Query {
                out.vectors.reverse();
            }
            Ok(out)
        }
        fn key(&self) -> ModelKey {
            self.0.key()
        }
        fn concurrency_ceiling(&self) -> usize {
            self.0.concurrency_ceiling()
        }
        fn max_batch_inputs(&self) -> usize {
            self.0.max_batch_inputs()
        }
        fn max_input_tokens(&self) -> usize {
            self.0.max_input_tokens()
        }
        fn locality(&self) -> Locality {
            self.0.locality()
        }
        fn ready(&self) -> Result<(), EmbedError> {
            self.0.ready()
        }
        fn credential(&self) -> Option<umx_core::ports::Credential> {
            self.0.credential()
        }
    }

    #[test]
    #[should_panic(expected = "Query input 0 out of place")]
    fn order_clause_catches_a_query_only_reversal() {
        umx_contracts::embedder::em_c1_one_vector_per_input_in_order(|reach| {
            QueryReversed(embedder(reach))
        });
    }

    #[test]
    fn locality_is_local_unless_set() {
        let key = ModelKey::new("fake-bow", 8).unwrap();
        assert_eq!(FakeEmbedder::new(key.clone()).locality(), Locality::Local);
        assert_eq!(
            FakeEmbedder::new(key)
                .with_locality(Locality::Hosted)
                .locality(),
            Locality::Hosted
        );
    }

    #[test]
    fn counters_count_provider_calls_only() {
        let fake = embedder(umx_contracts::embedder::Reach::Online);
        fake.embed(&[], TextKind::Document).unwrap();
        fake.embed(&["".to_string()], TextKind::Document)
            .unwrap_err();
        assert_eq!((fake.calls(), fake.texts_embedded()), (0, 0));
        fake.embed(&["a b".to_string(), "c".to_string()], TextKind::Document)
            .unwrap();
        assert_eq!((fake.calls(), fake.texts_embedded()), (1, 2));
        fake.set_offline(true);
        fake.embed(&["a".to_string()], TextKind::Document)
            .unwrap_err();
        assert_eq!(fake.calls(), 1);
    }

    #[test]
    fn failing_writes_change_nothing() {
        let (files, root, ()) = graph_files();
        files.write(&root, GraphFile::Nodes, b"a").unwrap();
        files.fail_writes(true);
        assert!(files.write(&root, GraphFile::Nodes, b"b").is_err());
        assert_eq!(
            files.read(&root, GraphFile::Nodes).unwrap().as_deref(),
            Some(&b"a"[..])
        );
    }
}
