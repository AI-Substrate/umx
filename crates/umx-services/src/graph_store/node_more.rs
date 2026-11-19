//! node edit | note | status | history (s05b; workshop 007 section 5, req-12 and req-58).

use super::node::{check_name, entity_key_value, field_kind, key_warnings};
use super::sources::SourceCheck;
use super::{
    Ctx, Done, GraphStore, Outcome, Page, alias_name, check_evidence, problems, resolve_node,
};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::codec::GraphFile;
use umx_core::entity::ENTITY_KEY;
use umx_core::evidence::{EvidenceKind, EvidencePointer, PointerError, parse_pointer};
use umx_core::locator::LOCATOR_FIELD;
use umx_core::name::RESERVED_NODE_FIELDS;
use umx_core::ontology::{FieldKind, nearest};
use umx_core::ports::GraphDir;
use umx_core::standard::STARTS_ON;
use umx_core::status::{LABELS, StatusMove, inferred_label};
use umx_core::tags::{TAGS, stored_tags};
use umx_core::value::{SetValue, kind_of, typed_value};
use umx_core::{Command, Id, Node, Ontology, Problem, ValidName, Value};
use umx_graph::Graph;

/// The standard field that only the status verb may change.
const STATUS: &str = "status";

#[derive(Clone, Debug, Default)]
pub struct NodeEditRequest {
    pub title: Option<String>,
    pub kind: Option<String>,
    /// Replaces the note.
    pub note: Option<String>,
    pub sets: Vec<(String, SetValue)>,
    pub unsets: Vec<String>,
    /// Re-points the node at a new source, or makes a plain node a resource (ws006 S3b). `--unset locator` (in
    /// `unsets`) makes a resource node plain again.
    pub locator: Option<String>,
    /// `--tag` and `--untag` (ws008 t2b): std tags to add, and tags (or `<facet>:*`) to remove. Refused in a graph
    /// without std.
    pub tag: Vec<String>,
    pub untag: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct NodeNoteRequest {
    pub append: String,
    pub evidence: Vec<EvidencePointer>,
}

/// `--evidence-kind` (or a batch line's `evidence_kind`) as a kind, checked whether or not any pointer uses it.
fn evidence_kind(kind: Option<&str>) -> Result<Option<EvidenceKind>, Problem> {
    kind.map(|kind| {
        EvidenceKind::parse(kind).ok_or_else(|| {
            problems::usage_invalid(
                "--evidence-kind is room or chat or screen or document or analyst",
                "node",
            )
        })
    })
    .transpose()
}

impl NodeNoteRequest {
    pub fn from_legacy(
        append: &str,
        evidence: &[String],
        kind: Option<&str>,
    ) -> Result<Self, Problem> {
        let kind = evidence_kind(kind)?;
        Ok(Self {
            append: append.into(),
            evidence: evidence
                .iter()
                .map(|pointer| EvidencePointer {
                    pointer: pointer.clone(),
                    kind,
                })
                .collect(),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct NodeStatusRequest {
    pub status: String,
    pub reason: Option<String>,
    pub label: Option<String>,
    /// `--in <anchor>` (ws008 TM6): where the move happened, resolved as `--raised-in`'s anchor is (edge add's end).
    pub anchor: Option<String>,
    /// `--evidence <pointer>` (ws008 TM6): the one pointer the move cites, in node evidence's grammar.
    pub evidence: Option<String>,
    pub evidence_kind: Option<String>,
}

/// ws008 TM6: a status move's `--evidence` pointer, read and checked as `node add --evidence` reads one, and the
/// warnings its check gives (a `umx:` source no loaded graph holds). `--evidence-kind` is checked whether or not a
/// pointer is given, as node add checks it (R345-1), so a bad kind refuses before anything moves.
pub(crate) fn status_evidence<'g>(
    dir: &GraphDir,
    pointer: Option<&str>,
    evidence_kind: Option<&str>,
    sources: &SourceCheck,
    lookup: &dyn Fn(&str) -> super::resolve::Found<'g>,
) -> Result<(Option<Value>, Vec<Problem>), Problem> {
    let pointers: Vec<String> = pointer.map(String::from).into_iter().collect();
    let mut pointers = evidence_pointers(dir, &pointers, evidence_kind, sources, lookup)?;
    let warnings = super::resolve::check_evidence(&mut pointers, lookup);
    Ok((pointers.into_iter().next(), warnings))
}

/// ws008 TM6: an anchor as a move stores it, `umx:<graph id>/<node id>`, the same for a local or a far anchor and
/// whatever alias the graph has.
pub(crate) fn place_address(owner: Id, end: super::external::StoredEnd) -> String {
    alloc::format!(
        "umx:{}/{}",
        end.graph.unwrap_or(owner).to_text(),
        end.node.to_text()
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeEdited {
    pub alias: String,
    pub node: Node,
    /// Field names that changed, in the order given; empty when the edit changed nothing (and nothing was written).
    pub changed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeNoted {
    pub alias: String,
    pub node: Node,
    /// Evidence entries added; an entry already on the node is not added twice.
    pub evidence_added: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusMoved {
    pub alias: String,
    pub id: Id,
    pub change: StatusMove,
}

/// One row of `node history`: `kind` is `created` or `status`. `from` is None on the created row.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow {
    pub at: String,
    pub by: String,
    pub kind: &'static str,
    pub from: Option<String>,
    pub to: String,
    pub label: Option<String>,
    pub reason: Option<String>,
}

fn pointer_problem(e: PointerError) -> Problem {
    match e {
        PointerError::Invalid(reason) => problems::evidence_pointer_invalid(reason),
        PointerError::OutsideRoots => problems::evidence_outside_roots(),
    }
}

/// Parses every pointer for the graph in `dir`, or refuses with the first bad one (used by node note and node add).
/// `root:` sources are checked against their root now (ws006 S1).
pub(crate) fn evidence_pointers<'g>(
    dir: &GraphDir,
    evidence: &[String],
    evidence_kind: Option<&str>,
    sources: &SourceCheck,
    lookup: &dyn Fn(&str) -> super::resolve::Found<'g>,
) -> Result<Vec<Value>, Problem> {
    let req = NodeNoteRequest::from_legacy("", evidence, evidence_kind)?;
    typed_evidence_pointers(dir, &req.evidence, sources, lookup)
}

/// node add's pointers (plan 205): each entry's own kind, else `evidence_kind`, then node note's parser and checks.
/// `evidence_kind` is checked even when every entry names its own kind.
pub(crate) fn node_add_pointers<'g>(
    dir: &GraphDir,
    evidence: &[EvidencePointer],
    kind: Option<&str>,
    sources: &SourceCheck,
    lookup: &dyn Fn(&str) -> super::resolve::Found<'g>,
) -> Result<Vec<Value>, Problem> {
    let kind = evidence_kind(kind)?;
    let pointers: Vec<EvidencePointer> = evidence
        .iter()
        .map(|p| EvidencePointer {
            pointer: p.pointer.clone(),
            kind: p.kind.or(kind),
        })
        .collect();
    typed_evidence_pointers(dir, &pointers, sources, lookup)
}

pub(crate) fn typed_evidence_pointers<'g>(
    dir: &GraphDir,
    evidence: &[EvidencePointer],
    sources: &SourceCheck,
    lookup: &dyn Fn(&str) -> super::resolve::Found<'g>,
) -> Result<Vec<Value>, Problem> {
    let pointers = evidence
        .iter()
        .map(|p| {
            super::resolve::check_evidence_fragment(&p.pointer, lookup)?;
            parse_pointer(&p.pointer, p.kind.unwrap_or(EvidenceKind::DEFAULT), dir)
                .map_err(pointer_problem)
        })
        .collect::<Result<Vec<Value>, Problem>>()?;
    sources.check(dir, &pointers)?;
    Ok(pointers)
}

/// Adds the entries not already in `to`; returns how many were added.
pub(crate) fn add_evidence(to: &mut Vec<Value>, pointers: Vec<Value>) -> usize {
    let mut added = 0;
    for p in pointers {
        if !to.iter().any(|existing| same_evidence(existing, &p)) {
            to.push(p);
            added += 1;
        }
    }
    added
}

pub(crate) fn same_evidence(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            a.keys().filter(|key| key.as_str() != "by_session").count()
                == b.keys().filter(|key| key.as_str() != "by_session").count()
                && a.iter()
                    .filter(|(key, _)| key.as_str() != "by_session")
                    .all(|(key, value)| b.get(key) == Some(value))
        }
        _ => left == right,
    }
}

pub(crate) fn note_node<'g>(
    dir: &GraphDir,
    sources: &SourceCheck,
    node: &mut Node,
    req: NodeNoteRequest,
    now: &str,
    lookup: &dyn Fn(&str) -> super::resolve::Found<'g>,
) -> Result<(usize, Vec<Problem>), Problem> {
    if req.append.trim().is_empty() {
        return Err(problems::usage_invalid("--append needs some text", "node"));
    }
    let mut pointers = typed_evidence_pointers(dir, &req.evidence, sources, lookup)?;
    let warnings = check_evidence(&mut pointers, lookup);
    let kept = node.note.trim_end_matches('\n');
    node.note = if kept.is_empty() {
        req.append
    } else {
        format!("{kept}\n\n{}", req.append)
    };
    let added = add_evidence(&mut node.evidence, pointers);
    node.updated = now.into();
    Ok((added, warnings))
}

/// `node status`'s checks and change, shared by the verb and a batch. `(place, evidence)` are the move's `--in` and
/// `--evidence` as the caller resolved them (ws008 TM6).
#[allow(clippy::too_many_arguments)]
pub(crate) fn status_node(
    ctx: &Ctx,
    graph: &ValidName,
    ontology: &Ontology,
    node: &mut Node,
    req: NodeStatusRequest,
    (place, evidence): (Option<String>, Option<Value>),
    now: &str,
    warnings: &mut Vec<Problem>,
) -> Result<StatusMove, Problem> {
    let name = check_name(&req.status, "status", &|good| {
        problems::status_command(graph, node.id, Some(good))
    })?;
    if let Some(label) = req.label.as_deref()
        && !LABELS.contains(&label)
    {
        return Err(problems::usage_invalid(
            "--label is resolved or in_contention or back_on_table",
            "node",
        ));
    }
    if !ontology.has_status(&req.status) {
        warnings.push(problems::off_vocabulary_status(
            graph,
            &name,
            problems::names(nearest(
                &req.status,
                ontology.statuses.iter().map(String::as_str),
            )),
        ));
    }
    let reason = req
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    if reason.is_none() {
        warnings.push(problems::reason_missing(
            node.id,
            problems::status_command(graph, node.id, Some(&name)),
        ));
    }
    let change = StatusMove {
        at: now.into(),
        by: ctx.by.clone(),
        from: node.status.clone(),
        to: req.status,
        label: req
            .label
            .as_deref()
            .or_else(|| inferred_label(&node.status, name.as_str()))
            .map(String::from),
        reason: reason.map(String::from),
        place,
        evidence,
    };
    node.status_history.push(change.to_value());
    node.status = change.to.clone();
    node.updated = change.at.clone();
    Ok(change)
}

/// `node edit`'s checks and changes, shared by the verb and a batch so no check is in one and not the other
/// (security, s07): status is refused (`use_status_verb`), names are checked, and off-vocabulary names and field
/// kind mismatches are pushed to `warnings`. Changes `n` in memory only; returns the field names that changed, in
/// the order given (empty when nothing changed). The caller sets `updated` and writes.
pub(crate) fn edit_node(
    graph: &ValidName,
    o: &Ontology,
    g: &Graph,
    n: &mut Node,
    req: NodeEditRequest,
    warnings: &mut Vec<Problem>,
) -> Result<Vec<String>, Problem> {
    let id = n.id;
    let set_status = req.sets.iter().find(|(f, _)| f == STATUS);
    if set_status.is_some() || req.unsets.iter().any(|f| f == STATUS) {
        let to = set_status.and_then(|(_, v)| match v {
            SetValue::Raw(s) | SetValue::Str(s) => ValidName::new(s),
            SetValue::Json(j) => j.as_str().and_then(ValidName::new),
        });
        return Err(problems::use_status_verb(problems::status_command(
            graph,
            id,
            to.as_ref(),
        )));
    }
    let edit = || Command::new("node edit").address(graph, id);
    let std_tags = super::tags::is_std(o);
    let tagging = !req.tag.is_empty() || !req.untag.is_empty();
    if tagging && !std_tags {
        return Err(problems::tag_needs_std(graph));
    }
    let mut changed: Vec<String> = Vec::new();
    let mark = |field: &str, changed: &mut Vec<String>| {
        if !changed.iter().any(|c| c == field) {
            changed.push(field.to_string());
        }
    };

    if let Some(kind) = req.kind {
        let name = check_name(&kind, "node type", &|g| edit().word("--type").name(g))?;
        if kind == umx_core::standard::STD {
            return Err(problems::std_reserved());
        }
        if umx_core::provenance::user_node_warning(&kind).is_some() {
            warnings.push(problems::reserved_type(&name));
        }
        if o.type_decl(&kind).is_none() {
            warnings.push(problems::off_vocabulary_type(
                graph,
                &name,
                problems::names(nearest(&kind, o.type_names())),
            ));
        }
        if kind != n.kind {
            n.kind = kind;
            mark("type", &mut changed);
        }
    }
    if let Some(title) = req.title
        && title != n.title
    {
        n.title = title;
        mark("title", &mut changed);
    }
    if let Some(note) = req.note
        && note != n.note
    {
        n.note = note;
        mark("note", &mut changed);
    }
    for (field, value) in &req.sets {
        if let Some(refused) = problems::locator_name(field) {
            return Err(refused);
        }
        if RESERVED_NODE_FIELDS.contains(&field.as_str()) {
            return Err(problems::reserved_name(field));
        }
        if field == umx_core::standard::STD {
            return Err(problems::std_reserved());
        }
        let name = check_name(field, "field name", &|g| {
            edit().word("--set").name_with(g, '=', "<value>")
        })?;
        if field == ENTITY_KEY {
            match entity_key_value(value)? {
                Some(v) if n.fields.get(field) != Some(&v) => {
                    n.fields.insert(field.clone(), v);
                    mark(field, &mut changed);
                }
                Some(_) => {}
                None => {
                    if n.fields.remove(field).is_some() {
                        mark(field, &mut changed);
                    }
                }
            }
            continue;
        }
        if field == TAGS && std_tags {
            if tagging {
                return Err(problems::usage_invalid(
                    "give tags with --tag/--untag or --set tags, not both",
                    "node",
                ));
            }
            let given = super::tags::set_value(value)?;
            let have = n.fields.get(TAGS).map(stored_tags).unwrap_or_default();
            warnings.extend(super::tags::off_vocabulary(
                graph,
                o,
                &given
                    .iter()
                    .filter(|t| !have.contains(t))
                    .cloned()
                    .collect::<Vec<_>>(),
            ));
            match super::tags::stored(given) {
                Some(v) if n.fields.get(field) != Some(&v) => {
                    n.fields.insert(field.clone(), v);
                    mark(field, &mut changed);
                }
                Some(_) => {}
                None => {
                    if n.fields.remove(field).is_some() {
                        mark(field, &mut changed);
                    }
                }
            }
            continue;
        }
        let declared = field_kind(o, g, Some(&n.kind), field);
        let (v, mismatch) = typed_value(value, declared);
        if mismatch {
            warnings.push(problems::field_kind_mismatch(
                graph,
                &name,
                declared.map_or("unknown", FieldKind::as_str),
            ));
        }
        let in_ontology = o.field_kind(&n.kind, field).is_some();
        warnings.extend(super::time_check::field_warning(
            graph,
            super::time_check::Holder::Node(ValidName::new(&n.kind).as_ref()),
            &name,
            value,
            (declared, in_ontology),
            &v,
        ));
        if !in_ontology {
            let inferred = declared
                .or_else(|| kind_of(&v))
                .unwrap_or(FieldKind::String);
            let kind = ValidName::new(&n.kind);
            if let Some(kind) = kind {
                warnings.push(problems::off_vocabulary_field(
                    graph,
                    &kind,
                    &name,
                    inferred.as_str(),
                ));
            }
        }
        if n.fields.get(field) != Some(&v) {
            n.fields.insert(field.clone(), v);
            mark(field, &mut changed);
        }
    }
    for field in &req.unsets {
        if RESERVED_NODE_FIELDS.contains(&field.as_str()) {
            return Err(problems::reserved_name(field));
        }
        check_name(field, "field name", &|g| edit().word("--unset").name(g))?;
        if n.fields.remove(field).is_some() {
            mark(field, &mut changed);
        }
    }
    if tagging {
        use super::tags::{Reads, TagRetry};
        let retry = TagRetry {
            base: edit(),
            flags: alloc::vec![
                ("--tag", &req.tag[..], Reads::Tags),
                ("--untag", &req.untag[..], Reads::Patterns),
            ],
        };
        let add = super::tags::given(&req.tag, &retry)?;
        let remove = super::tags::patterns(&req.untag, &retry)?;
        let (value, added) = super::tags::edited(n.fields.get(TAGS), &add, &remove)?;
        warnings.extend(super::tags::off_vocabulary(graph, o, &added));
        if n.fields.get(TAGS) != value.as_ref() {
            match value {
                Some(v) => n.fields.insert(TAGS.to_string(), v),
                None => n.fields.remove(TAGS),
            };
            mark(TAGS, &mut changed);
        }
    }
    // TM3: an edit that makes the node an undated anchor (a new type, or starts_on removed) says so.
    if changed.iter().any(|c| c == "type" || c == STARTS_ON) {
        warnings.extend(super::time_check::anchor_warning(
            graph,
            &n.kind,
            &n.fields,
            Some(edit()),
        ));
    }
    Ok(changed)
}

/// A node's stored keys in stored form before any write of it (ws008 E1): a hand-edited key becomes canonical on the
/// node's next write; entries umx cannot use are kept as they are. In a std graph (`std_tags`) its tags too (ws008
/// t2b).
pub(crate) fn canonical_keys(node: &mut Node, std_tags: bool) {
    if let Some(v) = node.fields.get_mut(ENTITY_KEY) {
        *v = umx_core::entity::canonical(v);
    }
    if std_tags && let Some(v) = node.fields.get_mut(TAGS) {
        *v = umx_core::tags::canonical(v);
    }
}

impl GraphStore {
    /// Resolves `node` in the graph it names (or the selected one) for a write, once that graph's files are as its
    /// load left them (F144), and returns the alias and a copy of the node.
    pub(super) fn node_for_change(&self, ctx: &Ctx, node: &str) -> Result<(String, Node), Problem> {
        let alias = self.select_write(ctx, &[Some(node)])?;
        self.node_in(&alias, node)
    }

