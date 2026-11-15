//! node add | get | list | find.

use super::edge::{EdgeAdded, new_edge, sentence};
use super::node_more::{add_evidence, node_add_pointers};
use super::sources::SourceCheck;
use super::{
    Ctx, Done, GraphStore, Outcome, Page, alias_name, check_evidence, page, problems, resolve_node,
};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::entity::{ENTITY_KEY, KeyRefused, normalise_key, normalise_keys};
use umx_core::name::RESERVED_NODE_FIELDS;
use umx_core::ontology::{DEFAULT_STATUS, FieldKind, PART_OF, nearest};
use umx_core::ports::GraphDir;
use umx_core::problem::Candidate;
use umx_core::value::{SetValue, kind_of, typed_value, where_value};
use umx_core::{Command, Id, Node, Ontology, Problem, ValidName, Value};
use umx_graph::{Graph, NodeFilter, NodeRef, Where, parse_where};

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
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeAdded {
    pub alias: String,
    pub node: Node,
    pub parent_edge: Option<EdgeAdded>,
    /// The raised_in edge `--raised-in` added (ws008 TM4).
    pub raised_in_edge: Option<EdgeAdded>,
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
    if field == ENTITY_KEY {
        return Some(FieldKind::List);
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

/// A `--where` value. On entity_key, one key in stored form. On another list field (ws008 E1), a value with no comma
/// is one element to look for; with a comma it stays a whole list, compared to the whole field as before
/// (`tags=red,blue`). Anything else is typed by the field's kind.
fn list_where_value(field: &str, raw: &str, kind: Option<FieldKind>) -> Result<Value, Problem> {
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
    let mut fields = BTreeMap::new();
    for (name, value) in &req.sets {
        if let Some(refused) = problems::locator_name(name) {
            return Err(refused);
        }
        if RESERVED_NODE_FIELDS.contains(&name.as_str()) {
            return Err(problems::reserved_name(name));
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

impl GraphStore {
    pub fn node_add(&mut self, ctx: &Ctx, req: NodeAddRequest) -> Outcome<NodeAdded> {
        let alias = self.select_write(ctx, &[req.parent.as_deref()])?;
        let graph = alias_name(&alias);
        let l = self.get(&alias);
        let evidence = self.evidence_graphs(&alias);
        let mut warnings = Vec::new();
        let parent_text = req.parent.clone();
        // TM4: the anchor is resolved before anything is written, as edge add resolves an end; a refusal writes nothing.
        let anchor = match req.raised_in.as_deref() {
            Some(text) => {
                let (anchor, anchor_warnings) = self.raised_in_anchor(&alias, text)?;
                warnings.extend(anchor_warnings);
                warnings.extend(super::raised_in::vocabulary(&l.meta.ontology, &graph));
                Some(anchor)
            }
            None => None,
        };
        let sources = self.source_check();
        let draft = draft_node(
            ctx,
            &graph,
            &l.meta.ontology,
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
                    },
                    notes: warnings,
                });
            }
            warnings.extend(elsewhere);
        }
        let near = near_duplicates(&l.graph, &draft.title, &draft.kind);

        let parent = draft.parent;
        if parent.is_some() {
            warnings.extend(part_of_warning(&l.meta.ontology, &graph));
        }
        let allows = self.secret_allows();
        let done = self.attributed_write(ctx, &alias, |batch, session, next, now, meta| {
            let mut node = draft.into_node(next(batch.graph()), now, &ctx.by);
            warnings.extend(
                super::secrets::node_write_secrets(None, &node, &meta.ontology, meta.id, &allows)
                    .into_iter()
                    .map(|hit| hit.warning(&graph, node.id, "Node")),
            );
            node.record_author(session);
            super::provenance::stamp_evidence(&mut node.evidence, session);
            batch
                .insert_node(node.clone())
                .map_err(|e| problems::image_full(&e))?;
            let parent_edge = parent.map(|p| {
                let mut edge = new_edge(
                    next(batch.graph()),
                    node.id,
                    PART_OF,
                    p,
                    now,
                    &ctx.by,
                    BTreeMap::new(),
                );
                edge.record_author(session);
                batch.insert_edge(edge.clone());
                let g = batch.graph();
                let flagged = |id: Id| {
                    g.node(id).is_some_and(|n| {
                        super::secrets::title_suspect(|| &allows, meta.id, id, n.title())
                    })
                };
                EdgeAdded {
                    sentence: sentence(g, edge.from, &edge.kind, edge.to),
                    sentence_flagged: flagged(edge.from) || flagged(edge.to),
                    alias: alias.clone(),
                    edge,
                    existed: false,
                }
            });
            let raised_in_edge = anchor.as_ref().map(|a| {
                let mut edge =
                    super::raised_in::edge(next(batch.graph()), node.id, a, now, &ctx.by);
                edge.record_author(session);
                batch.insert_edge(edge.clone());
                EdgeAdded {
                    sentence: String::new(),
                    sentence_flagged: false,
                    alias: alias.clone(),
                    edge,
                    existed: false,
                }
            });
            Ok(NodeAdded {
                alias: alias.clone(),
                node,
                parent_edge,
                raised_in_edge,
            })
        })?;
        warnings.extend(done.notes);
        let mut value = done.value;
        if let Some(added) = &mut value.raised_in_edge {
            // After the commit only: the incoming index follows, and the sentence is read as edge add reads it.
            let l = &self.graphs[&alias];
            self.incoming.add_edge(
                l.instance,
                l.graph.edge(added.edge.id).expect("the raised_in edge"),
            );
            let row = self.edge_read_row(&ctx.clone().graph(&alias), &[], &alias, added.edge.id);
            added.sentence_flagged = row.as_ref().is_some_and(|row| {
                self.edge_titles_flagged(core::slice::from_ref(row))[0].contains(&true)
            });
            added.sentence = row.map(|row| row.sentence).unwrap_or_default();
        }
        if let Some(near) = near {
            warnings.push(near.warning(&graph, value.node.id));
        }
        warnings.extend(key_warnings(&graph, &self.get(&alias).graph, &value.node));
        warnings.extend(title_key_warnings(None, &value.node));
        let parent = has_part_of(&self.get(&alias).graph, value.node.id);
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
            let (field, op, raw) = parse_where(expr).map_err(|e| problems::where_invalid(e.0))?;
            let kind = field_kind(&l.meta.ontology, &l.graph, req.kind.as_deref(), &field);
            wheres.push(Where {
                value: list_where_value(&field, &raw, kind)?,
                field,
                op,
            });
        }
        let within = match &req.within {
            Some(w) => Some(super::resolve_node_in(l, ctx, &alias, w)?),
            None => None,
        };
        let scope = within.map(|root| ctx.read_scope(&l.graph).within(root));
        let filter = NodeFilter {
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
    pub fn node_find(
        &self,
        ctx: &Ctx,
        text: &str,
        kind: Option<&str>,
        within: Option<&str>,
        limit: usize,
        along: Option<&super::AlongRequest>,
    ) -> Outcome<Page<(Node, f64)>> {
        if along.is_some() {
            super::along::one_graph(ctx)?;
        }
        let alias = self.select(ctx, &[within])?;
        let l = self.get(&alias);
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
        // No title matched at all, before --along: a range that left every match out is not a miss.
        let empty = all.is_empty();
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
