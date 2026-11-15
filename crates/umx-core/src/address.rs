//! The address grammar (workshop 002 section 7, workshop 010 section 2, req-55, req-78):
//! `umx:<alias>[@<copy>][/<id>[#<fragment>]][?repo=<git url>]`, where the fragment is a field name or `&`-joined
//! selectors: `seq=`, `L`, `section=`, `quote=` with `prefix=`/`suffix=`, `page=`.
//!
//! One grammar serves addresses, evidence pointers and edge ends. Evidence stores its selector as an OBJECT
//! (`{"seq":[a,b]}`, `{"lines":[a,b]}`, `{"quote":..}`); edges and addresses use the canonical STRING. The two map
//! to each other without loss (graph-addressing-expert ruling R2). Pure: no I/O.
//!
//! Canonical form (what umx writes, and what dedup and promotion compare as plain strings):
//! - keys in the order `seq`, `L`, `section`, `quote`, `prefix`, `suffix`, `page`, joined by `&`;
//! - a one-number range collapses (`L7-L7` is `L7`), and lines read `L10-L24`;
//! - text is percent-encoded as UTF-8 bytes with upper-case hex; A-Z a-z 0-9 and `-._~!*+,:;=@` stay raw;
//! - no Unicode normalisation: the bytes are kept as given (matching a quote may normalise both sides later);
//! - an empty `prefix=` or `suffix=` is dropped, so one meaning has one string;
//! - `section=` splits on a raw `/`; a `/` inside a heading is `%2F`.
//!
//! Lenient input: raw spaces and non-ASCII, a raw `&` not followed by a known key, lower-case hex, and a `%` not
//! followed by two hex digits (a literal `%`). A literal `%20` in a quote is written `%2520`.
//!
//! Refused (`selector_invalid`): a repeated key (`seq=1&seq=2`), `prefix=` or `suffix=` without `quote=`, an empty
//! quote or heading, a range that ends before it starts, line or page 0, and unknown keys. A fragment that is
//! neither selectors nor a snake_case field is `field_invalid`; a bad `umx:` shape is `address_invalid`.

use crate::id::{ID_LEN, normalise_id_text};
use crate::name::is_snake_case;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{Map, Value};

pub const GRAPH_FRAGMENT_REASON: &str = "a graph address has no selector or fragment";

/// Why text is not an address, fragment or selector. `code` is a snake_case status; `reason` is umx's own words
/// (never the input), written to survive a slot word (no `,` `;` `#` `&` or quotes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrammarError {
    pub code: &'static str,
    pub reason: &'static str,
}

const fn selector(reason: &'static str) -> GrammarError {
    GrammarError {
        code: "selector_invalid",
        reason,
    }
}

const fn address(reason: &'static str) -> GrammarError {
    GrammarError {
        code: "address_invalid",
        reason,
    }
}

/// An inclusive range of whole numbers, `from <= to`.
pub type Range = [u64; 2];

/// The selectors of one fragment. Each kind appears at most once; at least one is present.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selectors {
    pub seq: Option<Range>,
    /// `L<from>-L<to>`, 1-based; stored as `lines` in evidence.
    pub lines: Option<Range>,
    /// A heading path, top level first. A heading may itself contain `/` (encoded in the canonical form).
    pub section: Option<Vec<String>>,
    pub quote: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    /// PDF pages, 1-based.
    pub page: Option<Range>,
}

/// What follows `#`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fragment {
    Field(String),
    Selectors(Selectors),
}

/// A parsed `umx:` address. No `id` means the graph; otherwise it is normalised Crockford text and may be a prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub alias: String,
    pub copy: Option<String>,
    pub id: Option<String>,
    pub fragment: Option<Fragment>,
    /// The fetch hint, as written. Never part of identity.
    pub repo: Option<String>,
}

/// Keys after `&` that start a new selector; a raw `&` followed by anything else is text (lenient input).
const KEYS: [&str; 6] = ["seq", "section", "quote", "prefix", "suffix", "page"];

