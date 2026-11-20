//! node add | get | list | find.

use super::edge::{EdgeAdded, new_edge, sentence};
use super::external::StoredEnd;
use super::node_more::{add_evidence, canonical_keys, node_add_pointers};
use super::sources::SourceCheck;
use super::{
    Ctx, Done, GraphStore, Outcome, Page, alias_name, check_evidence, page, problems, resolve_node,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::entity::{ENTITY_KEY, KeyRefused, normalise_key, normalise_keys};
use umx_core::name::RESERVED_NODE_FIELDS;
use umx_core::ontology::{DEFAULT_STATUS, FieldKind, PART_OF, nearest};
use umx_core::ports::GraphDir;
use umx_core::problem::Candidate;
use umx_core::standard::RAISED_IN;
use umx_core::tags::TAGS;
use umx_core::time::normalise_date;
use umx_core::value::{SetValue, kind_of, typed_value, where_value};
use umx_core::{Command, Id, Node, Ontology, Problem, ValidName, Value};
use umx_graph::{Graph, NodeFilter, NodeRef, Op, Where, parse_where};

/// Title score at or above which `node add` warns `near_duplicate` against a node of the same type (workshop 007
/// section 5).
pub const NEAR_DUPLICATE: f64 = 0.85;

/// The score a node of another type needs: near-identical wording (ws004 s30, the embeddings-expert). A lesson and a
/// technique worded alike are still worth a look; a component and a limitation that share a scaffold are not.
pub const NEAR_DUPLICATE_OTHER_TYPE: f64 = 0.95;

#[derive(Clone, Debug, Default)]
pub struct NodeAddRequest {
    pub kind: String,
    pub title: String,
    pub status: Option<String>,
    pub note: Option<String>,
    pub sets: Vec<(String, SetValue)>,
    pub parent: Option<String>,
    /// Pointers stored on the new node, parsed as for `node note`: each with its own kind, else `evidence_kind`
    /// (plan 205: a batch line may give either).
    pub evidence: Vec<umx_core::evidence::EvidencePointer>,
    pub evidence_kind: Option<String>,
    /// Makes the node a resource (ws006 S3b): a file, folder or URL in one of the four source forms. An empty title
    /// then defaults to the source's last part.
    pub locator: Option<String>,
    /// `--raised-in <anchor>` (ws008 TM4): a raised_in edge from the new node to this node, in the same write. In a
    /// batch line it overrides the batch's own `--raised-in`.
    pub raised_in: Option<String>,
    /// `--on-key resolve` (ws003 s08): when a node of this type in the written graph holds one of the add's entity
    /// keys, the add resolves to that node and only appends or fills ([`key_holder`], [`enrich`]).
    pub on_key_resolve: bool,
    /// `--tag` (ws008 t2b): std tags, each value one tag or a comma list. Refused in a graph without std.
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeAdded {
    pub alias: String,
    pub node: Node,
    pub parent_edge: Option<EdgeAdded>,
    /// The raised_in edge `--raised-in` added (ws008 TM4).
    pub raised_in_edge: Option<EdgeAdded>,
    /// ws003 s08: the add resolved to a node holding its entity key (`--on-key resolve`) instead of adding one.
    pub resolved: bool,
    /// What a resolved add changed on its node, in order: note, filled fields (entity_key among them), evidence.
    pub changed: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct NodeListRequest {
    pub kind: Option<String>,
    pub status: Option<String>,
    pub wheres: Vec<String>,
    pub within: Option<String>,
    pub limit: usize,
    pub offset: usize,
    /// `--along` (D71 step 12): keep only the rows a spine places in range, before paging.
    pub along: Option<super::AlongRequest>,
    /// `--tag` and `--no-tag` (ws008 t2b): each `--tag` a group (comma OR, groups AND), every `--no-tag` left out.
    pub tags: Vec<String>,
    pub no_tags: Vec<String>,
    /// The list covers several graphs: one that does not use std is skipped with `tag_filter_not_std` instead of
    /// refusing the read.
    pub several: bool,
}

impl Default for NodeListRequest {
    fn default() -> Self {
        NodeListRequest {
            kind: None,
            status: None,
            wheres: Vec::new(),
            within: None,
            limit: 100,
            offset: 0,
            along: None,
            tags: Vec::new(),
            no_tags: Vec::new(),
            several: false,
        }
    }
}

/// A node `node list` returned, read in place from the loaded graph: each column is read on its own, so a page of
/// hot columns never decodes whole lines (review A3, ap-35). Borrowed from the services: render it inside the read.
#[derive(Clone, Copy)]
pub struct ListedNode<'g>(NodeRef<'g>);

impl<'g> ListedNode<'g> {
    pub fn id(self) -> Id {
        self.0.id()
    }

    /// A standard or custom field, with `Node::field`'s names and values (graph-storage's contract).
    pub fn field(self, name: &str) -> Option<Value> {
        self.0.field(name)
    }

    /// The names of the custom fields this node holds, in name order: `Node::fields`' keys.
    pub fn custom_field_names(self) -> impl Iterator<Item = &'g str> {
        self.0.custom_field_names()
    }

    /// The node's whole locator, as a default read shows it. `field("locator")` answers nothing (its parts are the flat
    /// names), so a `--columns locator` projection reads it here. The kind is hot: a node with no locator decodes no line.
    pub fn locator(self) -> Option<umx_core::Locator> {
        self.0.field("locator_kind")?;
        self.0.to_node().locator
    }

    /// The whole node, decoded from its line, for a reader that needs all of it.
    pub fn to_node(self) -> Node {
        self.0.to_node()
    }

    /// The node as far as a row returning `columns` shows it, for the secret scan (F188 over node list, security N1
    /// on s16): id, type, title, the shown custom fields and `prov_key` (the provenance test), read in place, with
    /// every other part empty. A shown column the scan reads from the line (note, evidence, status history, the
    /// locator) takes the whole decoded node instead. The scan looks only at shown columns and judges an allow per
    /// field, so its names are the whole node's.
    pub(crate) fn shown(self, columns: &[String]) -> Node {
        const LINE: [&str; 5] = [
            "note",
            "evidence",
            "status_history",
            "locator",
            "locator_source",
        ];
        if columns.iter().any(|c| LINE.contains(&c.as_str())) {
            return self.to_node();
        }
        let n = self.0;
        Node {
            id: n.id(),
            kind: n.kind().into(),
            title: n.title().into(),
            status: String::new(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: String::new(),
            updated: String::new(),
            created_by: String::new(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: n
                .custom_field_names()
                .filter(|name| *name == "prov_key" || columns.iter().any(|c| c == name))
                .filter_map(|name| Some((name.to_string(), n.custom_field(name)?)))
                .collect(),
        }
    }
}

impl core::fmt::Debug for ListedNode<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ListedNode").field(&self.0.id()).finish()
    }
}

/// The kind a field has in this graph: declared on the type, else declared on any type, else fixed by its first
/// use (the lowest node id holding it).
pub(crate) fn field_kind(
    o: &Ontology,
    g: &Graph,
    type_name: Option<&str>,
    field: &str,
) -> Option<FieldKind> {
    match field {
        ENTITY_KEY => return Some(FieldKind::List),
        // The built-in timestamps compare as dates (UTC spans), as a declared date field does (D82).
        "created" | "updated" => return Some(FieldKind::Date),
        _ => {}
    }
    type_name
        .and_then(|t| o.field_kind(t, field))
        .or_else(|| {
            o.types
                .iter()
                .find_map(|t| t.fields.iter().find(|f| f.name == field).map(|f| f.kind))
        })
        .or_else(|| g.first_use(field).and_then(|v| kind_of(&v)))
}

/// `duplicate_key` for each key node `n` shares with another node of its type in `g` (ws008 E1), after a write.
pub(crate) fn key_warnings(graph: &ValidName, g: &Graph, n: &Node) -> Vec<Problem> {
    if !n.fields.contains_key(ENTITY_KEY) {
        return Vec::new();
    }
    let mut seen: Vec<(Id, Id)> = Vec::new();
    g.duplicate_entity_keys_of(n.id)
        .into_iter()
        .filter(|d| {
            let pair = (d.node, d.in_use);
            !seen.contains(&pair) && {
                seen.push(pair);
                true
            }
        })
        .map(|d| problems::duplicate_key(graph, &n.kind, &d))
        .collect()
}

/// `entity_key_is_title` for each key of `node` whose value is its title, compared without case and with runs of
/// spaces as one (ws008 E2b F7, D63a: a name stored as a confirmed identity). Only when this write gave the key or
/// the title: a node already holding both is not warned again.
pub(crate) fn title_key_warnings(before: Option<&Node>, node: &Node) -> Vec<Problem> {
    let Some(value) = node.fields.get(ENTITY_KEY) else {
        return Vec::new();
    };
    let plain = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let title = plain(&node.title);
    let old: Vec<String> = before
        .filter(|b| b.title == node.title)
        .and_then(|b| b.fields.get(ENTITY_KEY))
        .map(umx_core::entity::stored_keys)
        .unwrap_or_default();
    umx_core::entity::stored_keys(value)
        .iter()
        .filter(|k| !old.contains(k))
        .filter_map(|k| k.split_once(':'))
        .filter(|(_, v)| !title.is_empty() && plain(v) == title)
        .filter_map(|(scheme, _)| ValidName::new(scheme))
        .map(|scheme| problems::entity_key_is_title(node.id, &scheme))
        .collect()
}

/// The refusal for a key `normalise_key` turned down.
pub(crate) fn key_refused(r: &KeyRefused) -> Problem {
    let scheme = r
        .fixed
        .as_deref()
        .and_then(|f| f.split_once(':'))
        .map(|(s, _)| s);
    problems::entity_key_invalid(r.reason, scheme)
}

/// A `--set entity_key=...` value in stored form (ws008 E1): every key normalised and checked, repeats dropped.
/// None when no key is left, which leaves the node without the field. A bad key refuses the whole write.
pub(crate) fn entity_key_value(value: &SetValue) -> Result<Option<Value>, Problem> {
    let given = match value {
        SetValue::Raw(s) | SetValue::Str(s) => Value::String(s.clone()),
        SetValue::Json(v) => v.clone(),
    };
    let keys = normalise_keys(&given).map_err(|r| key_refused(&r))?;
    Ok((!keys.is_empty()).then(|| Value::Array(keys.into_iter().map(Value::String).collect())))
}

/// Splits a `--where` expression as [`parse_where`] does, then takes the value as written (D82): one matching pair of
/// quotes around the whole value, `'...'` or `"..."`, is dropped, so `updated>='2026-11-12'` reads as unquoted.
pub(crate) fn where_parts(expr: &str) -> Result<(String, Op, String), Problem> {
    let (field, op, raw) = parse_where(expr).map_err(|e| problems::where_invalid(e.0))?;
    Ok((field, op, unquote(&raw).to_string()))
}

/// `s` without one matching pair of surrounding quotes, if it has them.
fn unquote(s: &str) -> &str {
    ['\'', '"']
        .iter()
        .find_map(|q| s.strip_prefix(*q)?.strip_suffix(*q))
        .unwrap_or(s)
}

/// A `--where` value. On entity_key, one key in stored form. On another list field (ws008 E1), a value with no comma
/// is one element to look for; with a comma it stays a whole list, compared to the whole field as before
/// (`tags=red,blue`). On a date field (declared, or `created` and `updated`) an ordering operator with a value that
/// is not a date is refused with `where_invalid`, since a text compare would give a silently wrong range (D82); `=`
/// and `!=` still compare such a value as text, so a stored non-date stays findable. Anything else is typed by the
/// field's kind.
fn list_where_value(
    field: &str,
    op: Op,
    raw: &str,
    kind: Option<FieldKind>,
) -> Result<Value, Problem> {
    if kind == Some(FieldKind::Date)
        && !matches!(op, Op::Eq | Op::Ne)
        && normalise_date(raw).is_none()
    {
        return Err(problems::where_not_a_date(field));
    }
    if field == ENTITY_KEY {
        return normalise_key(raw)
            .map(Value::String)
            .map_err(|r| key_refused(&r));
    }
    Ok(match kind {
        Some(FieldKind::List) if !raw.contains(',') => Value::String(raw.to_string()),
        _ => where_value(raw, kind),
    })
}

/// A type, status, relation or field name: refused with `name_invalid` if it breaks rule 2. `fix` rebuilds the
/// command with the suggested name for the error's `next`.
pub(crate) fn check_name(
    name: &str,
    what: &'static str,
    fix: &dyn Fn(&ValidName) -> Command,
) -> Result<ValidName, Problem> {
    ValidName::new(name).ok_or_else(|| problems::name_invalid(name, what, fix))
}

fn add_command() -> Command {
    Command::new("node add")
        .word("--type")
        .word("<type>")
        .word("--title")
        .word("<title>")
}

/// A new node's checked parts before it has an id or times: what `node add` and a batch's node add both build.
pub(crate) struct NodeDraft {
    pub kind: String,
    pub title: String,
    pub status: String,
    pub note: String,
    pub fields: BTreeMap<String, Value>,
    pub evidence: Vec<Value>,
    pub parent: Option<Id>,
    pub locator: Option<umx_core::Locator>,
}

impl NodeDraft {
    pub(crate) fn into_node(self, id: Id, now: &str, by: &str) -> Node {
        Node {
            id,
            kind: self.kind,
            title: self.title,
            status: self.status,
            note: self.note,
            evidence: self.evidence,
            status_history: Vec::new(),
            created: now.to_string(),
            updated: now.to_string(),
            created_by: by.to_string(),
            locator: self.locator,
            authored_by_session: None,
            edited_by_session: None,
            fields: self.fields,
        }
    }
}

/// `node add`'s checks, shared by the verb and a batch so no check is in one and not the other (security, s07):
/// names, statuses and fields against the vocabulary (warnings pushed to `warnings`), the parent (`parent` resolves
/// it: by id or address for the verb, by ref in a batch) and the evidence pointers, in that order. The first
/// refusal is returned. `umx:` evidence sources are checked against the graphs `lookup` gives by alias
/// ([`check_evidence`]); missing targets warn, while virtual graph fragments refuse.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draft_node<'g>(
    ctx: &Ctx,
    graph: &ValidName,
    o: &Ontology,
    g: &Graph,
    dir: &GraphDir,
    sources: &SourceCheck,
    req: NodeAddRequest,
    parent: &mut dyn FnMut() -> Result<Option<Id>, Problem>,
    lookup: &dyn Fn(&str) -> super::resolve::Found<'g>,
    warnings: &mut Vec<Problem>,
) -> Result<NodeDraft, Problem> {
    let status = req
        .status
        .clone()
        .unwrap_or_else(|| DEFAULT_STATUS.to_string());
    let kind = check_name(&req.kind, "node type", &|g| {
        ctx.same_graph(
            Command::new("node add")
                .word("--type")
                .name(g)
                .word("--title")
                .word("<title>"),
        )
    })?;
    if req.kind == umx_core::standard::STD {
        return Err(problems::std_reserved());
    }
    if req.kind == "graph" {
        warnings.push(problems::reserved_type(&kind));
    }
    let status_name = check_name(&status, "status", &|g| {
        ctx.same_graph(add_command().word("--status").name(g))
    })?;
    if umx_core::provenance::user_node_warning(&req.kind).is_some() {
        warnings.push(problems::reserved_type(&kind));
    }
    if o.type_decl(&req.kind).is_none() {
        warnings.push(problems::off_vocabulary_type(
            graph,
            &kind,
            problems::names(nearest(&req.kind, o.type_names())),
        ));
    }
    if !o.has_status(&status) {
        let near = nearest(&status, o.statuses.iter().map(String::as_str));
        warnings.push(problems::off_vocabulary_status(
            graph,
            &status_name,
            problems::names(near),
        ));
    }
    let std_tags = super::tags::is_std(o);
    if !req.tags.is_empty() && !std_tags {
        return Err(problems::tag_needs_std(graph));
    }
    let mut fields = BTreeMap::new();
    for (name, value) in &req.sets {
        if let Some(refused) = problems::locator_name(name) {
            return Err(refused);
        }
        if RESERVED_NODE_FIELDS.contains(&name.as_str()) {
            return Err(problems::reserved_name(name));
        }
        if name == umx_core::standard::STD {
            return Err(problems::std_reserved());
        }
        let field = check_name(name, "field name", &|g| {
            ctx.same_graph(add_command().word("--set").name_with(g, '=', "<value>"))
        })?;
        if name == ENTITY_KEY {
            if let Some(v) = entity_key_value(value)? {
                fields.insert(name.clone(), v);
            }
            continue;
        }
        if name == TAGS && std_tags {
            if !req.tags.is_empty() {
                return Err(problems::usage_invalid(
                    "give tags with --tag or --set tags, not both",
                    "node",
                ));
            }
            if let Some(v) = super::tags::stored(super::tags::set_value(value)?) {
                fields.insert(name.clone(), v);
            }
            continue;
        }
        let declared = field_kind(o, g, Some(&req.kind), name);
        let (v, mismatch) = typed_value(value, declared);
        if mismatch {
            warnings.push(problems::field_kind_mismatch(
                graph,
                &field,
                declared.map_or("unknown", FieldKind::as_str),
            ));
        }
        let in_ontology = o.field_kind(&req.kind, name).is_some();
        warnings.extend(super::time_check::field_warning(
            graph,
            super::time_check::Holder::Node(Some(&kind)),
            &field,
            value,
            (declared, in_ontology),
            &v,
        ));
        if !in_ontology {
            let inferred = declared
                .or_else(|| kind_of(&v))
                .unwrap_or(FieldKind::String);
            warnings.push(problems::off_vocabulary_field(
                graph,
                &kind,
                &field,
                inferred.as_str(),
            ));
        }
        fields.insert(name.clone(), v);
    }
    if !req.tags.is_empty() {
        let retry = ctx.same_graph(
            Command::new("node add")
                .word("--type")
                .name(&kind)
                .word("--title")
                .word("<title>"),
        );
        let retry = super::tags::TagRetry {
            base: retry,
            flags: alloc::vec![("--tag", &req.tags[..], super::tags::Reads::Tags)],
        };
        let given = super::tags::given(&req.tags, &retry)?;
        if let Some(v) = super::tags::stored(given) {
            fields.insert(TAGS.to_string(), v);
        }
    }
    if std_tags && let Some(v) = fields.get(TAGS) {
        warnings.extend(super::tags::off_vocabulary(
            graph,
            o,
            &umx_core::tags::stored_tags(v),
        ));
    }
    let locator = req
        .locator
        .as_deref()
        .map(|text| super::resources::checked_locator(text, dir, sources, "node add"))
        .transpose()?;
    let parent = parent()?;
    let mut pointers = node_add_pointers(
        dir,
        &req.evidence,
        req.evidence_kind.as_deref(),
        sources,
        lookup,
    )?;
    warnings.extend(check_evidence(&mut pointers, lookup));
    let mut evidence = Vec::new();
    add_evidence(&mut evidence, pointers);
    warnings.extend(super::time_check::anchor_warning(
        graph, &req.kind, &fields, None,
    ));
    let title = match (&locator, req.title.is_empty()) {
        (Some(l), true) => super::resources::default_title(l),
        _ => req.title,
    };
    Ok(NodeDraft {
        kind: req.kind,
        title,
        status,
        note: req.note.unwrap_or_default(),
        fields,
        evidence,
        parent,
        locator,
    })
}

