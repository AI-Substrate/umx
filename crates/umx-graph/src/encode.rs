//! File lines from the columns: the same bytes `umx_core::codec` writes for the record (fields in struct order,
//! custom fields after them in name order, compact JSON). Tests compare the two on every fixture.

use crate::Graph;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use umx_core::Id;
use umx_core::Value;
use umx_core::codec::KeptLine;

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const HEX: &[u8; 16] = b"0123456789abcdef";

fn push_id(out: &mut Vec<u8>, id: Id) {
    let mut text = [0u8; 28];
    text[0] = b'"';
    text[27] = b'"';
    let mut v = id.0;
    for slot in text[1..27].iter_mut().rev() {
        *slot = CROCKFORD[(v & 31) as usize];
        v >>= 5;
    }
    out.extend_from_slice(&text);
}

/// A JSON string as serde_json writes it: `"` and `\` escaped, control characters as their short form or
/// `\u00xx`, everything else (non-ASCII included) as is.
pub(crate) fn push_str(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let short: &[u8] = match b {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0x08 => b"\\b",
            0x0c => b"\\f",
            0..=0x1f => b"",
            _ => continue,
        };
        out.extend_from_slice(&bytes[start..i]);
        if short.is_empty() {
            out.extend_from_slice(b"\\u00");
            out.push(HEX[usize::from(b >> 4)]);
            out.push(HEX[usize::from(b & 15)]);
        } else {
            out.extend_from_slice(short);
        }
        start = i + 1;
    }
    out.extend_from_slice(&bytes[start..]);
    out.push(b'"');
}

/// Lets `write!` append to a byte buffer.
struct Bytes<'a>(&'a mut Vec<u8>);

impl Write for Bytes<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// `Value`'s `Display` is serde_json's compact writer, the one codec uses (umx-graph has no JSON dependency of its
/// own; tests compare the bytes with codec).
fn push_value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::String(s) => push_str(out, s),
        v => write!(Bytes(out), "{v}").expect("writing to a Vec cannot fail"),
    }
}

fn push_fields(out: &mut Vec<u8>, fields: &[(alloc::boxed::Box<str>, Value)]) {
    for (k, v) in fields {
        out.push(b',');
        push_str(out, k);
        out.push(b':');
        push_value(out, v);
    }
}

/// The canonical bytes of an edge's evidence list, as the line writes it (the slot stores them, req-124).
pub(crate) fn evidence_bytes(evidence: &[Value]) -> alloc::boxed::Box<[u8]> {
    let mut out = Vec::new();
    push_value(&mut out, &Value::Array(evidence.to_vec()));
    out.into_boxed_slice()
}

pub(crate) fn edge_line(g: &Graph, e: usize, out: &mut Vec<u8>) {
    let id = g.e_ids[e];
    out.extend_from_slice(b"{\"id\":");
    push_id(out, id);
    out.extend_from_slice(b",\"type\":");
    push_str(out, g.names.text(g.e_kind.get(e)));
    let (from_graph, from) = g.end_parts(g.e_from[e]);
    let (to_graph, to) = g.end_parts(g.e_to[e]);
    if let Some(graph) = from_graph {
        out.extend_from_slice(b",\"from_graph\":");
        push_id(out, graph);
    }
    out.extend_from_slice(b",\"from\":");
    push_id(out, from);
    if let Some(graph) = to_graph {
        out.extend_from_slice(b",\"to_graph\":");
        push_id(out, graph);
    }
    out.extend_from_slice(b",\"to\":");
    push_id(out, to);
    out.extend_from_slice(b",\"created\":");
    g.e_created.write(e, id, out);
    out.extend_from_slice(b",\"created_by\":");
    push_str(out, g.names.text(g.e_by.get(e)));
    if let Some(session) = g.provenance.edge_authored.get(e) {
        out.extend_from_slice(b",\"authored_by_session\":");
        push_id(out, session);
    }
    if let Some(session) = g.provenance.edge_edited.get(id) {
        out.extend_from_slice(b",\"edited_by_session\":");
        push_id(out, session);
    }
    if let Some(evidence) = g.e_evidence.get(&id) {
        out.extend_from_slice(b",\"evidence\":");
        out.extend_from_slice(evidence);
    }
    if let Some(fields) = g.e_fields.get(&id) {
        push_fields(out, fields);
    }
    out.push(b'}');
}

/// The lines of one file in id order, each kept line just before the in-use line of its id (or in id order when
/// none is in use), kept lines of one id in the order given: `codec::encode_*_keeping`'s layout.
pub(crate) fn lines(
    ids: &[Id],
    kept: &[KeptLine],
    per_line: usize,
    mut line: impl FnMut(usize, &mut Vec<u8>),
) -> Vec<u8> {
    let mut out = Vec::with_capacity(ids.len() * per_line);
    let mut order: Vec<usize> = (0..kept.len()).collect();
    order.sort_by_key(|&i| kept[i].id);
    let mut order = order.into_iter().peekable();
    let put_kept = |i: usize, out: &mut Vec<u8>| {
        out.extend_from_slice(&kept[i].line);
        out.push(b'\n');
    };
    for (h, id) in ids.iter().enumerate() {
        while let Some(i) = order.next_if(|&i| kept[i].id <= *id) {
            put_kept(i, &mut out);
        }
        line(h, &mut out);
        out.push(b'\n');
    }
    for i in order {
        put_kept(i, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn strings_escape_as_serde_json_does() {
        let mut every: String = (0u8..0x80).map(char::from).collect();
        every.push_str("é ☃ 𝄞 \u{2028}");
        for s in [every.as_str(), "", "plain", "a\"b\\c"] {
            let mut out = Vec::new();
            push_str(&mut out, s);
            assert_eq!(out, serde_json::to_vec(s).unwrap(), "{s:?}");
        }
        for v in [
            serde_json::json!(1),
            serde_json::json!(1.0),
            serde_json::json!(-0.5e-7),
            serde_json::json!(u64::MAX),
            serde_json::json!(null),
            serde_json::json!([true, "a\"b", {"z": 1, "a": [1.5, {}]}]),
        ] {
            let mut out = Vec::new();
            push_value(&mut out, &v);
            assert_eq!(out, serde_json::to_vec(&v).unwrap(), "{v}");
        }
        let mut out = Vec::new();
        push_id(&mut out, Id(u128::MAX >> 2));
        assert_eq!(out, serde_json::to_vec(&Id(u128::MAX >> 2)).unwrap());
    }
}