/// True if a fragment is selectors rather than a field name: it has `=`, or starts `L` and a digit.
pub fn is_selector(fragment: &str) -> bool {
    fragment.contains('=') || starts_lines(fragment)
}

fn starts_lines(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() > 1 && b[0] == b'L' && b[1].is_ascii_digit()
}

/// Parses a graph or node address. Graph addresses have no fragment. The scheme may be any case; `?repo=` is split
/// off first (it is always last), then `#`, the first `/` and `@`.
pub fn parse_address(text: &str) -> Result<Address, GrammarError> {
    // The scheme is case-insensitive on input (`UMX:`, `Umx:`) and lower case in the canonical form (F3).
    let rest = match text.get(..4) {
        Some(scheme) if scheme.eq_ignore_ascii_case("umx:") => &text[4..],
        _ => return Err(address("an address starts umx:")),
    };
    let (rest, repo) = split_repo(rest)?;
    let (body, fragment) = match rest.split_once('#') {
        Some((body, fragment)) => (body, Some(fragment)),
        None => (rest, None),
    };
    let (graph, id) = match body.split_once('/') {
        Some((graph, id)) => (graph, Some(id)),
        None => (body, None),
    };
    let (alias, copy) = match graph.split_once('@') {
        Some((a, c)) => (a, Some(c)),
        None => (graph, None),
    };
    if !is_snake_case(alias) {
        return Err(address("the graph alias must be snake_case"));
    }
    if let Some(c) = copy
        && !is_copy_name(c)
    {
        return Err(address(
            "a copy name is lowercase letters and digits and . _ - (at most 64)",
        ));
    }
    if id.is_none() && fragment.is_some() {
        return Err(address(GRAPH_FRAGMENT_REASON));
    }
    let id = id
        .map(|id| match normalise_id_text(id) {
            Some(id) if !id.is_empty() && id.len() <= ID_LEN => Ok(id),
            _ => Err(address(
                "the id must be up to 26 Crockford base32 characters",
            )),
        })
        .transpose()?;
    let fragment = fragment.map(parse_fragment).transpose()?;
    Ok(Address {
        alias: alias.to_string(),
        copy: copy.map(ToString::to_string),
        id,
        fragment,
        repo: repo.map(ToString::to_string),
    })
}

/// Splits the `?repo=<url>` fetch hint off the end of address text (it is always last, so after any `#fragment`)
/// and checks its shape. The evidence parser calls this before it splits at `#`, for `umx:` sources only.
pub fn split_repo(text: &str) -> Result<(&str, Option<&str>), GrammarError> {
    let Some(i) = text.rfind("?repo=") else {
        return Ok((text, None));
    };
    let url = &text[i + "?repo=".len()..];
    let bad = |c: char| c.is_whitespace() || c.is_control() || c == '#' || c == '?';
    if url.is_empty() || url.contains(bad) {
        return Err(address(
            "the repo URL is empty or has a raw space or hash or question mark (percent-encode them)",
        ));
    }
    Ok((&text[..i], Some(url)))
}

/// `[a-z0-9][a-z0-9._-]{0,63}` (workshop 002 section 7.1).
fn is_copy_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|&c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

impl Address {
    /// The canonical text: id upper case, fragment canonical, copy and repo hint as written.
    pub fn canonical(&self) -> String {
        let mut out = String::from("umx:");
        out.push_str(&self.alias);
        if let Some(c) = &self.copy {
            out.push('@');
            out.push_str(c);
        }
        if let Some(id) = &self.id {
            out.push('/');
            out.push_str(id);
        }
        if let Some(f) = &self.fragment {
            out.push('#');
            out.push_str(&f.canonical());
        }
        if let Some(r) = &self.repo {
            out.push_str("?repo=");
            out.push_str(r);
        }
        out
    }
}