/// The nodes near a new node of type `kind` titled `title`: a title scoring at least [`NEAR_DUPLICATE`] on a node of
/// the same type, or [`NEAR_DUPLICATE_OTHER_TYPE`] on any other, whose words not shared are within a typo (ws004 s30).
/// Best first, at most three. Candidates come from the word index by a bound, not a scan of every title (ws004 s15).
pub(crate) fn near_duplicates(g: &Graph, title: &str, kind: &str) -> Option<Near> {
    let hits = g.near_duplicates(
        title,
        Some(kind),
        NEAR_DUPLICATE,
        NEAR_DUPLICATE_OTHER_TYPE,
        3,
    );
    let first = hits.first()?;
    let (first, first_kind) = (first.0.id(), first.0.kind().to_string());
    let candidates = hits
        .into_iter()
        .map(|(n, score)| Candidate {
            id: n.id().to_text(),
            title: Some(n.title().into()),
            score: Some(score),
        })
        .collect();
    Some(Near {
        first,
        first_kind,
        candidates,
    })
}

/// A new node's near duplicates: the best match's id and type, and every candidate.
pub(crate) struct Near {
    pub first: Id,
    pub first_kind: String,
    pub candidates: Vec<Candidate>,
}

impl Near {
    /// The `near_duplicate` warning for new node `subject`.
    pub(crate) fn warning(self, graph: &ValidName, subject: Id) -> Problem {
        problems::near_duplicate(
            graph,
            subject,
            self.first,
            &self.first_kind,
            self.candidates,
        )
    }

