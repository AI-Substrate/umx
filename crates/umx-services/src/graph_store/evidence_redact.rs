//! `umx node evidence <node> remove <pointer> --redact --reason <why>` (F183, security's ruling; review round 1).
//!
//! The pointers a redact scrubs are every pointer in the graph whose source has the identity of the one the named
//! node holds: the scheme folded (`umx:`, `http:`, `https:` in any case), a `umx:` fetch hint dropped and a `umx:`
//! id in canonical text, with path and URL text kept as written (R1). The pointer given names the node's by that
//! identity too, so its scheme's case is never the input's (F203 N4). On a node a live one moves to the trail as a
//! redacted entry `{kind, redacted, removed_at, by, by_session, reason}` and a trail one is redacted in place; an
//! edge drops it and records the edit. A status move citing one (ws008 TM6, F204) keeps the move, its `evidence`
//! made a redacted entry of the same shape in place.
//!
//! Before anything is persisted, every byte that stays is checked for the needles: the source spellings and
//! selector strings of every pointer scrubbed, and of the pointer given. Checked are every record the write adds or
//! replaces, read from the batch as it would be persisted: the participants with the attribution the write stamps
//! (keys, typed fields, the redacted entries' kept metadata and the session included; R2, F203 N1) and the session,
//! agent and other provenance records it upserts (review R1). Also checked: the stored line of each node it leaves
//! as it is (a hand edit's duplicate member included, F203 N2), each other edge as umx writes it (edges.jsonl is
//! always rewritten from the records, S365-1), each repeated line kept in nodes.jsonl and edges.jsonl, and graph.json
//! as it is on disk (R5). All are read as decoded JSON strings, so an escaped spelling counts (R4), and a needle
//! matches exactly or under Unicode's full case folding, one character at a time (R6, F203 N3). Any hit refuses the
//! whole redact, naming the record and a fixed place (a standard field's name, a list position, or a custom field's
//! ordinal), never the text or a custom field's name (R3). Once the write is in, the graph's undo line (the last line
//! it replaced) is dropped (F196).

use super::evidence_edit::{
    BY_SESSION, EVIDENCE_REASON_MISSING, EvidenceRemoved, REDACTED, REMOVED, REMOVED_AT, Redacted,
    check_reason, key, names, pointer_to_match, stamp_removals, trail, trail_len,
};
use super::graph::files_problem;
use super::{Ctx, Done, GraphStore, Outcome, alias_name, problems};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::address::split_repo;
use umx_core::codec::GraphFile;
use umx_core::evidence::EVIDENCE_REMOVED as TRAIL;
use umx_core::id::normalise_id_text;
use umx_core::name::{RESERVED_EDGE_FIELDS, RESERVED_NODE_FIELDS};
/// The place helpers (`field 3`, `field:note`, `evidence:1`, `edge:<id>/...`): the one source of every place string
/// the redact's candidates, `check --lint` and the read flags (`secret_suspected`) name, in umx-core (F224).
pub(super) use umx_core::place::{custom_place, edge_place, list_place, named_place};
use umx_core::problem::Candidate;
use umx_core::{Edge, Node, Value};

/// A pointer's source.
pub(super) fn source_of(p: &Value) -> &str {
    p.get("source").and_then(Value::as_str).unwrap_or("")
}

