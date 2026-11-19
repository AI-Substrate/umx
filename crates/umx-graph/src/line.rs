//! Record lines (s05 design note 1): the one function that turns a node line into its hot column values, the edge
//! line reader, and the decoders for a node's cold parts. A line the scanner refuses goes to `umx_core::codec`,
//! which gives serde's record or its exact error.

use crate::evidence_kinds::EvidenceSrc;
use crate::json::{self, Key, Kind, Num, Refused, Scanned, Val};
use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::codec::{self, DecodeError, GraphFile};
use umx_core::{Edge, Id, Locator, Node, Value};

/// A custom field's value as its sparse column keeps it. A string is kept as its place in the line (relative to the
/// line's start), so it costs nothing beside the image.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FieldVal {
    Num(Num),
    Str {
        rel: u32,
        len: u32,
        esc: bool,
    },
    Bool(bool),
    /// A list, an object or null: read from the line when asked.
    Other,
}

/// The hot values of one node line.
pub(crate) struct HotNode<'a> {
    pub id: Id,
    pub kind: Cow<'a, str>,
    pub title: Cow<'a, str>,
    /// The note, for the word index (req-68); the rest of the cold text is read from the line when asked.
    pub note: Cow<'a, str>,
    pub status: Cow<'a, str>,
    pub created: Cow<'a, str>,
    pub updated: Cow<'a, str>,
    pub by: Cow<'a, str>,
    pub authored: Option<Id>,
    pub edited: Option<Id>,
    pub prov_key: Option<Cow<'a, str>>,
    pub invalid_provenance: u8,
    /// A resource node's locator (ws006 S3); only kind, format and its key are kept hot.
    pub locator: Option<Locator>,
    /// The `entity_key` value when it is a list (ws008 E1); read for the entity key index.
    pub entity_key: Option<Cow<'a, Value>>,
    /// The `tags` value when it is a list (ws008 t2b); read for the tag index.
    pub tags: Option<Cow<'a, Value>>,
    /// The evidence, for the evidence kinds column (ws003 s22): its array's text in the line.
    pub evidence: EvidenceSrc<'a>,
}

const ID: u16 = 1;
const TYPE: u16 = 2;
const TITLE: u16 = 4;
const STATUS: u16 = 8;
const NOTE: u16 = 16;
const EVIDENCE: u16 = 32;
const HISTORY: u16 = 64;
const CREATED: u16 = 128;
const UPDATED: u16 = 256;
const BY: u16 = 512;
const ALL_NODE: u16 = 1023;
const AUTHORED: u16 = 1024;
const EDITED: u16 = 2048;
const LOCATOR: u16 = 4096;

fn node_key(key: &str) -> u16 {
    match key {
        "id" => ID,
        "type" => TYPE,
        "title" => TITLE,
        "status" => STATUS,
        "note" => NOTE,
        "evidence" => EVIDENCE,
        "status_history" => HISTORY,
        "created" => CREATED,
        "updated" => UPDATED,
        "created_by" => BY,
        "authored_by_session" => AUTHORED,
        "edited_by_session" => EDITED,
        "locator" => LOCATOR,
        _ => 0,
    }
}

/// Marks `bit` seen; a key seen twice is serde's `duplicate field` error, left to it.
fn see(seen: &mut u16, bit: u16) -> Scanned<()> {
    if *seen & bit != 0 {
        return Err(Refused);
    }
    *seen |= bit;
    Ok(())
}

fn want(v: Val, kind: Kind) -> Scanned<()> {
    if v.kind == kind { Ok(()) } else { Err(Refused) }
}

fn str_of<'a>(s: &'a str, v: Val) -> Scanned<Cow<'a, str>> {
    want(v, Kind::Str)?;
    Ok(json::text(&s[v.start..v.end], v.esc))
}

fn id_of(s: &str, v: Val) -> Scanned<Id> {
    str_of(s, v)?.parse().map_err(|_| Refused)
}

fn key_text(s: &str, k: Key) -> Cow<'_, str> {
    json::text(&s[k.start..k.end], k.esc)
}

fn field_val(s: &str, v: Val) -> FieldVal {
    match v.kind {
        Kind::Num => FieldVal::Num(Num::parse(&s[v.start..v.end])),
        Kind::Bool(b) => FieldVal::Bool(b),
        Kind::Str if v.end - v.start < 1 << 31 => FieldVal::Str {
            rel: v.start as u32,
            len: (v.end - v.start) as u32,
            esc: v.esc,
        },
        _ => FieldVal::Other,
    }
}

