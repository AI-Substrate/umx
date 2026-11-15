//! Evidence pointers (req-18, workshop 007 section 5): `<source>#<selectors>`, where the selectors are the address
//! grammar's (`seq=`, `L`, `section=`, `quote=` with `prefix=`/`suffix=`, `page=`; see [`crate::address`]). The
//! selector is stored as an object (R2). Reading the source and the hash land with the SourceReader port, which
//! also does the symlink half of req-47.

use crate::address::{Fragment, GRAPH_FRAGMENT_REASON, parse_fragment, split_repo};
use crate::config::{REPO_ROOT, is_root_name};
use crate::ports::GraphDir;
use alloc::string::String;
use alloc::vec::Vec;
use serde_json::{Value, json};

/// The node key that holds removed evidence (req-126): the one name the reserved lists, the services and any later
/// reader use.
pub const EVIDENCE_REMOVED: &str = "evidence_removed";

/// Where evidence came from (req-18).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceKind {
    Room,
    Chat,
    Screen,
    Document,
    Analyst,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidencePointer {
    pub pointer: String,
    pub kind: Option<EvidenceKind>,
}

/// A pointer without its own kind: it takes the op's `evidence_kind`, else [`EvidenceKind::DEFAULT`].
impl From<String> for EvidencePointer {
    fn from(pointer: String) -> Self {
        EvidencePointer {
            pointer,
            kind: None,
        }
    }
}

impl From<&str> for EvidencePointer {
    fn from(pointer: &str) -> Self {
        String::from(pointer).into()
    }
}

impl EvidenceKind {
    pub const DEFAULT: EvidenceKind = EvidenceKind::Document;

    pub fn parse(s: &str) -> Option<EvidenceKind> {
        Some(match s {
            "room" => EvidenceKind::Room,
            "chat" => EvidenceKind::Chat,
            "screen" => EvidenceKind::Screen,
            "document" => EvidenceKind::Document,
            "analyst" => EvidenceKind::Analyst,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceKind::Room => "room",
            EvidenceKind::Chat => "chat",
            EvidenceKind::Screen => "screen",
            EvidenceKind::Document => "document",
            EvidenceKind::Analyst => "analyst",
        }
    }

    /// Spoken words and screen captures reach the graph through captions or capture, so they are lossy; chat,
    /// documents and an analyst's own inference are recorded exactly.
    pub fn fidelity(self) -> &'static str {
        match self {
            EvidenceKind::Room | EvidenceKind::Screen => "lossy",
            EvidenceKind::Chat | EvidenceKind::Document | EvidenceKind::Analyst => "exact",
        }
    }
}

/// Why a pointer was refused. Reasons are umx's own words, never the pointer text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerError {
    Invalid(&'static str),
    OutsideRoots,
}

/// Parses a pointer into the stored evidence object: `{ source, selector, kind, fidelity, hash: null }`.
/// Relative paths are relative to the graph folder and may not use `..`; an absolute path must be inside the graph
/// folder; `root:<name>/<path>` is a path under a source root (req-47, ws006 S1), with the same refusals, stored as
/// written; `file://` and other schemes than http(s) and `umx:` addresses are refused. URLs are stored, never
/// fetched. Whether a root's name is configured, and where it is, is the caller's check ([`root_source`]).
pub fn parse_pointer(
    text: &str,
    kind: EvidenceKind,
    graph: &GraphDir,
) -> Result<Value, PointerError> {
    // A `umx:` source may end in a `?repo=` hint after its selectors: split it off first, as the address grammar
    // does, and keep it on the stored source. Other sources keep `?repo=` as text (a quote may contain it).
    let is_umx = text
        .get(..4)
        .is_some_and(|p| p.eq_ignore_ascii_case("umx:"));
    let (text, repo) = if is_umx {
        split_repo(text).map_err(|e| PointerError::Invalid(e.reason))?
    } else {
        (text, None)
    };
    let (source, fragment) = text.split_once('#').ok_or(PointerError::Invalid(
        "it needs a selector (seq= L section= quote= or page=) after the source",
    ))?;
    if is_umx && !source[4..].contains('/') {
        return Err(PointerError::Invalid(GRAPH_FRAGMENT_REASON));
    }
    check_source(source, graph)?;
    let selector = parse_selector(fragment)?;
    let source = match repo {
        Some(url) => alloc::format!("{source}?repo={url}"),
        None => source.into(),
    };
    Ok(json!({
        "source": source,
        "selector": selector,
        "kind": kind.as_str(),
        "fidelity": kind.fidelity(),
        "hash": Value::Null,
    }))
}

