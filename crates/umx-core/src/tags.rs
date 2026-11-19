//! Std tags (ws008 t2b, req-117): the std `tags` field ([`TAGS`]) is a list on any node of a std graph. A tag is
//! `<value>` or `<facet>:<value>`, each part snake_case (`[a-z][a-z0-9_]{0,63}`). Tags are stored lowercased,
//! deduplicated and sorted, so their order never makes a git diff. This module is the one place a tag is checked and
//! put in stored form: the writes, the load warnings, the tag index and the `--tag` filters all read it, so the index
//! holds exactly what a read compares.
//!
//! A graph declares tags with `umx ontology add tag <facet>:<value>` or `<facet>:*` ([`Ontology::tags`]). Tags are
//! free-form until a facet has a declared tag; then an undeclared tag in that facet is off the vocabulary (a
//! warning, never a refusal).
//!
//! [`Ontology::tags`]: crate::ontology::Ontology::tags

use crate::name::is_snake_case;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::Value;

/// The std list field that holds a node's tags.
pub const TAGS: &str = crate::standard::TAGS;

/// The most tags one node holds: more is refused at write and ignored past this at load.
pub const MAX_TAGS: usize = 64;

/// The facet wildcard: `<facet>:*` declares (or, in a filter, matches) every value of a facet.
pub const ANY_VALUE: &str = "*";

/// Why a tag was refused, with the tag in a form that would be accepted when there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRefused {
    pub reason: &'static str,
    /// The snake_case form, as `(facet, value)`.
    pub fixed: Option<(Option<String>, String)>,
}

impl TagRefused {
    fn new(reason: &'static str, fixed: Option<(Option<String>, String)>) -> TagRefused {
        TagRefused { reason, fixed }
    }
}

/// A tag's parts: its facet (if any) and its value. Both are snake_case once checked.
pub fn split(tag: &str) -> (Option<&str>, &str) {
    match tag.split_once(':') {
        Some((facet, value)) => (Some(facet), value),
        None => (None, tag),
    }
}

/// One tag in stored form: trimmed and lowercased, then each part must be snake_case. A refusal carries the
/// snake_case form when one can be made (`origin:Double-Diamond` is refused with `origin:double_diamond`).
pub fn normalise_tag(given: &str) -> Result<String, TagRefused> {
    let tag = given.trim().to_ascii_lowercase();
    if tag.is_empty() {
        return Err(TagRefused::new("a tag is empty", None));
    }
    let (facet, value) = split(&tag);
    if facet.is_some_and(|f| !is_snake_case(f)) || !is_snake_case(value) {
        let fixed = match facet {
            Some(f) => snake(f).zip(snake(value)).map(|(f, v)| (Some(f), v)),
            None => snake(value).map(|v| (None, v)),
        };
        let reason = if value.contains(':') {
            "a tag has at most one facet"
        } else {
            "each part of a tag must be a lowercase letter then up to 63 lowercase letters digits or underscores"
        };
        return Err(TagRefused::new(reason, fixed));
    }
    Ok(tag)
}

/// A part in snake_case: lowercased, every run of other characters one `_`, leading digits and `_` dropped, at most
/// 64 characters. None if nothing usable remains.
fn snake(part: &str) -> Option<String> {
    let mut out = snake_whole(part);
    out.truncate(64);
    while out.ends_with('_') {
        out.pop();
    }
    is_snake_case(&out).then_some(out)
}

/// Whether [`snake`] would cut `part` to fit a tag: its snake_case form, uncut, is longer than 64 characters.
pub fn snake_cut(part: &str) -> bool {
    snake_whole(part).trim_end_matches('_').len() > 64
}

/// `part` lowercased with every run of other characters one `_`, leading digits and `_` dropped, uncut.
fn snake_whole(part: &str) -> String {
    let mut out = String::new();
    for c in part.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            if out.is_empty() && c.is_ascii_digit() {
                continue;
            }
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out
}

/// Why a tags value was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagsRefused {
    /// One tag is not a tag.
    Tag(TagRefused),
    /// An element of the list is not a string.
    NotAString,
    /// More than [`MAX_TAGS`] distinct tags.
    TooMany(usize),
}

/// The tags a value gives, in stored form: each normalised, then sorted with repeats dropped. A string is one tag or
/// several, comma separated; a list must hold only strings. Empty entries are skipped. A bad tag refuses the whole
/// value.
pub fn normalise_tags(value: &Value) -> Result<Vec<String>, TagsRefused> {
    let given: Vec<&str> = match value {
        Value::String(s) => s.split(',').collect(),
        Value::Array(items) => items
            .iter()
            .map(|i| i.as_str().ok_or(TagsRefused::NotAString))
            .collect::<Result<_, _>>()?,
        _ => return Err(TagsRefused::NotAString),
    };
    let mut tags = Vec::with_capacity(given.len());
    for g in given.into_iter().filter(|g| !g.trim().is_empty()) {
        tags.push(normalise_tag(g).map_err(TagsRefused::Tag)?);
    }
    finish(tags)
}