/// A node line's hot values, and its custom fields in name order into `fields` (cleared first). Refused for anything
/// the scanner is unsure of; then `codec` decides.
pub(crate) fn node_hot<'a>(
    line: &'a [u8],
    fields: &mut Vec<(Cow<'a, str>, FieldVal)>,
) -> Scanned<HotNode<'a>> {
    fields.clear();
    let mut seen = 0u16;
    let empty = Cow::Borrowed("");
    let mut n = HotNode {
        id: Id(0),
        kind: empty.clone(),
        title: empty.clone(),
        note: empty.clone(),
        status: empty.clone(),
        created: empty.clone(),
        updated: empty.clone(),
        by: empty,
        authored: None,
        edited: None,
        prov_key: None,
        invalid_provenance: 0,
        locator: None,
        entity_key: None,
        tags: None,
        evidence: EvidenceSrc::Record(&[]),
    };
    json::object(line, |s, k, v| {
        let key = key_text(s, k);
        let bit = node_key(&key);
        if bit == 0 {
            if key == "prov_key" {
                if v.kind == Kind::Str {
                    let text = str_of(s, v)?;
                    if umx_core::provenance::ProvKey::parse(&text).is_err() {
                        n.invalid_provenance |= 1;
                    }
                    n.prov_key = Some(text);
                } else {
                    n.invalid_provenance |= 1;
                }
            }
            if key == umx_core::entity::ENTITY_KEY && v.kind == Kind::Arr {
                n.entity_key = Some(Cow::Owned(json::value(s, v)));
            }
            if key == umx_core::tags::TAGS && v.kind == Kind::Arr {
                n.tags = Some(Cow::Owned(json::value(s, v)));
            }
            let val = field_val(s, v);
            fields.push((key, val));
            return Ok(());
        }
        see(&mut seen, bit)?;
        match bit {
            ID => n.id = id_of(s, v)?,
            TYPE => n.kind = str_of(s, v)?,
            TITLE => n.title = str_of(s, v)?,
            STATUS => n.status = str_of(s, v)?,
            NOTE => n.note = str_of(s, v)?,
            EVIDENCE => {
                want(v, Kind::Arr)?;
                n.evidence = EvidenceSrc::Line(&s[v.start..v.end]);
            }
            HISTORY => want(v, Kind::Arr)?,
            CREATED => n.created = str_of(s, v)?,
            UPDATED => n.updated = str_of(s, v)?,
            BY => n.by = str_of(s, v)?,
            AUTHORED | EDITED => match id_of(s, v) {
                Ok(id) if bit == AUTHORED => n.authored = Some(id),
                Ok(id) => n.edited = Some(id),
                Err(_) => {
                    n.invalid_provenance |= if bit == AUTHORED { 2 } else { 4 };
                    fields.push((key, field_val(s, v)));
                }
            },
            // A value that is not a locator is refused here, so the codec keeps it raw (S241-1).
            LOCATOR => n.locator = Some(locator_of(s, v)?),
            _ => unreachable!("a known node key"),
        }
        Ok(())
    })?;
    if seen & ALL_NODE != ALL_NODE {
        return Err(Refused);
    }
    fields.sort_by(|a, b| a.0.cmp(&b.0));
    if fields.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(Refused);
    }
    Ok(n)
}

/// A `locator` value: an object that is a locator, else refused (the codec then gives its exact error).
fn locator_of(s: &str, v: Val) -> Scanned<Locator> {
    want(v, Kind::Obj)?;
    Locator::from_value(&json::value(s, v)).ok_or(Refused)
}

/// The record on a line the scanner refused, or the codec's error for it as line `no` of `file`.
pub(crate) fn decode_node(line: &[u8], no: usize) -> Result<Node, DecodeError> {
    codec::decode_nodes(line)
        .map(|mut d| {
            d.records
                .pop()
                .expect("a line that is not blank holds one record")
        })
        .map_err(|e| DecodeError { line: no, ..e })
}

pub(crate) fn decode_edge(line: &[u8], no: usize) -> Result<Edge, DecodeError> {
    codec::decode_edges(line)
        .map(|mut d| {
            d.records
                .pop()
                .expect("a line that is not blank holds one record")
        })
        .map_err(|e| DecodeError {
            file: GraphFile::Edges,
            line: no,
            ..e
        })
}

/// A node's note, evidence, history and custom fields, read from its line.
pub(crate) struct ColdNode {
    pub note: String,
    pub evidence: Vec<Value>,
    pub history: Vec<Value>,
    pub locator: Option<Locator>,
    pub fields: BTreeMap<String, Value>,
}

fn array(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        _ => unreachable!("checked as an array by the scanner"),
    }
}

/// The cold parts of a line the load accepted through the scanner. Refused for a line it had to leave to the codec.
pub(crate) fn node_cold(line: &[u8]) -> Scanned<ColdNode> {
    let mut c = ColdNode {
        note: String::new(),
        evidence: Vec::new(),
        history: Vec::new(),
        locator: None,
        fields: BTreeMap::new(),
    };
    let mut seen = 0u16;
    json::object(line, |s, k, v| {
        let key = key_text(s, k);
        match node_key(&key) {
            0 => {
                if c.fields
                    .insert(key.into_owned(), json::value(s, v))
                    .is_some()
                {
                    return Err(Refused);
                }
            }
            NOTE => {
                see(&mut seen, NOTE)?;
                c.note = str_of(s, v)?.into_owned();
            }
            EVIDENCE => {
                see(&mut seen, EVIDENCE)?;
                want(v, Kind::Arr)?;
                c.evidence = array(json::value(s, v));
            }
            HISTORY => {
                see(&mut seen, HISTORY)?;
                want(v, Kind::Arr)?;
                c.history = array(json::value(s, v));
            }
            bit @ (AUTHORED | EDITED) => {
                see(&mut seen, bit)?;
                if id_of(s, v).is_err() {
                    c.fields.insert(key.into_owned(), json::value(s, v));
                }
            }
            LOCATOR => {
                see(&mut seen, LOCATOR)?;
                c.locator = Some(locator_of(s, v)?);
            }
            bit => see(&mut seen, bit)?,
        }
        Ok(())
    })?;
    if seen & ALL_NODE != ALL_NODE {
        return Err(Refused);
    }
    Ok(c)
}

