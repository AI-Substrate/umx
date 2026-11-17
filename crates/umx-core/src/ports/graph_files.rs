//! GraphFiles port (the exemplar, blueprint 001 d-8): read, write and list one graph's files as bytes.

use crate::codec::GraphFile;
use crate::ports::ModelKey;
pub use crate::scan::{EntryKind, FindLimits, Found, FoundGraph};
use crate::vectors::{ShardFile, ShardKind};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// An absolute, normalised graph folder path: starts with `/`, no `.` or `..` parts, no empty parts, no trailing
/// `/`. Whether it is inside the allowed roots is the adapter's check (req-47).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GraphDir(String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphDirError {
    NotAbsolute,
    DotSegment,
}

impl GraphDir {
    pub fn parse(path: &str) -> Result<GraphDir, GraphDirError> {
        if !path.starts_with('/') {
            return Err(GraphDirError::NotAbsolute);
        }
        let mut out = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            if part == "." || part == ".." || part.contains('\0') {
                return Err(GraphDirError::DotSegment);
            }
            out.push('/');
            out.push_str(part);
        }
        if out.is_empty() {
            out.push('/');
        }
        Ok(GraphDir(out))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True if this folder is `root` or inside it, compared by whole path parts.
    pub fn is_within(&self, root: &GraphDir) -> bool {
        root.0 == "/"
            || self.0 == root.0
            || (self.0.starts_with(root.0.as_str())
                && self.0.as_bytes().get(root.0.len()) == Some(&b'/'))
    }

    pub fn join(&self, child: &str) -> Result<GraphDir, GraphDirError> {
        GraphDir::parse(&alloc::format!("{}/{}", self.0, child))
    }
}

impl fmt::Display for GraphDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilesError {
    /// The folder is outside the roots this adapter may touch (req-47). Nothing was read or written.
    OutsideRoots(GraphDir),
    /// The operating system refused; the message names the operation, never file contents.
    Io(String),
    /// The caller asked for something the port never does: an empty set, a file named twice in one set, or
    /// removing `.gitignore` (a user file as much as umx's). A programming error; nothing changed.
    Invalid(&'static str),
    /// A crash left a set half applied, and these targets changed since (a pull, a checkout, a hand edit), so
    /// recovery will not write over them. Every call on the folder returns this, and changes nothing, until a
    /// person resolves it; the files, the temp files and the marker are left as they are.
    RecoveryConflict(Vec<FileRef>),
    /// The file is over its kind's cap ([`FileRef::cap`]): a read refuses it without reading its body, and a set
    /// that would write it changes nothing. `size` is the file's bytes (or the change's), `cap` the most allowed.
    TooLarge { file: FileRef, size: u64, cap: u64 },
    /// A file the set checks is not as the caller last saw it: its stamp differs from the expected one (a checkout,
    /// a pull, a hand edit, or only a touch). The content files (graph.json, nodes.jsonl, edges.jsonl) come first in
    /// [`GraphFile`] order, then any `also` file ([`GraphFiles::write_set_checked_also`]). Nothing was written and
    /// the set's temp files were removed. Never retried by the port: the caller reads the files again.
    Changed(Vec<FileRef>),
    /// [`GraphFiles::relocate`] would cross a file system (`EXDEV`): umx never copies a graph folder, so nothing
    /// moved (ws006 S23, architect).
    CrossDevice,
}

/// [`GraphFiles::relocate`]'s refusal when the new folder (or anything at its path) exists: nothing moved.
pub const RELOCATE_TARGET_EXISTS: &str = "relocate: the new folder exists";
/// [`GraphFiles::relocate`]'s refusal when the new folder's parent does not exist: nothing moved.
pub const RELOCATE_PARENT_MISSING: &str = "relocate: the new folder's parent does not exist";
/// [`GraphFiles::relocate`]'s refusal when the new folder is inside the folder it moves: nothing moved.
pub const RELOCATE_INSIDE_ITSELF: &str = "relocate: the new folder is inside the folder it moves";

/// What a content file looked like on disk when umx last read or wrote it: opaque, compared only for equality, and
/// never proof of anything but "unchanged since". Absent is a stamp too. The adapter defines the parts: fs takes
/// (device, inode, size, mtime ns, ctime ns) from fstat on Unix, and on Windows will take the change time, the
/// volume serial and the file index; the fake takes a generation it bumps on every write. A touch changes it, so it
/// refuses a write whose bytes did not change (fail closed). Not a hash: it cannot see a change that keeps every part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp(Option<[u64; 5]>);

impl FileStamp {
    /// No file there.
    pub const ABSENT: FileStamp = FileStamp(None);