    pub(super) fn node_in(&self, alias: &str, node: &str) -> Result<(String, Node), Problem> {
        let l = self.get(alias);
        let id = resolve_node(l, alias, node)?;
        Ok((
            alias.to_string(),
            l.graph.node(id).expect("resolved").to_node(),
        ))
    }

    /// Replaces a node in memory and writes nodes.jsonl; on a failed write memory is put back. Returns the write's
    /// warnings.
    pub(super) fn write_node(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        mut node: Node,
        evidence_start: usize,
    ) -> Outcome<Node> {
        canonical_keys(&mut node, super::tags::is_std(self.get(alias).ontology()));
        let id = node.id;
        let before = self.get(alias).graph.node(id).map(|old| old.to_node());
        let allows = self.secret_allows();
        let mut warnings = Vec::new();
        let mut done = self.attributed_write(ctx, alias, |batch, session, _, _, meta| {
            warnings.extend(
                super::secrets::node_write_secrets(
                    before.as_ref(),
                    &node,
                    meta.ontology,
                    meta.id,
                    &allows,
                )
                .into_iter()
                .map(|hit| hit.warning(&alias_name(alias), id, "Node")),
            );
            if !umx_core::provenance::is_provenance(&node) {
                node.record_edit(session);
            }
            super::provenance::stamp_evidence(&mut node.evidence[evidence_start..], session);
            let moves = before.as_ref().map_or(0, |old| old.status_history.len());
            super::provenance::stamp_status_evidence(&mut node.status_history[moves..], session);
            super::evidence_edit::stamp_removals(
                &mut node,
                before.as_ref().map_or(0, super::evidence_edit::trail_len),
                session,
            );
            batch
                .insert_node(node.clone())
                .map_err(|e| problems::image_full(&e))?;
            Ok(node)
        })?;
        warnings.append(&mut done.notes);
        done.notes = warnings;
        // A repeated id stays repeated through every write; say so each time (req-67).
        if let Some(r) = self.graphs[alias].repeat(GraphFile::Nodes, id) {
            done.notes
                .push(problems::duplicate_id(&alias_name(alias), &r));
        }
        done.notes.extend(key_warnings(
            &alias_name(alias),
            &self.get(alias).graph,
            &done.value,
        ));
        done.notes.extend(super::node::title_key_warnings(
            before.as_ref(),
            &done.value,
        ));
        Ok(done)
    }

