//! Spines (time review R2, brief 5.2): an ordered projection of a graph. A spine names its anchors (the nodes that
//! are its bands, and the link that nests them in a parent), what orders them (a date field, a step number or a
//! chain relation, first that applies, worked out among the anchors under one parent) and which links place other
//! nodes on them, strongest first. `umx graph spine propose` builds these from a graph and stores nothing; the
//! builder confirms one later (TM5), and this is the type it stores. Pure: no graph, no I/O.

use crate::name::is_valid_name;
use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `place` entry that means "by position inside the anchor's source", not a relation.
pub const PLACE_EVIDENCE: &str = "evidence";

/// The only relation that nests anchors in a parent (graph-addressing ruling T1): a step orders only among the
/// anchors with the same `part_of` parent.
pub const INSIDE: &str = crate::ontology::PART_OF;

/// At most this many spines one graph declares, and order and place entries one declaration holds (security N1):
/// a declaration comes from a socket client too, so its size is bounded where it is checked.
pub const MAX_SPINES: usize = 32;
pub const MAX_ORDER: usize = 16;
pub const MAX_PLACE: usize = 16;
/// A title's length bound, in characters (as a description's).
pub const MAX_TITLE_CHARS: usize = crate::ontology::MAX_DESCRIPTION_CHARS;

/// One spine, as proposed or declared. Every name in it is a vocabulary name ([`SpineDecl::check`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpineDecl {
    pub name: String,
    pub title: String,
    /// The builder's primary reading of the graph. Never true in a proposal.
    pub main: bool,
    pub anchors: Anchors,
    /// Fields or chain relations, first that applies.
    pub order: Vec<String>,
    /// Relations, strongest first; [`PLACE_EVIDENCE`] for evidence position.
    pub place: Vec<String>,
}

/// Which nodes are the bands, and the relation (if any) that nests them in a parent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchors {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub inside: Option<String>,
}

/// Where in a [`SpineDecl`] a name is not a plain identifier. Never carries the name itself (it may be graph text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadName {
    Name,
    AnchorType,
    Inside,
    Order(usize),
    Place(usize),
}

impl SpineDecl {
    /// Within [`MAX_ORDER`] and [`MAX_PLACE`].
    pub fn within_caps(&self) -> bool {
        self.order.len() <= MAX_ORDER && self.place.len() <= MAX_PLACE
    }

    /// The title as stored: one line, 1 to [`MAX_TITLE_CHARS`] characters, no leading or trailing space. Kept
    /// exactly as given, so one that would need trimming is refused, never changed.
    pub fn title_ok(&self) -> bool {
        let t = self.title.as_str();
        !t.is_empty()
            && t.trim() == t
            && !t.contains(['\n', '\r'])
            && t.chars().count() <= MAX_TITLE_CHARS
    }

    /// A declaration as JSON gives it (`graph spine set --file`): exactly the keys a stored one has. None for any
    /// other shape; the reason is not kept (it may quote the input).
    pub fn from_value(v: &Value) -> Option<SpineDecl> {
        SpineDecl::deserialize(v).ok()
    }

    /// Every name is a vocabulary name: snake_case and not a reserved word. `place` may also hold
    /// [`PLACE_EVIDENCE`], which is one already.
    pub fn check(&self) -> Result<(), BadName> {
        let ok = |s: &str| is_valid_name(s);
        if !ok(&self.name) {
            return Err(BadName::Name);
        }
        if !ok(&self.anchors.kind) {
            return Err(BadName::AnchorType);
        }
        if self.anchors.inside.as_deref().is_some_and(|i| !ok(i)) {
            return Err(BadName::Inside);
        }
        if let Some(i) = self.order.iter().position(|o| !ok(o)) {
            return Err(BadName::Order(i));
        }
        if let Some(i) = self.place.iter().position(|p| !ok(p)) {
            return Err(BadName::Place(i));
        }
        Ok(())
    }
}

/// Whether graph.json's stored spines are ones umx could have written (R366-3): each passes [`SpineDecl::check`], the
/// caps and the title rule, the names are unique, at most one is main, and there are at most [`MAX_SPINES`]. A file
/// that fails is refused at load, so nothing later meets a stored name it cannot use.
pub fn stored_ok(spines: &[SpineDecl]) -> bool {
    let mut names = alloc::collections::BTreeSet::new();
    spines.len() <= MAX_SPINES
        && spines.iter().filter(|s| s.main).count() <= 1
        && spines.iter().all(|s| {
            s.check().is_ok() && s.within_caps() && s.title_ok() && names.insert(s.name.as_str())
        })
}