    /// A file there, from the adapter's own parts. Adapters only: umx-core and the services never build or read
    /// the parts, they only compare stamps.
    pub const fn from_parts(parts: [u64; 5]) -> FileStamp {
        FileStamp(Some(parts))
    }
}

/// The stamps of a graph folder's three content files (never `.gitignore`, never shards; ws002 F144).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphStamps {
    pub graph: FileStamp,
    pub nodes: FileStamp,
    pub edges: FileStamp,
}

impl GraphStamps {
    /// The three files stamped, in [`GraphFile`] order.
    pub const FILES: [GraphFile; 3] = [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges];

    /// All three absent: a folder with no graph yet.
    pub const ABSENT: GraphStamps = GraphStamps {
        graph: FileStamp::ABSENT,
        nodes: FileStamp::ABSENT,
        edges: FileStamp::ABSENT,
    };

    /// The stamp of `file`; None for `.gitignore`, which is never stamped.
    pub fn get(&self, file: GraphFile) -> Option<FileStamp> {
        match file {
            GraphFile::Graph => Some(self.graph),
            GraphFile::Nodes => Some(self.nodes),
            GraphFile::Edges => Some(self.edges),
            GraphFile::GitIgnore => None,
        }
    }

    /// Sets the stamp of `file`; `.gitignore` is ignored.
    pub fn set(&mut self, file: GraphFile, stamp: FileStamp) {
        match file {
            GraphFile::Graph => self.graph = stamp,
            GraphFile::Nodes => self.nodes = stamp,
            GraphFile::Edges => self.edges = stamp,
            GraphFile::GitIgnore => {}
        }
    }

    /// The files whose stamp in `now` differs from this one, in [`GraphFile`] order.
    pub fn changed(&self, now: &GraphStamps) -> Vec<GraphFile> {
        GraphStamps::FILES
            .into_iter()
            .filter(|f| self.get(*f) != now.get(*f))
            .collect()
    }
}

/// What a set that stands left behind: how it ended, and the content files' stamps after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committed {
    pub written: Written,
    /// Each content file the set wrote, stamped by fstat on its temp file's handle after the rename (never by
    /// path); [`FileStamp::ABSENT`] for one it removed; the others as the check before the commit found them.
    pub stamps: GraphStamps,
}

impl fmt::Display for FilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilesError::OutsideRoots(dir) => write!(f, "{dir} is outside the allowed roots"),
            FilesError::Io(m) => f.write_str(m),
            FilesError::Invalid(m) => write!(f, "invalid file change: {m}"),
            FilesError::RecoveryConflict(files) => {
                f.write_str("an interrupted write cannot be finished, these files changed since:")?;
                for file in files {
                    write!(f, " {file}")?;
                }
                Ok(())
            }
            FilesError::TooLarge { file, size, cap } => {
                write!(
                    f,
                    "{file} is {size} bytes, over the {cap} bytes umx holds for it"
                )
            }
            FilesError::Changed(files) => {
                f.write_str("these files changed on disk since umx read them:")?;
                for file in files {
                    write!(f, " {file}")?;
                }
                Ok(())
            }
            FilesError::CrossDevice => f.write_str("the new folder is on another file system"),
        }
    }
}

/// What a successful write left behind. Either way the new bytes are in place and a read returns them, so the
/// caller must not undo the change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Written {
    /// The new bytes are in place and on disk.
    Durable,
    /// The change stands, but finishing it on disk failed after the point of no return (a rename, or a folder
    /// sync, after the set's marker was synced). Every later call on the folder sees the new state, because
    /// recovery finishes the renames first; only a crash before the OS flushes could bring back the old bytes. A
    /// warning, never an error: the caller keeps the change and never undoes it (anti-pattern ap-13).
    NotSynced(FilesError),
}