    /// The `near_duplicate_line` warning for new node `subject` of a batch, whose best match line `earlier` adds.
    pub(crate) fn line_warning(self, graph: &ValidName, subject: Id, earlier: u32) -> Problem {
        problems::near_duplicate_line(
            graph,
            subject,
            earlier,
            self.first,
            &self.first_kind,
            self.candidates,
        )
    }
}

/// ws002 X1: the `segment_without_parent` hint for a node of the std type segment with no part_of edge out of it
/// (D71 R5: segments nest, part_of their meeting, event or segment). `has_parent` says whether it has one; its next
/// links it to a parent.
pub(crate) fn segment_hint(graph: &ValidName, node: &Node, has_parent: bool) -> Option<Problem> {
    if node.kind != umx_core::standard::SEGMENT || has_parent {
        return None;
    }
    let fix = Command::new("edge add")
        .id(node.id)
        .word(PART_OF)
        .word("<parent>")
        .word("--graph")
        .name(graph);
    Some(problems::segment_without_parent(node.id, fix))
}

/// Whether a node of a committed graph has a part_of edge out of it, to a node of its graph or of another (review
/// R4: a local-to-external edge is in the external adjacency, not the local one).
pub(crate) fn has_part_of(g: &Graph, id: Id) -> bool {
    g.node_ix(id).is_some_and(|ix| {
        g.edges_out(ix).any(|step| step.kind.as_str() == PART_OF)
            || g.edges_out_external(ix)
                .any(|(_, kind, _, _)| kind.as_str() == PART_OF)
    })
}