/// Parses the text after `#`: a field name (req-55) or selectors (req-78).
pub fn parse_fragment(text: &str) -> Result<Fragment, GrammarError> {
    if text.is_empty() {
        return Err(address("the fragment is empty"));
    }
    if is_selector(text) {
        return parse_selectors(text).map(Fragment::Selectors);
    }
    if is_snake_case(text) {
        return Ok(Fragment::Field(text.to_string()));
    }
    Err(GrammarError {
        code: "field_invalid",
        reason: "the fragment must be a snake_case field name or selectors like seq=1-5 or L10-L24",
    })
}

impl Fragment {
    pub fn canonical(&self) -> String {
        match self {
            Fragment::Field(f) => f.clone(),
            Fragment::Selectors(s) => s.canonical(),
        }
    }
}

/// Parses `&`-joined selectors. Lenient: text may hold raw spaces and non-ASCII, and a raw `&` that is not followed
/// by a known key and `=` (or `L` and a digit) belongs to the text before it. Text values are percent-decoded.
pub fn parse_selectors(text: &str) -> Result<Selectors, GrammarError> {
    let mut s = Selectors::default();
    for part in split_parts(text) {
        if starts_lines(part) {
            put(&mut s.lines, lines_range(&part[1..])?)?;
            continue;
        }
        let (key, value) = part.split_once('=').ok_or(selector(
            "use seq= L section= quote= prefix= suffix= or page=",
        ))?;
        match key {
            "seq" => put(&mut s.seq, range(value)?)?,
            "page" => {
                let r = range(value)?;
                if r[0] == 0 {
                    return Err(selector("page numbers start at 1"));
                }
                put(&mut s.page, r)?
            }
            "section" => {
                let path = value
                    .split('/')
                    .map(decode)
                    .collect::<Result<Vec<_>, _>>()?;
                if path.iter().any(String::is_empty) {
                    return Err(selector("a section heading is empty"));
                }
                put(&mut s.section, path)?
            }
            "quote" => {
                let q = decode(value)?;
                if q.is_empty() {
                    return Err(selector("the quote is empty"));
                }
                put(&mut s.quote, q)?
            }
            "prefix" => put(&mut s.prefix, decode(value)?)?,
            "suffix" => put(&mut s.suffix, decode(value)?)?,
            _ => {
                return Err(selector(
                    "unknown selector (use seq= L section= quote= prefix= suffix= or page=)",
                ));
            }
        }
    }
    s.drop_empty();
    s.check()?;
    Ok(s)
}

/// Splits at each `&` that starts a known selector.
fn split_parts(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, _) in text.match_indices('&') {
        let after = &text[i + 1..];
        let known = starts_lines(after)
            || KEYS
                .iter()
                .any(|k| after.strip_prefix(k).is_some_and(|r| r.starts_with('=')));
        if known {
            parts.push(&text[start..i]);
            start = i + 1;
        }
    }
    parts.push(&text[start..]);
    parts
}

fn put<T>(slot: &mut Option<T>, value: T) -> Result<(), GrammarError> {
    if slot.is_some() {
        return Err(selector("a selector kind appears twice"));
    }
    *slot = Some(value);
    Ok(())
}

fn number(text: &str) -> Result<u64, GrammarError> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(selector("a range must be whole numbers like 120-140"));
    }
    text.parse()
        .map_err(|_| selector("a range must be whole numbers like 120-140"))
}

fn ordered(a: u64, b: u64) -> Result<Range, GrammarError> {
    if a > b {
        return Err(selector("a range must not end before it starts"));
    }
    Ok([a, b])
}

/// `n` or `n-m`.
fn range(text: &str) -> Result<Range, GrammarError> {
    match text.split_once('-') {
        Some((a, b)) => ordered(number(a)?, number(b)?),
        None => {
            let a = number(text)?;
            Ok([a, a])
        }
    }
}

/// After the leading `L`: `n`, `n-m` or `n-Lm`. Lines start at 1.
fn lines_range(text: &str) -> Result<Range, GrammarError> {
    let r = match text.split_once('-') {
        Some((a, b)) => ordered(number(a)?, number(b.strip_prefix('L').unwrap_or(b))?)?,
        None => {
            let a = number(text)?;
            [a, a]
        }
    };
    if r[0] == 0 {
        return Err(selector("line numbers start at 1"));
    }
    Ok(r)
}