    /// Changes title, type, note and custom fields. Status is refused (`use_status_verb`) so every move is recorded.
    pub fn node_edit(
        &mut self,
        ctx: &Ctx,
        node: &str,
        req: NodeEditRequest,
    ) -> Outcome<NodeEdited> {
        let (alias, mut n) = self.node_for_change(ctx, node)?;
        let graph = alias_name(&alias);
        let mut req = req;
        let new_locator = req.locator.take();
        let unsets = req.unsets.len();
        req.unsets.retain(|f| f != LOCATOR_FIELD);
        let unset_locator = req.unsets.len() < unsets;
        let l = self.get(&alias);
        let mut warnings = Vec::new();
        let mut changed = edit_node(&graph, l.ontology(), &l.graph, &mut n, req, &mut warnings)?;
        if new_locator.is_some() || unset_locator {
            let (moved, more) =
                self.edit_locator(&alias, &mut n, new_locator.as_deref(), unset_locator)?;
            if moved && !changed.iter().any(|c| c == LOCATOR_FIELD) {
                changed.push(LOCATOR_FIELD.into());
            }
            warnings.extend(more);
        }
        if !changed.is_empty() {
            n.updated = self.now();
            let evidence_start = n.evidence.len();
            let done = self.write_node(ctx, &alias, n, evidence_start)?;
            n = done.value;
            warnings.extend(done.notes);
            let parent = super::node::has_part_of(&self.get(&alias).graph, n.id);
            warnings.extend(super::node::segment_hint(&graph, &n, parent));
        }
        Ok(Done {
            value: NodeEdited {
                alias,
                node: n,
                changed,
            },
            notes: warnings,
        })
    }