/// The warning for a parent edge in a graph whose vocabulary lacks part_of.
pub(crate) fn part_of_warning(o: &Ontology, graph: &ValidName) -> Option<Problem> {
    (!o.has_relation(PART_OF)).then(|| {
        let part_of = ValidName::new(PART_OF).expect("part_of is a valid name");
        problems::off_vocabulary_relation(graph, &part_of, Vec::new())
    })
}

/// `--on-key resolve` with `--locator`: a resource node is found by its locator already (workshop 010 section 3).
const ON_KEY_LOCATOR: &str =
    "--on-key and --locator: a resource node is already found by its locator; give one of them";

/// `--on-key resolve` with no entity key to look up (ws003 s08).
const ON_KEY_NO_KEY: &str =
    "--on-key resolve needs an entity key: give --set entity_key=<scheme>:<value>";

/// ws003 s08: the node an on_key add resolves to, looked up by each of its keys (`fields`' entity_key, in stored form)
/// under its own type in `g`, the written graph as a batch has left it so far. Every holder of a key is put through
/// `in_scope` first (a provenance node, which no write names, is skipped as if it held nothing), and only then is the
/// key judged: two holders left, or keys held by two nodes, is `Err` with two of them, both in scope: never the lowest
/// id, never a guess. Other graphs and other types are never read.
pub(crate) fn key_holder(
    g: &Graph,
    kind: &str,
    fields: &BTreeMap<String, Value>,
    in_scope: impl Fn(Id) -> bool,
) -> Result<Option<Id>, (Id, Id)> {
    let Some(value) = fields.get(ENTITY_KEY) else {
        return Ok(None);
    };
    let mut found: Option<Id> = None;
    for key in umx_core::entity::stored_keys(value) {
        let (first, more) = g.entity_holders(kind, &key);
        let Some(first) = first.map(|n| n.id()) else {
            continue;
        };
        let mut holders = alloc::vec![first];
        if more {
            // Only for a key with two or more holders: the others, each a duplicate of the lowest (the one in use).
            holders.extend(
                g.duplicate_entity_keys_of(first)
                    .into_iter()
                    .filter(|d| d.in_use == first && d.key == key)
                    .map(|d| d.node),
            );
        }
        holders.retain(|id| in_scope(*id));
        let holder = match holders.as_slice() {
            [] => continue,
            [one] => *one,
            [a, b, ..] => return Err((*a, *b)),
        };
        match found {
            Some(id) if id != holder => return Err((id, holder)),
            _ => found = Some(holder),
        }
    }
    Ok(found)
}