/// A file a graph folder holds: a graph file at its top, a shard (vectors, `embeddings/<model@dims>/<xx>.tsv`,
/// workshop 008 section 4.1; or similar_to lists, `similar/<model@dims>/<xx>.tsv`, section 9), or a similarity
/// folder's `similar/<model@dims>/_meta.tsv`. Shards go through the same sets as the graph files, so a change that
/// touches both is all or nothing. Every part of a path is checked (a fixed name, a `ModelKey` slug, `[0-9a-z]{2}`),
/// so none can leave the graph folder.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileRef {
    Graph(GraphFile),
    Shard(ShardFile),
    /// `similar/<model@dims>/_meta.tsv`: what that folder's lists were computed from (staleness check). The `_`
    /// cannot clash with a shard name.
    SimilarMeta(ModelKey),
    /// `receipts.jsonl` at the folder's top: the receipts of batches that carried an idempotency key (ws003 s09,
    /// [`crate::receipts`]). Read and written only by keyed batches, in the batch's own set; not a content file, so
    /// never in [`GraphStamps`] (a keyed batch checks its stamp through `also`).
    Receipts,
}

/// The receipts file's name.
pub const RECEIPTS_FILE: &str = "receipts.jsonl";

/// The file name of a similarity folder's staleness line.
pub const SIMILAR_META: &str = "_meta.tsv";

/// The most bytes umx reads or writes for each kind of file (workshop 008; security). nodes.jsonl and
/// edges.jsonl match umx-graph's u32 offsets; the others are far past any real graph's and bound what a pulled or
/// hand-made file can make the daemon allocate.
pub const GRAPH_JSON_CAP: u64 = 1 << 20;
pub const RECORDS_CAP: u64 = u32::MAX as u64;
pub const SHARD_CAP: u64 = 64 << 20;
pub const GITIGNORE_CAP: u64 = 64 << 10;
pub const SIMILAR_META_CAP: u64 = 1 << 20;
pub const RECEIPTS_CAP: u64 = crate::receipts::READ_CAP;

impl FileRef {
    /// The most bytes this file may hold: a read of a bigger one, or a set that would write one, is
    /// [`FilesError::TooLarge`].
    pub const fn cap(&self) -> u64 {
        match self {
            FileRef::Graph(GraphFile::Graph) => GRAPH_JSON_CAP,
            FileRef::Graph(GraphFile::Nodes | GraphFile::Edges) => RECORDS_CAP,
            FileRef::Graph(GraphFile::GitIgnore) => GITIGNORE_CAP,
            FileRef::Shard(_) => SHARD_CAP,
            FileRef::SimilarMeta(_) => SIMILAR_META_CAP,
            FileRef::Receipts => RECEIPTS_CAP,
        }
    }

    /// The path relative to the graph folder: `nodes.jsonl`, `embeddings/<model@dims>/<xx>.tsv`,
    /// `similar/<model@dims>/<xx>.tsv` or `similar/<model@dims>/_meta.tsv`.
    pub fn path(&self) -> String {
        match self {
            FileRef::Graph(f) => f.file_name().to_string(),
            FileRef::Shard(s) => s.to_string(),
            FileRef::SimilarMeta(key) => {
                format!("{}/{SIMILAR_META}", ShardKind::Similar.folder(key))
            }
            FileRef::Receipts => RECEIPTS_FILE.to_string(),
        }
    }

    /// The folder a file lives in, relative to the graph folder: `None` for a graph file (the folder itself).
    pub fn folder(&self) -> Option<String> {
        match self {
            FileRef::Graph(_) | FileRef::Receipts => None,
            FileRef::Shard(s) => Some(s.folder()),
            FileRef::SimilarMeta(key) => Some(ShardKind::Similar.folder(key)),
        }
    }

    /// The file a relative path names, if it is exactly one [`FileRef::path`] writes; anything else is `None`.
    pub fn parse(path: &str) -> Option<FileRef> {
        let graph = [
            GraphFile::Graph,
            GraphFile::Nodes,
            GraphFile::Edges,
            GraphFile::GitIgnore,
        ]
        .into_iter()
        .find(|f| f.file_name() == path);
        if let Some(f) = graph {
            return Some(FileRef::Graph(f));
        }
        if path == RECEIPTS_FILE {
            return Some(FileRef::Receipts);
        }
        let (top, rest) = path.split_once('/')?;
        let kind = [ShardKind::Vectors, ShardKind::Similar]
            .into_iter()
            .find(|k| k.top() == top)?;
        let (key, name) = rest.split_once('/')?;
        let key = ModelKey::parse(key).ok()?;
        let file = match (kind, name) {
            (ShardKind::Similar, SIMILAR_META) => FileRef::SimilarMeta(key),
            _ => FileRef::Shard(ShardFile::from_file_name_in(kind, &key, name)?),
        };
        (file.path() == path).then_some(file)
    }
}

