//! Embedder contract. A factory builds the adapter for a `Reach`: `Online` can embed; `Offline` cannot reach its
//! provider (for a real adapter: no network or a dead endpoint), and must say so with `Unavailable`.
//!
//! Hosted models are not bit-for-bit repeatable, so "the same vector" means cosine at least `SAME`.

use umx_core::ports::{EmbedError, Embedder, TextKind};

pub const CLAUSES: &[&str] = &[
    "em_c1_one_vector_per_input_in_order",
    "em_c2_width_is_key_dims",
    "em_c3_vectors_are_unit_length",
    "em_c4_same_text_and_kind_give_the_same_vector",
    "em_c5_empty_batch_is_empty_even_offline",
    "em_c6_distinct_texts_give_distinct_vectors",
    "em_c7_identical_texts_in_one_batch_match",
    "em_c8_blank_input_is_refused_by_index",
    "em_c9_offline_is_unavailable",
    "em_c10_over_cap_input_is_refused_or_reported",
    "em_c11_ready_is_ok_online",
    "em_c12_a_hosted_adapter_names_its_credential_after_ready",
    "em_c13_reported_tokens_are_none_or_count_the_batch",
];

/// `em_c10` builds an input longer than `max_input_tokens()` and is skipped above this cap, where that input would
/// cost too much on every run. Adapters that can, configure a small cap in their contract factory.
pub const LONG_INPUT_CAP: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Online,
    Offline,
}

/// Cosine at or above which two vectors count as the same.
pub const SAME: f32 = 0.999;

const TEXTS: [&str; 4] = [
    "The cat sat on the warm mat.",
    "Quarterly revenue grew by twelve percent.",
    "Photosynthesis turns sunlight into sugar.",
    "A distributed lock prevents two writers.",
];

fn texts(words: &[&str]) -> Vec<String> {
    words.iter().map(|t| t.to_string()).collect()
}

fn embed<P: Embedder>(port: &P, words: &[&str], kind: TextKind) -> Vec<Vec<f32>> {
    let out = port.embed(&texts(words), kind).expect("embed");
    assert!(out.truncated.is_empty(), "short texts are never truncated");
    out.vectors
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    dot / (norm(a) * norm(b))
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

pub fn em_c1_one_vector_per_input_in_order<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let port = factory(Reach::Online);
    for kind in [TextKind::Document, TextKind::Query] {
        let batch = embed(&port, &TEXTS, kind);
        assert_eq!(batch.len(), TEXTS.len());
        for (i, text) in TEXTS.iter().enumerate() {
            let alone = embed(&port, &[text], kind);
            assert!(
                cosine(&batch[i], &alone[0]) >= SAME,
                "{kind:?} input {i} out of place"
            );
        }
    }
}

pub fn em_c2_width_is_key_dims<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let port = factory(Reach::Online);
    let dims = port.key().dims();
    for kind in [TextKind::Document, TextKind::Query] {
        for v in embed(&port, &TEXTS, kind) {
            assert_eq!(v.len(), dims);
        }
    }
}

pub fn em_c3_vectors_are_unit_length<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let port = factory(Reach::Online);
    for kind in [TextKind::Document, TextKind::Query] {
        for v in embed(&port, &TEXTS, kind) {
            assert!(v.iter().all(|x| x.is_finite()));
            assert!((norm(&v) - 1.0).abs() < 1e-3, "norm {}", norm(&v));
        }
    }
}

/// Holds for each kind separately and across two instances; a document and a query vector of one text may differ.
pub fn em_c4_same_text_and_kind_give_the_same_vector<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let (one, two) = (factory(Reach::Online), factory(Reach::Online));
    for kind in [TextKind::Document, TextKind::Query] {
        let a = embed(&one, &TEXTS, kind);
        let b = embed(&one, &TEXTS, kind);
        let c = embed(&two, &TEXTS, kind);
        for i in 0..TEXTS.len() {
            assert!(
                cosine(&a[i], &b[i]) >= SAME,
                "{kind:?} {i} changed between calls"
            );
            assert!(
                cosine(&a[i], &c[i]) >= SAME,
                "{kind:?} {i} changed between instances"
            );
        }
    }
    assert_eq!(one.key(), two.key());
}

/// An empty batch answers empty, online and offline. This shows a real adapter did not need its provider (an
/// offline call would fail); it cannot see a call that returns nothing, so an adapter with a call counter (the
/// fake's `calls()`) proves "no call" in its own tests.
pub fn em_c5_empty_batch_is_empty_even_offline<P: Embedder>(factory: impl Fn(Reach) -> P) {
    for reach in [Reach::Online, Reach::Offline] {
        let out = factory(reach)
            .embed(&[], TextKind::Document)
            .expect("an empty batch");
        assert!(
            out.vectors.is_empty() && out.truncated.is_empty(),
            "{reach:?}"
        );
    }
}

pub fn em_c6_distinct_texts_give_distinct_vectors<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let v = embed(&factory(Reach::Online), &TEXTS, TextKind::Document);
    for i in 0..v.len() {
        for j in i + 1..v.len() {
            assert!(cosine(&v[i], &v[j]) < SAME, "inputs {i} and {j} collide");
        }
    }
}