/// ws003 s08: `entity_type_mismatch` for each node of another type in `g` holding one of `node`'s keys, once per node.
/// An on_key add never resolves across types, so it added `node` beside that node; the fix retypes the new one.
pub(crate) fn other_type_holders(
    graph: &ValidName,
    g: &Graph,
    node: &Node,
    in_scope: impl Fn(Id) -> bool,
) -> Vec<Problem> {
    let Some(value) = node.fields.get(ENTITY_KEY) else {
        return Vec::new();
    };
    let mut seen: Vec<Id> = Vec::new();
    let mut out = Vec::new();
    for key in umx_core::entity::stored_keys(value) {
        for (kind, holder) in g.entity_key_types(&key) {
            let id = holder.id();
            if kind == node.kind || id == node.id || !in_scope(id) || seen.contains(&id) {
                continue;
            }
            seen.push(id);
            out.push(problems::entity_type_mismatch(
                (graph, id, kind),
                (graph, node.id, &node.kind),
                "hold the same entity key",
            ));
        }
    }
    out
}

/// ws003 s08: what node `before`, found by an on_key add's key, takes from the add's draft: append or fill, never
/// overwrite. Returns the node as it would be stored, what changed (note, filled fields in name order with entity_key
/// among them, evidence) and what it kept because it holds another value (title, a given status, note, fields). The
/// title, type and status never change; evidence is deduped as `node note` dedups it; keys it lacks are appended.
pub(crate) fn enrich(
    before: &Node,
    draft: NodeDraft,
    status_given: bool,
    std_tags: bool,
) -> Result<(Node, Vec<String>, Vec<problems::Held>), Problem> {
    use problems::Held;
    let mut n = before.clone();
    let (mut changed, mut held): (Vec<String>, Vec<Held>) = (Vec::new(), Vec::new());
    if draft.title != n.title {
        held.push(Held::Title);
    }
    if status_given && draft.status != n.status {
        held.push(Held::Status);
    }
    if !draft.note.is_empty() {
        if n.note.is_empty() {
            n.note = draft.note;
            changed.push("note".into());
        } else if n.note != draft.note {
            held.push(Held::Note);
        }
    }
    for (name, value) in draft.fields {
        if name == ENTITY_KEY {
            if append_keys(&mut n, &value)? {
                changed.push(name);
            }
            continue;
        }
        match n.fields.get(&name) {
            None => {
                n.fields.insert(name.clone(), value);
                changed.push(name);
            }
            Some(v) if *v == value => {}
            Some(_) if std_tags && name == TAGS => held.push(Held::Tags),
            Some(_) => held.extend(ValidName::new(&name).map(Held::Field)),
        }
    }
    if add_evidence(&mut n.evidence, draft.evidence) > 0 {
        changed.push("evidence".into());
    }
    Ok((n, changed, held))
}

/// Appends the keys of `given` (stored form) that node `n` lacks; whether any was. More than [`MAX_KEYS`] usable keys
/// is refused as a `--set` of them would be.
///
/// [`MAX_KEYS`]: umx_core::entity::MAX_KEYS
fn append_keys(n: &mut Node, given: &Value) -> Result<bool, Problem> {
    let Some(held) = n.fields.get(ENTITY_KEY) else {
        n.fields.insert(ENTITY_KEY.into(), given.clone());
        return Ok(true);
    };
    let mut value = umx_core::entity::canonical(held);
    let Value::Array(items) = &mut value else {
        return Ok(false);
    };
    let mut added = false;
    for key in umx_core::entity::stored_keys(given) {
        if !items.iter().any(|i| i.as_str() == Some(key.as_str())) {
            items.push(Value::String(key));
            added = true;
        }
    }
    let usable = items
        .iter()
        .filter(|i| i.as_str().is_some_and(|s| normalise_key(s).is_ok()))
        .count();
    if usable > umx_core::entity::MAX_KEYS {
        return Err(problems::entity_key_invalid(
            "a node holds at most 32 keys",
            None,
        ));
    }
    if added {
        n.fields.insert(ENTITY_KEY.into(), value);
    }
    Ok(added)
}

/// The stored far ends of the `kind` edges out of node `id`, found through `g`'s adjacency (its degree, not every edge)
/// and each checked against `live`, the record as the write holds it now (a batch's adjacency is not rebuilt until it
/// commits). ws003 s08 compares a resolved add's parent and raised_in anchor with these, by stored ends.
pub(crate) fn ends_out(
    g: &Graph,
    id: Id,
    kind: &str,
    live: impl Fn(Id) -> Option<umx_core::Edge>,
) -> BTreeSet<StoredEnd> {
    let Some(ix) = g.node_ix(id) else {
        return BTreeSet::new();
    };
    g.edges_out(ix)
        .map(|step| step.edge)
        .chain(g.edges_out_external(ix).map(|(edge, ..)| edge))
        .filter_map(|edge| live(g.edge_at(edge).id()))
        .filter(|e| e.kind == kind && e.from == id && e.from_graph.is_none())
        .map(|e| StoredEnd {
            graph: e.to_graph,
            node: e.to,
        })
        .collect()
}

