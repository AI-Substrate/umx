//! This machine's lint terms (F139, security's shape): `$UMX_HOME/lint/lint-terms.json`, a list of words to keep out
//! of graphs, read through a LocalState port instance of its own (owner-only 0600 in a 0700 folder; a group- or
//! world-readable file is refused). A person writes it by hand; umx only reads it, fresh on each `check --lint`. It
//! is never in a graph folder, in config, on the socket or in a log, and a hit never names the term or the text.
//!
//! Format: `{"umx_lint_terms": 1, "terms": [{"term": "<text>", "graphs": ["<graph id>", ...]}]}`. `graphs` left out
//! means every graph. Any other shape, a newer version, a term outside the length bounds or more than [`MAX_TERMS`]
//! makes the whole file unusable: the scan is skipped with one hint naming the file.

use crate::id::Id;
use alloc::string::String;
use alloc::vec::Vec;
use serde::Deserialize;

pub const LINT_TERMS_VERSION: u32 = 1;
/// The owner-only folder under `$UMX_HOME` that holds the terms file.
pub const LINT_DIR: &str = "lint";
pub const LINT_FILE: &str = "lint-terms.json";
/// At most this many terms (security).
pub const MAX_TERMS: usize = 256;
/// A term's length bounds, in characters.
pub const MIN_TERM_CHARS: usize = 2;
pub const MAX_TERM_CHARS: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    umx_lint_terms: u32,
    terms: Vec<LintTerm>,
}

/// One term, and the graphs it applies to (None: every graph).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintTerm {
    pub term: String,
    #[serde(default)]
    pub graphs: Option<Vec<Id>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LintTerms {
    pub terms: Vec<LintTerm>,
}

impl LintTerms {
    /// The file's terms, or None when it cannot be used (see the module docs).
    pub fn decode(bytes: &[u8]) -> Option<LintTerms> {
        let raw: Raw = serde_json::from_slice(bytes).ok()?;
        let fits = |t: &LintTerm| {
            let n = t.term.chars().count();
            (MIN_TERM_CHARS..=MAX_TERM_CHARS).contains(&n) && t.term.trim() == t.term
        };
        (raw.umx_lint_terms == LINT_TERMS_VERSION
            && raw.terms.len() <= MAX_TERMS
            && raw.terms.iter().all(fits))
        .then_some(LintTerms { terms: raw.terms })
    }

    /// The terms that apply to graph `id`.
    pub fn for_graph(&self, id: Id) -> impl Iterator<Item = &str> {
        self.terms
            .iter()
            .filter(move |t| t.graphs.as_ref().is_none_or(|g| g.contains(&id)))
            .map(|t| t.term.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usable_file_names_its_terms_and_their_graphs_and_anything_else_is_unusable() {
        let g = Id(7);
        let ok = LintTerms::decode(
            br#"{"umx_lint_terms":1,"terms":[{"term":"Zorblax"},{"term":"Quillon","graphs":["00000000000000000000000007"]}]}"#,
        )
        .unwrap();
        assert_eq!(ok.for_graph(g).collect::<Vec<_>>(), ["Zorblax", "Quillon"]);
        assert_eq!(ok.for_graph(Id(8)).collect::<Vec<_>>(), ["Zorblax"]);
        let long = alloc::format!(
            r#"{{"umx_lint_terms":1,"terms":[{{"term":"{}"}}]}}"#,
            "x".repeat(MAX_TERM_CHARS + 1)
        );
        let many = alloc::format!(
            r#"{{"umx_lint_terms":1,"terms":[{}]}}"#,
            alloc::vec![r#"{"term":"ab"}"#; MAX_TERMS + 1].join(",")
        );
        for bad in [
            &br#"{"umx_lint_terms":2,"terms":[]}"#[..],
            br#"{"umx_lint_terms":1,"terms":[],"more":1}"#,
            br#"{"umx_lint_terms":1,"terms":[{"term":"a"}]}"#,
            br#"{"umx_lint_terms":1,"terms":[{"term":" padded"}]}"#,
            br#"{"umx_lint_terms":1,"terms":[{"term":"ok","zone":"x"}]}"#,
            br#"not json"#,
            long.as_bytes(),
            many.as_bytes(),
        ] {
            assert_eq!(
                LintTerms::decode(bad),
                None,
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
