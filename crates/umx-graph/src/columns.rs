//! Borrowed column readers for whole-graph reads that must not build a node per row (spine propose, ws005 s07):
//! one custom field's holders in handle order, and a node's evidence and status moves read from its line alone.
//! A custom field read by a handle resolved once (`FieldId`, ws005 s14), so a per-node read does no name lookup.

use crate::Graph;
use crate::json::{self, Kind, Refused, Scanned};
use crate::line::{self, FieldVal};
use crate::view::{NodeIx, NodeRef};
use alloc::borrow::Cow;
use alloc::string::ToString;
use umx_core::Value;
use umx_core::time::{DateSpan, Precision};

/// One custom field value as its column holds it, borrowed where the column keeps it. A string borrows the node's
/// line (it is owned only when it holds an escape); a list, object or null lives in the line and is decoded.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldRef<'a> {
    UInt(u64),
    Int(i64),
    Float(f64),
    Str(Cow<'a, str>),
    Bool(bool),
    Other(Value),
}

impl FieldRef<'_> {
    /// The value as `NodeRef::field` gives it.
    pub fn to_value(&self) -> Value {
        match self {
            FieldRef::UInt(u) => Value::from(*u),
            FieldRef::Int(i) => Value::from(*i),
            FieldRef::Float(f) => json::Num::F(*f).value(),
            FieldRef::Str(s) => Value::String(s.clone().into_owned()),
            FieldRef::Bool(b) => Value::Bool(*b),
            FieldRef::Other(v) => v.clone(),
        }
    }
}

/// An interned custom field name, resolved once by `Graph::field_id` so a per-node read does no name lookup.
///
/// Valid only for the `Graph` that issued it. Names are append-only within a graph (interned, never removed), so a
/// `FieldId` stays good across that graph's writes. A `FieldId` from another graph, or from before a reload, reads a
/// wrong or absent field: a caller bug. Resolve it per graph, per query.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FieldId(pub(crate) u32);

/// A node's `evidence` and `status_history` as JSON array text, read from its line through the load's scanner.
///
/// The text is the node line's own bytes, or, for a line the scanner leaves to the codec, the codec's values written
/// out again: equal as JSON to `to_node().evidence` and `to_node().status_history`, never byte-equal to anything.
/// Deserialise it to read it; never write it back into a line.
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceAndMoves<'g> {
    evidence: Cow<'g, str>,
    moves: Cow<'g, str>,
}

impl EvidenceAndMoves<'_> {
    /// The evidence pointers: a JSON array.
    pub fn evidence_json(&self) -> &str {
        &self.evidence
    }

    /// The status moves, oldest first: a JSON array.
    pub fn moves_json(&self) -> &str {
        &self.moves
    }
}

impl Graph {
    /// The handle of custom field `name`, or `None` when this graph never interned the name (every read is null).
    /// An interned name no node holds as a field gives a handle that `NodeRef::field_ref` answers `None` for.
    pub fn field_id(&self, name: &str) -> Option<FieldId> {
        self.names.get(name).map(FieldId)
    }

    /// Every node holding custom field `name`, in handle order, with its value: only the rows the column holds are
    /// visited. Provenance nodes are left out unless `with_provenance`. An unknown name visits nothing.
    pub fn each_field<'g>(
        &'g self,
        name: &str,
        with_provenance: bool,
        mut f: impl FnMut(NodeIx, FieldRef<'g>),
    ) {
        let Some(col) = self.names.get(name).and_then(|n| self.f_cols.get(&n)) else {
            return;
        };
        col.each_in_order(|h, v| {
            let ix = NodeIx(h);
            if !with_provenance && self.is_provenance(ix) {
                return;
            }
            f(ix, self.field_ref_at(h, name, v));
        });
    }

    /// Every node whose custom field `name` reads as a date, in handle order, with its span of UTC time and precision
    /// from the typed time column (D71 step 10): no line is read and no text parsed. Provenance nodes are left out
    /// unless `with_provenance`. An unknown name visits nothing.
    pub fn each_date(
        &self,
        name: &str,
        with_provenance: bool,
        mut f: impl FnMut(NodeIx, DateSpan, Precision),
    ) {
        let Some(col) = self.names.get(name).and_then(|n| self.f_cols.get(&n)) else {
            return;
        };
        col.each_date(|h, d, p| {
            let ix = NodeIx(h);
            if with_provenance || !self.is_provenance(ix) {
                f(ix, d, p);
            }
        });
    }