/// A node add's part_of and raised_in edges, built and put in `batch` in the one write: the new node's, or (ws003
/// s08) a resolved node's when it lacked them.
/// `suspect` says whether a node title is one the secret check flags (F234), for the parent sentence.
#[allow(clippy::too_many_arguments)]
fn node_edges(
    batch: &mut umx_graph::Batch<'_>,
    session: Id,
    next: &mut dyn FnMut(&Graph) -> Id,
    (now, by): (&str, &str),
    alias: &str,
    node: Id,
    parent: Option<Id>,
    anchor: Option<&super::raised_in::Anchor>,
    suspect: &dyn Fn(Id, &str) -> bool,
) -> (Option<EdgeAdded>, Option<EdgeAdded>) {
    let parent_edge = parent.map(|p| {
        let mut edge = new_edge(
            next(batch.graph()),
            node,
            PART_OF,
            p,
            now,
            by,
            BTreeMap::new(),
        );
        edge.record_author(session);
        batch.insert_edge(edge.clone());
        let g = batch.graph();
        let flagged = |id: Id| g.node(id).is_some_and(|n| suspect(id, n.title()));
        EdgeAdded {
            sentence: sentence(g, edge.from, &edge.kind, edge.to),
            sentence_flagged: flagged(edge.from) || flagged(edge.to),
            alias: alias.to_string(),
            edge,
            existed: false,
        }
    });
    let raised_in_edge = anchor.map(|a| {
        let mut edge = super::raised_in::edge(next(batch.graph()), node, a, now, by);
        edge.record_author(session);
        batch.insert_edge(edge.clone());
        EdgeAdded {
            sentence: String::new(),
            sentence_flagged: false,
            alias: alias.to_string(),
            edge,
            existed: false,
        }
    });
    (parent_edge, raised_in_edge)
}

