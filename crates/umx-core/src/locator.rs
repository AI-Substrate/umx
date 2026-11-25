//! Resource locators (ws006 S3, req-77; workshop 010 sections 3 and 9a): where a resource node's thing lives. A
//! resource node is an ordinary node with one more standard field, `locator`. Pure: parsing, normalising and format
//! detection work on the locator text only; reading a source's bytes is a port's job (S4).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{REPO_ROOT, is_root_name};

/// The node's standard key on disk.
pub const LOCATOR_FIELD: &str = "locator";

/// The node's standard key and the names `field()` and Cypher answer from it. Never stored flat: a write refuses
/// each as a custom field name (`locator_field_invalid`). `locator` itself answers nothing in a read: its parts are
/// the flat names.
pub const LOCATOR_NAMES: &[&str] = &[
    "locator",
    "locator_kind",
    "locator_source",
    "locator_format",
    "locator_hash",
    "locator_length",
    "locator_seq_max",
    "locator_of",
    "locator_selector",
];

/// Where a resource lives. The passage form (`passage`, with `of`, `selector` and `quote`) is reserved for excerpts
/// (S6): until then a `locator` that holds it is not a locator, and loads as a kept custom field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocatorKind {
    File,
    Folder,
    Url,
}

impl LocatorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LocatorKind::File => "file",
            LocatorKind::Folder => "folder",
            LocatorKind::Url => "url",
        }
    }
}

/// What the resource is, which decides the selectors it supports (S4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    TranscriptNdjson,
    Markdown,
    Text,
    Docx,
    Pdf,
    Html,
    Other,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::TranscriptNdjson => "transcript_ndjson",
            Format::Markdown => "markdown",
            Format::Text => "text",
            Format::Docx => "docx",
            Format::Pdf => "pdf",
            Format::Html => "html",
            Format::Other => "other",
        }
    }

    /// The format a source's text suggests, from the extension of its last path part (case does not matter). A
    /// folder has none. Anything else, an extensionless URL included, is `other`: umx never fetches a URL, and S4
    /// adds the first-bytes check once sources are read.
    pub fn detect(kind: LocatorKind, source: &str) -> Option<Format> {
        if kind == LocatorKind::Folder {
            return None;
        }
        let path = match kind {
            LocatorKind::Url => url_path(source),
            _ => source,
        };
        let last = path.rsplit('/').next().unwrap_or(path);
        let ext = match last.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
            _ => return Some(Format::Other),
        };
        Some(match ext.as_str() {
            "ndjson" => Format::TranscriptNdjson,
            "md" | "markdown" => Format::Markdown,
            "txt" | "text" => Format::Text,
            "docx" => Format::Docx,
            "pdf" => Format::Pdf,
            "html" | "htm" => Format::Html,
            _ => Format::Other,
        })
    }
}

/// A resource node's locator, as stored: keys in workshop 010 9a's order, absent ones left out. `source` is stored
/// normalised; `hash`, `length` and `seq_max` are what umx saw when it last read the source (usage 220), none for a
/// URL, and `hash_state` says why there is or is not a hash.
/// Keys a newer umx added load and are written back after these, in name order (forward compatibility,
/// graph-addressing-expert); an unknown kind is not a locator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locator {
    pub kind: LocatorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq_max: Option<u64>,
    /// Why the source has, or has not, a hash ([`crate::source_hash::HashState`]); none for a folder, and for a
    /// locator stored before umx hashed sources. Kept as text so a state a newer umx adds loads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_state: Option<String>,
    /// A refresh accepted `hash_state` without a hash (usage 220): `umx check` skips it while it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_accepted: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Why locator text was refused. Reasons are umx's own words, never the text, and hold no `,`, `;`, `_` or `'`
/// (they fill a message slot that keeps only plain text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceError {
    Invalid(&'static str),
    /// An absolute path: committed files never hold one (the caller suggests the `root:` form).
    Absolute,
    /// `..`, `~` or a path that leaves its root.
    OutsideRoots,
}

impl Locator {
    /// A new locator from what a person typed: one of the four source forms (a path relative to the graph folder,
    /// `root:repo/<path>`, `root:<name>/<path>` or an http(s) URL), normalised, with its format detected. A
    /// trailing `/` on a path makes a folder. Nothing has been read, so `hash`, `length` and `seq_max` are none.
    pub fn parse(text: &str) -> Result<Locator, SourceError> {
        let (kind, source) = normalise(text)?;
        Ok(Locator {
            kind,
            format: Format::detect(kind, &source),
            source,
            hash: None,
            length: None,
            seq_max: None,
            hash_state: None,
            hash_accepted: None,
            extra: BTreeMap::new(),
        })
    }