/// One custom field's value, read from the line.
pub(crate) fn node_field(line: &[u8], name: &str) -> Scanned<Option<Value>> {
    let mut found = None;
    json::object(line, |s, k, v| {
        if key_text(s, k) == name {
            if found.is_some() {
                return Err(Refused);
            }
            found = Some(json::value(s, v));
        }
        Ok(())
    })?;
    Ok(found)
}

/// An edge line's values, strings borrowed from the line where they hold no escape.
pub(crate) struct HotEdge<'a> {
    pub id: Id,
    pub kind: Cow<'a, str>,
    pub from_graph: Option<Id>,
    pub from: Id,
    pub to_graph: Option<Id>,
    pub to: Id,
    pub created: Cow<'a, str>,
    pub by: Cow<'a, str>,
    pub authored: Option<Id>,
    pub edited: Option<Id>,
    /// The `evidence` slot (req-124): empty unless the line holds an evidence list there.
    pub evidence: Vec<Value>,
    pub fields: BTreeMap<String, Value>,
}

impl HotEdge<'_> {
    pub fn of(e: Edge) -> HotEdge<'static> {
        HotEdge {
            id: e.id,
            kind: Cow::Owned(e.kind),
            from_graph: e.from_graph,
            from: e.from,
            to_graph: e.to_graph,
            to: e.to,
            created: Cow::Owned(e.created),
            by: Cow::Owned(e.created_by),
            authored: e.authored_by_session,
            edited: e.edited_by_session,
            evidence: e.evidence,
            fields: e.fields,
        }
    }
}

/// One edge line through the scanner.
pub(crate) fn edge(line: &[u8]) -> Scanned<HotEdge<'_>> {
    let mut seen = 0u16;
    let empty = Cow::Borrowed("");
    let mut e = HotEdge {
        id: Id(0),
        kind: empty.clone(),
        from_graph: None,
        from: Id(0),
        to_graph: None,
        to: Id(0),
        created: empty.clone(),
        by: empty,
        authored: None,
        edited: None,
        evidence: Vec::new(),
        fields: BTreeMap::new(),
    };
    json::object(line, |s, k, v| {
        let key = key_text(s, k);
        if key == umx_core::model::EVIDENCE {
            // A repeated key is refused like any other, whatever the values (graph-storage-expert): the codec
            // decides the line. An evidence list fills the slot; any other value stays a custom field (umx_core's
            // Edge, the same rule).
            see(&mut seen, 1024)?;
            let value = json::value(s, v);
            let slot = match value {
                Value::Array(list) if umx_core::model::is_evidence_list(&list) => {
                    e.evidence = list;
                    None
                }
                other => Some(other),
            };
            if let Some(other) = slot {
                e.fields.insert(key.into_owned(), other);
            }
            return Ok(());
        }
        let bit = match &*key {
            "id" => 1,
            "type" => 2,
            "from" => 4,
            "to" => 8,
            "created" => 16,
            "created_by" => 32,
            "from_graph" => 64,
            "to_graph" => 128,
            "authored_by_session" => 256,
            "edited_by_session" => 512,
            _ => {
                if e.fields
                    .insert(key.into_owned(), json::value(s, v))
                    .is_some()
                {
                    return Err(Refused);
                }
                return Ok(());
            }
        };
        see(&mut seen, bit)?;
        match bit {
            1 => e.id = id_of(s, v)?,
            2 => e.kind = str_of(s, v)?,
            4 => e.from = id_of(s, v)?,
            8 => e.to = id_of(s, v)?,
            16 => e.created = str_of(s, v)?,
            32 => e.by = str_of(s, v)?,
            64 => e.from_graph = Some(id_of(s, v)?),
            128 => e.to_graph = Some(id_of(s, v)?),
            // A session key that is not an id stays a raw custom field (plan 097 C1), as the codec reads it.
            bit => match id_of(s, v) {
                Ok(id) if bit == 256 => e.authored = Some(id),
                Ok(id) => e.edited = Some(id),
                Err(_) => {
                    e.fields.insert(key.into_owned(), json::value(s, v));
                }
            },
        }
        Ok(())
    })?;
    if seen & 63 != 63 {
        return Err(Refused);
    }
    Ok(e)
}
