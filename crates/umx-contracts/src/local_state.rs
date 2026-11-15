//! LocalState contract. A factory returns a fresh port with no state file and a guard dropped after the clause;
//! `spoil` makes the existing file untrusted (as a symlink or a file open to others would be); `remove` deletes the
//! file and leaves its folder.

use umx_core::ports::{LocalState, LocalStateError};

pub const CLAUSES: &[&str] = &[
    "ls_c1_write_then_read_is_identical",
    "ls_c2_read_missing_is_none",
    "ls_c3_an_untrusted_file_is_neither_read_nor_written",
    "ls_c4_path_names_the_state_file",
    "ls_c5_a_removed_file_is_refused_not_empty",
    "ls_c6_an_oversized_file_is_refused",
];

pub fn ls_c1_write_then_read_is_identical<P: LocalState, G>(
    factory: impl Fn() -> (P, G),
    _spoil: impl Fn(&P),
    _remove: impl Fn(&P),
) {
    let (port, _guard) = factory();
    let bytes = b"{\"umx_state\": 1, \"graphs\": []}\n\xe2\x9c\x93\n".to_vec();
    port.write(&bytes).unwrap();
    assert_eq!(port.read().unwrap(), Some(bytes));
    // A shorter write replaces the whole file: no tail of the old one is left.
    port.write(b"{}\n").unwrap();
    assert_eq!(port.read().unwrap().as_deref(), Some(&b"{}\n"[..]));
}

pub fn ls_c2_read_missing_is_none<P: LocalState, G>(
    factory: impl Fn() -> (P, G),
    _spoil: impl Fn(&P),
    _remove: impl Fn(&P),
) {
    let (port, _guard) = factory();
    assert_eq!(port.read().unwrap(), None);
}

pub fn ls_c3_an_untrusted_file_is_neither_read_nor_written<P: LocalState, G>(
    factory: impl Fn() -> (P, G),
    spoil: impl Fn(&P),
    _remove: impl Fn(&P),
) {
    let (port, _guard) = factory();
    port.write(b"kept\n").unwrap();
    spoil(&port);
    assert!(matches!(port.read(), Err(LocalStateError::Refused { .. })));
    assert!(matches!(
        port.write(b"replaced\n"),
        Err(LocalStateError::Refused { .. })
    ));
}

pub fn ls_c4_path_names_the_state_file<P: LocalState, G>(
    factory: impl Fn() -> (P, G),
    _spoil: impl Fn(&P),
    _remove: impl Fn(&P),
) {
    let (port, _guard) = factory();
    assert!(
        port.path().as_str().ends_with("/state/graph-policies.json"),
        "{}",
        port.path()
    );
}

/// Deleting the file (by hand, by an agent) must not read as "no record yet": that would lower every secured graph.
pub fn ls_c5_a_removed_file_is_refused_not_empty<P: LocalState, G>(
    factory: impl Fn() -> (P, G),
    _spoil: impl Fn(&P),
    remove: impl Fn(&P),
) {
    let (port, _guard) = factory();
    port.write(b"{}\n").unwrap();
    remove(&port);
    assert!(matches!(port.read(), Err(LocalStateError::Refused { .. })));
}

/// F45: a state file over SMALL_FILE_CAP is refused, never read whole (the graph store then treats every graph as
/// secured).
pub fn ls_c6_an_oversized_file_is_refused<P: LocalState, G>(
    factory: impl Fn() -> (P, G),
    _spoil: impl Fn(&P),
    _remove: impl Fn(&P),
) {
    let (port, _guard) = factory();
    port.write(&vec![
        b' ';
        umx_core::ports::config_files::SMALL_FILE_CAP as usize
            + 1
    ])
    .unwrap();
    match port.read() {
        Err(LocalStateError::Refused { reason, .. }) => {
            assert_eq!(reason, umx_core::ports::config_files::REFUSED_TOO_LARGE)
        }
        other => panic!(
            "an oversized state file was not refused: {:?}",
            other.map(|b| b.map(|b| b.len()))
        ),
    }
}

/// One `#[test]` per LocalState clause plus a test that the list is exactly `CLAUSES`. Usage:
/// `local_state_contract!(factory, spoil, remove; ls_c1_<what>(), ...)`.
#[macro_export]
macro_rules! local_state_contract {
    ($factory:expr, $spoil:expr, $remove:expr; $($clause:ident()),+ $(,)?) => {
        mod local_state_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::local_state::$clause($factory, $spoil, $remove)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::local_state::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