fn check_source(source: &str, graph: &GraphDir) -> Result<(), PointerError> {
    if source.is_empty() {
        return Err(PointerError::Invalid("the source before # is empty"));
    }
    if let Some(rest) = source.strip_prefix("root:") {
        return check_root_source(rest);
    }
    let lower = source.to_ascii_lowercase();
    if lower.starts_with("file:") {
        return Err(PointerError::OutsideRoots);
    }
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("umx:") {
        return Ok(());
    }
    if lower.contains("://") {
        return Err(PointerError::Invalid(
            "only http and https URLs, umx addresses and paths are sources",
        ));
    }
    if source.contains('\0') || source.contains('\\') {
        return Err(PointerError::Invalid("the path has a NUL or backslash"));
    }
    if source.starts_with('~') || source.split('/').any(|part| part == "..") {
        return Err(PointerError::OutsideRoots);
    }
    if source.starts_with('/') {
        let inside = GraphDir::parse(source).is_ok_and(|p| p.is_within(graph));
        if !inside {
            return Err(PointerError::OutsideRoots);
        }
    }
    Ok(())
}

/// `<name>/<path>` after `root:`.
fn check_root_source(rest: &str) -> Result<(), PointerError> {
    let (name, path) = rest
        .split_once('/')
        .ok_or(PointerError::Invalid("a root source is root:<name>/<path>"))?;
    if name != REPO_ROOT && !is_root_name(name) {
        return Err(PointerError::Invalid(
            "a root name is snake_case and at most 32 characters",
        ));
    }
    if path.is_empty() {
        return Err(PointerError::Invalid(
            "the path after root:<name>/ is empty",
        ));
    }
    if path.contains('\0') || path.contains('\\') {
        return Err(PointerError::Invalid("the path has a NUL or backslash"));
    }
    if path.starts_with('/') || path.starts_with('~') || path.split('/').any(|part| part == "..") {
        return Err(PointerError::OutsideRoots);
    }
    Ok(())
}

/// A stored source's root name and the path under it, for `root:<name>/<path>`; None for any other source.
pub fn root_source(source: &str) -> Option<(&str, &str)> {
    source.strip_prefix("root:")?.split_once('/')
}

fn parse_selector(fragment: &str) -> Result<Value, PointerError> {
    match parse_fragment(fragment) {
        Ok(Fragment::Selectors(s)) => Ok(s.to_object()),
        Ok(Fragment::Field(_)) => Err(PointerError::Invalid(
            "after the source use a selector (seq= L section= quote= or page=) not a field name",
        )),
        Err(e) => Err(PointerError::Invalid(e.reason)),
    }
}

/// Scans the stored pointer's source and selector text, never fetching its source.
pub fn suspected_secret(pointer: &Value) -> Option<crate::secrets::Rule> {
    if let Some(source) = pointer.get("source").and_then(Value::as_str) {
        let source = percent_decoded(source);
        if let Some(rule) = crate::secrets::scan(&source) {
            return Some(rule);
        }
        if let Some((scheme, _)) = source.split_once(':')
            && (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            && let Some((_, query)) = source.split_once('?')
        {
            for part in query.split('&') {
                if let Some((key, value)) = part.split_once('=')
                    && key.eq_ignore_ascii_case("x-amz-signature")
                    && let Some(rule) = crate::secrets::scan(&alloc::format!("sig={value}"))
                {
                    return Some(rule);
                }
            }
        }
    }
    pointer
        .get("selector")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|selector| selector.values())
        .find_map(|value| match value {
            Value::String(text) => crate::secrets::scan(&percent_decoded(text)),
            Value::Array(parts) => parts
                .iter()
                .filter_map(Value::as_str)
                .find_map(|text| crate::secrets::scan(&percent_decoded(text))),
            _ => None,
        })
}