    /// `node edit --locator <x>` and `--unset locator` (ws006 S3b; decided with the graph-addressing-expert): the node
    /// keeps its id and edges; a locator another node holds is `locator_in_use`; the new one starts with no hash or
    /// length; `locator_changed` counts what cited the old source. `--unset locator` also clears a kept custom field
    /// of that name. A node holding a kept field cannot take a locator (`locator_field_invalid`). A new locator another
    /// visible graph holds is named (`resource_elsewhere`), as on add. Returns whether the node changed, and the
    /// warnings.
    fn edit_locator(
        &self,
        alias: &str,
        n: &mut Node,
        text: Option<&str>,
        unset: bool,
    ) -> Result<(bool, Vec<Problem>), Problem> {
        use super::resources::{checked_locator, citations, holder, named, spellings};
        let graph = alias_name(alias);
        let l = self.get(alias);
        let top = self.source_repo_top(&l.dir);
        let new = match (text, unset) {
            (Some(_), true) => {
                return Err(problems::locator_invalid(
                    "--locator and --unset locator together",
                ));
            }
            (Some(text), false) => {
                if n.has_kept_locator_field() {
                    return Err(problems::locator_field_invalid(
                        LOCATOR_FIELD,
                        "the node holds a kept custom field named locator (rename it first)",
                    ));
                }
                let locator = checked_locator(text, &l.dir, &self.source_check(), "node edit")?;
                let names = named(&locator, &l.dir, top.as_ref());
                match holder(&l.graph, &spellings(&names, &l.dir, top.as_ref())) {
                    Some(id) if id == n.id => return Ok((false, Vec::new())),
                    Some(id) => return Err(problems::locator_in_use(&graph, id)),
                    None => Some(locator),
                }
            }
            (None, _) => None,
        };
        if new.is_none() && n.locator.is_none() {
            return Ok((n.fields.remove(LOCATOR_FIELD).is_some(), Vec::new()));
        }
        let mut warnings: Vec<Problem> = new
            .as_ref()
            .map(|new| self.resource_elsewhere(alias, new, &l.dir))
            .unwrap_or_default();
        let old = core::mem::replace(&mut n.locator, new);
        warnings.extend(old.map(|old| {
            let names = named(&old, &l.dir, top.as_ref());
            let (edges, evidence) = citations(&l.graph, n.id, &names, &l.dir, top.as_ref());
            problems::locator_changed(&graph, n.id, edges, evidence)
        }));
        Ok((true, warnings))
    }