/// The identity a redact matches sources on (R1): `umx:`, `http:` and `https:` folded to lower case, a `umx:` fetch
/// hint dropped and its id in canonical text; the rest as written (paths and URL text are case-sensitive).
pub(super) fn identity(source: &str) -> String {
    for scheme in ["umx:", "https://", "http://"] {
        let Some(head) = source.get(..scheme.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(scheme) {
            continue;
        }
        let rest = &source[scheme.len()..];
        if scheme != "umx:" {
            return format!("{scheme}{rest}");
        }
        let body = split_repo(rest).map_or(rest, |(body, _)| body);
        return match body.split_once('/') {
            Some((alias, id)) => match normalise_id_text(id).filter(|id| !id.is_empty()) {
                Some(id) => format!("umx:{alias}/{id}"),
                None => format!("umx:{body}"),
            },
            None => format!("umx:{body}"),
        };
    }
    source.to_string()
}

/// Folds case one character at a time with Unicode's full case folding (`ß` and `ẞ` to `ss`, `ς` to `σ`, `ſ` to `s`,
/// ligatures spelt out; F203 N3): lower case first, then [`super::casefold::fold_lowered`]. One character at a time,
/// the result never depends on context (no final sigma, R6). F139's term scan matches with this same fold.
pub(super) fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match super::casefold::fold_lowered(c) {
            Some(folded) => out.push_str(folded),
            None => out.push(c),
        }
    }
    out
}

/// The text a redact refuses to leave anywhere (security), each needle case-folded. Folding one character at a time
/// maps a text that holds a needle exactly to one that holds the folded needle, so the folded check is also the
/// exact one (R6).
struct Needles(Vec<String>);

impl Needles {
    fn add(&mut self, text: &str) {
        let folded = fold(text);
        if !folded.is_empty() && !self.0.contains(&folded) {
            self.0.push(folded);
        }
    }

    /// A pointer's stored source and every string value in its selector (a quote, a heading; not the selector's
    /// own key names, which every such pointer shares).
    fn add_pointer(&mut self, p: &Value) {
        self.add(source_of(p));
        let mut texts = Vec::new();
        if let Some(selector) = p.get("selector") {
            selector_values(selector, &mut texts);
        }
        for t in texts {
            self.add(&t);
        }
    }

    /// Whether `text` holds a needle, in any case (R6).
    fn hit(&self, text: &str) -> bool {
        let folded = fold(text);
        self.0.iter().any(|n| folded.contains(n.as_str()))
    }

    /// Whether a record's line may hold a needle in one of its JSON strings, without parsing it (F197). A line of
    /// ASCII with no escape holds each string's characters as they are, and folding ASCII is lower-casing it, so a
    /// folded string is a substring of the lower-cased line: a needle not in that line is in none of its strings
    /// (and a needle with other characters cannot be). Any other line may hold one. `buf` is scratch space.
    fn may_hit(&self, line: &[u8], buf: &mut Vec<u8>) -> bool {
        if !line.is_ascii() || line.contains(&b'\\') {
            return true;
        }
        buf.clear();
        buf.extend(line.iter().map(u8::to_ascii_lowercase));
        let lower = core::str::from_utf8(buf).unwrap_or_default();
        self.0
            .iter()
            .any(|n| n.is_ascii() && lower.contains(n.as_str()))
    }
}

/// Every string value in a selector.
pub(super) fn selector_values(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|i| selector_values(i, out)),
        Value::Object(map) => map.values().for_each(|i| selector_values(i, out)),
        _ => {}
    }
}

