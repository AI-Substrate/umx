//! SourceReader contract. A factory returns a fresh port, the one (canonical) root it may use, a folder outside
//! the roots, and a guard dropped after the clause. Layout under the root: `src/` (a source root) holding `a.md`,
//! `sub/`, `link_in -> src/sub`, `link_out -> <outside>` and `dangling -> <outside>/missing`; a plain file
//! `file.txt`; and `link_root -> <outside>`. `<outside>` exists.

use umx_core::ports::{GraphDir, SourceError, SourceReader};

pub const CLAUSES: &[&str] = &[
    "sr_c1_a_root_inside_is_canonical",
    "sr_c2_a_root_outside_the_roots_is_refused",
    "sr_c3_a_root_that_is_a_file_or_missing_is_not_a_folder",
    "sr_c4_dotdot_and_absolute_paths_are_refused",
    "sr_c5_a_symlink_out_is_refused",
    "sr_c6_a_missing_tail_is_fine",
];

fn at(root: &GraphDir, rel: &str) -> GraphDir {
    root.join(rel).expect("a path under the root")
}

pub fn sr_c1_a_root_inside_is_canonical<P: SourceReader, G>(
    factory: impl Fn() -> (P, GraphDir, GraphDir, G),
) {
    let (port, root, _outside, _guard) = factory();
    assert_eq!(port.canonical_root(&at(&root, "src")), Ok(at(&root, "src")));
    assert_eq!(
        port.canonical_root(&at(&root, "src/link_in")),
        Ok(at(&root, "src/sub")),
        "a symlink inside the roots resolves to its target"
    );
}

pub fn sr_c2_a_root_outside_the_roots_is_refused<P: SourceReader, G>(
    factory: impl Fn() -> (P, GraphDir, GraphDir, G),
) {
    let (port, root, outside, _guard) = factory();
    for folder in [
        outside.clone(),
        at(&root, "link_root"),
        GraphDir::parse("/").unwrap(),
        at(&outside, "missing"),
    ] {
        assert_eq!(
            port.canonical_root(&folder),
            Err(SourceError::OutsideRoots),
            "{folder}"
        );
    }
}

pub fn sr_c3_a_root_that_is_a_file_or_missing_is_not_a_folder<P: SourceReader, G>(
    factory: impl Fn() -> (P, GraphDir, GraphDir, G),
) {
    let (port, root, _outside, _guard) = factory();
    for folder in [at(&root, "file.txt"), at(&root, "missing")] {
        assert_eq!(
            port.canonical_root(&folder),
            Err(SourceError::NotFolder),
            "{folder}"
        );
    }
}

pub fn sr_c4_dotdot_and_absolute_paths_are_refused<P: SourceReader, G>(
    factory: impl Fn() -> (P, GraphDir, GraphDir, G),
) {
    let (port, root, _outside, _guard) = factory();
    let src = at(&root, "src");
    for path in ["../file.txt", "sub/../../file.txt", "..", "/etc/passwd"] {
        assert_eq!(
            port.check_path(&src, path),
            Err(SourceError::OutsideRoots),
            "{path:?}"
        );
    }
}

pub fn sr_c5_a_symlink_out_is_refused<P: SourceReader, G>(
    factory: impl Fn() -> (P, GraphDir, GraphDir, G),
) {
    let (port, root, _outside, _guard) = factory();
    let src = at(&root, "src");
    for path in ["link_out", "link_out/x.md", "dangling", "dangling/x.md"] {
        assert_eq!(
            port.check_path(&src, path),
            Err(SourceError::OutsideRoots),
            "{path}"
        );
    }
    assert_eq!(port.check_path(&src, "link_in/x.md"), Ok(()));
}

pub fn sr_c6_a_missing_tail_is_fine<P: SourceReader, G>(
    factory: impl Fn() -> (P, GraphDir, GraphDir, G),
) {
    let (port, root, _outside, _guard) = factory();
    let src = at(&root, "src");
    for path in ["a.md", "sub", "sub/new.md", "new/deeper/file.md", "a.md/x"] {
        assert_eq!(port.check_path(&src, path), Ok(()), "{path}");
    }
}

/// One `#[test]` per SourceReader clause plus a test that the list is exactly `CLAUSES`. Usage:
/// `source_reader_contract!(factory; sr_c1_<what>(), ...)`.
#[macro_export]
macro_rules! source_reader_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod source_reader_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::source_reader::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::source_reader::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