/// Percent-decodes text. Lenient: a `%` not followed by two hex digits is a literal `%`.
fn decode(text: &str) -> Result<String, GrammarError> {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (b.get(i + 1).and_then(hex), b.get(i + 2).and_then(hex))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).map_err(|_| selector("the percent-encoded text is not UTF-8"))
}

fn hex(b: &u8) -> Option<u8> {
    (*b as char).to_digit(16).map(|d| d as u8)
}

/// Bytes kept raw in canonical text: RFC 3986 unreserved plus the sub-delimiters that mean nothing to this grammar,
/// a markdown link, a shell or a Cypher single-quoted string. Everything else is `%XX` (upper case).
fn keep(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!*+,:;=@".contains(&b)
}

fn encode(text: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in text.as_bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
    }
}

fn push_range(out: &mut String, key: &str, r: Range, l: bool) {
    use core::fmt::Write;
    out.push_str(key);
    let _ = write!(out, "{}", r[0]);
    if r[1] != r[0] {
        out.push('-');
        if l {
            out.push('L');
        }
        let _ = write!(out, "{}", r[1]);
    }
}

impl Selectors {
    fn check(&self) -> Result<(), GrammarError> {
        if (self.prefix.is_some() || self.suffix.is_some()) && self.quote.is_none() {
            return Err(selector("prefix= and suffix= need quote="));
        }
        if *self == Selectors::default() {
            return Err(selector("there is no selector"));
        }
        Ok(())
    }

    /// The canonical string: keys in the order seq, L, section, quote, prefix, suffix, page; text percent-encoded.
    pub fn canonical(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(r) = self.seq {
            let mut p = String::new();
            push_range(&mut p, "seq=", r, false);
            parts.push(p);
        }
        if let Some(r) = self.lines {
            let mut p = String::new();
            push_range(&mut p, "L", r, true);
            parts.push(p);
        }
        if let Some(path) = &self.section {
            let mut p = String::from("section=");
            for (i, h) in path.iter().enumerate() {
                if i > 0 {
                    p.push('/');
                }
                encode(h, &mut p);
            }
            parts.push(p);
        }
        for (key, text) in [
            ("quote=", &self.quote),
            ("prefix=", &self.prefix),
            ("suffix=", &self.suffix),
        ] {
            if let Some(t) = text {
                let mut p = String::from(key);
                encode(t, &mut p);
                parts.push(p);
            }
        }
        if let Some(r) = self.page {
            let mut p = String::new();
            push_range(&mut p, "page=", r, false);
            parts.push(p);
        }
        parts.join("&")
    }

    /// The stored evidence object: `seq`, `lines` and `page` as `[from, to]`, `section` as an array of headings,
    /// `quote`, `prefix` and `suffix` as decoded text.
    pub fn to_object(&self) -> Value {
        let mut m = Map::new();
        let pair = |r: Range| Value::Array(alloc::vec![r[0].into(), r[1].into()]);
        if let Some(r) = self.seq {
            m.insert("seq".into(), pair(r));
        }
        if let Some(r) = self.lines {
            m.insert("lines".into(), pair(r));
        }
        if let Some(path) = &self.section {
            m.insert(
                "section".into(),
                Value::Array(path.iter().cloned().map(Value::String).collect()),
            );
        }
        for (key, text) in [
            ("quote", &self.quote),
            ("prefix", &self.prefix),
            ("suffix", &self.suffix),
        ] {
            if let Some(t) = text {
                m.insert(key.into(), Value::String(t.clone()));
            }
        }
        if let Some(r) = self.page {
            m.insert("page".into(), pair(r));
        }
        Value::Object(m)
    }

