//! ModelFetcher port (req-74; blueprint c-port-model-fetcher): downloads a pinned model revision's files into the
//! models folder, checking each file's size and sha256 before it is kept under its final name. A deliberate user
//! step through `umx models fetch`, never automatic on embed. Adapters: hf.model-fetcher (Hugging Face over https,
//! umx-adapter-model-fetch), fake.model-fetcher.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// One file of a model revision, pinned: its name in the revision, its sha256 (lower-case hex) and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFileSpec {
    /// One plain path component ([`safe_file_name`]).
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Where a model comes from: a Hugging Face repo at a fixed commit, and the files to take from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSource {
    /// `org/name`.
    pub repo: String,
    /// A full commit hash, never a branch: the files under it never change.
    pub commit: String,
    pub files: Vec<ModelFileSpec>,
}

impl ModelSource {
    /// The revision's folder under the models folder: `<models>/<org>--<name>/<commit>`, as the local embedder
    /// reads it.
    pub fn folder(&self, models: &str) -> String {
        alloc::format!(
            "{}/{}/{}",
            models.trim_end_matches('/'),
            self.repo.replace('/', "--"),
            self.commit
        )
    }

    /// Every file's size added up: what the fetch needs on disk at most.
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// Whether `name` is one plain path component: not empty, no `/` or `\`, not `.` or `..`, no leading dot (temp files
/// are `.<name>.part`), no control character. Spec names are checked before any path is built from them.
pub fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

/// Progress through one file: bytes written so far of its pinned size. Sizes only, never a URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchProgress {
    pub file: String,
    pub done: u64,
    pub total: u64,
}

/// What became of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFetched {
    pub file: String,
    pub size: u64,
    /// False when the file was there already with the pinned sha256, and nothing was downloaded.
    pub downloaded: bool,
}

/// Why a fetch stopped, by step. No variant carries a URL, a query string or a response body: only hosts, file
/// names, sizes and hashes. A failed file never leaves anything under its final name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// A spec file name that is not one plain path component, or a folder or file that is a symlink or not the
    /// user's.
    Path { file: String, reason: String },
    /// Could not connect, or the connection failed or timed out.
    Connect { host: String, reason: String },
    /// A redirect to a host off the allow-list, or not over https.
    Redirect { host: String },
    /// An answer other than the file or an allowed redirect.
    Status { host: String, status: u16 },
    /// No Content-Length, or one or the bytes received not the pinned size.
    Size {
        file: String,
        expected: u64,
        got: Option<u64>,
    },
    /// The bytes received do not hash to the pinned sha256: nothing is kept.
    Hash {
        file: String,
        expected: String,
        got: String,
    },
    /// Writing or renaming in the models folder failed.
    Write { file: String, reason: String },
    /// The disk filled while writing.
    Disk { file: String },
    /// Checked under the models lock before any byte is fetched: the files still to arrive need more than is free.
    Space { needed: u64, available: u64 },
}

impl FetchError {
    /// The step that failed, as the catalogue names it.
    pub fn step(&self) -> &'static str {
        match self {
            FetchError::Path { .. } => "path",
            FetchError::Connect { .. } => "connect",
            FetchError::Redirect { .. } => "redirect",
            FetchError::Status { .. } => "status",
            FetchError::Size { .. } => "size",
            FetchError::Hash { .. } => "hash",
            FetchError::Write { .. } => "write",
            FetchError::Disk { .. } | FetchError::Space { .. } => "disk",
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Path { file, reason } => write!(f, "{file}: {reason}"),
            FetchError::Connect { host, reason } => write!(f, "connecting to {host}: {reason}"),
            FetchError::Redirect { host } => {
                write!(
                    f,
                    "a redirect to {host}, which is not an allowed https host"
                )
            }
            FetchError::Status { host, status } => write!(f, "{host} answered {status}"),
            FetchError::Size {
                file,
                expected,
                got: Some(got),
            } => write!(f, "{file} is {got} bytes, not the pinned {expected}"),
            FetchError::Size {
                file,
                expected,
                got: None,
            } => write!(
                f,
                "{file} came with no size (the pinned size is {expected})"
            ),
            FetchError::Hash {
                file,
                expected,
                got,
            } => write!(f, "{file} has sha256 {got}, not the pinned {expected}"),
            FetchError::Write { file, reason } => write!(f, "writing {file}: {reason}"),
            FetchError::Disk { file } => write!(f, "the disk filled while writing {file}"),
            FetchError::Space { needed, available } => {
                write!(f, "{needed} bytes are needed and {available} are free")
            }
        }
    }
}

/// Fetches a pinned model revision into a models folder.
pub trait ModelFetcher: Send + Sync {
    /// Fetches every file of `source` into its folder under `models` ([`ModelSource::folder`]; folders created
    /// 0700). A file already there, a regular file (not a symlink) with the pinned sha256, is not downloaded again.
    /// Each download goes to a temp name, is checked for size and sha256, and only then renamed to its final name
    /// (0600). Stops at the first file that fails.
    fn fetch(
        &self,
        source: &ModelSource,
        models: &str,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<Vec<FileFetched>, FetchError>;

    /// This adapter's id in the adapter matrix (blueprint d-6), such as `fake.model-fetcher`.
    fn adapter_id(&self) -> &'static str;
}