/// What sort of clock a spine runs on (brief 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    /// Real dates and times.
    Calendar,
    /// A position inside one thing (a sequence number in a transcript).
    Local,
    /// Steps or a chain: order and nothing else.
    Order,
}

impl Clock {
    pub fn as_str(self) -> &'static str {
        match self {
            Clock::Calendar => "calendar",
            Clock::Local => "local",
            Clock::Order => "order",
        }
    }
}

/// How the nodes of a graph sit on one spine. Each node is counted once, by its strongest placement (brief 5.3:
/// its own field, a link, its evidence position, then by rule), so the five parts add up to `total`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cover {
    pub total: usize,
    pub by_field: usize,
    pub by_link: usize,
    pub by_evidence: usize,
    /// Placed only by the service's rule (ws008 TM5): beside the nodes it links to that are placed by data.
    pub by_rule: usize,
    pub not_placed: usize,
}

impl Cover {
    pub fn placed(self) -> usize {
        self.total - self.not_placed
    }
}

/// How the placed nodes spread over a spine's bands. A spine that puts every node in one band is no use (B5).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spread {
    pub bands: usize,
    /// The largest band's share of the nodes in bands, 0 to 1, to 3 places; 0 when there are none.
    pub largest_band_share: f64,
    pub distinct_values: usize,
}

/// What a position counts in: transcript sequence numbers or file lines. Positions in different units never meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unit {
    Seq,
    Lines,
}

impl Unit {
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Seq => "seq",
            Unit::Lines => "lines",
        }
    }
}

/// Where one evidence pointer sits in its source: the named source (see [`source_key`]), the unit and an inclusive
/// range. Only a `seq` or `lines` selector has a position.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub source: String,
    pub unit: Unit,
    pub start: u64,
    pub end: u64,
}

impl Position {
    /// The position of a stored evidence object (`{ source, selector: { seq | lines: [from, to] } }`), `seq` before
    /// `lines` when it has both. None for a source that names no file position (see [`source_key`]) or a selector
    /// with neither range.
    pub fn of_evidence(evidence: &Value) -> Option<Position> {
        let selector = evidence.get("selector")?;
        let (unit, range) = [(Unit::Seq, "seq"), (Unit::Lines, "lines")]
            .into_iter()
            .find_map(|(unit, key)| Some((unit, selector.get(key)?.as_array()?)))?;
        let (start, end) = match range.as_slice() {
            [a, b] => (a.as_u64()?, b.as_u64()?),
            _ => return None,
        };
        Position::new(evidence.get("source")?.as_str()?, unit, start, end)
    }

    fn new(source: &str, unit: Unit, start: u64, end: u64) -> Option<Position> {
        let source = source_key(source)?;
        (start <= end).then_some(Position {
            source,
            unit,
            start,
            end,
        })
    }
}

/// What [`positions`] reads of one stored pointer, borrowed from the text. Anything else in it is skipped.
#[derive(Deserialize)]
struct Head<'a> {
    #[serde(borrow, default)]
    source: Option<Cow<'a, str>>,
    #[serde(default)]
    selector: Option<Ranges>,
}

#[derive(Deserialize)]
struct Ranges {
    #[serde(default)]
    seq: Option<[u64; 2]>,
    #[serde(default)]
    lines: Option<[u64; 2]>,
}

impl Head<'_> {
    fn position(&self) -> Option<Position> {
        let r = self.selector.as_ref()?;
        let (unit, [a, b]) = match (r.seq, r.lines) {
            (Some(s), _) => (Unit::Seq, s),
            (None, Some(l)) => (Unit::Lines, l),
            (None, None) => return None,
        };
        Position::new(self.source.as_deref()?, unit, a, b)
    }
}

/// Pushes each pointer's position as the array streams past, so no list of pointers is built.
struct Into<'v>(&'v mut Vec<Position>);

impl<'de> serde::de::Visitor<'de> for Into<'_> {
    type Value = ();

    fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("an array of evidence pointers")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(head) = seq.next_element::<Head<'de>>()? {
            self.0.extend(head.position());
        }
        Ok(())
    }
}