/// A JSON text read for its strings (R4, R5): every key and string value decoded (escapes included), duplicate
/// members kept, nothing rebuilt. None when the text is not JSON.
struct Json<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Json<'a> {
    fn new(b: &'a [u8]) -> Self {
        Json { b, at: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.at).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.peek()? == c).then(|| self.at += 1)
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.b.get(self.at..self.at + 4)?;
        self.at += 4;
        u32::from_str_radix(core::str::from_utf8(digits).ok()?, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.at += 1;
            match c {
                b'"' => return Some(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = self.peek()?;
                    self.at += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi)
                                && self.b.get(self.at..self.at + 2) == Some(b"\\u")
                            {
                                self.at += 2;
                                let lo = self.hex4()?;
                                0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF)
                            } else {
                                hi
                            };
                            char::from_u32(code).unwrap_or('\u{FFFD}')
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c => out.push(c),
            }
        }
    }

    /// Any value: its strings go to `out`.
    fn value(&mut self, out: &mut Vec<String>) -> Option<()> {
        self.ws();
        match self.peek()? {
            b'"' => out.push(self.string()?),
            b'{' => {
                self.at += 1;
                self.ws();
                if self.peek()? == b'}' {
                    self.at += 1;
                    return Some(());
                }
                loop {
                    out.push(self.string()?);
                    self.eat(b':')?;
                    self.value(out)?;
                    self.ws();
                    match self.peek()? {
                        b',' => self.at += 1,
                        b'}' => {
                            self.at += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.at += 1;
                self.ws();
                if self.peek()? == b']' {
                    self.at += 1;
                    return Some(());
                }
                loop {
                    self.value(out)?;
                    self.ws();
                    match self.peek()? {
                        b',' => self.at += 1,
                        b']' => {
                            self.at += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            _ => {
                let start = self.at;
                while !matches!(
                    self.peek(),
                    None | Some(b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t')
                ) {
                    self.at += 1;
                }
                if self.at == start {
                    return None;
                }
            }
        }
        Some(())
    }

    /// Every string in a whole JSON text (one value, or one value per line), or None when it is not JSON.
    fn all(b: &'a [u8]) -> Option<Vec<String>> {
        let mut j = Json::new(b);
        let mut out = Vec::new();
        loop {
            j.ws();
            if j.peek().is_none() {
                return Some(out);
            }
            j.value(&mut out)?;
        }
    }

    /// A one-object line's members: each key with the strings of its value (by element when it is an array), so a
    /// hit can be placed. None when the line is not one JSON object.
    fn members(b: &'a [u8]) -> Option<Vec<(String, Vec<Vec<String>>)>> {
        let mut j = Json::new(b);
        j.eat(b'{')?;
        let mut members = Vec::new();
        j.ws();
        if j.peek()? == b'}' {
            return Some(members);
        }
        loop {
            let key = j.string()?;
            j.eat(b':')?;
            j.ws();
            let mut parts = Vec::new();
            if j.peek()? == b'[' {
                j.at += 1;
                j.ws();
                if j.peek()? == b']' {
                    j.at += 1;
                } else {
                    loop {
                        let mut strings = Vec::new();
                        j.value(&mut strings)?;
                        parts.push(strings);
                        j.ws();
                        match j.peek()? {
                            b',' => j.at += 1,
                            b']' => {
                                j.at += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
            } else {
                let mut strings = Vec::new();
                j.value(&mut strings)?;
                parts.push(strings);
            }
            members.push((key, parts));
            j.ws();
            match j.peek()? {
                b',' => j.at += 1,
                b'}' => return Some(members),
                _ => return None,
            }
        }
    }
}

/// One member of a record's line as stored: its key, its number among the record's custom fields (None for a
/// standard field) and the strings of its value (by element when it is an array).
pub(super) struct Member {
    pub key: String,
    pub custom: Option<usize>,
    pub parts: Vec<Vec<String>>,
}

/// A record line's members in stored order, each custom one numbered from 1 among the record's custom fields as the
/// line holds them (a member written twice counts twice). The one count behind `field N` for both the redact's
/// candidates and `check --lint`'s places, so the two always agree (F214). None when the line is not one JSON object.
pub(super) fn members(line: &[u8], standard: &[&str]) -> Option<Vec<Member>> {
    let mut custom = 0;
    Json::members(line.trim_ascii()).map(|members| {
        members
            .into_iter()
            .map(|(key, parts)| {
                let known = standard.contains(&key.as_str());
                if !known {
                    custom += 1;
                }
                Member {
                    custom: (!known).then_some(custom),
                    key,
                    parts,
                }
            })
            .collect()
    })
}

/// Where in one record's line the needles sit, as fixed places (R3): a standard field by its name, a position in a
/// standard list (`evidence:2`), or a custom field by its ordinal among the record's custom fields (`field 3`, see
/// [`members`]), never its name. A line umx cannot read is one place, `record`, so the check fails closed.
fn places(line: &[u8], standard: &[&str], needles: &Needles) -> Vec<String> {
    let Some(members) = members(line, standard) else {
        return alloc::vec!["record".to_string()];
    };
    let mut out = Vec::new();
    for Member { key, custom, parts } in members {
        let place = |at: Option<usize>| match (custom, at) {
            (None, Some(i)) => list_place(&key, i),
            (None, None) => key.clone(),
            (Some(n), Some(i)) => list_place(&custom_place(n), i),
            (Some(n), None) => custom_place(n),
        };
        if needles.hit(&key) {
            out.push(place(None));
            continue;
        }
        let list = (custom.is_none() && key_is_list(&key)) || parts.len() > 1;
        for (i, strings) in parts.iter().enumerate() {
            if strings.iter().any(|s| needles.hit(s)) {
                out.push(place(list.then_some(i)));
            }
        }
    }
    out
}

/// The standard fields that are lists, placed by position.
fn key_is_list(key: &str) -> bool {
    matches!(key, "evidence" | "status_history" | TRAIL)
}

/// Whether a pointer is one the redact scrubs.
fn scrubbed(p: &Value, target: &str) -> bool {
    identity(source_of(p)) == target
}

/// [`names`] with both sources read by their identity (R1), so the scheme's case is never the input's (F203 N4).
fn names_by_identity(given: &Value, stored: &Value) -> bool {
    let by_identity = |p: &Value| {
        let mut p = p.clone();
        let source = identity(source_of(&p));
        p["source"] = Value::String(source);
        p
    };
    names(&by_identity(given), &by_identity(stored))
}

/// The pointers a node's status moves cite (ws008 TM6).
fn status_pointers(n: &Node) -> impl Iterator<Item = &Value> {
    n.status_history.iter().filter_map(|m| m.get("evidence"))
}

/// Whether a node holds a pointer the redact scrubs, live, in its trail or cited by a status move (R7: only these
/// must have a usable trail).
fn participates(n: &Node, target: &str) -> bool {
    n.evidence.iter().any(|p| scrubbed(p, target))
        || status_pointers(n).any(|p| scrubbed(p, target))
        || n.fields
            .get(TRAIL)
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|e| e.get(REMOVED).is_some_and(|p| scrubbed(p, target)))
            })
}

/// A redacted trail entry: the pointer's kind, `redacted`, and when, who and why. No source, selector or hash.
fn redacted_entry(kind: Option<&Value>, reason: &str, kept: &[(&str, &Value)]) -> Value {
    let mut entry = Value::Object(Default::default());
    if let Some(kind) = kind {
        entry["kind"] = kind.clone();
    }
    entry[REDACTED] = Value::Bool(true);
    for (k, v) in kept {
        entry[*k] = (*v).clone();
    }
    entry["reason"] = Value::String(reason.into());
    entry
}

/// Redacts a participating node's pointers with the identity (see the module docs); returns how many, and the
/// positions of the status moves whose pointer it redacted (F204: only those get this redact's session, R345-3).
fn redact_node(
    n: &mut Node,
    target: &str,
    reason: &str,
    now: &str,
    by: &str,
) -> Result<(usize, Vec<usize>), umx_core::Problem> {
    let mut entries = trail(n)?;
    let mut count = 0;
    for entry in entries.iter_mut() {
        if entry.get(REMOVED).is_some_and(|p| scrubbed(p, target)) {
            let kind = entry[REMOVED].get("kind").cloned();
            let kept: Vec<(&str, Value)> = [REMOVED_AT, "by", BY_SESSION]
                .into_iter()
                .filter_map(|k| entry.get(k).map(|v| (k, v.clone())))
                .collect();
            let kept: Vec<(&str, &Value)> = kept.iter().map(|(k, v)| (*k, v)).collect();
            *entry = redacted_entry(kind.as_ref(), reason, &kept);
            count += 1;
        }
    }
    let (gone, live): (Vec<Value>, Vec<Value>) =
        n.evidence.drain(..).partition(|p| scrubbed(p, target));
    n.evidence = live;
    let (now, by) = (Value::String(now.into()), Value::String(by.into()));
    for p in &gone {
        entries.push(redacted_entry(
            p.get("kind"),
            reason,
            &[(REMOVED_AT, &now), ("by", &by)],
        ));
        count += 1;
    }
    if !entries.is_empty() || n.fields.contains_key(TRAIL) {
        n.fields.insert(TRAIL.to_string(), Value::Array(entries));
    }
    // F204: a status move citing it keeps the move; only its pointer is redacted, in place.
    let mut moves = Vec::new();
    for (i, change) in n.status_history.iter_mut().enumerate() {
        if let Some(p) = change.get_mut("evidence").filter(|p| scrubbed(p, target)) {
            *p = redacted_entry(p.get("kind"), reason, &[(REMOVED_AT, &now), ("by", &by)]);
            count += 1;
            moves.push(i);
        }
    }
    Ok((count, moves))
}

/// Stamps the redacting session on the pointers of the status moves at `moves`, the ones this redact made
/// redacted entries (F204), as `stamp_removals` stamps the trail's. Other moves keep their bytes (R345-3).
fn stamp_status_redactions(n: &mut Node, moves: &[usize], session: umx_core::Id) {
    for &i in moves {
        if let Some(p) = n
            .status_history
            .get_mut(i)
            .and_then(|m| m.get_mut("evidence"))
        {
            p[BY_SESSION] = Value::String(session.to_text());
        }
    }
}

/// A refusal's place: the record (or file) and a fixed place label.
fn place(id: String, place: String) -> Candidate {
    Candidate {
        id,
        title: Some(place),
        score: None,
    }
}

impl GraphStore {
    /// `umx node evidence <node> remove <pointer> --redact --reason <text>` (see the module docs). The node must hold
    /// the pointer, live, in its trail or cited by one of its status moves; one identity must answer it (R1). The reason is checked as remove checks
    /// it, and may neither hold a needle nor look like a secret. Refusals name no pointer text (R3). The reply counts
    /// what was scrubbed. Other graphs are not touched.
    pub fn node_evidence_redact(
        &mut self,
        ctx: &Ctx,
        node: &str,
        pointer: &str,
        reason: Option<&str>,
    ) -> Outcome<EvidenceRemoved> {
        let reason = check_reason(reason, "evidence", EVIDENCE_REASON_MISSING)?;
        let (alias, named) = self.node_for_change(ctx, node)?;
        let given = pointer_to_match(&self.get(&alias).dir, pointer)?;
        let graph = alias_name(&alias);
        let id = named.id;

        // R1: the stored pointers the given one names on this node, live or in its trail, and their one identity. Both
        // are read by identity, so a URL scheme typed in another case names the stored one (F203 N4). R3: neither
        // refusal lists the node's pointers.
        let named_trail = trail(&named)?;
        let identities: BTreeSet<String> = named
            .evidence
            .iter()
            .chain(named_trail.iter().filter_map(|t| t.get(REMOVED)))
            .chain(status_pointers(&named))
            .filter(|p| names_by_identity(&given, p))
            .map(|p| identity(source_of(p)))
            .collect();
        let target = match identities.len() {
            0 => return Err(problems::evidence_not_found(&graph, id)),
            1 => identities.into_iter().next().unwrap_or_default(),
            n => return Err(problems::evidence_ambiguous(&graph, id, n)),
        };

        // The holders, found before anything is decoded (F197): a node can hold a pointer with the target identity
        // only if its line may hold the folded identity (`Needles::may_hit`), and only those are decoded. A `umx:`
        // identity writes its id in canonical form, which the stored text need not hold, so with one every node is.
        let l = self.get(&alias);
        let probe = Needles(alloc::vec![fold(&target)]);
        let every = target.starts_with("umx:");
        let mut buf = Vec::new();
        let mut holders: Vec<Node> = Vec::new();
        let mut held: BTreeSet<umx_core::Id> = BTreeSet::new();
        for r in l.graph.nodes() {
            if every || probe.may_hit(r.line(), &mut buf) {
                let n = r.to_node();
                if participates(&n, &target) {
                    held.insert(n.id);
                    holders.push(n);
                }
            }
        }
        let edges: Vec<Edge> = l.graph.edges().map(|e| e.to_edge()).collect();
        // The needles: the given pointer's text and every scrubbed pointer's, as stored (only holders hold one).
        let mut needles = Needles(Vec::new());
        needles.add(&key(&given).0);
        needles.add_pointer(&given);
        for n in &holders {
            let trail_pointers = n
                .fields
                .get(TRAIL)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|t| t.get(REMOVED));
            for p in n
                .evidence
                .iter()
                .chain(trail_pointers)
                .chain(status_pointers(n))
            {
                if scrubbed(p, &target) {
                    needles.add_pointer(p);
                }
            }
        }
        for e in &edges {
            for p in e.evidence.iter().filter(|p| scrubbed(p, &target)) {
                needles.add_pointer(p);
            }
        }
        if needles.hit(&reason) {
            return Err(problems::usage_invalid(
                "--reason may not hold the pointer or any part of it (say why in other words)",
                "evidence",
            ));
        }
        if umx_core::secrets::scan(&reason).is_some() {
            return Err(problems::usage_invalid(
                "--reason looks like it holds a secret (say why in other words)",
                "evidence",
            ));
        }

        // Every record as it stays (R2, R7: only holders change). A node the redact leaves as it is keeps its stored
        // line byte for byte, so that line is what is read (F203 N2); one that cannot hold a needle is passed over
        // unparsed (F197). The records the write puts down are read inside it, below.
        let now = self.now();
        let mut counts = Redacted::default();
        let mut changed_nodes: Vec<(Node, usize, Vec<usize>)> = Vec::new();
        let mut blocking: Vec<Candidate> = Vec::new();
        for r in l.graph.nodes() {
            let line = r.line();
            if held.contains(&r.id()) || !needles.may_hit(line, &mut buf) {
                continue;
            }
            for p in places(line, RESERVED_NODE_FIELDS, &needles) {
                blocking.push(place(r.id().to_text(), p));
            }
        }
        for mut n in holders {
            // A holder's trail length before the redact: the entries after it get this write's session.
            let from = trail_len(&n);
            let (count, moves) = redact_node(&mut n, &target, &reason, &now, &ctx.by)?;
            counts.pointers += count;
            counts.nodes += 1;
            n.updated = now.clone();
            changed_nodes.push((n, from, moves));
        }
        let mut changed_edges: Vec<Edge> = Vec::new();
        for mut e in edges {
            let before = e.evidence.len();
            e.evidence.retain(|p| !scrubbed(p, &target));
            if e.evidence.len() < before {
                changed_edges.push(e);
                continue;
            }
            // edges.jsonl is always rewritten from the records (below), so an unchanged edge is read as umx writes it.
            let line = umx_core::codec::encode_edges([&e]);
            if !needles.may_hit(&line, &mut buf) {
                continue;
            }
            for p in places(&line, RESERVED_EDGE_FIELDS, &needles) {
                blocking.push(place(e.id.to_text(), p));
            }
        }
        counts.edges = changed_edges.len();

        // What stays byte for byte: repeated lines (R4) and graph.json as it is on disk (R5), read as JSON strings.
        let kept = |line: &[u8]| Json::all(line).is_none_or(|ss| ss.iter().any(|s| needles.hit(s)));
        if l.graph.kept_node_lines().any(kept) {
            blocking.push(place(
                "nodes.jsonl".into(),
                "a repeated line (umx check lists it; keep one line per id)".into(),
            ));
        }
        if l.graph.kept_edge_lines().any(kept) {
            blocking.push(place(
                "edges.jsonl".into(),
                "a repeated line (umx check lists it; keep one line per id)".into(),
            ));
        }
        let dir = l.dir.clone();
        let meta = self
            .ports
            .files
            .read(&dir, GraphFile::Graph)
            .map_err(|e| files_problem(&dir, &self.roots, GraphFile::Graph, e))?
            .unwrap_or_default();
        if kept(&meta) {
            blocking.push(place("graph.json".into(), "the graph's metadata".into()));
        }

        // One checked write (F144's files_unchanged, temp, sync, rename) for every record that changes. edges.jsonl is
        // in it even when no edge changes: umx-graph keeps no edge line bytes, so an edge line as stored (a hand edit's
        // duplicate member) would otherwise stay on disk unread (S365-1).
        let mut extra =
            super::edge::external_files(&changed_edges.iter().collect::<Vec<_>>()).to_vec();
        extra.push(GraphFile::Edges);
        let done = self.attributed_write_with(ctx, &alias, &extra, |batch, session, _, _, _| {
            for (mut n, from, moves) in changed_nodes {
                if !umx_core::provenance::is_provenance(&n) {
                    n.record_edit(session);
                }
                stamp_removals(&mut n, from, session);
                stamp_status_redactions(&mut n, &moves, session);
                batch.insert_node(n).map_err(|e| problems::image_full(&e))?;
            }
            for mut e in changed_edges {
                e.record_edit(session);
                batch.insert_edge(e);
            }
            // Every record this write adds or replaces, read as it would be persisted: the participants with the
            // attribution the write stamps on them (F203 N1), and the session, agent and other provenance records it
            // upserted (review R1). A refusal returns before anything is persisted: the batch is dropped and undone.
            let touched = batch.touched();
            for id in touched.nodes.added.iter().chain(&touched.nodes.edited) {
                if let Some(n) = batch.graph().node(*id) {
                    for p in places(n.line(), RESERVED_NODE_FIELDS, &needles) {
                        blocking.push(place(id.to_text(), p));
                    }
                }
            }
            for id in touched.edges.added.iter().chain(&touched.edges.edited) {
                if let Some(e) = batch.edge_record(*id) {
                    for p in places(
                        &umx_core::codec::encode_edges([&e]),
                        RESERVED_EDGE_FIELDS,
                        &needles,
                    ) {
                        blocking.push(place(id.to_text(), p));
                    }
                }
            }
            if !blocking.is_empty() {
                let mut seen = BTreeSet::new();
                blocking.retain(|c| seen.insert((c.id.clone(), c.title.clone())));
                return Err(problems::evidence_redact_blocked(&graph, blocking));
            }
            Ok(())
        })?;
        // F196 (storage review): the graph's undo slot holds the last line the write replaced, a scrubbed one; drop it.
        if let Some(l) = self.graphs.get_mut(&alias) {
            l.graph.forget_undo();
        }
        let mut warnings = alloc::vec![problems::redacted_in_files_only(id)];
        warnings.extend(done.notes);
        Ok(Done {
            value: EvidenceRemoved {
                alias,
                id,
                removed: None,
                redacted: Some(counts),
            },
            notes: warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    /// F213: `Needles::may_hit` is sound only if folding ASCII is lower-casing it. No ASCII character has a fold
    /// beyond lower case, so a regenerated table that gave one would fail here.
    #[test]
    fn folding_ascii_is_lower_casing_it() {
        for c in (0u8..=0x7f).map(char::from) {
            assert_eq!(super::super::casefold::fold_lowered(c), None, "{c:?}");
        }
        let ascii: alloc::string::String = (0u8..=0x7f).map(char::from).collect();
        assert_eq!(super::fold(&ascii), ascii.to_ascii_lowercase());
    }
}