    /// A flat locator name's value (workshop 010 9a: `field()` and Cypher answer them, nothing stores them). None for
    /// `locator` itself (its parts are the names), for a part this locator lacks, and for the passage names (S6).
    pub fn field(&self, name: &str) -> Option<Value> {
        let text = |s: &str| Some(Value::String(s.into()));
        match name {
            "locator_kind" => text(self.kind.as_str()),
            "locator_source" => text(&self.source),
            "locator_format" => self.format.and_then(|f| text(f.as_str())),
            "locator_hash" => self.hash.as_deref().and_then(text),
            "locator_length" => self.length.map(Value::from),
            "locator_seq_max" => self.seq_max.map(Value::from),
            _ => None,
        }
    }

    /// The locator a stored `locator` value holds, or None when it is not one (an unknown key or kind, a wrong type).
    pub fn from_value(value: &Value) -> Option<Locator> {
        Locator::deserialize(value).ok()
    }

    /// The locator index key (workshop 010 9a): kind, then the normalised source. A stored source that is not
    /// normalised (a hand edit) is keyed as it normalises; one that cannot be normalised is keyed as stored.
    pub fn key(&self) -> Vec<u8> {
        let source = self.key_source();
        let mut key = Vec::with_capacity(self.kind.as_str().len() + 1 + source.len());
        key.extend_from_slice(self.kind.as_str().as_bytes());
        key.push(0);
        key.extend_from_slice(source.as_bytes());
        key
    }

    /// Whether the stored source is already in normal form for its kind (a load warns when it is not).
    pub fn is_normalised(&self) -> bool {
        self.normalised_source().as_deref() == Some(self.source.as_str())
    }

    /// The source as the index keys it: its normal form, or the stored text when it has none. Every comparison of
    /// what two locators name uses it; the stored text is never rewritten.
    pub fn key_source(&self) -> String {
        self.normalised_source()
            .unwrap_or_else(|| self.source.clone())
    }

    fn normalised_source(&self) -> Option<String> {
        let text = match self.kind {
            LocatorKind::Folder if !self.source.ends_with('/') => {
                alloc::format!("{}/", self.source)
            }
            _ => self.source.clone(),
        };
        match normalise(&text) {
            Ok((kind, source)) if kind == self.kind => Some(source),
            _ => None,
        }
    }
}

/// The kind and normal form of locator text (workshop 010 section 3). Paths lose `./` parts, doubled `/` and the
/// trailing `/` that makes a folder; URLs get a lowercased scheme and host, lose a default port and any fragment,
/// keep their query, and an empty path becomes `/`.
pub fn normalise(text: &str) -> Result<(LocatorKind, String), SourceError> {
    if text.is_empty() {
        return Err(SourceError::Invalid("the locator is empty"));
    }
    if text.contains('\0') || text.contains('\\') {
        return Err(SourceError::Invalid("the locator has a NUL or backslash"));
    }
    if text.chars().any(hidden) {
        return Err(SourceError::Invalid(
            "the locator has a control or bidi or zero-width character",
        ));
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return normal_url(text).map(|u| (LocatorKind::Url, u));
    }
    if lower.starts_with("file:") {
        return Err(SourceError::Absolute);
    }
    if lower.starts_with("umx:") {
        return Err(SourceError::Invalid(
            "a umx address names a node and not a resource (link to it with an edge)",
        ));
    }
    if lower.contains("://") {
        return Err(SourceError::Invalid(
            "only http(s) URLs and paths and root: sources are locators",
        ));
    }
    let kind = if text.ends_with('/') {
        LocatorKind::Folder
    } else {
        LocatorKind::File
    };
    if let Some(rest) = text.strip_prefix("root:") {
        let (name, path) = rest
            .split_once('/')
            .ok_or(SourceError::Invalid("a root source is root:<name>/<path>"))?;
        if name != REPO_ROOT && !is_root_name(name) {
            return Err(SourceError::Invalid(
                "a root name is lower snake case and at most 32 characters",
            ));
        }
        let path = relative_path(path, SourceError::OutsideRoots, false)?;
        return Ok((kind, alloc::format!("root:{name}/{path}")));
    }
    // A graph-folder path may climb with leading `..` (usage 223); the write confines it to the graph's repo.
    Ok((kind, relative_path(text, SourceError::Absolute, true)?))
}