/// Appends the positions of a node's evidence, given as its JSON array text, to `out` in order: the same as
/// [`Position::of_evidence`] on each pointer, without building the pointers. A pointer that is not in the shape umx
/// writes sends the whole array the slow way, through [`Position::of_evidence`].
pub fn positions(evidence_json: &str, out: &mut Vec<Position>) {
    use serde::Deserializer as _;
    let len = out.len();
    let mut de = serde_json::Deserializer::from_str(evidence_json);
    if de
        .deserialize_seq(Into(out))
        .and_then(|()| de.end())
        .is_err()
    {
        out.truncate(len);
        if let Ok(items) = serde_json::from_str::<Vec<Value>>(evidence_json) {
            out.extend(items.iter().filter_map(Position::of_evidence));
        }
    }
}

/// The `at` of a status move, borrowed from the text.
#[derive(Deserialize)]
struct MoveAt<'a> {
    #[serde(borrow, default)]
    at: Option<Cow<'a, str>>,
}

/// When the last of a node's status moves (given as its JSON array text) was made, as the start of its date span in
/// seconds; None when there is none or its `at` is not a date.
pub fn latest_move_start(moves_json: &str) -> Option<i64> {
    let at = match serde_json::from_str::<Vec<MoveAt<'_>>>(moves_json) {
        Ok(moves) => moves.into_iter().next_back()?.at?,
        Err(_) => {
            let moves = serde_json::from_str::<Vec<Value>>(moves_json).ok()?;
            Cow::Owned(moves.last()?.get("at")?.as_str()?.into())
        }
    };
    crate::time::date_span(&at).map(|s| s.start)
}

/// The name two pointers must share to be compared: the source's locator normal form, so `./t.vtt`, `t.vtt` and
/// `.//t.vtt` are one source. None for a `umx:` source (a passage of a node is not a file position), a folder, or
/// text that does not normalise: those are never compared, not even as text.
// TODO: swap in S3b's named/key_source rule once it is on main, so a graph-folder path and its `root:repo/`
// form are one source too. One line: the body of this function.
pub fn source_key(source: &str) -> Option<String> {
    if source
        .get(..4)
        .is_some_and(|p| p.eq_ignore_ascii_case("umx:"))
    {
        return None;
    }
    match crate::locator::normalise(source) {
        Ok((kind, normal)) if kind != crate::locator::LocatorKind::Folder => Some(normal),
        _ => None,
    }
}