impl From<GraphFile> for FileRef {
    fn from(file: GraphFile) -> FileRef {
        FileRef::Graph(file)
    }
}

impl From<ShardFile> for FileRef {
    fn from(shard: ShardFile) -> FileRef {
        FileRef::Shard(shard)
    }
}

impl fmt::Display for FileRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path())
    }
}

/// One change in a set: replace a file with these bytes, or remove it (`None`; already absent is fine).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change<'a> {
    pub file: FileRef,
    pub bytes: Option<&'a [u8]>,
}

impl Change<'_> {
    /// The checks every adapter makes before writing anything: a non-empty set, each file at most once, no removal
    /// of `.gitignore`, and no file over its cap (umx never writes a file it would refuse to read).
    pub fn validate(changes: &[Change<'_>]) -> Result<(), FilesError> {
        if changes.is_empty() {
            return Err(FilesError::Invalid("empty set"));
        }
        for (i, c) in changes.iter().enumerate() {
            if changes[..i].iter().any(|d| d.file == c.file) {
                return Err(FilesError::Invalid("a file named twice in one set"));
            }
            if c.file == FileRef::Graph(GraphFile::GitIgnore) && c.bytes.is_none() {
                return Err(FilesError::Invalid("umx never removes .gitignore"));
            }
            if let Some(bytes) = c.bytes
                && bytes.len() as u64 > c.file.cap()
            {
                return Err(FilesError::TooLarge {
                    file: c.file.clone(),
                    size: bytes.len() as u64,
                    cap: c.file.cap(),
                });
            }
        }
        Ok(())
    }
}

/// Where a test stops a later `write_set`, as if the process died there (contract clauses gf_c16 to gf_c19). A
/// crashed call returns `Err(FilesError::Io("crash"))` and cleans nothing up. Tests only; umx-compose never sets one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    /// Every temp file is written and synced; no marker yet.
    AfterTemps,
    /// The marker is synced (the point of no return); no rename yet.
    AfterMarker,
    /// The first change is applied; the others are not.
    AfterFirstRename,
}

impl FilesError {
    pub fn io(message: impl ToString) -> FilesError {
        FilesError::Io(message.to_string())
    }
}

