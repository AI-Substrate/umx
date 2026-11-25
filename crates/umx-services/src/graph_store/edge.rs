//! edge add | list | delete | edit.

use super::along::edge_why;
use super::check_evidence;
use super::external::{StoredEnd, find_edge, ordered_ends, set_ends, whole_node_edge};
use super::node::check_name;
use super::node_more::{add_evidence, typed_evidence_pointers};
use super::sources::SourceCheck;
use super::{
    Ctx, Done, GraphStore, Outcome, Page, Why, alias_name, page, persist, problems, split_address,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use umx_core::codec::{GraphFile, Repeat};
use umx_core::evidence::{EvidenceKind, EvidencePointer};
use umx_core::model::EVIDENCE;
use umx_core::name::RESERVED_EDGE_FIELDS;
use umx_core::ontology::{DERIVED_RELATIONS, nearest};
use umx_core::ui::Touched;
use umx_core::value::{SetValue, kind_of, typed_value};
use umx_core::{Command, Edge, FieldKind, Id, Ontology, Problem, ValidName, Value};
use umx_graph::{EdgeRef, Graph, ProvenanceEdge};

#[derive(Clone, Debug, PartialEq)]
pub struct EdgeAdded {
    pub alias: String,
    pub edge: Edge,
    /// "<from title> <relation> <to title>", so the agent can check the direction (req-59). Graph text.
    pub sentence: String,
    /// F234: the sentence holds an end's title umx's secret check flags (the rule of a read's `from_title`).
    pub sentence_flagged: bool,
    /// True when the same from, type and to already existed and nothing was added.
    pub existed: bool,
}

/// What `edge delete` removed, or what `edge edit` made of the edge (F75).
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeChanged {
    pub alias: String,
    /// The edge as removed, or as it is after the edit.
    pub edge: Edge,
    /// "<from title> <relation> <to title>" for that edge (req-59). Graph text.
    pub sentence: String,
    /// F234: the sentence holds a local end's title umx's secret check flags.
    pub sentence_flagged: bool,
    /// An edit's changes: `type`, `direction`, then each custom field set or unset, in the order given; empty when it
    /// changed nothing (and nothing was written).
    pub changed: Vec<String>,
}

/// `edge edit`'s changes: a new relation, whether to swap the ends, and custom fields to set and unset (F134).
#[derive(Clone, Debug, Default)]
pub struct EdgeEditRequest {
    pub relation: Option<String>,
    pub reverse: bool,
    pub sets: Vec<(String, SetValue)>,
    pub unsets: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReadEdge {
    pub id: Option<Id>,
    pub kind: String,
    pub from_graph: Option<Id>,
    pub from: Id,
    pub to_graph: Option<Id>,
    pub to: Id,
    pub created: String,
    pub created_by: String,
    pub authored_by_session: Option<Id>,
    pub edited_by_session: Option<Id>,
    /// The edge's evidence slot (req-124); empty on a derived row.
    pub evidence: Vec<Value>,
    pub fields: BTreeMap<String, Value>,
}

impl ReadEdge {
    pub fn has_external_end(&self) -> bool {
        self.from_graph.is_some() || self.to_graph.is_some()
    }

    /// The distinct `kind`s in `evidence`, sorted (as [`Edge::evidence_kinds`]).
    pub fn evidence_kinds(&self) -> Vec<&str> {
        let kinds: BTreeSet<&str> = self
            .evidence
            .iter()
            .filter_map(|e| e.get("kind")?.as_str())
            .collect();
        kinds.into_iter().collect()
    }

    /// A custom `evidence` field that is not an evidence list (as [`Edge::invalid_evidence_field`]).
    pub fn invalid_evidence_field(&self) -> Option<&Value> {
        self.fields.get(EVIDENCE)
    }

    /// The stored edge this row reads, or None for a derived row (no id).
    pub fn into_stored(self) -> Option<Edge> {
        Some(Edge {
            id: self.id?,
            kind: self.kind,
            from_graph: self.from_graph,
            from: self.from,
            to_graph: self.to_graph,
            to: self.to,
            created: self.created,
            created_by: self.created_by,
            authored_by_session: self.authored_by_session,
            edited_by_session: self.edited_by_session,
            evidence: self.evidence,
            fields: self.fields,
        })
    }
}

impl From<Edge> for ReadEdge {
    fn from(edge: Edge) -> Self {
        Self {
            id: Some(edge.id),
            kind: edge.kind,
            from_graph: edge.from_graph,
            from: edge.from,
            to_graph: edge.to_graph,
            to: edge.to,
            created: edge.created,
            created_by: edge.created_by,
            authored_by_session: edge.authored_by_session,
            edited_by_session: edge.edited_by_session,
            evidence: edge.evidence,
            fields: edge.fields,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EdgeRow {
    pub graph: Id,
    pub alias: String,
    pub edge: ReadEdge,
    pub from_graph: Id,
    pub to_graph: Id,
    pub from_state: Option<&'static str>,
    pub to_state: Option<&'static str>,
    pub from_candidates: Vec<String>,
    pub to_candidates: Vec<String>,
    pub from_title: String,
    pub to_title: String,
    pub sentence: String,
}

#[derive(Clone, Debug)]
pub struct EdgeListRequest {
    pub from: Option<String>,
    pub to: Option<String>,
    pub kind: Option<String>,
    pub limit: usize,
    pub offset: usize,
    /// `--along` (D71 step 12): keep only the edges whose ends the spine both places in range.
    pub along: Option<super::AlongRequest>,
}

impl Default for EdgeListRequest {
    fn default() -> Self {
        EdgeListRequest {
            from: None,
            to: None,
            kind: None,
            limit: 100,
            offset: 0,
            along: None,
        }
    }
}

pub(crate) fn new_edge(
    id: Id,
    from: Id,
    kind: &str,
    to: Id,
    now: &str,
    by: &str,
    fields: BTreeMap<String, Value>,
) -> Edge {
    Edge {
        id,
        kind: kind.to_string(),
        from_graph: None,
        from,
        to_graph: None,
        to,
        created: now.to_string(),
        created_by: by.to_string(),
        authored_by_session: None,
        edited_by_session: None,
        evidence: Vec::new(),
        fields,
    }
}

fn title(g: &Graph, id: Id) -> String {
    g.node(id)
        .map_or_else(|| id.to_text(), |n| n.title().into())
}

/// The edge read as a sentence; underscores in the relation read as spaces.
pub(crate) fn sentence(g: &Graph, from: Id, relation: &str, to: Id) -> String {
    sentence_text(&title(g, from), relation, &title(g, to))
}

pub(crate) fn sentence_text(from: &str, relation: &str, to: &str) -> String {
    format!("{} {} {}", from, relation.replace('_', " "), to)
}

/// `edge add`'s first check: the relation's name, and never one umx derives (DERIVED_RELATIONS), before its ends are
/// resolved. `edge edit --relation` makes the same check, so no edge is retyped into a derived relation (R196 B1).
pub(crate) fn check_relation(ctx: &Ctx, relation: &str) -> Result<ValidName, Problem> {
    check_relation_for(relation, &|g| {
        ctx.same_graph(Command::new("edge add").word("<from>").name(g).word("<to>"))
    })
}

/// [`check_relation`] with the command a corrected name is shown in.
pub(crate) fn check_relation_for(
    relation: &str,
    fix: &dyn Fn(&ValidName) -> Command,
) -> Result<ValidName, Problem> {
    let rel = check_name(relation, "relation", fix)?;
    if relation == umx_core::standard::STD {
        return Err(problems::std_reserved());
    }
    // ws008 E2c (graph-addressing-expert): a denial is written only by `umx entity deny`, so none exists without a
    // reason.
    if relation == umx_core::ontology::NOT_SAME_AS {
        return Err(problems::entity_link_invalid(
            "a not_same_as edge is written only by umx entity deny, with a reason",
            Command::new("entity deny")
                .word("<a>")
                .word("<b>")
                .word("--reason")
                .word("<text>"),
        ));
    }
    match DERIVED_RELATIONS.iter().find(|k| **k == relation) {
        // similar_to edges are made only by `umx similarity rebuild` (ws004 s09).
        Some(&umx_core::ontology::SIMILAR_TO) => Err(problems::similar_to_reserved()),
        // authored_by and edited_by come from node provenance (ws007) and are never stored.
        Some(_) => Err(problems::derived_relation(&rel)),
        None => match umx_core::provenance::validate_user_edge(relation) {
            Ok(()) => Ok(rel),
            Err(_) => Err(problems::reserved_relation(&rel)),
        },
    }
}

/// `edge add`'s checks once its ends are resolved, shared by the verb and a batch (security, s07): an off-vocabulary
/// relation warns. An existing edge is unchanged only when every supplied field already has its typed value;
/// otherwise the add refuses. New fields are typed by the relation's declared field (F92).
pub(crate) fn plan_edge(
    ctx: &Ctx,
    graph: &ValidName,
    o: &Ontology,
    existing: Option<&Edge>,
    rel: &ValidName,
    sets: &[(String, SetValue)],
    warnings: &mut Vec<Problem>,
) -> Result<Option<BTreeMap<String, Value>>, Problem> {
    for (name, _) in sets {
        if name == umx_core::standard::STD {
            return Err(problems::std_reserved());
        }
        if RESERVED_EDGE_FIELDS.contains(&name.as_str()) {
            return Err(problems::reserved_name(name));
        }
    }
    vocabulary(graph, o, rel, warnings);
    let mut fields = BTreeMap::new();
    let mut differing = BTreeSet::new();
    let mut field_warnings = Vec::new();
    for (name, value) in sets {
        let (field, v) = edge_field(graph, o, rel, name, value, &mut field_warnings, &|g| {
            ctx.same_graph(
                Command::new("edge add")
                    .word("<from>")
                    .name(rel)
                    .word("<to>")
                    .word("--set")
                    .name_with(g, '=', "<value>"),
            )
        })?;
        if existing.is_some_and(|edge| edge.fields.get(name) != Some(&v)) {
            differing.insert(field);
        }
        fields.insert(name.clone(), v);
    }
    if let Some(edge) = existing {
        if !differing.is_empty() {
            return Err(problems::edge_exists_differs(
                edge.id,
                differing.into_iter().collect(),
            ));
        }
        warnings.push(problems::edge_exists(graph, edge.from, rel));
        return Ok(None);
    }
    warnings.extend(field_warnings);
    Ok(Some(fields))
}

/// One custom field an edge write sets, as `edge add` and `edge edit` check it (F92, F134): the name checked (`fix`
/// spells the command again with a good name), and the value typed by the relation's declared field, with the time
/// checks (TM2), a kind mismatch, or an undeclared field warned. The caller refuses reserved names first.
fn edge_field(
    graph: &ValidName,
    o: &Ontology,
    rel: &ValidName,
    name: &str,
    value: &SetValue,
    warnings: &mut Vec<Problem>,
    fix: &dyn Fn(&ValidName) -> Command,
) -> Result<(ValidName, Value), Problem> {
    let field = check_name(name, "field name", fix)?;
    // F92: the relation's declared fields type the value, as a node's type does; an undeclared one warns.
    let declared = o.relation_field_kind(rel.as_str(), name);
    let (v, mismatch) = typed_value(value, declared);
    warnings.extend(super::time_check::field_warning(
        graph,
        super::time_check::Holder::Edge(rel),
        &field,
        value,
        (declared, declared.is_some()),
        &v,
    ));
    match declared {
        Some(kind) if mismatch => {
            warnings.push(problems::field_kind_mismatch(graph, &field, kind.as_str()));
        }
        Some(_) => {}
        None => {
            let inferred = kind_of(&v).unwrap_or(FieldKind::String);
            warnings.push(problems::off_vocabulary_edge_field(
                graph,
                rel,
                &field,
                inferred.as_str(),
            ));
        }
    }
    Ok((field, v))
}

/// The evidence an `edge add` gives (req-124, ws006 S5b): pointers as `--evidence` writes them, and their kind.
#[derive(Clone, Debug, Default)]
pub struct EdgeEvidence {
    pub pointers: Vec<String>,
    pub kind: Option<String>,
}

/// An edge's evidence pointers, parsed and checked exactly as node add's are (shared by the verb and a batch): bad
/// pointers and `root:` problems refuse (S1); `umx:` sources warn dangling_evidence or evidence_unchecked against the
/// graphs `lookup` gives (S0, open graphs only, never a load).
pub(crate) fn edge_evidence<'g>(
    dir: &umx_core::ports::GraphDir,
    given: &EdgeEvidence,
    sources: &SourceCheck,
    lookup: &dyn Fn(&str) -> super::resolve::Found<'g>,
    warnings: &mut Vec<Problem>,
) -> Result<Vec<Value>, Problem> {
    if given.pointers.is_empty() {
        if given.kind.is_some() {
            return Err(problems::usage_invalid(
                "--evidence-kind needs --evidence",
                "edge",
            ));
        }
        return Ok(Vec::new());
    }
    let kind = given
        .kind
        .as_deref()
        .map(|k| {
            EvidenceKind::parse(k).ok_or_else(|| {
                problems::usage_invalid(
                    "--evidence-kind is room or chat or screen or document or analyst",
                    "edge",
                )
            })
        })
        .transpose()?;
    let given: Vec<EvidencePointer> = given
        .pointers
        .iter()
        .map(|pointer| EvidencePointer {
            pointer: pointer.clone(),
            kind,
        })
        .collect();
    let mut pointers = typed_evidence_pointers(dir, &given, sources, lookup)?;
    warnings.extend(check_evidence(&mut pointers, lookup));
    warnings.extend(sources.warnings(&pointers));
    let mut unique = Vec::new();
    add_evidence(&mut unique, pointers);
    Ok(unique)
}

/// Appends `pointers` to the existing edge `e` (edge add on an edge that exists, req-124): never dropped. Returns how
/// many were new. When some were, the `edge_exists` warning [`plan_edge`] pushed (the last warning) becomes
/// `evidence_appended`. An edge whose own custom `evidence` field is not evidence is refused with
/// `evidence_field_invalid` and nothing changes: a record never holds both (graph-storage-expert), and evidence is
/// never dropped (architect, B1). The caller has written nothing yet.
pub(crate) fn append_to_existing(
    graph: &ValidName,
    e: &mut Edge,
    pointers: Vec<Value>,
    warnings: &mut Vec<Problem>,
) -> Result<usize, Problem> {
    if pointers.is_empty() {
        return Ok(0);
    }
    if e.fields.contains_key(EVIDENCE) {
        return Err(problems::evidence_field_invalid(graph, e.id));
    }
    let added = add_evidence(&mut e.evidence, pointers);
    if added > 0 {
        if warnings.last().is_some_and(|w| w.code == "edge_exists") {
            warnings.pop();
        }
        warnings.push(problems::evidence_appended(e.id, added));
    }
    Ok(added)
}

/// `relation_endpoint_unexpected` when a new `rel` edge's end is outside the types the relation expects (ws008 T2b,
/// req-99: std v1's in a std graph, widened by `ontology add relation --from/--to`; soft, the edge is stored). An end
/// whose type is not read (`None`: a far node in a graph not open) is never warned about. Shared by edge add and a
/// batch's edge add.
pub(crate) fn endpoint_warning(
    graph: &ValidName,
    o: &Ontology,
    rel: &ValidName,
    from: Option<&str>,
    to: Option<&str>,
) -> Option<Problem> {
    let miss = o.endpoint_miss(rel.as_str(), from, to, false)?;
    Some(problems::relation_endpoint_unexpected(
        graph,
        rel,
        &ValidName::new(&miss.kind)?,
        miss.expected
            .iter()
            .filter_map(|t| ValidName::new(t))
            .collect(),
    ))
}

/// Warns `off_vocabulary_relation` when the ontology does not declare `rel` and it is not a std relation (`same_as`).
fn vocabulary(graph: &ValidName, o: &Ontology, rel: &ValidName, warnings: &mut Vec<Problem>) {
    let relation = rel.as_str();
    if !o.has_relation(relation) && !umx_core::ontology::STD_RELATIONS.contains(&relation) {
        let near = nearest(relation, o.relations.iter().map(String::as_str));
        warnings.push(problems::off_vocabulary_relation(
            graph,
            rel,
            problems::names(near),
        ));
    }
}

/// The edge `text` names (a full id or address) as `live` holds it: the graph, or a batch as it is so far. Edges
/// have no prefix lookup.
pub(crate) fn resolve_edge(
    alias: &str,
    text: &str,
    live: impl Fn(Id) -> Option<Edge>,
) -> Result<Edge, Problem> {
    let (_, id_text) = split_address(text);
    id_text
        .parse::<Id>()
        .ok()
        .and_then(live)
        .ok_or_else(|| problems::edge_not_found(id_text, &alias_name(alias)))
}

/// Whether `edge delete` and `edge edit` may change the edge: never a derived relation. An edge with an end in
/// another graph may be changed by the graph that stores it (F122): the far end stays the stored address, never
/// resolved, and the incoming index follows the write after it commits.
pub(crate) fn check_changeable(e: &Edge) -> Result<(), Problem> {
    if let Some(kind) = DERIVED_RELATIONS.iter().find(|k| **k == e.kind) {
        return Err(problems::edge_derived(e.id, kind));
    }
    // ws008 E2c (graph-addressing-expert): a denial changes only through entity deny and undeny, so it never loses its
    // reason.
    if e.kind == umx_core::ontology::NOT_SAME_AS {
        return Err(problems::entity_link_invalid(
            "a not_same_as edge changes only through umx entity deny and undeny",
            Command::new("entity undeny").word("<a>").word("<b>"),
        ));
    }
    Ok(())
}

/// graph.json joins a write that touches an edge with an end in another graph (graph-storage-expert, F122: the external
/// edges set graph.json's format, so such a write never takes the one-file path).
pub(crate) fn external_files(edges: &[&Edge]) -> &'static [GraphFile] {
    if edges.iter().any(|e| e.has_external_end()) {
        &[GraphFile::Graph]
    } else {
        &[]
    }
}

/// The edge read as a sentence from what its storing graph `g` holds: a local end by its title there, an end in
/// another graph by its stored address only, never resolved (security, F122: the reply is the same whether the far
/// node exists, is secured, loaded or not).
pub(crate) fn stored_sentence(g: &Graph, e: &Edge) -> String {
    let end = |graph: Option<Id>, node: Id| match graph {
        Some(graph) => format!("umx:{graph}/{node}"),
        None => title(g, node),
    };
    sentence_text(&end(e.from_graph, e.from), &e.kind, &end(e.to_graph, e.to))
}

/// The edge's id as `g` repeats it in edges.jsonl (earlier lines kept, the last in use), if it does.
fn repeat(g: &Graph, id: Id) -> Option<Repeat> {
    g.repeat_lines(GraphFile::Edges, id).map(|lines| Repeat {
        file: GraphFile::Edges,
        id,
        lines,
    })
}

/// Whether `edge delete` may remove the edge from `g`: never while its id has earlier lines kept in edges.jsonl,
/// which the next load would bring back (R196 B2). An edit is safe: the edited line stays the last, the one in use.
pub(crate) fn check_deletable(g: &Graph, graph: &ValidName, e: &Edge) -> Result<(), Problem> {
    match repeat(g, e.id) {
        Some(r) => Err(problems::edge_repeated(graph, &r)),
        None => Ok(()),
    }
}

/// After an edit of an edge whose id is repeated: say so, as every write to a repeated id does (req-67).
pub(crate) fn repeat_warning(g: &Graph, graph: &ValidName, id: Id) -> Option<Problem> {
    repeat(g, id).map(|r| problems::duplicate_id(graph, &r))
}

/// Whether the edge's `updated` and `updated_by` are umx's edit stamp, or absent: a canonical umx time and an
/// author. Anything else is the graph's own field, which an edit never overwrites (graph-storage-expert, F75).
pub(crate) fn stamp_or_none(fields: &BTreeMap<String, Value>) -> bool {
    match (fields.get("updated"), fields.get("updated_by")) {
        (None, None) => true,
        (Some(Value::String(at)), Some(Value::String(_))) => {
            at.contains('T') && umx_core::time::normalise_date(at).as_deref() == Some(at.as_str())
        }
        _ => false,
    }
}

/// `edge edit`'s checks and the edge it makes, shared by the verb and a batch: the edge must be changeable; the new
/// relation is checked like `edge add`'s (off-vocabulary warns); `--reverse` swaps the ends, except on a symmetric
/// relation, which warns; a symmetric result is stored with the lower id as `from` (D49). `--set` and `--unset`
/// change custom fields with `edge add`'s checks, typed by the relation the edge ends with (F134); a reserved name is
/// refused, except that `--unset evidence` removes a custom `evidence` field that is not an evidence list (what
/// `evidence_field_invalid` asks for). An edit that changes something must not make the edge equal to another (`find`
/// answers for the graph or batch as it is now) and stamps `updated` and `updated_by`. Gives the edge and what
/// changed (`type`, `direction`, then the fields); an edit that changes nothing gives the edge as it was and no
/// changes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_edge_edit(
    ctx: &Ctx,
    graph: &ValidName,
    owner: Id,
    o: &Ontology,
    current: &Edge,
    req: &EdgeEditRequest,
    stamp: (&str, &str),
    find: &dyn Fn(StoredEnd, &str, StoredEnd) -> Option<Id>,
    warnings: &mut Vec<Problem>,
) -> Result<(Edge, Vec<String>), Problem> {
    check_changeable(current)?;
    let kind = match &req.relation {
        Some(r) => {
            let rel = check_relation_for(r, &|g| {
                ctx.same_graph(
                    Command::new("edge edit")
                        .word("<edge>")
                        .word("--relation")
                        .name(g),
                )
            })?;
            vocabulary(graph, o, &rel, warnings);
            rel
        }
        None => ValidName::new(&current.kind).ok_or_else(|| problems::internal("edge_kind"))?,
    };
    // Ends as stored, graph-qualified (F122): an edit never changes an end, only their order. A reverse swaps the
    // two ends whole (graph and node), and a symmetric relation puts the lower (graph id, node id) first, a local end
    // counting as `owner`, as add does (D49). The far end is never resolved.
    let (mut from, mut to) = (
        StoredEnd {
            graph: current.from_graph,
            node: current.from,
        },
        StoredEnd {
            graph: current.to_graph,
            node: current.to,
        },
    );
    if req.reverse {
        if o.is_symmetric(kind.as_str()) {
            warnings.push(problems::edge_reverse_symmetric(current.id, &kind));
        } else {
            (from, to) = (to, from);
        }
    }
    let symmetric = o.is_symmetric(kind.as_str());
    let ordered = ordered_ends(owner, symmetric, from, to);
    if ordered != (from, to) {
        // F182: D49's rule moved the stored ends; say so, naming them as stored.
        warnings.push(problems::edge_direction_normalised(
            current.id,
            &kind,
            ordered.0.node,
            ordered.1.node,
        ));
    }
    let (from, to) = ordered;
    let mut changed: Vec<String> = Vec::new();
    if kind.as_str() != current.kind {
        changed.push("type".into());
    }
    if (from.graph, from.node, to.graph, to.node)
        != (
            current.from_graph,
            current.from,
            current.to_graph,
            current.to,
        )
    {
        changed.push("direction".into());
    }
    // F134: custom fields, set then unset, with edge add's checks under the relation the edge ends with.
    let mut fields = current.fields.clone();
    let mut named: BTreeSet<&str> = BTreeSet::new();
    let edit = |g: &ValidName, flag: &'static str| {
        let c = Command::new("edge edit").word("<edge>").word(flag);
        ctx.same_graph(if flag == "--set" {
            c.name_with(g, '=', "<value>")
        } else {
            c.name(g)
        })
    };
    for (name, value) in &req.sets {
        if RESERVED_EDGE_FIELDS.contains(&name.as_str()) {
            return Err(problems::reserved_name(name));
        }
        let (field, v) = edge_field(graph, o, &kind, name, value, warnings, &|g| {
            edit(g, "--set")
        })?;
        named.insert(name.as_str());
        if fields.get(name) != Some(&v) {
            fields.insert(name.clone(), v);
            if !changed.iter().any(|c| c == field.as_str()) {
                changed.push(field.as_str().to_string());
            }
        }
    }
    for name in &req.unsets {
        // A custom `evidence` that is not an evidence list is only a field (req-124): it may be removed.
        if RESERVED_EDGE_FIELDS.contains(&name.as_str())
            && !(name == "evidence" && fields.contains_key(name))
        {
            return Err(problems::reserved_name(name));
        }
        check_name(name, "field name", &|g| edit(g, "--unset"))?;
        named.insert(name.as_str());
        if fields.remove(name).is_some() && !changed.iter().any(|c| c == name) {
            changed.push(name.clone());
        }
    }
    if changed.is_empty() {
        return Ok((current.clone(), changed));
    }
    if changed.iter().any(|c| c == "type") {
        // F92: the edge's custom fields now belong to the new relation; one it does not declare warns, as on add
        // (the ones this edit sets were checked above, and the ones it unsets are gone).
        for (name, value) in &fields {
            if RESERVED_EDGE_FIELDS.contains(&name.as_str())
                || named.contains(name.as_str())
                || o.relation_field_kind(kind.as_str(), name).is_some()
            {
                continue;
            }
            if let Some(field) = ValidName::new(name) {
                let inferred = kind_of(value).unwrap_or(FieldKind::String);
                warnings.push(problems::off_vocabulary_edge_field(
                    graph,
                    &kind,
                    &field,
                    inferred.as_str(),
                ));
            }
        }
    }
    let other = find(from, kind.as_str(), to)
        .filter(|id| *id != current.id)
        .or_else(|| {
            symmetric
                .then(|| find(to, kind.as_str(), from))
                .flatten()
                .filter(|id| *id != current.id)
        });
    if let Some(other) = other {
        return Err(problems::edge_would_duplicate(graph, current.id, other));
    }
    if !stamp_or_none(&current.fields) {
        return Err(problems::edge_reserved_field_in_use(current.id));
    }
    let mut e = set_ends(current.clone(), from, to);
    e.kind = kind.as_str().to_string();
    (e.from, e.to) = (from.node, to.node);
    e.fields = fields;
    let (at, by) = stamp;
    e.fields
        .insert("updated".to_string(), Value::String(at.to_string()));
    e.fields
        .insert("updated_by".to_string(), Value::String(by.to_string()));
    Ok((e, changed))
}

impl GraphStore {
    /// `umx edge delete <edge>` (F75): removes one edge by id. Nothing else records it; the reply carries the edge.
    pub fn edge_delete(&mut self, ctx: &Ctx, edge: &str) -> Outcome<EdgeChanged> {
        let alias = self.select_write(ctx, &[Some(edge)])?;
        let l = self.get(&alias);
        let current = resolve_edge(&alias, edge, |id| {
            l.graph
                .edge(id)
                .filter(|e| l.read_scope().stored_edge(*e))
                .map(|e| e.to_edge())
        })?;
        check_changeable(&current)?;
        check_deletable(&l.graph, &alias_name(&alias), &current)?;
        let sentence = stored_sentence(&l.graph, &current);
        let sentence_flagged = self.stored_sentence_flagged(&alias, &current);
        let files = self.ports.files.clone();
        let l = self
            .graphs
            .get_mut(&alias)
            .expect("selected graph is loaded");
        l.graph.remove_edge(current.id);
        let mut touched = Touched::default();
        touched.edges.removed.insert(current.id);
        let mut which = alloc::vec![GraphFile::Edges];
        which.extend_from_slice(external_files(&[&current]));
        let warnings = match persist(files.as_ref(), &alias, l, &which, touched) {
            Ok(w) => w,
            Err(p) => {
                l.graph.insert_edge(current);
                return Err(p);
            }
        };
        // After the commit only (graph-addressing-expert, graph-storage-expert): the far ends' incoming entries follow
        // the write.
        self.incoming.remove_edge(l.instance, &current);
        let value = EdgeChanged {
            alias,
            edge: current,
            sentence,
            sentence_flagged,
            changed: Vec::new(),
        };
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// `umx edge edit <edge> [--relation <r>] [--reverse]` (F75): retypes or reverses one edge, keeping its id.
    pub fn edge_edit(
        &mut self,
        ctx: &Ctx,
        edge: &str,
        req: &EdgeEditRequest,
    ) -> Outcome<EdgeChanged> {
        let alias = self.select_write(ctx, &[Some(edge)])?;
        let graph = alias_name(&alias);
        let now = self.now();
        let l = self.get(&alias);
        let current = resolve_edge(&alias, edge, |id| {
            l.graph
                .edge(id)
                .filter(|e| l.read_scope().stored_edge(*e))
                .map(|e| e.to_edge())
        })?;
        let mut warnings = Vec::new();
        let (mut edited, changed) = plan_edge_edit(
            ctx,
            &graph,
            l.meta.id,
            l.ontology(),
            &current,
            req,
            (now.as_str(), ctx.actor_display()),
            &|a, k, b| find_edge(&l.graph, a, k, b).map(|e| e.id()),
            &mut warnings,
        )?;
        if !changed.is_empty() {
            warnings.extend(
                super::secrets::edge_write_secrets(&edited.fields, &[])
                    .into_iter()
                    .map(|hit| hit.warning(&graph, edited.id, "Edge")),
            );
        }
        if !changed.is_empty() {
            let extra = external_files(&[&current]);
            let done =
                self.attributed_write_with(ctx, &alias, extra, |batch, session, _, _, _| {
                    edited.record_edit(session);
                    batch.insert_edge(edited.clone());
                    Ok(edited)
                })?;
            edited = done.value;
            warnings.extend(done.notes);
            // After the commit only: the old record's entries out, the new one's in (graph-storage-expert, F122).
            let l = &self.graphs[&alias];
            self.incoming.remove_edge(l.instance, &current);
            self.incoming.add_edge(
                l.instance,
                l.graph.edge(edited.id).expect("the edited edge"),
            );
            warnings.extend(repeat_warning(&self.get(&alias).graph, &graph, edited.id));
        }
        let value = EdgeChanged {
            sentence: stored_sentence(&self.get(&alias).graph, &edited),
            sentence_flagged: self.stored_sentence_flagged(&alias, &edited),
            alias,
            edge: edited,
            changed,
        };
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    pub fn edge_add(
        &mut self,
        ctx: &Ctx,
        from: &str,
        relation: &str,
        to: &str,
        sets: &[(String, SetValue)],
    ) -> Outcome<EdgeAdded> {
        self.edge_add_with(ctx, from, relation, to, sets, &EdgeEvidence::default())
    }

    /// `umx edge add`, with `--evidence` (req-124): on a new edge the pointers fill its evidence slot; on an edge that
    /// exists they are appended ([`append_to_existing`]).
    pub fn edge_add_with(
        &mut self,
        ctx: &Ctx,
        from: &str,
        relation: &str,
        to: &str,
        sets: &[(String, SetValue)],
        given: &EdgeEvidence,
    ) -> Outcome<EdgeAdded> {
        let alias = self.select_author(ctx, &[Some(from), Some(to)])?;
        self.note_redirects(ctx, &[from, to]);
        let graph = alias_name(&alias);
        let rel = check_relation(ctx, relation)?;
        let (from_text, to_text) = (from, to);
        let (from, mut warnings) = self.write_end(&alias, from)?;
        let (to, to_warnings) = self.write_end(&alias, to)?;
        warnings.extend(to_warnings);
        // Read before a symmetric relation orders the ends.
        let end_types = (
            self.end_type(&alias, from, from_text),
            self.end_type(&alias, to, to_text),
        );
        let pointers = {
            let open = self.evidence_graphs(&alias);
            edge_evidence(
                &self.get(&alias).dir,
                given,
                &self.source_check(&alias),
                &|a| open.get(a).copied(),
                &mut warnings,
            )?
        };
        let l = self.get(&alias);
        let o = l.ontology();
        let symmetric = o.is_symmetric(relation);
        let (from, to) = ordered_ends(l.meta.id, symmetric, from, to);
        let found = |a, b| whole_node_edge(find_edge(&l.graph, a, relation, b), l.graph.edges());
        let existing = found(from, to)
            .or_else(|| symmetric.then(|| found(to, from)).flatten())
            .map(|edge| edge.to_edge());
        let fields = match plan_edge(ctx, &graph, o, existing.as_ref(), &rel, sets, &mut warnings)?
        {
            None => {
                let mut edge = existing.expect("the edge exists");
                let start = edge.evidence.len();
                if append_to_existing(&graph, &mut edge, pointers, &mut warnings)? > 0 {
                    warnings.extend(
                        super::secrets::edge_write_secrets(
                            &BTreeMap::new(),
                            &edge.evidence[start..],
                        )
                        .into_iter()
                        .map(|hit| hit.warning(&graph, edge.id, "Edge")),
                    );
                    let extra = external_files(&[&edge]);
                    let done = self.attributed_write_with(
                        ctx,
                        &alias,
                        extra,
                        |batch, session, _, _, _| {
                            edge.record_edit(session);
                            super::provenance::stamp_evidence(&mut edge.evidence[start..], session);
                            batch.insert_edge(edge.clone());
                            Ok(edge)
                        },
                    )?;
                    warnings.extend(done.notes);
                    edge = done.value;
                }
                let row = self
                    .edge_read_row(&ctx.clone().graph(&alias), &[], &alias, edge.id)
                    .expect("the writer named the author");
                let value = EdgeAdded {
                    sentence_flagged: self.edge_titles_flagged(core::slice::from_ref(&row))[0]
                        .contains(&true),
                    sentence: row.sentence,
                    alias,
                    edge,
                    existed: true,
                };
                return Ok(Done {
                    value,
                    notes: warnings,
                });
            }
            Some(fields) => fields,
        };
        warnings.extend(endpoint_warning(
            &graph,
            self.get(&alias).ontology(),
            &rel,
            end_types.0.as_deref(),
            end_types.1.as_deref(),
        ));
        let done = self.attributed_write(ctx, &alias, |batch, session, next, now, _| {
            let mut edge = set_ends(
                new_edge(
                    next(batch.graph()),
                    from.node,
                    relation,
                    to.node,
                    now,
                    &ctx.by,
                    fields,
                ),
                from,
                to,
            );
            edge.evidence = pointers;
            warnings.extend(
                super::secrets::edge_write_secrets(&edge.fields, &edge.evidence)
                    .into_iter()
                    .map(|hit| hit.warning(&graph, edge.id, "Edge")),
            );
            edge.record_author(session);
            super::provenance::stamp_evidence(&mut edge.evidence, session);
            batch.insert_edge(edge.clone());
            Ok(edge)
        })?;
        warnings.extend(done.notes);
        let edge = done.value;
        let l = &self.graphs[&alias];
        self.incoming
            .add_edge(l.instance, l.graph.edge(edge.id).expect("inserted edge"));
        let row = self
            .edge_read_row(&ctx.clone().graph(&alias), &[], &alias, edge.id)
            .expect("the author graph is selected");
        let value = EdgeAdded {
            sentence_flagged: self.edge_titles_flagged(core::slice::from_ref(&row))[0]
                .contains(&true),
            sentence: row.sentence,
            alias,
            edge,
            existed: false,
        };
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    pub fn edge_list(&self, ctx: &Ctx, req: EdgeListRequest) -> Outcome<Page<EdgeRow>> {
        self.edge_list_in(ctx, req, ctx)
    }

    /// `umx edge show <edge id|umx:<alias>/<edge id>>` (req-124): one edge, as edge list shows it, with its evidence in
    /// full. A full edge id only: edge ids come from list output, never typed. Read-only.
    pub fn edge_show(&self, ctx: &Ctx, text: &str) -> Outcome<EdgeRow> {
        let text = super::resolve::fold_scheme(text.trim());
        let parts = super::resolve::split(&text);
        if parts.fragment.is_some() || parts.repo_part.is_some() || parts.id.is_empty() {
            return Err(problems::usage_invalid(
                "edge show takes one edge id or address umx:<graph>/<edge id>",
                "edge",
            ));
        }
        let access = self.external_access(ctx)?;
        let refs = [Some(parts.body)];
        let alias = self.external_read_alias(ctx, &refs)?;
        let graph = alias_name(&alias);
        let found = super::resolve::full_id(parts.id)
            .filter(|id| self.get(&alias).graph.edge(*id).is_some());
        let Some(id) = found else {
            return Err(problems::edge_not_found(parts.id, &graph));
        };
        let row = self
            .edge_read_row(&access, &refs, &alias, id)
            .ok_or_else(|| problems::edge_not_found(parts.id, &graph))?;
        let mut warnings = Vec::new();
        if row.edge.invalid_evidence_field().is_some() {
            warnings.push(problems::evidence_field_kept(id));
        }
        Ok(Done {
            value: row,
            notes: warnings,
        })
    }

    pub(crate) fn edge_list_in(
        &self,
        ctx: &Ctx,
        req: EdgeListRequest,
        access: &Ctx,
    ) -> Outcome<Page<EdgeRow>> {
        let access = self.external_access(access)?;
        let refs = [req.from.as_deref(), req.to.as_deref()];
        let alias = self.external_read_alias(ctx, &refs)?;
        let l = self.get(&alias);
        let from = req
            .from
            .as_deref()
            .map(|f| super::resolve_read_node(l, ctx, &alias, f))
            .transpose()?;
        let to = req
            .to
            .as_deref()
            .map(|t| super::resolve_read_node(l, ctx, &alias, t))
            .transpose()?;
        // Candidates only: no policy is read before the hook, so a node with many incoming links from another graph
        // costs no policy read per link (review).
        let incident = from
            .or(to)
            .map(|node| self.incident_candidates(&access, &refs, &alias, node));
        // The clock hook marks the finish boundary; policies are read fresh while choosing the visible edges.
        self.ports.clock.now_ms();
        // Every candidate is filtered (so the total is exact), but nothing is built for it: rows are built for the
        // page alone (review A1, ap-35). An owner's visibility is a policy read, so it is read once per owner here
        // (after the hook), not once per edge. The candidates are sorted by owner, so `dedup` leaves each owner once.
        let mut owners: Vec<&str> = match &incident {
            Some(edges) => edges.iter().map(|(owner, _)| owner.as_str()).collect(),
            None => vec![alias.as_str()],
        };
        owners.dedup();
        let owners: Vec<(&str, bool)> = owners
            .into_iter()
            .map(|owner| (owner, self.external_visible(&access, &refs, owner)))
            .collect();
        let keep = |owner: &str, graph: Id, edge: EdgeRef<'_>| {
            req.kind.as_deref().is_none_or(|kind| edge.kind() == kind)
                && from.is_none_or(|node| {
                    edge.from() == node && edge.from_graph().unwrap_or(graph) == l.meta.id
                })
                && to.is_none_or(|node| {
                    edge.to() == node && edge.to_graph().unwrap_or(graph) == l.meta.id
                })
                && owners.iter().any(|(o, shown)| *o == owner && *shown)
                && self.edge_ends_shown(&access, &refs, owner, edge)
        };
        let mut listed: Vec<Listed<'_>> = match &incident {
            Some(edges) => edges
                .iter()
                .filter_map(|(owner, id)| {
                    let (graph, edge) = self.edge_record(owner, *id)?;
                    keep(owner, graph, edge).then_some(Listed::Stored(owner, edge))
                })
                .collect(),
            None => l
                .graph
                .edges()
                .filter(|edge| keep(&alias, l.meta.id, *edge))
                .map(|edge| Listed::Stored(&alias, edge))
                .collect(),
        };
        // --along keeps an edge when both its ends are kept: the range is a cut of the graph, and its links are the
        // links inside it. An edge with an end in another graph (or stored there) has an end no spine of this graph
        // places.
        let mut kept = req
            .along
            .as_ref()
            .map(|a| self.along(ctx, &alias, a))
            .transpose()?;
        if let Some(k) = kept.as_mut() {
            listed.retain(|row| {
                let why = match row {
                    Listed::Stored(owner, edge) if *owner == alias.as_str() => {
                        match (edge.from_ix(), edge.to_ix()) {
                            (Some(a), Some(b)) => edge_why(k.why(a), k.why(b)),
                            _ => Why::NotPlaced,
                        }
                    }
                    _ => Why::NotPlaced,
                };
                k.count(why, None);
                why == Why::In
            });
        }
        if ctx.include_provenance {
            listed.extend(
                l.graph
                    .provenance_edges()
                    .filter(|edge| {
                        req.kind.as_deref().is_none_or(|kind| edge.kind == kind)
                            && from.is_none_or(|id| edge.from.id() == id)
                            && to.is_none_or(|id| edge.to.id() == id)
                    })
                    .map(Listed::Derived),
            );
        }
        let mut rows = page(&alias, listed, req.limit, req.offset).map(|listed| match listed {
            Listed::Stored(owner, edge) => self.edge_row(&access, &refs, owner, edge),
            Listed::Derived(edge) => EdgeRow {
                graph: l.meta.id,
                alias: alias.clone(),
                edge: ReadEdge {
                    id: None,
                    kind: edge.kind.into(),
                    from_graph: None,
                    from: edge.from.id(),
                    to_graph: None,
                    to: edge.to.id(),
                    created: String::new(),
                    created_by: String::new(),
                    authored_by_session: None,
                    edited_by_session: None,
                    evidence: Vec::new(),
                    fields: BTreeMap::new(),
                },
                from_graph: l.meta.id,
                to_graph: l.meta.id,
                from_state: None,
                to_state: None,
                from_candidates: Vec::new(),
                to_candidates: Vec::new(),
                from_title: edge.from.title().into(),
                to_title: edge.to.title().into(),
                sentence: sentence_text(edge.from.title(), edge.kind, edge.to.title()),
            },
        });
        let mut warnings = Vec::new();
        // graph-addressing N1: an edge into or out of another graph is never in range and is counted as not placed,
        // though `graph spine show --unplaced` lists only this graph's nodes, so the hint names both kinds.
        const CROSS_GRAPH_EDGES: &str =
            "edges (an end the spine does not place, or in another graph)";
        rows.along = kept.map(|k| k.finish(CROSS_GRAPH_EDGES, "edges", &mut warnings));
        for row in &rows.rows {
            for (end, state, graph, node) in [
                ("from", row.from_state, row.from_graph, row.edge.from),
                ("to", row.to_state, row.to_graph, row.edge.to),
            ] {
                if state == Some("edge_broken") {
                    warnings.push(problems::edge_broken(
                        row.edge.id.expect("stored broken edge"),
                        end,
                        graph,
                        node,
                    ));
                }
            }
        }
        Ok(Done {
            value: rows,
            notes: warnings,
        })
    }

    pub(crate) fn edge_read_row(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        owner: &str,
        id: Id,
    ) -> Option<EdgeRow> {
        let edge = self.graphs.get(owner)?.graph.edge(id)?;
        (self.external_visible(ctx, refs, owner) && self.edge_ends_shown(ctx, refs, owner, edge))
            .then(|| self.edge_row(ctx, refs, owner, edge))
    }

    /// Whether this read may show stored edge `edge` of a visible graph `owner`: both local ends are in the read
    /// scope, and no loaded external end is a node the read hides. Reads policy fresh; builds nothing.
    fn edge_ends_shown(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        owner: &str,
        edge: EdgeRef<'_>,
    ) -> bool {
        if !ctx.read_scope(&self.graphs[owner].graph).stored_edge(edge) {
            return false;
        }
        for from in [true, false] {
            let end = StoredEnd::of(edge, from);
            if let Some(graph) = end.graph
                && let Some(target) = self.read_end(ctx, refs, graph, end.node).loaded
                && target.graph.node(end.node).is_some()
                && !ctx
                    .read_scope(&target.graph)
                    .with_provenance(self.provenance_in_read(ctx, refs, graph))
                    .contains(end.node)
            {
                return false;
            }
        }
        true
    }

    /// The row for a stored edge `edge_shown` passed: ends resolved, titles and the sentence built, the edge decoded.
    fn edge_row(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        owner: &str,
        edge: EdgeRef<'_>,
    ) -> EdgeRow {
        let l = &self.graphs[owner];
        let end = |from: bool| {
            let end = StoredEnd::of(edge, from);
            let graph = end.graph.unwrap_or(l.meta.id);
            let resolved = if end.graph.is_some() {
                self.read_end(ctx, refs, graph, end.node)
            } else {
                super::ReadEnd::local(l)
            };
            let title = resolved
                .loaded
                .filter(|_| resolved.state.is_none())
                .map_or_else(String::new, |loaded| {
                    if end.node == loaded.meta.id {
                        if loaded.meta.title.is_empty() {
                            self.graphs
                                .iter()
                                .find(|(_, value)| core::ptr::eq(*value, loaded))
                                .expect("resolved load belongs to the store")
                                .0
                                .clone()
                        } else {
                            loaded.meta.title.clone()
                        }
                    } else {
                        loaded
                            .graph
                            .node(end.node)
                            .map(|node| node.title().into())
                            .unwrap_or_default()
                    }
                });
            (graph, title, resolved.state, resolved.candidates)
        };
        let (from_graph, from_title, from_state, from_candidates) = end(true);
        let (to_graph, to_title, to_state, to_candidates) = end(false);
        let from_text = if from_state.is_none() {
            from_title.clone()
        } else {
            format!("umx:{from_graph}/{}", edge.from())
        };
        let to_text = if to_state.is_none() {
            to_title.clone()
        } else {
            format!("umx:{to_graph}/{}", edge.to())
        };
        let sentence = sentence_text(&from_text, edge.kind(), &to_text);
        EdgeRow {
            graph: l.meta.id,
            alias: owner.to_string(),
            edge: edge.to_edge().into(),
            from_graph,
            to_graph,
            from_state,
            to_state,
            from_candidates,
            to_candidates,
            from_title,
            to_title,
            sentence,
        }
    }
}

/// An edge list candidate that passed every filter, held until the page is taken: a stored edge of graph `owner`,
/// or a derived provenance edge.
#[derive(Clone)]
enum Listed<'a> {
    Stored(&'a str, EdgeRef<'a>),
    Derived(ProvenanceEdge<'a>),
}