/// Whether two positions share a stretch of the same source, in the same unit (inclusive ranges).
pub fn overlap(a: &Position, b: &Position) -> bool {
    a.unit == b.unit && a.source == b.source && a.start <= b.end && b.start <= a.end
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn decl() -> SpineDecl {
        SpineDecl {
            name: "segment".into(),
            title: "segment by starts_on".into(),
            main: false,
            anchors: Anchors {
                kind: "segment".into(),
                inside: Some(INSIDE.into()),
            },
            order: vec!["starts_on".into(), "step".into(), "precedes".into()],
            place: vec!["raised_in".into(), PLACE_EVIDENCE.into()],
        }
    }

    #[test]
    fn a_declaration_round_trips_with_type_as_its_key_and_refuses_unknown_keys() {
        let d = decl();
        let text = serde_json::to_string(&d).unwrap();
        assert!(text.contains("\"anchors\":{\"type\":\"segment\",\"inside\":\"part_of\"}"));
        assert_eq!(serde_json::from_str::<SpineDecl>(&text).unwrap(), d);
        let extra = text.replacen("\"main\":false", "\"main\":false,\"zone\":\"x\"", 1);
        assert!(serde_json::from_str::<SpineDecl>(&extra).is_err());
    }

    #[test]
    fn check_names_where_a_name_is_not_a_plain_identifier() {
        assert_eq!(decl().check(), Ok(()));
        let mut d = decl();
        d.order[1] = "Step".to_string();
        assert_eq!(d.check(), Err(BadName::Order(1)));
        let mut d = decl();
        d.place[0] = "match".to_string();
        assert_eq!(d.check(), Err(BadName::Place(0)));
        let mut d = decl();
        d.anchors.inside = Some("part-of".into());
        assert_eq!(d.check(), Err(BadName::Inside));
        let mut d = decl();
        d.anchors.kind = String::new();
        assert_eq!(d.check(), Err(BadName::AnchorType));
    }

    fn evidence(source: &str, selector: Value) -> Value {
        serde_json::json!({ "source": source, "selector": selector, "kind": "room", "fidelity": "x", "hash": null })
    }

    #[test]
    fn positions_meet_only_in_the_same_named_source_and_unit() {
        let seq = |s: &str, a: u64, b: u64| {
            Position::of_evidence(&evidence(s, serde_json::json!({ "seq": [a, b] }))).unwrap()
        };
        let a = seq("./t/day.ndjson", 10, 20);
        assert_eq!(a.source, "t/day.ndjson");
        assert!(overlap(&a, &seq("t//day.ndjson", 20, 30)));
        assert!(!overlap(&a, &seq("t/day.ndjson", 21, 30)));
        assert!(!overlap(&a, &seq("t/other.ndjson", 10, 20)));
        let lines = Position::of_evidence(&evidence(
            "t/day.ndjson",
            serde_json::json!({ "lines": [10, 20] }),
        ));
        assert!(!overlap(&a, &lines.unwrap()));
    }

    #[test]
    fn a_pointer_with_no_file_position_has_no_position() {
        for (source, selector) in [
            (
                "umx:demo/01J0000000000000000000000A",
                serde_json::json!({ "seq": [1, 2] }),
            ),
            (
                "UMX:demo/01J0000000000000000000000A",
                serde_json::json!({ "seq": [1, 2] }),
            ),
            ("t/", serde_json::json!({ "seq": [1, 2] })),
            ("t/day.ndjson", serde_json::json!({ "quote": "x" })),
            ("t/day.ndjson", serde_json::json!({ "seq": [5, 2] })),
            ("", serde_json::json!({ "seq": [1, 2] })),
        ] {
            assert_eq!(
                Position::of_evidence(&evidence(source, selector)),
                None,
                "{source}"
            );
        }
    }

    #[test]
    fn positions_from_text_match_of_evidence_on_every_pointer_shape() {
        use serde_json::json;
        let shapes = [
            json!({ "source": "./t\"q.ndjson", "selector": { "seq": [1, 4] } }),
            json!({ "source": "t.ndjson", "selector": { "lines": [2, 3], "quote": "x" }, "kind": "room" }),
            json!({ "source": "t.ndjson", "selector": { "seq": null, "lines": [2, 3] } }),
            json!({ "source": "t.ndjson", "selector": { "seq": "x", "lines": [2, 3] } }),
            json!({ "source": "t.ndjson", "selector": { "seq": [1] } }),
            json!({ "source": "t.ndjson", "selector": { "seq": [1.0, 2] } }),
            json!({ "source": "t.ndjson", "selector": "seq=1-2" }),
            json!({ "source": 7, "selector": { "seq": [1, 2] } }),
            json!({ "selector": { "seq": [1, 2] } }),
            json!("t.ndjson#seq=1-2"),
            json!({ "source": "umx:demo/01J0000000000000000000000A", "selector": { "seq": [1, 2] } }),
        ];
        // Appends after what `out` already holds, also when a bad pointer sends the array the slow way.
        let kept = Position::new("k.md", Unit::Lines, 1, 1).unwrap();
        let of = |text: &str| {
            let mut out = alloc::vec![kept.clone()];
            positions(text, &mut out);
            out
        };
        for shape in &shapes {
            let text = Value::Array(alloc::vec![shape.clone()]).to_string();
            let mut want = alloc::vec![kept.clone()];
            want.extend(Position::of_evidence(shape));
            assert_eq!(of(&text), want, "{text}");
        }
        let all = Value::Array(shapes.to_vec()).to_string();
        let want: Vec<Position> = shapes.iter().filter_map(Position::of_evidence).collect();
        assert_eq!(of(&all)[1..], want[..]);
        assert_eq!(want.len(), 4, "the quoted source, then lines three times");
    }

    #[test]
    fn the_latest_move_is_the_last_one_with_a_date() {
        let moves = r#"[{"to":"a","at":"2026-11-01T00:00:00Z"},{"to":"b","at":"2026-11-12"}]"#;
        assert_eq!(
            latest_move_start(moves),
            crate::time::date_span("2026-11-12").map(|s| s.start)
        );
        assert_eq!(latest_move_start("[]"), None);
        assert_eq!(latest_move_start(r#"[{"at":"soon"}]"#), None);
        assert_eq!(
            latest_move_start(r#"[{"at":7},{"at":"2026"}]"#),
            crate::time::date_span("2026").map(|s| s.start)
        );
    }
}