    /// Row `h`'s value `v` of custom field `name` as a `FieldRef`: the one mapping `each_field` and
    /// `NodeRef::field_ref` share.
    fn field_ref_at(&self, h: u32, name: &str, v: FieldVal) -> FieldRef<'_> {
        match v {
            FieldVal::Num(json::Num::U(u)) => FieldRef::UInt(u),
            FieldVal::Num(json::Num::I(i)) => FieldRef::Int(i),
            FieldVal::Num(json::Num::F(x)) => FieldRef::Float(x),
            FieldVal::Bool(b) => FieldRef::Bool(b),
            FieldVal::Str { rel, len, esc } => {
                let at = self.line_at[h as usize] as usize + rel as usize;
                let raw = core::str::from_utf8(&self.image[at..at + len as usize])
                    .expect("checked as UTF-8 at load");
                FieldRef::Str(json::text(raw, esc))
            }
            FieldVal::Other => FieldRef::Other(self.field_value(h as usize, name, v)),
        }
    }
}

/// The two keys' array spans from a line the scanner takes; refused (left to the codec) for any line it is unsure
/// of, including one where either key is missing, repeated or not an array.
fn scan(bytes: &[u8]) -> Scanned<(&str, &str)> {
    let (mut evidence, mut moves) = (None, None);
    json::object(bytes, |s, k, v| {
        let key = json::text(&s[k.start..k.end], k.esc);
        let slot = match &*key {
            "evidence" => &mut evidence,
            "status_history" => &mut moves,
            _ => return Ok(()),
        };
        if slot.is_some() || v.kind != Kind::Arr {
            return Err(Refused);
        }
        *slot = Some(&s[v.start..v.end]);
        Ok(())
    })?;
    evidence.zip(moves).ok_or(Refused)
}

