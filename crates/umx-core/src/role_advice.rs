//! Whether a declared string field sits in the wrong role group, judged from its values alone (search-05). Pure: the
//! caller reads the values; this only counts them. umx never changes a role itself: the advice becomes a hint naming
//! the `umx ontology set field` command, and the role stays as declared until someone runs it.
//!
//! Two mistakes, both seen in real graphs:
//! - labels declared content: a field holding a handful of short values repeated across many nodes (a category, a
//!   tier). Each value is embedded once per node, every one an embedder call, and the vectors flood search by meaning
//!   with near-identical matches. On one 536-node graph, label fields held 666 of 2,283 vectors (29%); one of them had
//!   3 distinct values and 536 vectors.
//! - prose declared data: a field of sentences, mostly different from node to node, is never found by meaning.
//!
//! Only `string` fields are judged: numbers, dates, yes/no and lists are always data, and a field umx never embeds
//! (provenance and identity keys) is never advised to become content.

use alloc::collections::BTreeSet;

use crate::ontology::{FieldDecl, FieldKind, Role, never_embedded_field};

/// At most this many distinct values for a content field to read as labels. A label set is a handful of words: the
/// case above had 3; twelve leaves room for a priority scale, a set of regions or of tiers.
pub const LABEL_MAX_DISTINCT: usize = 12;
/// Every value of a label field is at most this many characters: a word or a short phrase, not a sentence.
pub const LABEL_MAX_CHARS: usize = 40;
/// At least this many nodes hold a value before a field is judged a label field: below it the repeats prove nothing.
pub const LABEL_MIN_VALUES: usize = 6;
/// A value with at least this many words reads as prose: a sentence, not a name or a label.
pub const PROSE_MIN_WORDS: usize = 8;
/// At least this many nodes hold a value before a data field is judged prose.
pub const PROSE_MIN_VALUES: usize = 3;

/// What the values say about a field's role, with the counts a hint shows (never a value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleAdvice {
    /// A content field that holds labels: make it data. `values` is how many nodes hold a value, so how many field
    /// vectors (each an embedder call) it holds or will hold; `distinct` how many different values there are.
    BecomeData { values: usize, distinct: usize },
    /// A data field that holds prose: make it content. `long` of its `values` read as prose.
    BecomeContent { values: usize, long: usize },
}

