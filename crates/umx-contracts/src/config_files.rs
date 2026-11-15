//! ConfigFiles contract. A factory returns a fresh port with no config files, the one root it may use, and a guard
//! dropped after the clause. `<root>/repo` is a git repository (it holds `.git`); nothing else under the root is.

use umx_core::ports::{ConfigFile, ConfigFiles, ConfigFilesError, GraphDir};

pub const CLAUSES: &[&str] = &[
    "cf_c1_write_then_read_is_identical",
    "cf_c2_read_missing_is_none",
    "cf_c3_machine_and_repo_are_separate",
    "cf_c4_repo_outside_roots_is_refused",
    "cf_c5_repo_root_finds_git_within_roots",
    "cf_c6_path_names_the_file",
    "cf_c7_an_oversized_file_is_refused",
];

fn repo(root: &GraphDir) -> GraphDir {
    root.join("repo").expect("repo dir")
}

pub fn cf_c1_write_then_read_is_identical<P: ConfigFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let bytes = b"# umx config\nroots = [\"/a\"]\n\xe2\x9c\x93\n".to_vec();
    for file in [ConfigFile::Machine, ConfigFile::Repo(repo(&root))] {
        port.write(&file, &bytes).unwrap();
        assert_eq!(port.read(&file).unwrap(), Some(bytes.clone()));
        port.write(&file, b"short\n").unwrap();
        assert_eq!(port.read(&file).unwrap().as_deref(), Some(&b"short\n"[..]));
    }
}

pub fn cf_c2_read_missing_is_none<P: ConfigFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    assert_eq!(port.read(&ConfigFile::Machine).unwrap(), None);
    assert_eq!(port.read(&ConfigFile::Repo(repo(&root))).unwrap(), None);
}

pub fn cf_c3_machine_and_repo_are_separate<P: ConfigFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    port.write(&ConfigFile::Machine, b"machine\n").unwrap();
    assert_eq!(port.read(&ConfigFile::Repo(repo(&root))).unwrap(), None);
    port.write(&ConfigFile::Repo(repo(&root)), b"repo\n")
        .unwrap();
    assert_eq!(
        port.read(&ConfigFile::Machine).unwrap().as_deref(),
        Some(&b"machine\n"[..])
    );
}

pub fn cf_c4_repo_outside_roots_is_refused<P: ConfigFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, _root, _guard) = factory();
    let outside = ConfigFile::Repo(GraphDir::parse("/").unwrap());
    assert!(matches!(
        port.read(&outside),
        Err(ConfigFilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.write(&outside, b"x"),
        Err(ConfigFilesError::OutsideRoots(_))
    ));
}

pub fn cf_c5_repo_root_finds_git_within_roots<P: ConfigFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let deep = repo(&root).join("sub").unwrap().join("deeper").unwrap();
    assert_eq!(port.repo_root(&deep).unwrap(), Some(repo(&root)));
    assert_eq!(port.repo_root(&repo(&root)).unwrap(), Some(repo(&root)));
    assert_eq!(port.repo_root(&root.join("other").unwrap()).unwrap(), None);
    assert_eq!(
        port.repo_root(&GraphDir::parse("/").unwrap()).unwrap(),
        None
    );
}

pub fn cf_c6_path_names_the_file<P: ConfigFiles, G>(factory: impl Fn() -> (P, GraphDir, G)) {
    let (port, root, _guard) = factory();
    assert!(
        port.path(&ConfigFile::Machine)
            .as_str()
            .ends_with("/config.toml")
    );
    assert_eq!(
        port.path(&ConfigFile::Repo(repo(&root))).as_str(),
        format!("{}/.umx/config.toml", repo(&root))
    );
}

/// F45: a config file over SMALL_FILE_CAP (a repo's arrives by git pull) is refused, never read whole; one at the
/// cap reads.
pub fn cf_c7_an_oversized_file_is_refused<P: ConfigFiles, G>(
    factory: impl Fn() -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory();
    let cap = umx_core::ports::config_files::SMALL_FILE_CAP as usize;
    for file in [ConfigFile::Machine, ConfigFile::Repo(repo(&root))] {
        port.write(&file, &vec![b'#'; cap]).unwrap();
        assert_eq!(port.read(&file).unwrap().map(|b| b.len()), Some(cap));
        port.write(&file, &vec![b'#'; cap + 1]).unwrap();
        match port.read(&file) {
            Err(ConfigFilesError::Refused { reason, .. }) => {
                assert_eq!(reason, umx_core::ports::config_files::REFUSED_TOO_LARGE)
            }
            other => panic!(
                "an oversized config file was not refused: {:?}",
                other.map(|b| b.map(|b| b.len()))
            ),
        }
    }
}