impl GraphStore {
    pub fn node_add(&mut self, ctx: &Ctx, req: NodeAddRequest) -> Outcome<NodeAdded> {
        let alias = self.select_write(ctx, &[req.parent.as_deref()])?;
        let graph = alias_name(&alias);
        let on_key = req.on_key_resolve;
        if on_key && req.locator.is_some() {
            return Err(problems::usage_invalid(ON_KEY_LOCATOR, "node"));
        }
        let status_given = req.status.is_some();
        let l = self.get(&alias);
        let evidence = self.evidence_graphs(&alias);
        let mut warnings = Vec::new();
        let parent_text = req.parent.clone();
        // TM4: the anchor is resolved before anything is written, as edge add resolves an end; a refusal writes nothing.
        let anchor = match req.raised_in.as_deref() {
            Some(text) => {
                let (anchor, anchor_warnings) = self.raised_in_anchor(&alias, text)?;
                warnings.extend(anchor_warnings);
                warnings.extend(super::raised_in::vocabulary(l.ontology(), &graph));
                Some(anchor)
            }
            None => None,
        };
        let sources = self.source_check();
        let draft = draft_node(
            ctx,
            &graph,
            l.ontology(),
            &l.graph,
            &l.dir,
            &sources,
            req,
            &mut || {
                parent_text
                    .as_deref()
                    .map(|p| resolve_node(l, &alias, p))
                    .transpose()
            },
            &|a| evidence.get(a).copied(),
            &mut warnings,
        )?;
        if on_key {
            if !draft.fields.contains_key(ENTITY_KEY) {
                return Err(problems::usage_invalid(ON_KEY_NO_KEY, "node"));
            }
            let scope = super::ReadScope::new(&l.graph);
            match key_holder(&l.graph, &draft.kind, &draft.fields, |id| {
                scope.contains(id)
            }) {
                Err((a, b)) => return Err(problems::on_key_ambiguous(&graph, &draft.kind, a, b)),
                Ok(Some(id)) => {
                    return self.resolve_add(
                        ctx,
                        &alias,
                        id,
                        draft,
                        status_given,
                        anchor,
                        warnings,
                    );
                }
                Ok(None) => {}
            }
        }
        if let Some(locator) = &draft.locator {
            // One resource node per locator (workshop 010 section 3): adding it again returns that node.
            use super::resources::{holder, named, spellings};
            let top = self.source_repo_top(&l.dir);
            let names = named(locator, &l.dir, top.as_ref());
            let elsewhere = self.resource_elsewhere(&alias, locator, &l.dir);
            if let Some(id) = holder(&l.graph, &spellings(&names, &l.dir, top.as_ref())) {
                let node = l.graph.node(id).expect("an indexed node").to_node();
                let mut warnings = alloc::vec![problems::resource_exists(&graph, id)];
                warnings.extend(elsewhere);
                return Ok(Done {
                    value: NodeAdded {
                        alias,
                        node,
                        parent_edge: None,
                        raised_in_edge: None,
                        resolved: false,
                        changed: Vec::new(),
                    },
                    notes: warnings,
                });
            }
            warnings.extend(elsewhere);
        }
        let near = near_duplicates(&l.graph, &draft.title, &draft.kind);

        let parent = draft.parent;
        if parent.is_some() {
            warnings.extend(part_of_warning(l.ontology(), &graph));
        }
        let allows = self.secret_allows();
        let done = self.attributed_write(ctx, &alias, |batch, session, next, now, meta| {
            let mut node = draft.into_node(next(batch.graph()), now, &ctx.by);
            warnings.extend(
                super::secrets::node_write_secrets(None, &node, meta.ontology, meta.id, &allows)
                    .into_iter()
                    .map(|hit| hit.warning(&graph, node.id, "Node")),
            );
            node.record_author(session);
            super::provenance::stamp_evidence(&mut node.evidence, session);
            batch
                .insert_node(node.clone())
                .map_err(|e| problems::image_full(&e))?;
            let (parent_edge, raised_in_edge) = node_edges(
                batch,
                session,
                next,
                (now, &ctx.by),
                &alias,
                node.id,
                parent,
                anchor.as_ref(),
                &|id, title| super::secrets::title_suspect(|| &allows, meta.id, id, title),
            );
            Ok(NodeAdded {
                alias: alias.clone(),
                node,
                parent_edge,
                raised_in_edge,
                resolved: false,
                changed: Vec::new(),
            })
        })?;
        warnings.extend(done.notes);
        let mut value = done.value;
        self.raised_in_written(ctx, &alias, &mut value);
        if let Some(near) = near {
            warnings.push(near.warning(&graph, value.node.id));
        }
        let g = &self.get(&alias).graph;
        warnings.extend(key_warnings(&graph, g, &value.node));
        if on_key {
            let scope = super::ReadScope::new(g);
            warnings.extend(other_type_holders(&graph, g, &value.node, |id| {
                scope.contains(id)
            }));
        }
        warnings.extend(title_key_warnings(None, &value.node));
        let parent = has_part_of(g, value.node.id);
        warnings.extend(segment_hint(&graph, &value.node, parent));
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// After a node add's write (a new node's or a resolved one's, ws003 s08): its raised_in edge joins the incoming
    /// index, and its sentence is read as edge add reads it.
    fn raised_in_written(&mut self, ctx: &Ctx, alias: &str, value: &mut NodeAdded) {
        if let Some(added) = &mut value.raised_in_edge {
            let l = &self.graphs[alias];
            self.incoming.add_edge(
                l.instance,
                l.graph.edge(added.edge.id).expect("the raised_in edge"),
            );
            let row = self.edge_read_row(&ctx.clone().graph(alias), &[], alias, added.edge.id);
            added.sentence_flagged = row.as_ref().is_some_and(|row| {
                self.edge_titles_flagged(core::slice::from_ref(row))[0].contains(&true)
            });
            added.sentence = row.map(|row| row.sentence).unwrap_or_default();
        }
    }

    /// `node add --on-key resolve` found node `id` by its key (ws003 s08): append or fill it ([`enrich`]) and add the
    /// parent and raised_in edges it lacks, compared by stored ends, in one write; or write nothing when nothing would
    /// change. The parts it kept are named by the `on_key_field_held` hint, never their values.
    #[allow(clippy::too_many_arguments)]
    fn resolve_add(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        id: Id,
        draft: NodeDraft,
        status_given: bool,
        anchor: Option<super::raised_in::Anchor>,
        mut warnings: Vec<Problem>,
    ) -> Outcome<NodeAdded> {
        let graph = alias_name(alias);
        let l = self.get(alias);
        let before = l.graph.node(id).expect("a key holder").to_node();
        let parent = draft.parent;
        let (mut n, changed, mut held) = enrich(
            &before,
            draft,
            status_given,
            super::tags::is_std(l.ontology()),
        )?;
        let live = |e: Id| l.graph.edge(e).map(|e| e.to_edge());
        let parent = parent.filter(|p| {
            let ends = ends_out(&l.graph, id, PART_OF, live);
            if !ends.is_empty() && !ends.contains(&StoredEnd::local(*p)) {
                held.push(problems::Held::Parent);
            }
            ends.is_empty()
        });
        let anchor = anchor.filter(|a| !ends_out(&l.graph, id, RAISED_IN, live).contains(&a.end));
        if parent.is_some() {
            warnings.extend(part_of_warning(l.ontology(), &graph));
        }
        let node_changed = !changed.is_empty();
        let mut value = if node_changed || parent.is_some() || anchor.is_some() {
            canonical_keys(&mut n, super::tags::is_std(l.ontology()));
            let evidence_start = before.evidence.len();
            let allows = self.secret_allows();
            let done = self.attributed_write(ctx, alias, |batch, session, next, now, meta| {
                if node_changed {
                    warnings.extend(
                        super::secrets::node_write_secrets(
                            Some(&before),
                            &n,
                            meta.ontology,
                            meta.id,
                            &allows,
                        )
                        .into_iter()
                        .map(|hit| hit.warning(&graph, id, "Node")),
                    );
                    n.updated = now.to_string();
                    n.record_edit(session);
                    super::provenance::stamp_evidence(&mut n.evidence[evidence_start..], session);
                    batch
                        .insert_node(n.clone())
                        .map_err(|e| problems::image_full(&e))?;
                }
                let (parent_edge, raised_in_edge) = node_edges(
                    batch,
                    session,
                    next,
                    (now, &ctx.by),
                    alias,
                    id,
                    parent,
                    anchor.as_ref(),
                    &|id, title| super::secrets::title_suspect(|| &allows, meta.id, id, title),
                );
                Ok(NodeAdded {
                    alias: alias.to_string(),
                    node: if node_changed { n } else { before.clone() },
                    parent_edge,
                    raised_in_edge,
                    resolved: true,
                    changed,
                })
            })?;
            warnings.extend(done.notes);
            done.value
        } else {
            NodeAdded {
                alias: alias.to_string(),
                node: before.clone(),
                parent_edge: None,
                raised_in_edge: None,
                resolved: true,
                changed,
            }
        };
        self.raised_in_written(ctx, alias, &mut value);
        if node_changed {
            // As every node write says (req-67, ws008 E1).
            if let Some(r) = self.graphs[alias].repeat(umx_core::codec::GraphFile::Nodes, id) {
                warnings.push(problems::duplicate_id(&graph, &r));
            }
            warnings.extend(key_warnings(&graph, &self.get(alias).graph, &value.node));
            warnings.extend(title_key_warnings(Some(&before), &value.node));
        }
        if !held.is_empty() {
            warnings.push(problems::on_key_field_held(&graph, id, &held));
        }
        let parent = has_part_of(&self.get(alias).graph, id);
        warnings.extend(segment_hint(&graph, &value.node, parent));
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    pub fn node_get(&self, ctx: &Ctx, node: &str) -> Outcome<(String, Node)> {
        if node.starts_with("umx:") && (node.contains('#') || node.contains('?')) {
            return Err(problems::usage_invalid(
                "a graph address has no selector or fragment",
                "node",
            ));
        }
        let alias = self.select(ctx, &[Some(node)])?;
        let l = self.get(&alias);
        let id = super::resolve_read_node(l, ctx, &alias, node)?;
        let warnings = self.stale_note(&alias).into_iter().collect();
        let n = match id == l.meta.id {
            true => self.virtual_node(&alias),
            false => l.graph.node(id).expect("resolved").to_node(),
        };
        Ok(Done {
            value: (alias, n),
            notes: warnings,
        })
    }

    pub fn node_list(&self, ctx: &Ctx, req: NodeListRequest) -> Outcome<Page<ListedNode<'_>>> {
        let alias = self.select(ctx, &[req.within.as_deref()])?;
        let l = self.get(&alias);
        let mut wheres = Vec::new();
        for expr in &req.wheres {
            let (field, op, raw) = where_parts(expr)?;
            let kind = field_kind(l.ontology(), &l.graph, req.kind.as_deref(), &field);
            wheres.push(Where {
                value: list_where_value(&field, op, &raw, kind)?,
                field,
                op,
            });
        }
        let within = match &req.within {
            Some(w) => Some(super::resolve_node_in(l, ctx, &alias, w)?),
            None => None,
        };
        let scope = within.map(|root| ctx.read_scope(&l.graph).within(root));
        let tags = if req.tags.is_empty() && req.no_tags.is_empty() {
            umx_graph::TagFilter::default()
        } else if !super::tags::is_std(l.ontology()) {
            if req.several {
                let empty = page(&alias, Vec::<NodeRef<'_>>::new(), req.limit, req.offset);
                return Ok(Done {
                    value: empty.map(ListedNode),
                    notes: alloc::vec![problems::tag_filter_not_std(1)],
                });
            }
            return Err(problems::tag_needs_std(&alias_name(&alias)));
        } else {
            use super::tags::{Reads, TagRetry};
            let base = ctx.same_graph(Command::new("node list"));
            let base = match req.kind.as_deref().and_then(ValidName::new) {
                Some(k) => base.word("--type").name(&k),
                None => base,
            };
            let retry = TagRetry {
                base,
                flags: alloc::vec![
                    ("--tag", &req.tags[..], Reads::Patterns),
                    ("--no-tag", &req.no_tags[..], Reads::Patterns),
                ],
            };
            super::tags::filter(&req.tags, &req.no_tags, &retry)?
        };
        let filter = NodeFilter {
            tags,
            kind: req.kind,
            status: req.status,
            wheres,
            within: None,
        };
        let all: Vec<_> = l
            .graph
            .list_nodes(&filter)
            .into_iter()
            .filter(|n| {
                ctx.read_scope(&l.graph).contains_ix(n.ix())
                    && scope.as_ref().is_none_or(|s| s.contains_ix(n.ix()))
            })
            .collect();
        let Some(along) = &req.along else {
            return Ok(Done::ok(
                page(&alias, all, req.limit, req.offset).map(ListedNode),
            ));
        };
        let mut kept = self.along(ctx, &alias, along)?;
        let all: Vec<_> = all.into_iter().filter(|n| kept.admit(n.ix())).collect();
        let mut notes = Vec::new();
        let mut p = page(&alias, all, req.limit, req.offset).map(ListedNode);
        p.along = Some(kept.finish("nodes", "nodes", &mut notes));
        Ok(Done { value: p, notes })
    }

    /// Nodes by approximate title, best first, with their scores.
    /// Approximate title match (req-57), best first; `within` keeps only nodes under that node by part_of.
    #[allow(clippy::too_many_arguments)]
    pub fn node_find(
        &self,
        ctx: &Ctx,
        text: &str,
        kind: Option<&str>,
        within: Option<&str>,
        limit: usize,
        along: Option<&super::AlongRequest>,
        (tags, no_tags): (&[String], &[String]),
    ) -> Outcome<Page<(Node, f64)>> {
        if along.is_some() {
            super::along::one_graph(ctx)?;
        }
        let alias = self.select(ctx, &[within])?;
        let l = self.get(&alias);
        // ws008 t2b: --tag/--no-tag keep matches before the page is cut. node find reads one graph.
        let base = Command::new("node find").word("<text>");
        let base = match kind.and_then(ValidName::new) {
            Some(k) => base.word("--type").name(&k),
            None => base,
        };
        if !(tags.is_empty() && no_tags.is_empty()) && !super::tags::is_std(l.ontology()) {
            return Err(problems::tag_needs_std(&alias_name(&alias)));
        }
        let tagged = super::tags::read_filter(tags, no_tags, ctx.same_graph(base))?;
        let scope = match within {
            Some(w) => Some(
                ctx.read_scope(&l.graph)
                    .within(super::resolve_node_in(l, ctx, &alias, w)?),
            ),
            None => None,
        };
        let found = if ctx.include_provenance {
            l.graph.find_with_provenance(text, kind, usize::MAX)
        } else {
            l.graph.find(text, kind, usize::MAX)
        };
        let mut all: Vec<_> = found
            .into_iter()
            .filter(|(n, _)| ctx.read_scope(&l.graph).contains_ix(n.ix()))
            .filter(|(n, _)| scope.as_ref().is_none_or(|w| w.contains(n.id())))
            .collect();
        // No title matched at all, before --along and the tags: a range or tag filter that left every match out is not
        // a miss.
        let empty = all.is_empty();
        if let Some(f) = &tagged {
            all.retain(|(n, _)| l.graph.tags_pass(*n, f));
        }
        let mut notes = Vec::new();
        let info = match along {
            Some(a) => {
                let mut kept = self.along(ctx, &alias, a)?;
                all.retain(|(n, _)| kept.admit(n.ix()));
                Some(kept.finish("nodes", "nodes", &mut notes))
            }
            None => None,
        };
        let mut p = page(&alias, all, limit, 0).map(|(n, s)| (n.to_node(), s));
        p.along = info;
        let mut done = Done { value: p, notes };
        if empty {
            done.notes.push(problems::find_no_match());
        }
        done.notes.extend(self.stale_note(&alias));
        Ok(done)
    }
}