    /// Reads a stored evidence selector object back, with the same checks as the string form.
    pub fn from_object(value: &Value) -> Result<Selectors, GrammarError> {
        let bad = selector("the stored selector object is not valid");
        let map = value.as_object().ok_or(bad)?;
        let pair = |v: &Value| -> Result<Range, GrammarError> {
            match v.as_array().map(Vec::as_slice) {
                Some([a, b]) => ordered(a.as_u64().ok_or(bad)?, b.as_u64().ok_or(bad)?),
                _ => Err(bad),
            }
        };
        let text = |v: &Value| v.as_str().map(ToString::to_string).ok_or(bad);
        let mut s = Selectors::default();
        for (key, v) in map {
            match key.as_str() {
                "seq" => s.seq = Some(pair(v)?),
                "lines" => {
                    let r = pair(v)?;
                    if r[0] == 0 {
                        return Err(selector("line numbers start at 1"));
                    }
                    s.lines = Some(r);
                }
                "page" => {
                    let r = pair(v)?;
                    if r[0] == 0 {
                        return Err(selector("page numbers start at 1"));
                    }
                    s.page = Some(r);
                }
                "section" => {
                    let path = v
                        .as_array()
                        .ok_or(bad)?
                        .iter()
                        .map(text)
                        .collect::<Result<Vec<_>, _>>()?;
                    if path.is_empty() || path.iter().any(String::is_empty) {
                        return Err(selector("a section heading is empty"));
                    }
                    s.section = Some(path);
                }
                "quote" => {
                    let q = text(v)?;
                    if q.is_empty() {
                        return Err(selector("the quote is empty"));
                    }
                    s.quote = Some(q);
                }
                "prefix" => s.prefix = Some(text(v)?),
                "suffix" => s.suffix = Some(text(v)?),
                _ => return Err(bad),
            }
        }
        s.drop_empty();
        s.check()?;
        Ok(s)
    }

