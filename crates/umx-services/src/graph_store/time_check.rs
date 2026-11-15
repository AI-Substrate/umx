//! ws008 TM3 (req-147, D71 R4 and the time review's step 6): the time warnings a write gives, shared by node add, node
//! edit, a batch's node lines and edge fields, so no path warns and another does not. Warnings only: the write goes
//! ahead.

use crate::problems;
use alloc::collections::BTreeMap;
use alloc::string::String;

use umx_core::ontology::FieldKind;
use umx_core::standard::{ANCHOR_TYPES, STARTS_ON};
use umx_core::time::{reads_as_time, time_like_name, written_without_offset};
use umx_core::value::SetValue;
use umx_core::{Command, Problem, ValidName, Value};

/// What holds the field: a node of a type, or an edge of a relation (for the `ontology add field` a fix gives).
pub(crate) enum Holder<'a> {
    Node(Option<&'a ValidName>),
    Edge(&'a ValidName),
}

/// The text a field was given, when it was given as text.
fn given_text(given: &SetValue) -> Option<&str> {
    match given {
        SetValue::Raw(s) | SetValue::Str(s) => Some(s),
        SetValue::Json(v) => v.as_str(),
    }
}

/// `time_no_offset` for a date field given a date-time with no offset; else `time_as_text` for a text field (not
/// declared as a date) whose value reads as a time or whose name says it holds one. `kind` is the kind that typed the
/// value; `declared` whether the holder's ontology declares the field.
pub(crate) fn field_warning(
    graph: &ValidName,
    holder: Holder<'_>,
    field: &ValidName,
    given: &SetValue,
    (kind, declared): (Option<FieldKind>, bool),
    stored: &Value,
) -> Option<Problem> {
    if kind == Some(FieldKind::Date) {
        return given_text(given)
            .filter(|s| written_without_offset(s))
            .map(|_| problems::time_no_offset(graph, field));
    }
    let text = stored.as_str()?;
    let why = if reads_as_time(text) {
        "its value reads as a date or a clock time"
    } else if time_like_name(field.as_str()) {
        "its name ends in _at or _on"
    } else {
        return None;
    };
    let date = |c: Command| c.name_with(field, ':', "date").word("--graph").name(graph);
    let fix = match holder {
        _ if declared => Command::new("docs get").word("time"),
        Holder::Node(Some(kind)) => date(Command::new("ontology add field").name(kind)),
        Holder::Node(None) => Command::new("docs get").word("time"),
        Holder::Edge(relation) => date(
            Command::new("ontology add field")
                .word("--relation")
                .name(relation),
        ),
    };
    Some(problems::time_as_text(graph, field, why, fix))
}

/// `anchor_undated` for a node of an anchor type (meeting, event, segment, transcript) that has no `starts_on`.
/// `edit` is the node's own `node edit` command when it exists; a node being added is named `<node>`.
pub(crate) fn anchor_warning(
    graph: &ValidName,
    kind: &str,
    fields: &BTreeMap<String, Value>,
    edit: Option<Command>,
) -> Option<Problem> {
    if !ANCHOR_TYPES.contains(&kind) || fields.contains_key(STARTS_ON) {
        return None;
    }
    let kind = ValidName::new(kind)?;
    let starts_on = ValidName::new(STARTS_ON).expect("a std name");
    let fix = edit
        .unwrap_or_else(|| {
            Command::new("node edit")
                .word("<node>")
                .word("--graph")
                .name(graph)
        })
        .word("--set")
        .name_with(&starts_on, '=', "<date-time>");
    Some(problems::anchor_undated(graph, &kind, fix))
}

/// `umx check` (F211): each stored value of a date field, on a node or an edge, that is not a date in years 0001 to
/// 9999: text written with `field_kind_mismatch`, or a year 0000 stored before that range was set. It loads and stays
/// as written, and a compare reads it as text. Gives the record, whether it is an edge, and the field, in graph order.
pub(crate) fn stored_not_dates(
    graph: &umx_graph::Graph,
    o: &umx_core::Ontology,
) -> alloc::vec::Vec<(umx_core::Id, bool, String)> {
    let not_date = |v: &Value| match v {
        Value::Null => false,
        Value::String(s) => umx_core::time::normalise_date(s).is_none(),
        _ => true,
    };
    let mut out = alloc::vec::Vec::new();
    for n in graph.nodes() {
        for name in n.custom_field_names() {
            if o.field_kind(n.kind(), name) == Some(FieldKind::Date)
                && n.custom_field(name).is_some_and(|v| not_date(&v))
            {
                out.push((n.id(), false, name.into()));
            }
        }
    }
    let dated = |relation: &str| {
        o.relation_fields
            .get(relation)
            .is_some_and(|fields| fields.iter().any(|f| f.kind == FieldKind::Date))
    };
    for e in graph.edges().filter(|e| dated(e.kind())) {
        let edge = e.to_edge();
        for (name, v) in &edge.fields {
            if o.relation_field_kind(&edge.kind, name) == Some(FieldKind::Date) && not_date(v) {
                out.push((edge.id, true, name.clone()));
            }
        }
    }
    out
}