impl<'g> NodeRef<'g> {
    /// Custom field `f` of this node from its column, with no name lookup; `None` when the node does not hold it.
    /// The same value as `custom_field` for `f`'s name. `f` must come from this node's graph (see `FieldId`).
    pub fn field_ref(self, f: FieldId) -> Option<FieldRef<'g>> {
        let v = self.g.f_cols.get(&f.0)?.get(self.h)?;
        Some(self.g.field_ref_at(self.h, self.g.names.text(f.0), v))
    }

    /// Custom field `f`'s date from the typed time column (D71 step 10): its span of UTC time and precision, the same
    /// as `umx_core::time::date_span` of the value's text, or `None` when the node does not hold `f` or its value is
    /// not a string that reads as a date. No line is read and no text parsed.
    pub fn date_at(self, f: FieldId) -> Option<(DateSpan, Precision)> {
        self.g.f_cols.get(&f.0)?.date(self.h)
    }

    /// This node's evidence and status moves, read from its line alone, without decoding the note or the custom
    /// fields: borrowed from the line, or for a line the scanner is unsure of, decoded whole as `to_node` does.
    pub fn evidence_and_moves(self) -> EvidenceAndMoves<'g> {
        let bytes = self.g.line(self.h as usize);
        match scan(bytes) {
            Ok((evidence, moves)) => EvidenceAndMoves {
                evidence: Cow::Borrowed(evidence),
                moves: Cow::Borrowed(moves),
            },
            Err(_) => {
                let node =
                    line::decode_node(bytes, 0).expect("every line of the image decoded at load");
                EvidenceAndMoves {
                    evidence: Cow::Owned(Value::Array(node.evidence).to_string()),
                    moves: Cow::Owned(Value::Array(node.status_history).to_string()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use umx_core::Id;

    fn id(n: u128) -> Id {
        Id((0x0194_0000_0000_u128 << 80) | n)
    }

    fn line(n: u128, extra: &str, evidence: &str, history: &str) -> String {
        format!(
            "{{\"id\":\"{}\",\"type\":\"point\",\"title\":\"t{n}\",\"status\":\"open\",\"note\":\"a \\\"note\\\"\",\
             \"evidence\":{evidence},\"status_history\":{history},\"created\":\"2026-11-12T00:00:00Z\",\
             \"updated\":\"2026-11-12T00:00:00Z\",\"created_by\":\"t\"{extra}}}\n",
            id(n)
        )
    }

    fn graph(lines: &[String]) -> Graph {
        let bytes: Vec<u8> = lines.concat().into_bytes();
        Graph::load_nodes(bytes).expect("fixture loads").0
    }

    #[test]
    fn each_field_matches_field_on_every_node_for_every_value_kind_in_handle_order() {
        let values = [
            ",\"x\":7",
            ",\"x\":-3",
            ",\"x\":2.5",
            ",\"x\":\"plain\"",
            ",\"x\":\"tab\\there \\\"q\\\" \\u00e9\"",
            ",\"x\":true",
            ",\"x\":[1,\"a\"]",
            ",\"x\":{\"k\":1}",
            ",\"x\":null",
            "",
            ",\"y\":1",
        ];
        let lines: Vec<String> = values
            .iter()
            .enumerate()
            .map(|(i, extra)| line(i as u128 + 1, extra, "[]", "[]"))
            .collect();
        let g = graph(&lines);
        let mut seen = Vec::new();
        g.each_field("x", false, |ix, v| seen.push((ix.index(), v.to_value())));
        let expected: Vec<(usize, Value)> = g
            .nodes()
            .filter_map(|n| n.field("x").map(|v| (n.ix().index(), v)))
            .collect();
        assert_eq!(seen, expected);
        assert_eq!(seen.len(), 9);
        let mut none = 0;
        g.each_field("absent", false, |_, _| none += 1);
        assert_eq!(none, 0);
    }

    #[test]
    fn field_ref_equals_custom_field_and_each_field_on_every_node_for_every_value_kind() {
        let values = [
            ",\"x\":7",
            ",\"x\":18446744073709551615",
            ",\"x\":-3",
            ",\"x\":2.5",
            ",\"x\":\"plain\"",
            ",\"x\":\"tab\\there \\\"q\\\" \\u00e9\"",
            ",\"x\":true",
            ",\"x\":[1,\"a\"]",
            ",\"x\":{\"k\":1}",
            ",\"x\":null",
            "",
            ",\"y\":1",
        ];
        let lines: Vec<String> = values
            .iter()
            .enumerate()
            .map(|(i, extra)| line(i as u128 + 1, extra, "[]", "[]"))
            .collect();
        let g = graph(&lines);
        let x = g.field_id("x").expect("x is interned");
        let mut each = Vec::new();
        g.each_field("x", true, |ix, v| each.push((ix.index(), v)));
        let mut held = Vec::new();
        for n in g.nodes() {
            let got = n.field_ref(x);
            assert_eq!(
                got.as_ref().map(FieldRef::to_value),
                n.custom_field("x"),
                "{}",
                n.id()
            );
            if let Some(v) = got {
                held.push((n.ix().index(), v));
            }
        }
        assert_eq!(held, each);
        assert_eq!(held.len(), 10);
        assert!(matches!(held[1].1, FieldRef::UInt(u64::MAX)));
        assert!(matches!(&held[4].1, FieldRef::Str(Cow::Borrowed("plain"))));
        assert!(matches!(&held[5].1, FieldRef::Str(Cow::Owned(s)) if s == "tab\there \"q\" é"));
        assert_eq!(g.field_id("absent"), None);
        // Interned as a type name, never a field: a handle that no node holds.
        let kind = g.field_id("point").expect("the type name is interned");
        assert!(g.nodes().all(|n| n.field_ref(kind).is_none()));
    }

    #[test]
    fn evidence_and_moves_equal_the_decoded_record_on_every_node() {
        let ev = "[{\"source\":\"a \\\"b\\\".md\",\"selector\":{\"seq\":[3,9]},\"kind\":\"room\",\"fidelity\":\"x\",\"hash\":null}]";
        let mv = "[{\"from\":\"open\",\"to\":\"done\",\"at\":\"2026-11-12T01:00:00Z\",\"by\":\"t\",\"reason\":\"r\\u00e9\"}]";
        let t = id(6).to_text();
        // Nesting deeper than the scanner goes: the line is left to the codec.
        let deep = format!(",\"z\":{}1{}", "[".repeat(120), "]".repeat(120));
        let mut lines = vec![
            line(1, "", "[]", "[]"),
            line(2, "", ev, "[ ]"),
            line(3, "", "[]", mv),
            // Kept raw by the codec: a locator that is not one and a session that is not an id.
            line(4, ",\"locator\":7,\"authored_by_session\":\"bad\"", ev, mv),
            line(5, &deep, ev, mv),
            // A surrogate pair, which the scanner reads itself.
            line(7, ",\"z\":\"\\ud83d\\ude00\"", ev, mv),
        ];
        // Keys in another order than umx writes, with spaces inside the arrays.
        lines.push(format!(
            "{{\"status_history\": [ ],\"id\":\"{t}\",\"type\":\"point\",\"title\":\"t\",\"status\":\"open\",\
             \"note\":\"\",\"created\":\"2026-11-12T00:00:00Z\",\"updated\":\"2026-11-12T00:00:00Z\",\
             \"created_by\":\"t\",\"evidence\": [ {{\"source\":\"x.md\",\"selector\":{{\"lines\":[1, 2]}}}} ]}}\n"
        ));
        let g = graph(&lines);
        let as_values = |s: &str| serde_json::from_str::<Vec<Value>>(s).unwrap();
        for n in g.nodes() {
            let got = n.evidence_and_moves();
            let whole = n.to_node();
            assert_eq!(as_values(got.evidence_json()), whole.evidence, "{}", n.id());
            assert_eq!(
                as_values(got.moves_json()),
                whole.status_history,
                "{}",
                n.id()
            );
            let fallback = matches!(got.evidence, Cow::Owned(_));
            assert_eq!(
                fallback,
                n.id() == id(5),
                "only the deep line is decoded whole"
            );
        }
    }

    /// D71 step 10: the typed time column at load equals `date_span` of each value's decoded text, on lines the
    /// scanner reads and on a line it leaves to the codec. An escaped digit (`\u0032026-12-01`) is a date only once
    /// decoded; a number, a list and text that is no date hold none; each_date visits exactly the dated rows.
    #[test]
    fn date_at_is_date_span_of_the_decoded_text_on_every_row() {
        let values = [
            ",\"x\":\"2026-12-01\"",
            ",\"x\":\"\\u0032026-12-01\"",
            ",\"x\":\"2025-01\"",
            ",\"x\":\"1969-12-31\"",
            ",\"x\":\"1969-07-20T20:17:40-05:00\"",
            ",\"x\":\"0000\"",
            ",\"x\":\"2026-10-30T09:40:00+11:00\"",
            ",\"x\":\" 2025\"",
            ",\"x\":\"soon\"",
            ",\"x\":\"2026-13-01\"",
            ",\"x\":2025",
            ",\"x\":[\"2025\"]",
            "",
            // Left to the codec (a locator that is not one): its value is read from the record.
            ",\"locator\":7,\"x\":\"2024-02\"",
        ];
        let lines: Vec<String> = values
            .iter()
            .enumerate()
            .map(|(i, extra)| line(i as u128 + 1, extra, "[]", "[]"))
            .collect();
        let g = graph(&lines);
        let x = g.field_id("x").unwrap();
        let mut want = Vec::new();
        for n in g.nodes() {
            let d = match n.field("x") {
                Some(Value::String(s)) => umx_core::time::date_span(&s),
                _ => None,
            };
            assert_eq!(n.date_at(x), d.map(|d| (d, d.precision())), "{}", n.id());
            if let Some(d) = d {
                want.push((n.ix(), d, d.precision()));
            }
        }
        let mut seen = Vec::new();
        g.each_date("x", false, |ix, d, p| seen.push((ix, d, p)));
        assert_eq!(seen, want);
        // The escaped digit, the padded year, the date-time before 1970 and the codec's month are dates; text, year
        // 0000, a number and a list are not.
        assert_eq!(seen.len(), 8);
        let codec = g.node(id(14)).unwrap();
        assert!(matches!(codec.field_ref(x), Some(FieldRef::Other(_))));
        assert!(codec.date_at(x).is_some());
    }
}