    fn drop_empty(&mut self) {
        for t in [&mut self.prefix, &mut self.suffix] {
            if t.as_deref() == Some("") {
                *t = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One row of `fixtures/address/table.json`.
    struct Row {
        input: String,
        expect: Result<String, (String, Option<String>)>,
        object: Option<Value>,
    }

    fn table() -> Vec<Row> {
        let v: Value =
            serde_json::from_str(include_str!("../fixtures/address/table.json")).unwrap();
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| Row {
                input: r["input"].as_str().unwrap().to_string(),
                expect: match r["canonical"].as_str() {
                    Some(c) => Ok(c.to_string()),
                    None => Err((
                        r["error"].as_str().unwrap().to_string(),
                        r["reason"].as_str().map(ToString::to_string),
                    )),
                },
                object: r.get("object").cloned(),
            })
            .collect()
    }

    /// Parses a row's input: a full address, or a bare fragment written `#...`.
    fn parse(input: &str) -> Result<String, GrammarError> {
        match input.strip_prefix('#') {
            Some(f) => parse_fragment(f).map(|f| alloc::format!("#{}", f.canonical())),
            None => parse_address(input).map(|a| a.canonical()),
        }
    }

    #[test]
    fn every_fixture_row_gives_its_canonical_form_or_error() {
        let rows = table();
        assert!(rows.len() >= 40, "the table lost rows");
        for row in rows {
            let got = parse(&row.input);
            match &row.expect {
                Ok(canonical) => {
                    assert_eq!(got.as_ref().ok(), Some(canonical), "{}", row.input);
                    assert_eq!(
                        parse(canonical).as_ref(),
                        Ok(canonical),
                        "canonical is a fixed point: {}",
                        row.input
                    );
                }
                Err((code, reason)) => {
                    let e = got.expect_err(&row.input);
                    assert_eq!(e.code, code, "{}", row.input);
                    if let Some(r) = reason {
                        assert_eq!(e.reason, r, "{}", row.input);
                    }
                }
            }
        }
    }

    #[test]
    fn graph_addresses_have_no_node_id() {
        for input in ["umx:demo", "UMX:demo", "Umx:demo?repo=https://x/y.git"] {
            let a = parse_address(input).unwrap();
            assert_eq!(a.id, None);
            assert_eq!(a.canonical(), input.replacen(&input[..4], "umx:", 1));
        }
        assert_eq!(
            parse_address("umx:demo/01jc8x").unwrap().id.as_deref(),
            Some("01JC8X")
        );
    }

    /// R2: every selector row maps to its stored object and back to the same canonical string.
    #[test]
    fn selector_objects_map_both_ways_without_loss() {
        let mut checked = 0;
        for row in table() {
            let Ok(canonical) = &row.expect else { continue };
            let frag = match canonical.split_once('#') {
                Some((_, f)) => f.split("?repo=").next().unwrap(),
                None => continue,
            };
            let Ok(Fragment::Selectors(s)) = parse_fragment(frag) else {
                continue;
            };
            let object = s.to_object();
            if let Some(want) = &row.object {
                assert_eq!(&object, want, "{}", row.input);
            }
            let back = Selectors::from_object(&object).unwrap();
            assert_eq!(back, s, "{}", row.input);
            assert_eq!(back.canonical(), frag, "{}", row.input);
            checked += 1;
        }
        assert!(checked >= 20, "only {checked} selector rows");
    }

    #[test]
    fn stored_objects_from_the_first_parser_read_back() {
        for (object, canonical) in [
            (json!({ "seq": [120, 140] }), "seq=120-140"),
            (json!({ "lines": [7, 7] }), "L7"),
            (json!({ "lines": [10, 24] }), "L10-L24"),
            (
                json!({ "quote": "we ship & test", "prefix": "so", "suffix": "today" }),
                "quote=we%20ship%20%26%20test&prefix=so&suffix=today",
            ),
        ] {
            let s = Selectors::from_object(&object).unwrap();
            assert_eq!(s.canonical(), canonical);
            assert_eq!(s.to_object(), object, "byte for byte");
            assert_eq!(parse_selectors(canonical).unwrap(), s);
        }
        // The first parser stored an empty prefix and undecoded text; such objects read to one canonical string.
        let legacy = json!({ "quote": "50%20 off", "prefix": "" });
        let s = Selectors::from_object(&legacy).unwrap();
        assert_eq!(s.canonical(), "quote=50%2520%20off");
        assert_eq!(s.to_object(), json!({ "quote": "50%20 off" }));
    }

    #[test]
    fn bad_stored_objects_are_refused() {
        for v in [
            json!("seq=1-2"),
            json!({}),
            json!({ "seq": [2, 1] }),
            json!({ "seq": [1] }),
            json!({ "lines": [0, 3] }),
            json!({ "quote": "" }),
            json!({ "prefix": "a" }),
            json!({ "section": [] }),
            json!({ "section": ["a", ""] }),
            json!({ "t": [1, 2] }),
        ] {
            assert_eq!(
                Selectors::from_object(&v).unwrap_err().code,
                "selector_invalid",
                "{v}"
            );
        }
    }

    #[test]
    fn every_byte_value_survives_encode_and_decode() {
        let text: String = (1u32..0x2FF).filter_map(char::from_u32).collect();
        let s = Selectors {
            quote: Some(text.clone()),
            section: Some(alloc::vec![text.clone(), "a/b".into()]),
            ..Selectors::default()
        };
        let c = s.canonical();
        assert!(c.bytes().all(|b| keep(b) || b"%&=/".contains(&b)), "{c}");
        assert_eq!(parse_selectors(&c).unwrap(), s);
    }

    #[test]
    fn reasons_survive_a_slot_word() {
        let src = include_str!("address.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap()];
        let mut seen = 0;
        for key in ["selector(", "address(", "reason:"] {
            for (i, _) in src.match_indices(key) {
                let rest = src[i + key.len()..].trim_start();
                let Some(lit) = rest.strip_prefix('"') else {
                    continue;
                };
                let reason = &lit[..lit.find('"').unwrap()];
                seen += 1;
                assert!(
                    reason
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || " ._/+~:=<>()-".contains(c)),
                    "{reason}"
                );
            }
        }
        assert!(seen >= 15, "found only {seen} reasons");
    }
}