/// Advice for `field` from the values its nodes hold (empty and blank values are not counted), or None when its
/// role fits them or there are too few to tell.
pub fn role_advice<'a>(
    field: &FieldDecl,
    values: impl IntoIterator<Item = &'a str>,
) -> Option<RoleAdvice> {
    if field.kind != FieldKind::String || never_embedded_field(&field.name) {
        return None;
    }
    let values: alloc::vec::Vec<&str> = values
        .into_iter()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect();
    let distinct = values.iter().collect::<BTreeSet<_>>().len();
    match field.effective_role() {
        Role::Content => {
            let labels = values.len() >= LABEL_MIN_VALUES
                && distinct <= LABEL_MAX_DISTINCT
                && distinct * 2 <= values.len()
                && values.iter().all(|v| v.chars().count() <= LABEL_MAX_CHARS);
            labels.then_some(RoleAdvice::BecomeData {
                values: values.len(),
                distinct,
            })
        }
        Role::Data => {
            let long = values
                .iter()
                .filter(|v| v.split_whitespace().count() >= PROSE_MIN_WORDS)
                .count();
            let prose = values.len() >= PROSE_MIN_VALUES
                && long * 2 > values.len()
                && distinct * 5 >= values.len() * 4;
            prose.then_some(RoleAdvice::BecomeContent {
                values: values.len(),
                long,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    fn field(name: &str, kind: FieldKind, role: Option<Role>) -> FieldDecl {
        let mut f = FieldDecl::new(name, kind);
        f.role = role;
        f
    }

    fn content(name: &str) -> FieldDecl {
        field(name, FieldKind::String, Some(Role::Content))
    }

    fn data(name: &str) -> FieldDecl {
        field(name, FieldKind::String, Some(Role::Data))
    }

    fn prose(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("the supplier asked for a longer review window in round {i}"))
            .collect()
    }

    #[test]
    fn three_labels_over_many_nodes_should_be_data_and_say_the_calls_saved() {
        let values: Vec<&str> = ["gold", "silver", "bronze"]
            .iter()
            .cycle()
            .take(30)
            .copied()
            .collect();
        assert_eq!(
            role_advice(&content("tier"), values),
            Some(RoleAdvice::BecomeData {
                values: 30,
                distinct: 3
            })
        );
    }

    #[test]
    fn a_string_field_with_no_role_is_content_by_default_so_its_labels_are_pointed_out() {
        let values: Vec<&str> = ["a", "b"].iter().cycle().take(8).copied().collect();
        let f = field("tier", FieldKind::String, None);
        assert_eq!(
            role_advice(&f, values),
            Some(RoleAdvice::BecomeData {
                values: 8,
                distinct: 2
            })
        );
    }

    #[test]
    fn prose_declared_data_should_be_content() {
        let values = prose(5);
        assert_eq!(
            role_advice(&data("summary"), values.iter().map(String::as_str)),
            Some(RoleAdvice::BecomeContent { values: 5, long: 5 })
        );
    }

    #[test]
    fn a_field_whose_role_fits_its_values_gets_no_advice() {
        let labels: Vec<&str> = ["a", "b", "c"].iter().cycle().take(30).copied().collect();
        assert_eq!(role_advice(&data("tier"), labels), None);
        let values = prose(5);
        assert_eq!(
            role_advice(&content("summary"), values.iter().map(String::as_str)),
            None
        );
    }

    #[test]
    fn an_empty_field_or_one_value_says_nothing() {
        assert_eq!(role_advice(&content("tier"), []), None);
        assert_eq!(role_advice(&content("tier"), ["gold"]), None);
        assert_eq!(
            role_advice(
                &data("summary"),
                ["one long sentence that has well over eight words in it"]
            ),
            None
        );
        assert_eq!(
            role_advice(&content("tier"), ["", "  ", "", "", "", "", ""]),
            None
        );
    }

    #[test]
    fn numbers_dates_bools_and_lists_are_never_advised() {
        let labels: Vec<&str> = ["1", "2"].iter().cycle().take(20).copied().collect();
        for kind in [
            FieldKind::Int,
            FieldKind::Float,
            FieldKind::Bool,
            FieldKind::Date,
            FieldKind::List,
        ] {
            assert_eq!(
                role_advice(&field("n", kind, None), labels.iter().copied()),
                None,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_never_embedded_field_is_never_advised_to_become_content() {
        let values = prose(5);
        assert_eq!(
            role_advice(&data("prov_key"), values.iter().map(String::as_str)),
            None
        );
    }

    #[test]
    fn names_and_ids_are_not_labels_because_they_do_not_repeat() {
        let names: Vec<String> = (0..20).map(|i| format!("doc-{i}.md")).collect();
        assert_eq!(
            role_advice(&content("file"), names.iter().map(String::as_str)),
            None
        );
    }

    #[test]
    fn a_long_value_among_short_ones_means_it_is_not_a_label_field() {
        let mut values: Vec<&str> = ["gold", "silver"]
            .iter()
            .cycle()
            .take(10)
            .copied()
            .collect();
        values.push("a value far longer than any label would ever be in practice");
        assert_eq!(role_advice(&content("tier"), values), None);
    }

    #[test]
    fn the_limits_are_where_the_constants_say() {
        // LABEL_MIN_VALUES: one below says nothing.
        let few: Vec<&str> = ["a"]
            .iter()
            .cycle()
            .take(LABEL_MIN_VALUES - 1)
            .copied()
            .collect();
        assert_eq!(role_advice(&content("tier"), few), None);
        let enough: Vec<&str> = ["a"]
            .iter()
            .cycle()
            .take(LABEL_MIN_VALUES)
            .copied()
            .collect();
        assert!(role_advice(&content("tier"), enough).is_some());
        // LABEL_MAX_DISTINCT: one more distinct value is not a label set.
        let labels: Vec<String> = (0..=LABEL_MAX_DISTINCT).map(|i| format!("l{i}")).collect();
        let many: Vec<&str> = labels
            .iter()
            .map(String::as_str)
            .cycle()
            .take(labels.len() * 3)
            .collect();
        assert_eq!(role_advice(&content("tier"), many), None);
        // PROSE_MIN_WORDS: a value one word short is not prose.
        let short = "one two three four five six seven";
        assert_eq!(
            role_advice(&data("summary"), [short, short, short, short]),
            None
        );
        // Mostly distinct: prose that repeats is a boilerplate label, not prose.
        let same = "the supplier asked for a longer review window this time";
        assert_eq!(
            role_advice(&data("summary"), [same, same, same, same]),
            None
        );
    }
}