/// A character that can hide or reorder what a reader sees (security, N2): controls (C0, DEL, C1), bidi
/// marks, embeddings, overrides and isolates, and zero-width characters.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// A folder inside a git repo, relative to its top, as a Graphfile lists it (ws002 s17, D66): the locator path normal
/// form (R241-2: `/` separators, no `..`, `~`, absolute path, backslash or `:` in the first part), with no control or
/// hidden characters either, and `.` for the top itself. The normal form is returned; a Graphfile holds only paths
/// that are already in it.
pub fn repo_path(text: &str) -> Result<String, SourceError> {
    if text.contains('\0') || text.contains('\\') {
        return Err(SourceError::Invalid("the path has a NUL or backslash"));
    }
    if text
        .chars()
        .any(|c| c.is_control() || crate::entity::hidden(c))
    {
        return Err(SourceError::Invalid(
            "the path has a control or hidden character",
        ));
    }
    if text == "." {
        return Ok(text.to_string());
    }
    relative_path(text, SourceError::Absolute, false)
}

/// `text` without its leading `./` parts.
fn strip_dots(mut text: &str) -> &str {
    while let Some(rest) = text.strip_prefix("./") {
        text = rest;
    }
    text
}

/// A path inside the graph folder or a root, in normal form. Its `./` parts are cleaned first and every check runs on
/// what is left, so the result normalises to itself (review R241-2): a cleaned path is never read as another source
/// form. Absolute and home paths are refused, and so is a first part holding `:` (a scheme, a drive, or `root:`
/// behind a `./`; graph-addressing-expert). A leading `/` is `rooted`: absolute in
/// the graph folder, outside the root in a root.
/// `up` lets the path start with `..` parts, kept as written.
fn relative_path(path: &str, rooted: SourceError, up: bool) -> Result<String, SourceError> {
    let path = strip_dots(path);
    if path.starts_with('/') {
        return Err(rooted);
    }
    let path = normal_path(path, up)?;
    let first = path.split('/').find(|p| *p != "..").unwrap_or_default();
    if first.starts_with('~') {
        return Err(SourceError::OutsideRoots);
    }
    if first.contains(':') {
        return Err(SourceError::Invalid(
            "the first part of a path holds a colon (use root:<name>/<path> or an http(s) URL)",
        ));
    }
    Ok(path)
}

/// Joins a relative path's parts with one `/`, dropping empty and `.` parts; `..` is refused, except leading ones
/// when `up`.
fn normal_path(path: &str, up: bool) -> Result<String, SourceError> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if up && parts.iter().all(|p| *p == "..") => parts.push(part),
            ".." => return Err(SourceError::OutsideRoots),
            part => parts.push(part),
        }
    }
    if parts.iter().all(|p| *p == "..") {
        return Err(SourceError::Invalid("the path is empty"));
    }
    Ok(parts.join("/"))
}

fn normal_url(text: &str) -> Result<String, SourceError> {
    let (scheme, rest) = text
        .split_once("://")
        .expect("checked for http:// or https://");
    let scheme = scheme.to_ascii_lowercase();
    let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.is_empty() {
        return Err(SourceError::Invalid("the URL has no host"));
    }
    // A user or password in a URL is a credential that would be committed (security, N1).
    if authority.contains('@') {
        return Err(SourceError::Invalid(
            "a URL may not hold a user or password",
        ));
    }
    let mut authority = authority.to_ascii_lowercase();
    let default_port = if scheme == "https" { ":443" } else { ":80" };
    if authority.ends_with(default_port) {
        authority.truncate(authority.len() - default_port.len());
    }
    if authority.is_empty() || authority.starts_with(':') {
        return Err(SourceError::Invalid("the URL has no host"));
    }
    let tail = if tail.is_empty() || tail.starts_with('?') {
        alloc::format!("/{tail}")
    } else {
        tail.to_string()
    };
    Ok(alloc::format!("{scheme}://{authority}{tail}"))
}