/// `tags` sorted with repeats dropped, refused past [`MAX_TAGS`].
pub fn finish(mut tags: Vec<String>) -> Result<Vec<String>, TagsRefused> {
    tags.sort_unstable();
    tags.dedup();
    if tags.len() > MAX_TAGS {
        return Err(TagsRefused::TooMany(tags.len()));
    }
    Ok(tags)
}

/// The tags of a stored value that normalise, in stored form (sorted, repeats dropped), at most [`MAX_TAGS`] (the
/// lowest); anything else is skipped (a hand edit). What the tag index holds and every tag filter compares.
pub fn stored_tags(value: &Value) -> Vec<String> {
    let Value::Array(items) = value else {
        return Vec::new();
    };
    let mut tags: Vec<String> = items
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|t| normalise_tag(t).ok())
        .collect();
    tags.sort_unstable();
    tags.dedup();
    tags.truncate(MAX_TAGS);
    tags
}

/// What a stored `tags` value holds besides tags in stored form (a hand edit, a merge, an older umx): tags that
/// normalise but are not written as stored (case, order, repeats, spaces), and entries umx cannot use (not a string,
/// not a tag, past [`MAX_TAGS`], or a value that is not a list). Both are reported on load; neither stops it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoredFindings {
    pub not_canonical: bool,
    pub unusable: bool,
}

pub fn stored_findings(value: &Value) -> StoredFindings {
    let mut f = StoredFindings::default();
    let Value::Array(items) = value else {
        f.unusable = true;
        return f;
    };
    let mut usable = Vec::new();
    for item in items {
        match item.as_str().map(normalise_tag) {
            Some(Ok(tag)) => {
                if item.as_str() != Some(tag.as_str()) {
                    f.not_canonical = true;
                }
                usable.push(tag);
            }
            _ => f.unusable = true,
        }
    }
    if !usable.is_sorted_by(|a, b| a < b) {
        f.not_canonical = true;
    }
    usable.sort_unstable();
    usable.dedup();
    if usable.len() > MAX_TAGS {
        f.unusable = true;
    }
    f
}

/// A stored value with every usable tag in stored form, sorted and deduplicated, then every entry umx cannot use
/// kept as it is, in order: what a node's next write stores, so a hand-edited tag becomes canonical without losing
/// anything. A value that is not a list is kept as it is.
pub fn canonical(value: &Value) -> Value {
    let Value::Array(items) = value else {
        return value.clone();
    };
    let mut tags = Vec::new();
    let mut other: Vec<Value> = Vec::new();
    for item in items {
        match item.as_str().map(normalise_tag) {
            Some(Ok(tag)) => tags.push(tag),
            _ => {
                if !other.contains(item) {
                    other.push(item.clone());
                }
            }
        }
    }
    tags.sort_unstable();
    tags.dedup();
    Value::Array(tags.into_iter().map(Value::String).chain(other).collect())
}

/// What a `--tag` filter or a declaration names: one tag, or every value of a facet (`<facet>:*`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pattern {
    Tag(String),
    Facet(String),
}

impl Pattern {
    /// A tag (normalised as [`normalise_tag`]) or `<facet>:*`.
    pub fn parse(given: &str) -> Result<Pattern, TagRefused> {
        let trimmed = given.trim();
        if let Some(facet) = trimmed.strip_suffix(":*") {
            let facet = facet.to_ascii_lowercase();
            return if is_snake_case(&facet) {
                Ok(Pattern::Facet(facet))
            } else {
                Err(TagRefused::new(
                    "each part of a tag must be a lowercase letter then up to 63 lowercase letters digits or underscores",
                    snake(&facet).map(|f| (Some(f), ANY_VALUE.to_string())),
                ))
            };
        }
        normalise_tag(trimmed).map(Pattern::Tag)
    }

    /// Whether stored tag `tag` matches.
    pub fn matches(&self, tag: &str) -> bool {
        match self {
            Pattern::Tag(t) => t == tag,
            Pattern::Facet(f) => split(tag).0 == Some(f.as_str()),
        }
    }

    /// The facet it names, if any.
    pub fn facet(&self) -> Option<&str> {
        match self {
            Pattern::Tag(t) => split(t).0,
            Pattern::Facet(f) => Some(f),
        }
    }

    /// As written: the tag, or `<facet>:*`.
    pub fn text(&self) -> String {
        match self {
            Pattern::Tag(t) => t.clone(),
            Pattern::Facet(f) => alloc::format!("{f}:{ANY_VALUE}"),
        }
    }
}

