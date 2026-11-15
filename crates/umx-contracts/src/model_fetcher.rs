//! ModelFetcher contract (req-74; architect and security, s14). A factory builds a world over a fresh temp models
//! folder that serves four cases: the good file, the same size with other bytes (a hash mismatch), a body longer than
//! the pinned size, and a redirect to a host off the allow-list. The real adapter serves them from a stub HTTP server
//! on 127.0.0.1, never the network.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use umx_core::ports::{FetchError, FileFetched, ModelFetcher, ModelFileSpec, ModelSource};

pub const CLAUSES: &[&str] = &[
    "mf_c1_fetch_keeps_each_verified_file_0600_in_a_0700_folder",
    "mf_c2_a_present_verified_file_is_not_downloaded_again",
    "mf_c3_a_hash_mismatch_leaves_nothing",
    "mf_c4_an_oversize_body_is_refused_unwritten",
    "mf_c5_a_redirect_off_the_allow_list_is_refused",
    "mf_c6_a_damaged_file_is_replaced",
    "mf_c7_an_unsafe_file_name_is_refused_before_any_path",
    "mf_c8_a_symlink_at_the_final_name_is_never_trusted",
];

/// The good file's bytes: synthetic, no model.
pub fn good_bytes() -> Vec<u8> {
    b"umx model fetch fixture: synthetic bytes, no model\n".repeat(64)
}

/// sha256 of [`good_bytes`].
pub const GOOD_SHA256: &str = "878608ed6c3dbf68eb4557b083cda5918e51690b608938f57d4a43b77c1e1602";

pub const FILE: &str = "fixture.gguf";

/// What a world serves for a source's repo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    /// [`good_bytes`].
    Good,
    /// As many bytes as the good file, but other ones.
    Mismatch,
    /// The good bytes and more.
    Oversize,
    /// A redirect to a host that is not allowed.
    OffList,
}

impl Case {
    pub fn repo(self) -> &'static str {
        match self {
            Case::Good => "fixture/good",
            Case::Mismatch => "fixture/mismatch",
            Case::Oversize => "fixture/oversize",
            Case::OffList => "fixture/offlist",
        }
    }

    /// The bytes served for this case (the off-list case serves the good bytes, if it is ever reached).
    pub fn bytes(self) -> Vec<u8> {
        let mut b = good_bytes();
        match self {
            Case::Good | Case::OffList => {}
            Case::Mismatch => b[0] ^= 1,
            Case::Oversize => b.extend_from_slice(b"and more"),
        }
        b
    }

    pub fn of_repo(repo: &str) -> Option<Case> {
        [Case::Good, Case::Mismatch, Case::Oversize, Case::OffList]
            .into_iter()
            .find(|c| c.repo() == repo)
    }
}

/// The source asking for [`FILE`] at the good file's size and sha256, from `case`'s repo.
pub fn source(case: Case) -> ModelSource {
    ModelSource {
        repo: case.repo().to_string(),
        commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        files: vec![ModelFileSpec {
            path: FILE.to_string(),
            sha256: GOOD_SHA256.to_string(),
            size: good_bytes().len() as u64,
        }],
    }
}

/// A port over a fresh, empty models folder.
pub trait FetchWorld {
    type Port: ModelFetcher;
    fn port(&self) -> &Self::Port;
    /// The models folder (absolute; exists, empty).
    fn models(&self) -> &str;
}

fn fetch<W: FetchWorld>(w: &W, case: Case) -> Result<Vec<FileFetched>, FetchError> {
    w.port().fetch(&source(case), w.models(), &mut |_| {})
}

/// Every entry under the case's folder (temp files included), by name.
fn left<W: FetchWorld>(w: &W, case: Case) -> Vec<String> {
    let folder = source(case).folder(w.models());
    match std::fs::read_dir(&folder) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
        Err(_) => Vec::new(),
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

pub fn mf_c1_fetch_keeps_each_verified_file_0600_in_a_0700_folder<W: FetchWorld>(
    factory: impl Fn() -> W,
) {
    let w = factory();
    let mut seen = Vec::new();
    let got = w
        .port()
        .fetch(&source(Case::Good), w.models(), &mut |p| seen.push(p))
        .expect("the good file fetches");
    let size = good_bytes().len() as u64;
    assert_eq!(
        got,
        vec![FileFetched {
            file: FILE.into(),
            size,
            downloaded: true
        }]
    );
    let folder = source(Case::Good).folder(w.models());
    let file = Path::new(&folder).join(FILE);
    assert_eq!(std::fs::read(&file).unwrap(), good_bytes());
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(Path::new(&folder)), 0o700);
    assert_eq!(left(&w, Case::Good), vec![FILE.to_string()]);
    let last = seen.last().expect("progress was reported");
    assert_eq!(
        (last.done, last.total, last.file.as_str()),
        (size, size, FILE)
    );
    assert!(w.port().adapter_id().ends_with(".model-fetcher"));
}