/// The path of a normalised URL, for format detection: after the host, before any query.
fn url_path(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    path.split_once('?').map_or(path, |(before, _)| before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn norm(text: &str) -> (LocatorKind, String) {
        normalise(text).unwrap()
    }

    #[test]
    fn each_source_form_normalises_to_one_key() {
        use LocatorKind::*;
        for (text, kind, normal) in [
            ("docs/brief.md", File, "docs/brief.md"),
            ("./docs//brief.md", File, "docs/brief.md"),
            ("docs/./brief.md", File, "docs/brief.md"),
            ("./../..//x/a.md", File, "../../x/a.md"),
            ("transcripts/", Folder, "transcripts"),
            ("root:repo/specs/a.md", File, "root:repo/specs/a.md"),
            ("root:repo/specs//./a.md", File, "root:repo/specs/a.md"),
            ("root:acme/t/016.ndjson", File, "root:acme/t/016.ndjson"),
            (
                "HTTPS://Example.COM:443/A/b?q=1#frag",
                Url,
                "https://example.com/A/b?q=1",
            ),
            ("http://example.com:80", Url, "http://example.com/"),
            (
                "http://example.com:8080?x",
                Url,
                "http://example.com:8080/?x",
            ),
        ] {
            assert_eq!(norm(text), (kind, normal.to_string()), "{text}");
        }
        assert_eq!(
            Locator::parse("./docs//brief.md").unwrap().key(),
            Locator::parse("docs/brief.md").unwrap().key()
        );
        assert_ne!(
            Locator::parse("docs/").unwrap().key(),
            Locator::parse("docs").unwrap().key(),
            "a folder and a file of one name are two locators"
        );
    }

    #[test]
    fn refused_sources_say_why_without_echoing_them() {
        for (text, err) in [
            ("/abs/path.md", SourceError::Absolute),
            ("file:///etc/passwd", SourceError::Absolute),
            // usage 223: leading `..` parses (the write confines it to the graph's repo); any other `..` is refused.
            ("../a/../x", SourceError::OutsideRoots),
            ("../~/x", SourceError::OutsideRoots),
            ("docs/../../x", SourceError::OutsideRoots),
            ("~/notes.md", SourceError::OutsideRoots),
            ("root:acme/../x", SourceError::OutsideRoots),
            ("root:acme//etc", SourceError::OutsideRoots),
            // review R241-2: `./` parts are cleaned before the checks, and what is left stays a path.
            ("./~/notes.md", SourceError::OutsideRoots),
            ("././~", SourceError::OutsideRoots),
            (".//abs.md", SourceError::Absolute),
            ("root:repo/./~/x", SourceError::OutsideRoots),
            ("root:repo/.//etc", SourceError::OutsideRoots),
        ] {
            assert_eq!(normalise(text), Err(err), "{text}");
        }
        for text in [
            "",
            "umx:demo/01ABC",
            "ftp://h/x",
            "root:Bad-Name/x",
            "root:acme",
            "a\\b",
            "https:///x",
            "./",
            "./umx:demo/01ABC",
            "./file:///etc/passwd",
            "./file:x",
            "./root:repo/x.md",
            "./HTTP://Example.com",
            "https://:443",
            "https://user:pass@example.com/a",
            "https://token@example.com/",
            "docs/a\u{202E}dm.exe",
            "docs/\u{200B}a.md",
            "docs/a\u{0007}.md",
            "docs/a\u{FEFF}.md",
            "https://example.com/a\u{2066}b",
            "http://:8080/x",
            "C:/notes.md",
            "a:b/c.md",
            "./a:b",
            "root:repo/./root:acme/x",
            "root:repo/c:/x",
        ] {
            let Err(SourceError::Invalid(reason)) = normalise(text) else {
                panic!("{text} is not refused as invalid");
            };
            // The reason fills a message slot that keeps only these (umx-protocol Fill::text).
            assert!(
                reason
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || " ._/+~:=<>()-".contains(c)),
                "{reason}"
            );
        }
    }

    /// review and graph-addressing-expert (R241-2): normalising is a fixed point. Every accepted text, built
    /// from parts that each once slipped a check (`./`, `~`, `root:`, schemes, drives), normalises to a source that
    /// normalises to itself, and its stored locator says it is in normal form.
    #[test]
    fn normalising_twice_changes_nothing() {
        const PARTS: &[&str] = &[
            "",
            ".",
            "..",
            "~",
            "a",
            "b.md",
            "root:repo",
            "root:acme",
            "file:",
            "umx:x",
            "http:",
            "HTTPS:",
            "C:",
            "x:y",
            "%2e%2e",
            "Host.COM:443",
        ];
        let mut accepted = 0;
        let mut texts = alloc::vec![String::new()];
        for _ in 0..4 {
            let mut longer = Vec::new();
            for t in &texts {
                for p in PARTS {
                    let text = if t.is_empty() {
                        (*p).to_string()
                    } else {
                        alloc::format!("{t}/{p}")
                    };
                    if let Ok((kind, source)) = normalise(&text) {
                        accepted += 1;
                        let stored = match kind {
                            LocatorKind::Folder => alloc::format!("{source}/"),
                            _ => source.clone(),
                        };
                        assert_eq!(normalise(&stored), Ok((kind, source.clone())), "{text}");
                        assert!(Locator::parse(&text).unwrap().is_normalised(), "{text}");
                    }
                    longer.push(text);
                }
            }
            texts = longer;
        }
        assert!(
            accepted > 1000,
            "the property ran over real sources ({accepted})"
        );
    }

    #[test]
    fn format_comes_from_the_extension() {
        use Format::*;
        for (text, format) in [
            ("t/016.ndjson", Some(TranscriptNdjson)),
            ("docs/Brief.MD", Some(Markdown)),
            ("a.markdown", Some(Markdown)),
            ("notes.txt", Some(Text)),
            ("deck.docx", Some(Docx)),
            ("law.pdf", Some(Pdf)),
            ("page.htm", Some(Html)),
            ("data.csv", Some(Other)),
            ("Makefile", Some(Other)),
            (".profile", Some(Other)),
            ("docs/", None),
            ("https://example.com/a/report.pdf?v=2", Some(Pdf)),
            ("https://example.com/", Some(Other)),
        ] {
            assert_eq!(Locator::parse(text).unwrap().format, format, "{text}");
        }
    }

    #[test]
    fn a_locator_writes_its_keys_in_order_and_keeps_a_newer_umxs_keys() {
        let mut l = Locator::parse("t/016.ndjson").unwrap();
        l.hash = Some("sha256:9f2c".into());
        l.length = Some(1843302);
        l.seq_max = Some(3511);
        assert_eq!(
            serde_json::to_string(&l).unwrap(),
            r#"{"kind":"file","format":"transcript_ndjson","source":"t/016.ndjson","hash":"sha256:9f2c","length":1843302,"seq_max":3511}"#
        );
        let url = Locator::parse("https://example.com/").unwrap();
        assert_eq!(
            serde_json::to_string(&url).unwrap(),
            r#"{"kind":"url","format":"other","source":"https://example.com/"}"#
        );
        assert_eq!(
            Locator::from_value(&serde_json::to_value(&l).unwrap()),
            Some(l)
        );
        let newer = r#"{"kind":"file","source":"a","hash":"h","seq_max":2,"z_later":{"n":1},"a_later":[1]}"#;
        let kept = Locator::from_value(&serde_json::from_str(newer).unwrap()).unwrap();
        assert_eq!(kept.extra.len(), 2);
        assert_eq!(
            serde_json::to_string(&kept).unwrap(),
            r#"{"kind":"file","source":"a","hash":"h","seq_max":2,"a_later":[1],"z_later":{"n":1}}"#
        );
        for bad in [
            json!({ "kind": "passage", "of": "x", "selector": "seq=1" }),
            json!({ "kind": "file", "format": "csv", "source": "a" }),
            json!({ "kind": "file" }),
            json!("docs/a.md"),
        ] {
            assert_eq!(Locator::from_value(&bad), None, "{bad}");
        }
    }

    #[test]
    fn every_locator_name_is_a_reserved_node_field() {
        for name in LOCATOR_NAMES {
            assert!(crate::name::RESERVED_NODE_FIELDS.contains(name), "{name}");
        }
    }

    #[test]
    fn a_hand_edited_source_is_keyed_as_it_normalises() {
        let stored = Locator {
            source: "./docs//brief.md".into(),
            ..Locator::parse("docs/brief.md").unwrap()
        };
        assert!(!stored.is_normalised());
        assert_eq!(stored.key(), Locator::parse("docs/brief.md").unwrap().key());
        assert!(Locator::parse("docs/brief.md").unwrap().is_normalised());
        let folder = Locator::parse("transcripts/").unwrap();
        assert!(folder.is_normalised(), "a stored folder has no trailing /");
        let absolute = Locator {
            source: "/abs/x.md".into(),
            ..Locator::parse("x.md").unwrap()
        };
        assert!(!absolute.is_normalised());
        assert_eq!(absolute.key(), b"file\0/abs/x.md");
    }
}