    /// Appends a paragraph to the note and adds evidence in one write (req-58). Every pointer is checked before
    /// anything changes.
    pub fn node_note(&mut self, ctx: &Ctx, node: &str, req: NodeNoteRequest) -> Outcome<NodeNoted> {
        let (alias, mut n) = self.node_for_change(ctx, node)?;
        let evidence = self.evidence_graphs(&alias);
        let evidence_start = n.evidence.len();
        let (evidence_added, mut warnings) = note_node(
            &self.get(&alias).dir,
            &self.source_check(),
            &mut n,
            req,
            &self.now(),
            &|a| evidence.get(a).copied(),
        )?;
        let done = self.write_node(ctx, &alias, n, evidence_start)?;
        warnings.extend(done.notes);
        Ok(Done {
            value: NodeNoted {
                alias,
                node: done.value,
                evidence_added,
            },
            notes: warnings,
        })
    }

    /// Moves a node's status and records the move on the node (req-12). Without a reason the move still happens,
    /// with warning `reason_missing`. The label is the one given, else the one inferred from the move.
    pub fn node_status(
        &mut self,
        ctx: &Ctx,
        node: &str,
        status: &str,
        reason: Option<&str>,
        label: Option<&str>,
    ) -> Outcome<StatusMoved> {
        let req = NodeStatusRequest {
            status: status.into(),
            reason: reason.map(String::from),
            label: label.map(String::from),
            ..NodeStatusRequest::default()
        };
        self.node_status_with(ctx, node, req)
    }