pub trait GraphFiles: Send + Sync {
    /// A graph file's or shard's bytes, or None if it does not exist. A file over its cap ([`FileRef::cap`]) is
    /// `TooLarge`, and its body is never read (contract clause gf_c26).
    fn read_ref(&self, dir: &GraphDir, file: &FileRef) -> Result<Option<Vec<u8>>, FilesError>;
    /// A graph file's bytes: `read_ref`, so every adapter has one read path to get right.
    fn read(&self, dir: &GraphDir, file: GraphFile) -> Result<Option<Vec<u8>>, FilesError> {
        self.read_ref(dir, &file.into())
    }
    /// Applies every change, or none of them, as seen by any later call on this folder, also after a crash and
    /// restart, creating the folder if needed. Each file is replaced whole (a reader sees the old bytes or the new,
    /// never a mix). The one write path: the unchecked wrappers live in [`crate::setup::GraphFilesSetup`], outside
    /// `ports` and re-exported nowhere, so umx-services cannot reach them (a umx-arch rule, F144).
    ///
    /// With `expect`, the adapter stamps the three content files as late as the set allows: after every temp file
    /// is synced, the old targets are read for the marker and the marker is synced, just before the first rename
    /// (one change: just before its rename). Any that differs is [`FilesError::Changed`], with nothing written: the
    /// temps and the marker are removed. A known residual: a change that lands between that last stat and the
    /// rename is not seen (microseconds; the same honesty as a file swapped between a read and a write). `None`
    /// checks nothing (a new graph, the vector cache) and still returns the stamps. Recovery still compares
    /// against the marker: a matching stamp is never proof a set completed.
    ///
    /// Every `Err` means nothing changed (contract clause gf_c13). Once the change stands the call returns `Ok`,
    /// even if a later step failed; that step is reported as `Written::NotSynced`.
    ///
    /// [`GraphFiles::write_set_checked_also`] with no `also` file. A default only: adapters implement
    /// `write_set_checked_also` and never override this, so there is one write path.
    fn write_set_checked(
        &self,
        dir: &GraphDir,
        changes: &[Change<'_>],
        expect: Option<&GraphStamps>,
    ) -> Result<Committed, FilesError> {
        self.write_set_checked_also(dir, changes, expect, &[])
    }
    /// [`GraphFiles::write_set_checked`], also checking each `also` file against its stamp (ws003 s09: a keyed
    /// batch's `receipts.jsonl`, stamped when it was read; absent is a stamp too). Each is checked at exactly the
    /// points the content files are (before the marker, and just before the first rename), in the same pass and with
    /// the same residual. A mismatch is [`FilesError::Changed`] naming every file that differs, content files first,
    /// with nothing written. The returned stamps are the content files' only.
    fn write_set_checked_also(
        &self,
        dir: &GraphDir,
        changes: &[Change<'_>],
        expect: Option<&GraphStamps>,
        also: &[(FileRef, FileStamp)],
    ) -> Result<Committed, FilesError>;
    /// A file's bytes (None if absent) and its stamp, both from the one open handle: the stamp is the file the bytes
    /// came from, so a change made after the open shows as changed at the next check. Capped as [`GraphFiles::read_ref`].
    fn read_ref_stamped(
        &self,
        dir: &GraphDir,
        file: &FileRef,
    ) -> Result<(Option<Vec<u8>>, FileStamp), FilesError>;
    /// A content file's bytes and stamp: [`GraphFiles::read_ref_stamped`]. A default only: adapters never override
    /// it, so there is one stamped read.
    fn read_stamped(
        &self,
        dir: &GraphDir,
        file: GraphFile,
    ) -> Result<(Option<Vec<u8>>, FileStamp), FilesError> {
        self.read_ref_stamped(dir, &file.into())
    }
    /// The three content files' stamps as they are now, by path, never following a link (a link swapped in stamps
    /// as changed). For the services' up-front check and the reads' stale warning.
    fn stamps(&self, dir: &GraphDir) -> Result<GraphStamps, FilesError>;
    /// The graph files present in the folder, in `GraphFile` order; empty if the folder does not exist.
    fn list(&self, dir: &GraphDir) -> Result<Vec<GraphFile>, FilesError>;
    /// The folder's physical identity: every spelling of one folder (symlinks, `/tmp` and `/private/tmp`) gives
    /// the same value, and resolving it again gives it unchanged. The folder need not exist yet.
    fn resolve(&self, dir: &GraphDir) -> Result<GraphDir, FilesError>;
    /// The shard files of one kind of one vector space, sorted; empty if there are none. A similarity folder's
    /// `_meta.tsv` is not a shard.
    fn list_shards(
        &self,
        dir: &GraphDir,
        key: &ModelKey,
        kind: ShardKind,
    ) -> Result<Vec<ShardFile>, FilesError>;
    /// The vector spaces with a folder under `embeddings/` (a valid `model@dims` name), sorted.
    fn list_vector_spaces(&self, dir: &GraphDir) -> Result<Vec<ModelKey>, FilesError>;
    /// The graph folders (holding `graph.json`, or only a write set's marker: [`crate::scan::graph_in`]) at or
    /// under `root`, for `umx graph scan` (ws002 s17): a walk that
    /// never follows a symlink, never enters a skipped folder ([`crate::scan::skipped_folder`]), a found graph folder or another
    /// repository's top (reported in `nested_repos`), and stops at `limits` with `truncated`; a folder it cannot list
    /// is passed over and counted in `unlisted`. Each graph's
    /// `graph.json` comes back too, read without following a link and only up to [`crate::scan::FOUND_GRAPH_JSON_CAP`]. Every
    /// adapter walks with [`crate::scan::walk_for_graphs`], so all agree. `root` must be inside the roots.
    fn find_graphs(&self, root: &GraphDir, limits: FindLimits) -> Result<Found, FilesError>;
    /// The graph in `dir` itself, as [`GraphFiles::find_graphs`] would report it ([`crate::scan::graph_in`]): its
    /// `graph.json` read without following a link and only up to the cap, and with no recovery run, so a folder
    /// whose write was cut short is reported `pending` and left exactly as it is (`graph scan` only looks; review
    /// R289-3, graph-storage-expert). Ok(None) when `dir` is not there, or holds neither a `graph.json` nor a write
    /// set's marker; a marker alone is `Some`, pending with no `graph.json` (F176). Err when `dir` could not be
    /// listed. `dir` must be inside the roots.
    fn peek_graph(&self, dir: &GraphDir) -> Result<Option<FoundGraph>, FilesError>;
    /// Moves the graph folder `from` to `to` whole, in one rename (`umx graph move`, ws006 S23, architect): never a
    /// copy. Both must be inside the roots (`OutsideRoots`), `from` a real folder, not a symlink (`OutsideRoots`),
    /// with a cut-short write set finished first as any call does. `to` must not exist
    /// (`Invalid(`[`RELOCATE_TARGET_EXISTS`]`)`; a file there counts, and so does one made after the checks: the rename
    /// never replaces), its parent must ([`RELOCATE_PARENT_MISSING`]), and it must not be inside `from`
    /// ([`RELOCATE_INSIDE_ITSELF`]); a rename across file systems is [`FilesError::CrossDevice`]. Everything inside
    /// `from` moves with it: files, folders, symlinks and a cut-short write's marker. Every `Err` means nothing
    /// moved. Not a write set: services checks `files_unchanged` first.
    fn relocate(&self, from: &GraphDir, to: &GraphDir) -> Result<(), FilesError>;
    /// This adapter's id in the adapter matrix (blueprint d-6), such as `fs.graph-files`. describe() and
    /// `umx daemon status` read it from the wired object (E-1).
    fn adapter_id(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_ref_path_round_trips_and_nothing_else_parses() {
        let key = ModelKey::new("text-embedding-3-small", 256).unwrap();
        let shard = FileRef::Shard(ShardFile::new(key.clone(), "b7").unwrap());
        assert_eq!(shard.path(), "embeddings/text-embedding-3-small@256/b7.tsv");
        let similar =
            FileRef::Shard(ShardFile::of_kind(ShardKind::Similar, key.clone(), "b7").unwrap());
        assert_eq!(similar.path(), "similar/text-embedding-3-small@256/b7.tsv");
        let meta = FileRef::SimilarMeta(key);
        assert_eq!(meta.path(), "similar/text-embedding-3-small@256/_meta.tsv");
        for f in [
            shard,
            similar,
            meta,
            GraphFile::Nodes.into(),
            GraphFile::GitIgnore.into(),
            FileRef::Receipts,
        ] {
            assert_eq!(FileRef::parse(&f.path()), Some(f));
        }
        for bad in [
            "embeddings/m@4/../b7.tsv",
            "embeddings/m@4/b7.tsv/x",
            "embeddings/M@4/b7.tsv",
            "embeddings/m@4/B7.tsv",
            "embeddings//b7.tsv",
            "/nodes.jsonl",
            "embeddings/m@4/.umx-tmp-1-b7.tsv",
            "embeddings/m@4/_meta.tsv",
            "similar/m@4/_meta.tsv/x",
            "similar/m@4/_Meta.tsv",
            "similar/m@4/B7.tsv",
            "similarity/m@4/b7.tsv",
            "similar/m@8193/b7.tsv",
            "/receipts.jsonl",
            "receipts.jsonl/x",
            "embeddings/receipts.jsonl",
            "Receipts.jsonl",
        ] {
            assert_eq!(FileRef::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn graph_dirs() {
        assert_eq!(GraphDir::parse("/tmp//g/").unwrap().as_str(), "/tmp/g");
        assert_eq!(GraphDir::parse("tmp/g"), Err(GraphDirError::NotAbsolute));
        assert_eq!(
            GraphDir::parse("/tmp/../etc"),
            Err(GraphDirError::DotSegment)
        );
        assert_eq!(GraphDir::parse("/tmp/./g"), Err(GraphDirError::DotSegment));
        let root = GraphDir::parse("/tmp").unwrap();
        assert!(GraphDir::parse("/tmp/g").unwrap().is_within(&root));
        assert!(root.is_within(&root));
        assert!(!GraphDir::parse("/tmpx/g").unwrap().is_within(&root));
        assert!(
            GraphDir::parse("/a")
                .unwrap()
                .is_within(&GraphDir::parse("/").unwrap())
        );
    }
}