pub fn em_c7_identical_texts_in_one_batch_match<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let v = embed(
        &factory(Reach::Online),
        &[TEXTS[0], TEXTS[0], TEXTS[0]],
        TextKind::Query,
    );
    assert_eq!(v.len(), 3);
    assert!(cosine(&v[0], &v[1]) >= SAME && cosine(&v[0], &v[2]) >= SAME);
}

pub fn em_c8_blank_input_is_refused_by_index<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let port = factory(Reach::Online);
    for (blank, index) in [("", 0), (" \t\n ", 2)] {
        let mut batch = texts(&TEXTS[..3]);
        batch[index] = blank.to_string();
        assert_eq!(
            port.embed(&batch, TextKind::Document),
            Err(EmbedError::EmptyInput { index })
        );
    }
}

pub fn em_c9_offline_is_unavailable<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let port = factory(Reach::Offline);
    match port.embed(&texts(&TEXTS), TextKind::Document) {
        Err(EmbedError::Unavailable { next, .. }) => {
            assert!(!next.trim().is_empty(), "a next hint")
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

/// An input over the cap is refused (`InputTooLong` naming it) or embedded and listed in `truncated`; never cut
/// silently. Other inputs in the batch are not reported.
pub fn em_c10_over_cap_input_is_refused_or_reported<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let port = factory(Reach::Online);
    let cap = port.max_input_tokens();
    if cap > LONG_INPUT_CAP {
        eprintln!("em_c10 skipped: max_input_tokens {cap} > {LONG_INPUT_CAP}");
        return;
    }
    // Every word is at least one token, so cap + 1 words is over the cap for any tokenizer.
    let long: Vec<String> = (0..=cap).map(|i| format!("w{i}")).collect();
    let batch = vec![TEXTS[0].to_string(), long.join(" "), TEXTS[1].to_string()];
    match port.embed(&batch, TextKind::Document) {
        Err(EmbedError::InputTooLong { index }) => assert_eq!(index, 1),
        Ok(out) => {
            assert_eq!(out.truncated, vec![1], "the long input must be reported");
            assert_eq!(out.vectors.len(), 3);
        }
        Err(other) => panic!("expected InputTooLong or a truncation report, got {other:?}"),
    }
}

/// `ready()` on a reachable provider with credentials is Ok, every time it is asked.
pub fn em_c11_ready_is_ok_online<P: Embedder>(factory: impl Fn(Reach) -> P) {
    let port = factory(Reach::Online);
    for _ in 0..2 {
        if let Err(e) = port.ready() {
            panic!("ready online: {e}");
        }
    }
}

/// After `ready()` succeeds online, an adapter that sends text off the machine names the credential that worked
/// (`credential()` is `Some`, with a kind and where it came from); a local one may say none. Each adapter's own tests
/// add that `from` never holds the secret, since only they know its value.
pub fn em_c12_a_hosted_adapter_names_its_credential_after_ready<P: Embedder>(
    factory: impl Fn(Reach) -> P,
) {
    let port = factory(Reach::Online);
    if let Err(e) = port.ready() {
        panic!("ready online: {e}");
    }
    if port.locality() == umx_core::ports::Locality::Hosted {
        let credential = port
            .credential()
            .expect("a hosted adapter names its credential after ready()");
        assert!(!credential.kind.is_empty() && !credential.from.is_empty());
    }
}

/// What the provider counted for a batch (`Embedded::tokens`, ws004 s25) corrects the pace's charge: `None` is always
/// valid (local models, providers that do not say), and `Some(n)` for a non-empty batch is at least one token, since a
/// count of none would let the pace send for free.
pub fn em_c13_reported_tokens_are_none_or_count_the_batch<P: Embedder>(
    factory: impl Fn(Reach) -> P,
) {
    let port = factory(Reach::Online);
    for kind in [TextKind::Document, TextKind::Query] {
        let out = port.embed(&texts(&TEXTS), kind).expect("embed");
        if let Some(n) = out.tokens {
            assert!(
                n >= 1,
                "{kind:?}: a batch of {} texts counted {n} tokens",
                TEXTS.len()
            );
        }
    }
}

/// `ready()` sends no text, ready or not. `sent` counts the texts the adapter has sent its provider so far (the
/// fake's counter, or the request bodies a stub server recorded). Each adapter's tests call it for both reaches,
/// because only the adapter's own test can see what left it.
pub fn ready_sends_no_text<P: Embedder>(port: &P, sent: impl Fn() -> usize) {
    let before = sent();
    let _ = port.ready();
    assert_eq!(sent(), before, "ready() sent text to the provider");
}

/// One `#[test]` per Embedder clause plus a test that fails if the list is not exactly `CLAUSES`. Usage:
/// `embedder_contract!(factory; em_c1_<what>(), ...)` where `factory: Fn(Reach) -> impl Embedder`, each clause
/// written as a call so umx-arch's r-fake-per-port token scan sees it.
#[macro_export]
macro_rules! embedder_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod embedder_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::embedder::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::embedder::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
