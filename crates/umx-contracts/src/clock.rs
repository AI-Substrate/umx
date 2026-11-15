//! Clock contract.

use umx_core::ports::Clock;

pub const CLAUSES: &[&str] = &[
    "ck_c1_now_is_a_plausible_epoch_ms",
    "ck_c2_now_never_goes_backwards",
];

pub fn ck_c1_now_is_a_plausible_epoch_ms<P: Clock>(factory: impl Fn() -> P) {
    let now = factory().now_ms();
    // 2020-01-01 to 2200-01-01: catches seconds or microseconds passed off as milliseconds.
    assert!(
        (1_577_836_800_000..7_258_118_400_000).contains(&now),
        "{now}"
    );
}

pub fn ck_c2_now_never_goes_backwards<P: Clock>(factory: impl Fn() -> P) {
    let clock = factory();
    let mut last = clock.now_ms();
    for _ in 0..1000 {
        let now = clock.now_ms();
        assert!(now >= last);
        last = now;
    }
}

/// One `#[test]` per Clock clause, named in the call so r-fake-per-port finds each name in the caller's source,
/// plus a test that fails if the list is not exactly `CLAUSES`. Usage: `clock_contract!(factory; <prefix>_c1_<what>(), ...)`, each clause written as a call so umx-arch's r-fake-per-port token scan sees it.
#[macro_export]
macro_rules! clock_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod clock_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::clock::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::clock::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