/// A declaration for `umx ontology add tag`: `<facet>:<value>` or `<facet>:*`, in stored form. A tag without a facet
/// declares nothing (tags are free-form), so it is refused.
pub fn parse_declaration(given: &str) -> Result<String, TagRefused> {
    let p = Pattern::parse(given)?;
    if p.facet().is_none() {
        return Err(TagRefused::new(
            "a declared tag names its facet as facet:value or facet:*",
            None,
        ));
    }
    Ok(p.text())
}

/// The tags among `tags` that are off the vocabulary `declared` (stored declarations): a tag whose facet has a
/// declared tag, while neither it nor `<facet>:*` is declared. A tag in an undeclared facet, or with no facet, never
/// is.
pub fn off_vocabulary<'a>(tags: &'a [String], declared: &[String]) -> Vec<&'a str> {
    tags.iter()
        .map(String::as_str)
        .filter(|tag| {
            let Some(facet) = split(tag).0 else {
                return false;
            };
            let in_facet = |d: &&String| split(d).0 == Some(facet);
            let mut facet_decls = declared.iter().filter(in_facet).peekable();
            facet_decls.peek().is_some()
                && !facet_decls.any(|d| d == tag || split(d).1 == ANY_VALUE)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use serde_json::json;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn a_tag_is_lowercased_and_each_part_must_be_snake_case() {
        assert_eq!(
            normalise_tag(" Needs_Review ").as_deref(),
            Ok("needs_review")
        );
        assert_eq!(
            normalise_tag("origin:double_diamond").as_deref(),
            Ok("origin:double_diamond")
        );
        let r = normalise_tag("origin:Double-Diamond").unwrap_err();
        assert_eq!(
            r.fixed,
            Some((Some("origin".into()), "double_diamond".into()))
        );
        assert_eq!(
            normalise_tag("a:b:c").unwrap_err().reason,
            "a tag has at most one facet"
        );
        assert_eq!(normalise_tag("2024").unwrap_err().fixed, None);
        assert_eq!(
            normalise_tag("q3 2024").unwrap_err().fixed,
            Some((None, "q3_2024".into()))
        );
        assert!(
            normalise_tag("").is_err()
                && normalise_tag(":x").is_err()
                && normalise_tag("x:").is_err()
        );
        assert!(normalise_tag(&"a".repeat(65)).is_err());
        assert!(normalise_tag(&alloc::format!("{0}:{0}", "a".repeat(64))).is_ok());
    }

    #[test]
    fn tags_are_stored_sorted_without_repeats() {
        assert_eq!(normalise_tags(&json!(["b", "a", "A"])), Ok(s(&["a", "b"])));
        assert_eq!(normalise_tags(&json!("b, a,,a")), Ok(s(&["a", "b"])));
        assert_eq!(normalise_tags(&json!([1])), Err(TagsRefused::NotAString));
        assert!(matches!(
            normalise_tags(&json!(["a-b"])),
            Err(TagsRefused::Tag(_))
        ));
        let many: Vec<String> = (0..65).map(|i| alloc::format!("t{i}")).collect();
        assert_eq!(normalise_tags(&json!(many)), Err(TagsRefused::TooMany(65)));
    }

    #[test]
    fn a_stored_value_is_read_as_its_usable_tags_and_kept_on_canonicalising() {
        let v = json!(["b", "Needs_Review", 3, "bad tag", "b"]);
        assert_eq!(stored_tags(&v), s(&["b", "needs_review"]));
        assert_eq!(
            stored_findings(&v),
            StoredFindings {
                not_canonical: true,
                unusable: true
            }
        );
        assert_eq!(canonical(&v), json!(["b", "needs_review", 3, "bad tag"]));
        assert_eq!(
            stored_findings(&json!(["a", "b"])),
            StoredFindings::default()
        );
        assert!(stored_findings(&json!(["b", "a"])).not_canonical);
        assert!(stored_findings(&json!("a")).unusable);
        assert_eq!(canonical(&json!("a")), json!("a"));
    }

    #[test]
    fn patterns_and_declarations() {
        assert_eq!(
            Pattern::parse("Origin:*"),
            Ok(Pattern::Facet("origin".into()))
        );
        assert!(Pattern::parse("origin:*").unwrap().matches("origin:x"));
        assert!(!Pattern::parse("origin:*").unwrap().matches("originx"));
        assert!(Pattern::parse("x").unwrap().matches("x"));
        assert_eq!(parse_declaration("origin:*").as_deref(), Ok("origin:*"));
        assert!(parse_declaration("needs_review").is_err());
    }

    #[test]
    fn only_a_declared_facet_has_tags_off_its_vocabulary() {
        let tags = s(&["origin:a", "origin:b", "phase:x", "loose"]);
        assert!(off_vocabulary(&tags, &[]).is_empty());
        assert_eq!(off_vocabulary(&tags, &s(&["origin:a"])), vec!["origin:b"]);
        assert!(off_vocabulary(&tags, &s(&["origin:a", "origin:*"])).is_empty());
    }
}
