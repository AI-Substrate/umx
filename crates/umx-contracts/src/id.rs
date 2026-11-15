//! IdGenerator contract.

use std::collections::BTreeSet;
use std::sync::Arc;
use umx_core::ports::IdGenerator;

pub const CLAUSES: &[&str] = &[
    "id_c1_ids_are_unique",
    "id_c2_each_id_is_greater_than_the_last",
    "id_c3_threads_share_one_sequence",
];

pub fn id_c1_ids_are_unique<P: IdGenerator>(factory: impl Fn() -> P) {
    let ids = factory();
    let set: BTreeSet<_> = (0..10_000).map(|_| ids.next_id()).collect();
    assert_eq!(set.len(), 10_000);
}

pub fn id_c2_each_id_is_greater_than_the_last<P: IdGenerator>(factory: impl Fn() -> P) {
    let ids = factory();
    let mut last = ids.next_id();
    for _ in 0..10_000 {
        let next = ids.next_id();
        assert!(next > last, "{next} after {last}");
        assert!(next.to_text() > last.to_text(), "text order is id order");
        last = next;
    }
}

pub fn id_c3_threads_share_one_sequence<P: IdGenerator + 'static>(factory: impl Fn() -> P) {
    let ids = Arc::new(factory());
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let ids = Arc::clone(&ids);
            std::thread::spawn(move || (0..1000).map(|_| ids.next_id()).collect::<Vec<_>>())
        })
        .collect();
    let mut all = BTreeSet::new();
    for h in handles {
        let mine = h.join().unwrap();
        assert!(mine.windows(2).all(|w| w[0] < w[1]));
        all.extend(mine);
    }
    assert_eq!(all.len(), 4000);
}

/// One `#[test]` per IdGenerator clause, named in the call so r-fake-per-port finds each name in the caller's source,
/// plus a test that fails if the list is not exactly `CLAUSES`. Usage: `id_generator_contract!(factory; <prefix>_c1_<what>(), ...)`, each clause written as a call so umx-arch's r-fake-per-port token scan sees it.
#[macro_export]
macro_rules! id_generator_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod id_generator_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::id::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::id::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