/// What flowspace3's config folder holds when a [`FS3_CLAUSES`] factory builds the port.
#[derive(Clone, Copy, Debug)]
pub enum Fs3 {
    Missing,
    File(&'static [u8]),
    /// `config.toml` is a symlink to a file with these bytes.
    Symlink(&'static [u8]),
    /// `config.toml` holds these bytes and has this mode (group- or world-writable).
    Mode(&'static [u8], u32),
}

/// The clauses for `ConfigFile::Flowspace3` (ws004 s27). Their factory takes what the folder holds.
pub const FS3_CLAUSES: &[&str] = &[
    "cf_f1_flowspace3_reads_and_is_never_written",
    "cf_f2_a_symlinked_flowspace3_file_is_refused",
    "cf_f3_flowspace3_path_names_its_config_toml",
    "cf_f4_a_flowspace3_file_others_can_write_is_refused",
];

pub fn cf_f1_flowspace3_reads_and_is_never_written<P: ConfigFiles, G>(
    factory: impl Fn(Fs3) -> (P, G),
) {
    let (port, _guard) = factory(Fs3::Missing);
    assert_eq!(port.read(&ConfigFile::Flowspace3).unwrap(), None);
    let (port, _guard) = factory(Fs3::File(b"[embedder]\nactive = \"a\"\n"));
    let read = || port.read(&ConfigFile::Flowspace3).unwrap();
    assert_eq!(
        read().as_deref(),
        Some(&b"[embedder]\nactive = \"a\"\n"[..])
    );
    match port.write(&ConfigFile::Flowspace3, b"x") {
        Err(ConfigFilesError::Refused { reason, .. }) => {
            assert_eq!(reason, umx_core::ports::config_files::REFUSED_READ_ONLY)
        }
        other => panic!("a write of flowspace3's config was not refused: {other:?}"),
    }
    assert_eq!(
        read().as_deref(),
        Some(&b"[embedder]\nactive = \"a\"\n"[..])
    );
    assert_eq!(port.read(&ConfigFile::Machine).unwrap(), None);
}

pub fn cf_f2_a_symlinked_flowspace3_file_is_refused<P: ConfigFiles, G>(
    factory: impl Fn(Fs3) -> (P, G),
) {
    let (port, _guard) = factory(Fs3::Symlink(b"[embedder]\n"));
    match port.read(&ConfigFile::Flowspace3) {
        Err(ConfigFilesError::Refused { reason, .. }) => {
            assert_eq!(reason, umx_core::ports::config_files::REFUSED_SYMLINK)
        }
        other => panic!("a symlinked flowspace3 config was read: {other:?}"),
    }
}

pub fn cf_f3_flowspace3_path_names_its_config_toml<P: ConfigFiles, G>(
    factory: impl Fn(Fs3) -> (P, G),
) {
    let (port, _guard) = factory(Fs3::Missing);
    assert!(
        port.path(&ConfigFile::Flowspace3)
            .as_str()
            .ends_with("/flowspace3/config.toml")
    );
}

/// The same owner and mode rule as the machine file (architect F1): a file group or others can write is refused.
/// The owner half of the rule is the same check (`judge` in the fs adapter); a test cannot make a file another user
/// owns without root.
pub fn cf_f4_a_flowspace3_file_others_can_write_is_refused<P: ConfigFiles, G>(
    factory: impl Fn(Fs3) -> (P, G),
) {
    for mode in [0o664, 0o646] {
        let (port, _guard) = factory(Fs3::Mode(b"[embedder]\n", mode));
        match port.read(&ConfigFile::Flowspace3) {
            Err(ConfigFilesError::Refused { reason, .. }) => {
                assert_eq!(reason, umx_core::ports::config_files::REFUSED_WRITABLE)
            }
            other => panic!("a flowspace3 config with mode {mode:o} was read: {other:?}"),
        }
    }
}

/// What `<root>/repo/Graphfile` is when a [`GRAPHFILE_CLAUSES`] factory builds the port. `<root>/repo` holds `.git`;
/// `<root>/other` is a folder that does not.
#[derive(Clone, Copy, Debug)]
pub enum Gf {
    Missing,
    /// `Graphfile` is a symlink to a file with these bytes.
    Symlink(&'static [u8]),
}

/// The clauses for `ConfigFile::Graphfile` (ws002 s17, architect).
pub const GRAPHFILE_CLAUSES: &[&str] = &[
    "cf_g1_a_graphfile_round_trips_byte_for_byte",
    "cf_g2_a_missing_graphfile_reads_none",
    "cf_g3_a_symlinked_graphfile_is_refused",
    "cf_g4_a_graphfile_outside_the_roots_is_refused",
    "cf_g5_an_oversized_graphfile_is_refused",
    "cf_g6_swap_writes_only_over_what_was_expected",
    "cf_g7_a_graphfile_away_from_a_repo_top_is_refused",
    "cf_g8_the_graphfile_is_at_the_repo_top",
];

fn graphfile(root: &GraphDir) -> ConfigFile {
    ConfigFile::Graphfile(repo(root))
}

fn refused_for<T: core::fmt::Debug>(got: Result<T, ConfigFilesError>, want: &str, what: &str) {
    match got {
        Err(ConfigFilesError::Refused { reason, .. }) => assert_eq!(reason, want, "{what}"),
        other => panic!("{what}: not refused: {other:?}"),
    }
}

pub fn cf_g1_a_graphfile_round_trips_byte_for_byte<P: ConfigFiles, G>(
    factory: impl Fn(Gf) -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory(Gf::Missing);
    let bytes =
        b"# Graphfile\ngraphfile = 1\n\n[[graph]]\npath = \"\xc3\xa9\"\n\xe2\x9c\x93\n".to_vec();
    port.write(&graphfile(&root), &bytes).unwrap();
    assert_eq!(port.read(&graphfile(&root)).unwrap(), Some(bytes));
    port.write(&graphfile(&root), b"graphfile = 1\n").unwrap();
    assert_eq!(
        port.read(&graphfile(&root)).unwrap().as_deref(),
        Some(&b"graphfile = 1\n"[..])
    );
    assert_eq!(
        port.read(&ConfigFile::Repo(repo(&root))).unwrap(),
        None,
        "not the repo's config"
    );
}

pub fn cf_g2_a_missing_graphfile_reads_none<P: ConfigFiles, G>(
    factory: impl Fn(Gf) -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory(Gf::Missing);
    assert_eq!(port.read(&graphfile(&root)).unwrap(), None);
}

/// Never through a symlink: not read, not written, not swapped (security, s17).
pub fn cf_g3_a_symlinked_graphfile_is_refused<P: ConfigFiles, G>(
    factory: impl Fn(Gf) -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory(Gf::Symlink(b"graphfile = 1\n"));
    let symlink = umx_core::ports::config_files::REFUSED_SYMLINK;
    refused_for(port.read(&graphfile(&root)), symlink, "read");
    refused_for(port.write(&graphfile(&root), b"x"), symlink, "write");
    refused_for(
        port.swap(&graphfile(&root), Some(b"graphfile = 1\n"), b"x"),
        symlink,
        "swap",
    );
}

pub fn cf_g4_a_graphfile_outside_the_roots_is_refused<P: ConfigFiles, G>(
    factory: impl Fn(Gf) -> (P, GraphDir, G),
) {
    let (port, _root, _guard) = factory(Gf::Missing);
    let outside = ConfigFile::Graphfile(GraphDir::parse("/").unwrap());
    assert!(matches!(
        port.read(&outside),
        Err(ConfigFilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.write(&outside, b"x"),
        Err(ConfigFilesError::OutsideRoots(_))
    ));
    assert!(matches!(
        port.swap(&outside, None, b"x"),
        Err(ConfigFilesError::OutsideRoots(_))
    ));
}

/// Read up to 1 MiB, refused over it; a write is not capped here (the codec refuses to encode over the cap).
pub fn cf_g5_an_oversized_graphfile_is_refused<P: ConfigFiles, G>(
    factory: impl Fn(Gf) -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory(Gf::Missing);
    let cap = umx_core::ports::config_files::GRAPHFILE_CAP as usize;
    assert!(cap > umx_core::ports::config_files::SMALL_FILE_CAP as usize);
    port.write(&graphfile(&root), &vec![b'#'; cap]).unwrap();
    assert_eq!(
        port.read(&graphfile(&root)).unwrap().map(|b| b.len()),
        Some(cap)
    );
    port.write(&graphfile(&root), &vec![b'#'; cap + 1]).unwrap();
    refused_for(
        port.read(&graphfile(&root)).map(|b| b.map(|b| b.len())),
        umx_core::ports::config_files::REFUSED_GRAPHFILE_TOO_LARGE,
        "oversized",
    );
}

/// The race: what the caller read changed before its swap (another writer), so the swap writes nothing.
pub fn cf_g6_swap_writes_only_over_what_was_expected<P: ConfigFiles, G>(
    factory: impl Fn(Gf) -> (P, GraphDir, G),
) {
    use umx_core::ports::Swapped;
    let (port, root, _guard) = factory(Gf::Missing);
    let file = graphfile(&root);
    assert_eq!(
        port.swap(&file, None, b"one\n").unwrap(),
        Swapped::Done,
        "absent, as expected"
    );
    assert_eq!(port.read(&file).unwrap().as_deref(), Some(&b"one\n"[..]));
    assert_eq!(
        port.swap(&file, None, b"two\n").unwrap(),
        Swapped::Changed,
        "expected absent, but a file appeared"
    );
    let read = port.read(&file).unwrap();
    port.write(&file, b"another writer\n").unwrap();
    assert_eq!(
        port.swap(&file, read.as_deref(), b"mine\n").unwrap(),
        Swapped::Changed
    );
    assert_eq!(
        port.read(&file).unwrap().as_deref(),
        Some(&b"another writer\n"[..]),
        "untouched"
    );
    let now = port.read(&file).unwrap();
    assert_eq!(
        port.swap(&file, now.as_deref(), b"mine\n").unwrap(),
        Swapped::Done
    );
    assert_eq!(port.read(&file).unwrap().as_deref(), Some(&b"mine\n"[..]));
    refused_for(
        port.swap(&ConfigFile::Flowspace3, None, b"x"),
        umx_core::ports::config_files::REFUSED_READ_ONLY,
        "flowspace3",
    );
}

/// Only at a repo's top: a folder without `.git` is refused, read or write (security, s17).
pub fn cf_g7_a_graphfile_away_from_a_repo_top_is_refused<P: ConfigFiles, G>(
    factory: impl Fn(Gf) -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory(Gf::Missing);
    let other = ConfigFile::Graphfile(root.join("other").unwrap());
    let not_top = umx_core::ports::config_files::REFUSED_NOT_REPO_TOP;
    refused_for(port.read(&other), not_top, "read");
    refused_for(port.write(&other, b"x"), not_top, "write");
}

pub fn cf_g8_the_graphfile_is_at_the_repo_top<P: ConfigFiles, G>(
    factory: impl Fn(Gf) -> (P, GraphDir, G),
) {
    let (port, root, _guard) = factory(Gf::Missing);
    assert_eq!(
        port.path(&graphfile(&root)).as_str(),
        format!("{}/Graphfile", repo(&root))
    );
}

/// One `#[test]` per [`GRAPHFILE_CLAUSES`] clause plus a test that the list is exactly `GRAPHFILE_CLAUSES`.
#[macro_export]
macro_rules! config_files_graphfile_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod config_files_graphfile_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::config_files::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::config_files::GRAPHFILE_CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}

/// One `#[test]` per [`FS3_CLAUSES`] clause plus a test that the list is exactly `FS3_CLAUSES`.
#[macro_export]
macro_rules! config_files_flowspace3_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod config_files_flowspace3_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::config_files::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::config_files::FS3_CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}

/// One `#[test]` per ConfigFiles clause plus a test that the list is exactly `CLAUSES`. Usage:
/// `config_files_contract!(factory; cf_c1_<what>(), ...)`.
#[macro_export]
macro_rules! config_files_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod config_files_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::config_files::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::config_files::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