fn percent_decoded(text: &str) -> String {
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let (Some(a), Some(b)) = (bytes.get(at + 1), bytes.get(at + 2))
            && let (Some(a), Some(b)) = (digit(*a), digit(*b))
        {
            decoded.push(a * 16 + b);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> GraphDir {
        GraphDir::parse("/work/repo/graph").unwrap()
    }

    fn ok(text: &str) -> Value {
        parse_pointer(text, EvidenceKind::Room, &g()).unwrap()
    }

    fn err(text: &str) -> PointerError {
        parse_pointer(text, EvidenceKind::Document, &g()).unwrap_err()
    }

    #[test]
    fn write_scan_sees_decoded_quotes_and_signed_queries_without_flagging_plain_paths() {
        let token = alloc::format!("ghp_{}", "a".repeat(36));
        let encoded = token.replace('g', "%67").replace('_', "%5F");
        for pointer in [
            alloc::format!("a.md#quote={encoded}"),
            alloc::format!("a.md#quote=clean&prefix={encoded}"),
            alloc::format!("a.md#section=Overview/{encoded}"),
            alloc::format!("https://x.test/a?token={encoded}#L1"),
            alloc::format!("https://x.test/a?sig={encoded}#L1"),
            alloc::format!("https://x.test/a?X-Amz-Signature={}#L1", "f".repeat(64)),
        ] {
            assert!(suspected_secret(&ok(&pointer)).is_some());
        }
        for pointer in [
            "src/aws_secret_key.rs#L1",
            "https://x.test/src/aws_secret_key.rs#L1",
            "a.md#quote=A 100% complete example",
        ] {
            assert_eq!(suspected_secret(&ok(pointer)), None);
        }
    }

    #[test]
    fn three_selector_forms_parse() {
        let v = ok("transcripts/012.ndjson#seq=120-140");
        assert_eq!(v["selector"], json!({ "seq": [120, 140] }));
        assert_eq!(
            (v["kind"].as_str(), v["fidelity"].as_str()),
            (Some("room"), Some("lossy"))
        );
        assert_eq!(v["hash"], Value::Null);
        assert_eq!(ok("a.md#L10-L24")["selector"], json!({ "lines": [10, 24] }));
        assert_eq!(ok("a.md#L7")["selector"], json!({ "lines": [7, 7] }));
        assert_eq!(
            ok("a.md#quote=we ship & test&prefix=so&suffix=today")["selector"],
            json!({ "quote": "we ship & test", "prefix": "so", "suffix": "today" })
        );
        assert_eq!(ok("https://x.test/a#L1")["source"], "https://x.test/a");
        assert_eq!(
            ok("/work/repo/graph/notes.md#L1")["source"],
            "/work/repo/graph/notes.md"
        );
    }

    #[test]
    fn graph_fragments_are_not_evidence_pointers() {
        for pointer in [
            "umx:demo#L1",
            "UMX:demo#title",
            "umx:demo#section=A/B?repo=https://x/y.git",
        ] {
            assert_eq!(err(pointer), PointerError::Invalid(GRAPH_FRAGMENT_REASON));
        }
    }

    #[test]
    fn escapes_and_file_urls_are_outside_roots() {
        for p in [
            "../x.md#L1",
            "a/../../x.md#L1",
            "/etc/passwd#L1",
            "/work/repo/graphs/x#L1",
            "file:///work/repo/graph/a.md#L1",
            "FILE:a#L1",
            "~/a.md#L1",
        ] {
            assert_eq!(err(p), PointerError::OutsideRoots, "{p}");
        }
    }

    #[test]
    fn root_sources_are_stored_as_written() {
        let v = ok("root:acme/transcripts/016.ndjson#seq=1-5");
        assert_eq!(v["source"], "root:acme/transcripts/016.ndjson");
        assert_eq!(v["selector"], json!({ "seq": [1, 5] }));
        assert_eq!(
            ok("root:repo/docs/x.md#L1")["source"],
            "root:repo/docs/x.md"
        );
        assert_eq!(
            root_source("root:acme/transcripts/016.ndjson"),
            Some(("acme", "transcripts/016.ndjson"))
        );
        assert_eq!(root_source("transcripts/016.ndjson"), None);
        for p in [
            "root:acme/../x#L1",
            "root:acme/a/../../x#L1",
            "root:acme//etc/passwd#L1",
            "root:acme/~/x#L1",
            "root:repo/../x#L1",
        ] {
            assert_eq!(err(p), PointerError::OutsideRoots, "{p}");
        }
        for p in [
            "root:acme#L1",
            "root:acme/#L1",
            "root:Acme/x#L1",
            "root:a-b/x#L1",
            "root:/x#L1",
            "root:acme/a\\b#L1",
        ] {
            assert!(matches!(err(p), PointerError::Invalid(_)), "{p}");
        }
    }

    #[test]
    fn malformed_pointers_are_invalid() {
        for p in [
            "a.md",
            "#L1",
            "a.md#L0",
            "a.md#L9-L3",
            "a.md#seq=x",
            "a.md#quote=",
            "a.md#problem",
            "a.md#t=1-2",
            "a.md#",
            "ftp://x/a#L1",
        ] {
            assert!(matches!(err(p), PointerError::Invalid(_)), "{p}");
        }
    }

    /// The address fixture table run by the evidence path: every fragment row stores its object, or is refused.
    #[test]
    fn the_address_fixture_table_runs_through_evidence() {
        let t: Value =
            serde_json::from_str(include_str!("../fixtures/address/table.json")).unwrap();
        let (mut ran, mut umx_rows) = (0, 0);
        for row in t["rows"].as_array().unwrap() {
            let input = row["input"].as_str().unwrap();
            if input.starts_with("umx:") {
                umx_rows += check_umx_row(row);
                continue;
            }
            if !input.starts_with('#') {
                continue;
            }
            ran += 1;
            let got = parse_pointer(&alloc::format!("a.md{input}"), EvidenceKind::Document, &g());
            let canonical = row["canonical"].as_str().map(|c| &c[1..]);
            match canonical {
                Some(c) if crate::address::is_selector(c) => {
                    let selector = &got.expect(input)["selector"];
                    if let Some(want) = row.get("object") {
                        assert_eq!(selector, want, "{input}");
                    }
                    let back = crate::address::Selectors::from_object(selector).unwrap();
                    assert_eq!(back.canonical(), c, "{input}");
                }
                _ => {
                    let PointerError::Invalid(reason) = got.expect_err(input) else {
                        panic!("{input}")
                    };
                    if let Some(r) = row["reason"].as_str() {
                        assert_eq!(reason, r, "{input}");
                    }
                }
            }
        }
        assert!(ran >= 50, "only {ran} fragment rows");
        assert!(
            umx_rows >= 4,
            "only {umx_rows} umx: rows with selectors or a bad hint"
        );
    }

    /// A `umx:` address row through evidence (graph-addressing-expert F1): a row with selectors stores them, and its
    /// source keeps the `?repo=` hint; a row whose hint is bad is refused with the grammar's reason. Returns 1 if it
    /// ran.
    fn check_umx_row(row: &Value) -> usize {
        let input = row["input"].as_str().unwrap();
        let got = parse_pointer(input, EvidenceKind::Document, &g());
        match (row["canonical"].as_str(), row.get("object")) {
            (Some(canonical), Some(object)) => {
                let v = got.expect(input);
                assert_eq!(&v["selector"], object, "{input}");
                let (before, hint) = input.split_once('#').unwrap();
                let hint = hint.rfind("?repo=").map_or("", |i| &hint[i..]);
                assert_eq!(v["source"], alloc::format!("{before}{hint}"), "{input}");
                assert!(canonical.ends_with(hint), "{input}");
                1
            }
            (None, _) if input.contains("?repo=") => {
                let PointerError::Invalid(reason) = got.expect_err(input) else {
                    panic!("{input}")
                };
                if let Some(r) = row["reason"].as_str() {
                    assert_eq!(reason, r, "{input}");
                }
                1
            }
            _ => 0,
        }
    }

    /// Only a `umx:` source has a fetch hint: in a file pointer, `?repo=` is quote text.
    #[test]
    fn a_repo_hint_is_split_off_umx_sources_only() {
        let v = ok("umx:demo/01JC8XA4H7Q1V9K2M3N4P5R6S7#seq=1-2?repo=https://x/y.git");
        assert_eq!(
            v["source"],
            "umx:demo/01JC8XA4H7Q1V9K2M3N4P5R6S7?repo=https://x/y.git"
        );
        assert_eq!(v["selector"], json!({ "seq": [1, 2] }));
        let v = ok("a.md#quote=see ?repo=https://x");
        assert_eq!(v["source"], "a.md");
        assert_eq!(v["selector"], json!({ "quote": "see ?repo=https://x" }));
    }
}