pub fn mf_c2_a_present_verified_file_is_not_downloaded_again<W: FetchWorld>(
    factory: impl Fn() -> W,
) {
    let w = factory();
    fetch(&w, Case::Good).expect("first fetch");
    let again = fetch(&w, Case::Good).expect("second fetch");
    assert!(!again[0].downloaded, "{again:?}");
}

pub fn mf_c3_a_hash_mismatch_leaves_nothing<W: FetchWorld>(factory: impl Fn() -> W) {
    let w = factory();
    match fetch(&w, Case::Mismatch) {
        Err(FetchError::Hash {
            file,
            expected,
            got,
        }) => {
            assert_eq!(file, FILE);
            assert_eq!(expected, GOOD_SHA256);
            assert_ne!(got, GOOD_SHA256);
        }
        other => panic!("expected a hash refusal: {other:?}"),
    }
    assert!(
        left(&w, Case::Mismatch).is_empty(),
        "{:?}",
        left(&w, Case::Mismatch)
    );
}

pub fn mf_c4_an_oversize_body_is_refused_unwritten<W: FetchWorld>(factory: impl Fn() -> W) {
    let w = factory();
    assert!(
        matches!(fetch(&w, Case::Oversize), Err(FetchError::Size { .. })),
        "expected a size refusal"
    );
    assert!(
        left(&w, Case::Oversize).is_empty(),
        "{:?}",
        left(&w, Case::Oversize)
    );
}

pub fn mf_c5_a_redirect_off_the_allow_list_is_refused<W: FetchWorld>(factory: impl Fn() -> W) {
    let w = factory();
    match fetch(&w, Case::OffList) {
        Err(FetchError::Redirect { host }) => {
            assert!(
                !host.contains('/') && !host.contains('?'),
                "a host only: {host}"
            );
        }
        other => panic!("expected a redirect refusal: {other:?}"),
    }
    assert!(left(&w, Case::OffList).is_empty());
}

pub fn mf_c6_a_damaged_file_is_replaced<W: FetchWorld>(factory: impl Fn() -> W) {
    let w = factory();
    fetch(&w, Case::Good).expect("first fetch");
    let file = Path::new(&source(Case::Good).folder(w.models())).join(FILE);
    std::fs::write(&file, b"damaged").unwrap();
    let again = fetch(&w, Case::Good).expect("refetch");
    assert!(again[0].downloaded);
    assert_eq!(std::fs::read(&file).unwrap(), good_bytes());
}

pub fn mf_c7_an_unsafe_file_name_is_refused_before_any_path<W: FetchWorld>(
    factory: impl Fn() -> W,
) {
    let w = factory();
    for bad in ["../escape.gguf", ".hidden", "a/b.gguf"] {
        let mut s = source(Case::Good);
        s.files[0].path = bad.to_string();
        assert!(
            matches!(
                w.port().fetch(&s, w.models(), &mut |_| {}),
                Err(FetchError::Path { .. })
            ),
            "{bad}"
        );
    }
    assert!(
        std::fs::read_dir(w.models()).unwrap().next().is_none(),
        "nothing was created"
    );
}

pub fn mf_c8_a_symlink_at_the_final_name_is_never_trusted<W: FetchWorld>(factory: impl Fn() -> W) {
    let w = factory();
    // A file with the right bytes elsewhere, and a symlink to it at the final name.
    let folder = source(Case::Good).folder(w.models());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    let elsewhere = Path::new(w.models()).join("elsewhere.gguf");
    std::fs::write(&elsewhere, good_bytes()).unwrap();
    let file = Path::new(&folder).join(FILE);
    std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
    let got = fetch(&w, Case::Good).expect("fetch");
    assert!(got[0].downloaded, "a symlink is not a present file");
    let meta = std::fs::symlink_metadata(&file).unwrap();
    assert!(
        meta.file_type().is_file(),
        "the final name is a regular file now"
    );
    assert_eq!(
        std::fs::read(&elsewhere).unwrap(),
        good_bytes(),
        "the link's target is untouched"
    );
}

/// `model_fetcher_contract!(factory; mf_c1_<what>(), ...)` with `factory: Fn() -> impl FetchWorld`, each clause
/// listed once (the generated `every_clause_is_listed` test checks it).
#[macro_export]
macro_rules! model_fetcher_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod model_fetcher_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::model_fetcher::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::model_fetcher::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