    /// `umx node status`, with `--in <anchor>` and `--evidence <pointer>` (ws008 TM6, req-150): the anchor resolves
    /// as `--raised-in`'s does (edge add's end: its refusals, byte for byte, and its warnings), and the pointer as
    /// node evidence's; both are checked before anything is written.
    pub fn node_status_with(
        &mut self,
        ctx: &Ctx,
        node: &str,
        req: NodeStatusRequest,
    ) -> Outcome<StatusMoved> {
        let (alias, mut n) = self.node_for_change(ctx, node)?;
        let id = n.id;
        let graph = alias_name(&alias);
        let mut warnings = Vec::new();
        let evidence_start = n.evidence.len();
        let place = match req.anchor.as_deref() {
            Some(text) => {
                let (anchor, anchor_warnings) = self.raised_in_anchor(&alias, text)?;
                warnings.extend(anchor_warnings);
                Some(place_address(self.get(&alias).meta.id, anchor.end))
            }
            None => None,
        };
        let open = self.evidence_graphs(&alias);
        let (evidence, evidence_warnings) = status_evidence(
            &self.get(&alias).dir,
            req.evidence.as_deref(),
            req.evidence_kind.as_deref(),
            &self.source_check(),
            &|a| open.get(a).copied(),
        )?;
        warnings.extend(evidence_warnings);
        let mut change = status_node(
            ctx,
            &graph,
            self.get(&alias).ontology(),
            &mut n,
            req,
            (place, evidence),
            &self.now(),
            &mut warnings,
        )?;
        let stored = self.write_node(ctx, &alias, n, evidence_start)?;
        // The reply shows the pointer as stored, with the session the write stamped on it (R345-2).
        change.evidence = stored
            .value
            .status_history
            .last()
            .and_then(|v| v.get("evidence").cloned());
        warnings.extend(stored.notes);
        Ok(Done {
            value: StatusMoved { alias, id, change },
            notes: warnings,
        })
    }

    /// Creation, then every status move, oldest first; with more than `limit`, only the newest `limit` (still oldest
    /// first) and `truncated`. Edits other than status show in `umx diff` (req-60), later.
    pub fn node_history(&self, ctx: &Ctx, node: &str, limit: usize) -> Outcome<Page<HistoryRow>> {
        let (alias, n) = self.node_get(ctx, node)?.value;
        let moves: Vec<StatusMove> = n
            .status_history
            .iter()
            .map(StatusMove::from_value)
            .collect();
        let first_status = moves.first().map_or(n.status.clone(), |m| m.from.clone());
        let mut rows = Vec::with_capacity(moves.len() + 1);
        rows.push(HistoryRow {
            at: n.created.clone(),
            by: n.created_by.clone(),
            kind: "created",
            from: None,
            to: first_status,
            label: None,
            reason: None,
        });
        rows.extend(moves.into_iter().map(|m| HistoryRow {
            at: m.at,
            by: m.by,
            kind: "status",
            from: Some(m.from),
            to: m.to,
            label: m.label,
            reason: m.reason,
        }));
        // The newest `limit` rows, still oldest first: agents ask what changed lately.
        let total = rows.len();
        let skip = total.saturating_sub(limit);
        let warnings = self.stale_note(&alias).into_iter().collect();
        Ok(Done {
            value: Page {
                alias,
                rows: rows.into_iter().skip(skip).collect(),
                total: Some(total),
                truncated: skip > 0,
                graphs: None,
                along: None,
                covered: None,
            },
            notes: warnings,
        })
    }
}
