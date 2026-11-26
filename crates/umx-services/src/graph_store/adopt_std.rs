//! `umx ontology adopt-std` (ws008 T3, req-101): moves a graph without std onto std v1 in one write set. graph.json
//! gets `std: 1` and loses the declarations std now supplies ([`umx_core::adopt::fold`]); each `--map <from>=<to>`
//! renames a type or relation on its own lines, ids and addresses kept, stamped as `node edit --type` and `edge edit
//! --relation` stamp. Every conflict is found first and blocks the write; `--dry-run` writes nothing. Only the named
//! graph is read or written: no other graph is named in any reply.

use super::edge::{check_relation_for, stamp_or_none};
use super::node::{check_name, key_warnings};
use super::ontology::cap;
use super::{Ctx, Done, GraphStore, Loaded, alias_name, persist, problems};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use umx_core::adopt::{self, FieldAt, Names, RenameClash, Renaming};
use umx_core::codec::GraphFile;
use umx_core::ontology::{DERIVED_RELATIONS, NOT_SAME_AS, PROVENANCE_RELATIONS, PROVENANCE_TYPES};
use umx_core::ui::Touched;
use umx_core::{Command, Edge, Id, Problem, ValidName, standard};

/// What `ontology adopt-std` did, or would do with `--dry-run`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdAdopted {
    pub alias: String,
    pub std: u32,
    pub dry_run: bool,
    /// The graph already used std (`already_std`): nothing was planned or written.
    pub unchanged: bool,
    /// The graph's own entries std now supplies, dropped from graph.json.
    pub folded: Names,
    pub folded_fields: Vec<FieldAt>,
    /// The graph's own fields kept on std types and relations (extensions and overrides).
    pub extended: Vec<FieldAt>,
    /// The graph's own names std does not have, kept as they are.
    pub custom: Names,
    pub renamed: Vec<MapDone>,
    /// Each `--field-to-tag` field: what moved into tags (ws008 t2b, req-117).
    pub field_to_tag: Vec<FieldMoved>,
    /// The renamed nodes and those a field moved off: the services recheck their vectors, as for any node write.
    pub nodes: Vec<Id>,
}

/// One `--field-to-tag <field>`: how many nodes moved values into tags, the tags they gained, the nodes that keep a
/// value in the field, how many distinct values became a tag another value of the field also became, and whether the
/// field's declarations went (once no node keeps a value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldMoved {
    pub field: String,
    pub nodes: usize,
    pub tags: usize,
    pub kept: usize,
    pub collapsed: usize,
    pub declaration_removed: bool,
}

/// What a write does to one node: a new type (`--map`), and fields set (Some) or removed (None).
#[derive(Clone, Debug, Default)]
struct NodeChange {
    kind: Option<String>,
    fields: Vec<(String, Option<umx_core::Value>)>,
}

/// The `--field-to-tag` values, checked (ws008 t2b): each a field name given once, not std's (nor `tags`), not read
/// by a declared spine, and a field of nodes: declared on a node type or held by a user node. `own` is the graph's
/// stored ontology before any fold.
fn tag_fields(
    graph: &ValidName,
    l: &Loaded,
    own: &umx_core::Ontology,
    raw: &[String],
) -> Result<Vec<ValidName>, Vec<Problem>> {
    let show = |_: &ValidName| Command::new("ontology show").word("--graph").name(graph);
    let mut out: Vec<ValidName> = Vec::new();
    let mut errors = Vec::new();
    for given in raw {
        let field = match check_name(given, "field name", &show) {
            Ok(f) => f,
            Err(p) => {
                errors.push(p);
                continue;
            }
        };
        if out.contains(&field) {
            errors.push(problems::usage_invalid(
                "each --field-to-tag names a different field",
                "ontology",
            ));
            continue;
        }
        let f = field.as_str();
        let refused = |reason, fix| problems::adopt_field_to_tag_refused(graph, f, reason, fix);
        let on_nodes = own
            .types
            .iter()
            .any(|t| t.fields.iter().any(|d| d.name == f))
            || l.graph
                .nodes()
                .filter(|n| !l.graph.is_provenance(n.ix()))
                .any(|n| n.custom_field_names().any(|name| name == f));
        let on_edges = own
            .relation_fields
            .values()
            .any(|fs| fs.iter().any(|d| d.name == f))
            || l.graph.edges().any(|e| e.to_edge().fields.contains_key(f));
        let problem = if f == umx_core::tags::TAGS || standard::std_field(f).is_some() {
            Some(refused(
                "it is a std field",
                Command::new("docs get").word("ontology"),
            ))
        } else if l.meta.spines.iter().any(|s| s.order.iter().any(|o| o == f)) {
            Some(refused(
                "a spine reads it",
                Command::new("graph spine show").name(graph),
            ))
        } else if !on_nodes {
            Some(refused(
                if on_edges {
                    "only edges have it"
                } else {
                    "no node type declares it and no node holds it"
                },
                show(graph),
            ))
        } else {
            None
        };
        match problem {
            Some(p) => errors.push(p),
            None => out.push(field),
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// The first values each warning lists.
const LISTED: usize = 20;

/// One node's plan for one `--field-to-tag` field.
struct FieldPlan {
    field: usize,
    plan: adopt::FieldTags,
}

/// `--field-to-tag` over every user node (ws008 t2b): each field's values become tags through
/// [`adopt::field_to_tags`], then each node's new tags merge into its tags once, all or nothing
/// ([`adopt::merged_tags`]): past 64 none of the node's values move. Every count is taken from that final outcome.
/// Returns what each field did, the nodes' changes, and `adopt_tag_value_invalid` per field and reason. A value that
/// holds anything secret-shaped (as stored or upper cased) stays and is listed masked; one that is not text is shown
/// by its shape only.
fn plan_tags(
    graph: &ValidName,
    l: &Loaded,
    fields: &[ValidName],
) -> (Vec<FieldMoved>, BTreeMap<Id, NodeChange>, Vec<Problem>) {
    // Per node, in node order: each field's plan.
    let mut plans: BTreeMap<Id, Vec<FieldPlan>> = BTreeMap::new();
    for (i, field) in fields.iter().enumerate() {
        let f = field.as_str();
        let secret = |s: &str| {
            super::tags::secret_shaped(s)
                || umx_core::secrets::field_suspect(f, s).is_some()
                || umx_core::secrets::field_suspect(f, &s.to_ascii_uppercase()).is_some()
        };
        for n in l.graph.nodes().filter(|n| !l.graph.is_provenance(n.ix())) {
            let Some(value) = n.custom_field(f) else {
                continue;
            };
            let plan = adopt::field_to_tags(f, &value, &[], &secret);
            plans
                .entry(n.id())
                .or_default()
                .push(FieldPlan { field: i, plan });
        }
    }
    let mut done: Vec<FieldMoved> = fields
        .iter()
        .map(|f| FieldMoved {
            field: f.as_str().to_string(),
            nodes: 0,
            tags: 0,
            kept: 0,
            collapsed: 0,
            declaration_removed: false,
        })
        .collect();
    let mut texts: Vec<BTreeMap<String, BTreeSet<String>>> = vec![BTreeMap::new(); fields.len()];
    let mut stays: Vec<BTreeMap<adopt::Unmovable, Vec<(Id, String)>>> =
        vec![BTreeMap::new(); fields.len()];
    let mut changes: BTreeMap<Id, NodeChange> = BTreeMap::new();
    for (id, node_plans) in plans {
        let current_value = l
            .graph
            .node(id)
            .and_then(|n| n.custom_field(umx_core::tags::TAGS));
        let current = current_value
            .as_ref()
            .map(umx_core::tags::stored_tags)
            .unwrap_or_default();
        let mut added: Vec<String> = node_plans
            .iter()
            .flat_map(|p| p.plan.moved.iter().map(|(_, t)| t.clone()))
            .filter(|t| !current.contains(t))
            .collect();
        added.sort();
        added.dedup();
        // The node's tags value once merged (entries umx cannot use kept), or None past the cap.
        let merged = adopt::merged_tags(&current, &added).and_then(|_| {
            super::tags::edited(current_value.as_ref(), &added, &[])
                .ok()
                .map(|(v, _)| v)
        });
        let mut change = NodeChange::default();
        for FieldPlan { field, plan } in node_plans {
            let name = fields[field].as_str();
            let mut kept = plan.kept;
            for (v, why) in &plan.unmovable {
                stays[field]
                    .entry(*why)
                    .or_default()
                    .push((id, adopt::shown_value(v, *why)));
            }
            if merged.is_none() && !plan.moved.is_empty() {
                // Past the cap: nothing moves; the value stays whole.
                for (text, _) in &plan.moved {
                    stays[field]
                        .entry(adopt::Unmovable::TagLimit)
                        .or_default()
                        .push((id, text.clone()));
                }
                done[field].kept += 1;
                continue;
            }
            if kept.is_some() {
                done[field].kept += 1;
            }
            if !plan.moved.is_empty() {
                done[field].nodes += 1;
                let mine: BTreeSet<&String> = plan.moved.iter().map(|(_, t)| t).collect();
                done[field].tags += mine.iter().filter(|t| added.contains(t)).count();
                for (text, tag) in &plan.moved {
                    texts[field]
                        .entry(tag.clone())
                        .or_default()
                        .insert(text.clone());
                }
            }
            // A value that moved, or an empty list, leaves the node; what stays is written back.
            if !plan.moved.is_empty() || kept.is_none() {
                change.fields.push((name.to_string(), kept.take()));
            }
        }
        if let Some(value) = merged.filter(|_| !added.is_empty()) {
            change
                .fields
                .push((umx_core::tags::TAGS.to_string(), value));
        }
        if !change.fields.is_empty() {
            changes.insert(id, change);
        }
    }
    let mut warnings = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        let f = field.as_str();
        done[i].collapsed = texts[i].values().map(|t| t.len() - 1).sum();
        done[i].declaration_removed = done[i].kept == 0
            && l.meta
                .ontology
                .types
                .iter()
                .any(|t| t.fields.iter().any(|d| d.name == f));
        for (why, values) in &stays[i] {
            let listed = values
                .iter()
                .take(LISTED)
                .map(|(id, shown)| umx_core::problem::Candidate {
                    id: id.to_text(),
                    title: Some(shown.clone()),
                    score: None,
                })
                .collect();
            warnings.push(problems::adopt_tag_value_invalid(
                graph,
                field,
                why.as_str(),
                values.len(),
                listed,
            ));
        }
    }
    (done, changes, warnings)
}

/// Removes field `f`'s declarations from every node type, and from their default columns (its values are tags now).
fn drop_declarations(o: &mut umx_core::Ontology, f: &str) {
    for t in &mut o.types {
        t.fields.retain(|d| d.name != f);
        t.columns.retain(|c| c != f);
    }
}

/// One `--map`: what it renamed and how many lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapDone {
    pub what: Renaming,
    pub from: String,
    pub to: String,
    pub nodes: usize,
    pub edges: usize,
}

/// A stored edge's ends and relation: (from graph, from, relation, to graph, to).
type Triple<'a> = (Option<Id>, Id, &'a str, Option<Id>, Id);

/// Per (type or relation, std field, on edges): how many records hold a value of another kind, the first of them, and std's kind.
type ValueMisses = BTreeMap<(String, &'static str, bool), (usize, Id, umx_core::FieldKind)>;

struct Map {
    what: Renaming,
    from: ValidName,
    to: ValidName,
}

/// Whether a stored edge is one a std graph keeps for umx alone: of a derived relation, or of a provenance relation
/// between nodes that are not all provenance records (the `reserved_relation_stored` rule).
fn reserved_edge(l: &Loaded, kind: &str, ends: [Option<umx_graph::NodeIx>; 2]) -> bool {
    DERIVED_RELATIONS.contains(&kind)
        || (PROVENANCE_RELATIONS.contains(&kind)
            && ends
                .into_iter()
                .flatten()
                .any(|ix| !l.graph.is_provenance(ix)))
}

/// The type and relation names the graph's own lines use: user nodes' types, and stored edges' relations other than
/// umx's own provenance edges.
fn used_names(l: &Loaded) -> (BTreeSet<&str>, BTreeSet<&str>) {
    let types = l
        .graph
        .nodes()
        .filter(|n| !l.graph.is_provenance(n.ix()))
        .map(|n| n.kind())
        .collect();
    let relations = l
        .graph
        .edges()
        .filter(|e| {
            !PROVENANCE_RELATIONS.contains(&e.kind())
                || reserved_edge(l, e.kind(), [e.from_ix(), e.to_ix()])
        })
        .map(|e| e.kind())
        .collect();
    (types, relations)
}

/// The `--map` values, checked: `<from>=<to>`, two names, each source once and never another map's target; the
/// source a type or a relation of the graph, not both; the target a name a std graph lets the graph use.
fn maps(graph: &ValidName, l: &Loaded, raw: &[String]) -> Result<Vec<Map>, Vec<Problem>> {
    let show = |_: &ValidName| Command::new("ontology show").word("--graph").name(graph);
    let (types, relations) = used_names(l);
    let o = &l.meta.ontology;
    let mut out: Vec<Map> = Vec::new();
    let mut errors = Vec::new();
    for spec in raw {
        let Some((from, to)) = spec.split_once('=') else {
            errors.push(problems::usage_invalid(
                "--map takes <from>=<to> (a type or relation and its new name)",
                "ontology",
            ));
            continue;
        };
        let (from, to) = match (
            check_name(from, "type or relation", &show),
            check_name(to, "type or relation", &show),
        ) {
            (Ok(f), Ok(t)) => (f, t),
            (f, t) => {
                errors.extend(f.err());
                errors.extend(t.err());
                continue;
            }
        };
        if from == to {
            errors.push(problems::usage_invalid(
                "--map renames a name to another name",
                "ontology",
            ));
            continue;
        }
        let is_type = o.type_decl(from.as_str()).is_some() || types.contains(from.as_str());
        let is_relation = o.has_relation(from.as_str())
            || o.relation_fields.contains_key(from.as_str())
            || relations.contains(from.as_str());
        let what = match (is_type, is_relation) {
            (true, true) => {
                errors.push(problems::adopt_map_ambiguous(graph, &from));
                continue;
            }
            (false, false) => {
                errors.push(problems::adopt_map_unknown(graph, &from));
                continue;
            }
            (true, false) => Renaming::Type,
            (false, true) => Renaming::Relation,
        };
        // A target is refused as every other write refuses that name: a relation as `edge add` refuses it, a type as
        // `node add` and `ontology add type` do. `not_same_as` edges are umx entity deny's: never renamed either way.
        let refused = match what {
            Renaming::Type if to.as_str() == standard::STD => Some(problems::std_reserved()),
            Renaming::Type if to.as_str() == "graph" => Some(problems::reserved_name_because(
                "graph",
                "umx names each graph's own node with it",
            )),
            Renaming::Type if PROVENANCE_TYPES.contains(&to.as_str()) => {
                Some(problems::provenance_type_reserved(to.as_str()))
            }
            Renaming::Type => None,
            Renaming::Relation if from.as_str() == NOT_SAME_AS => {
                check_relation_for(from.as_str(), &show).err()
            }
            Renaming::Relation => check_relation_for(to.as_str(), &show).err(),
        };
        if let Some(p) = refused {
            errors.push(p);
            continue;
        }
        if out
            .iter()
            .any(|m| m.from == from || m.to == from || m.from == to)
        {
            errors.push(problems::usage_invalid(
                "each --map renames a different name and none renames a name another --map gives",
                "ontology",
            ));
            continue;
        }
        out.push(Map { what, from, to });
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

impl GraphStore {
    /// `umx ontology adopt-std [--map <from>=<to>]... [--dry-run]` (see the module doc). Conflicts come back as
    /// errors, with `--dry-run`'s rename suggestions as hints beside them; nothing is written while any is left.
    pub fn ontology_adopt_std(
        &mut self,
        ctx: &Ctx,
        map: &[String],
        dry_run: bool,
        field_to_tag: &[String],
    ) -> Result<Done<StdAdopted>, Vec<Problem>> {
        let alias = self.select_write(ctx, &[]).map_err(|p| vec![p])?;
        let graph = alias_name(&alias);
        let l = self.get(&alias);
        let empty = |std: u32, unchanged: bool| StdAdopted {
            alias: alias.clone(),
            std,
            dry_run,
            unchanged,
            folded: Names::default(),
            folded_fields: Vec::new(),
            extended: Vec::new(),
            custom: Names::default(),
            renamed: Vec::new(),
            field_to_tag: Vec::new(),
            nodes: Vec::new(),
        };
        if let Some(std) = l.meta.ontology.std_version() {
            if field_to_tag.is_empty() {
                return Ok(Done {
                    value: empty(std, true),
                    notes: vec![problems::already_std(&graph, std)],
                });
            }
            // ws008 t2b: a std graph takes --field-to-tag alone; --map renames only as a graph adopts std.
            if !map.is_empty() {
                return Err(vec![problems::usage_invalid(
                    "--map renames only while a graph adopts std; this graph already uses std",
                    "ontology",
                )]);
            }
            let fields = tag_fields(&graph, l, &l.meta.ontology, field_to_tag)?;
            let (moved, changes, warnings) = plan_tags(&graph, l, &fields);
            let mut ontology = l.meta.ontology.clone();
            for m in moved.iter().filter(|m| m.declaration_removed) {
                drop_declarations(&mut ontology, &m.field);
            }
            let mut value = empty(std, false);
            value.field_to_tag = moved;
            if dry_run {
                return Ok(Done {
                    value,
                    notes: warnings,
                });
            }
            let spines = l.meta.spines.clone();
            let changes: Vec<(Id, NodeChange)> = changes.into_iter().collect();
            let mut notes = self
                .adopt_write(ctx, &alias, (ontology, spines), &changes, &[])
                .map_err(|p| vec![p])?;
            notes.splice(0..0, warnings);
            value.nodes = changes.iter().map(|(id, _)| *id).collect();
            return Ok(Done { value, notes });
        }
        let fields = tag_fields(&graph, l, &l.meta.ontology, field_to_tag);
        let maps = maps(&graph, l, map);
        let (maps, fields) = match (maps, fields) {
            (Ok(m), Ok(f)) => (m, f),
            (m, f) => {
                let mut errors = m.err().unwrap_or_default();
                errors.extend(f.err().unwrap_or_default());
                return Err(errors);
            }
        };
        let (used_types, used_relations) = used_names(l);
        let mut own = l.meta.ontology.clone();
        let mut spines = l.meta.spines.clone();
        let mut errors = Vec::new();
        for m in &maps {
            let renamed = adopt::rename(
                &mut own,
                &mut spines,
                m.what,
                m.from.as_str(),
                m.to.as_str(),
                &|r| used_relations.contains(r),
            );
            match renamed {
                Ok(()) => {}
                Err(RenameClash::Merge(item)) => errors.push(problems::adopt_map_merge(
                    &graph,
                    m.what.as_str(),
                    (&m.from, &m.to),
                    &item,
                )),
                Err(RenameClash::Symmetry) => {
                    errors.push(problems::adopt_map_symmetry(&graph, &m.from, &m.to))
                }
            }
        }
        let map_of = |what: Renaming, name: &str| {
            maps.iter()
                .position(|m| m.what == what && m.from.as_str() == name)
        };
        // The lines each map renames: user nodes of the type, and every stored edge of the relation but umx's own
        // provenance edges.
        let mut nodes: Vec<(Id, usize)> = Vec::new();
        for n in l.graph.nodes().filter(|n| !l.graph.is_provenance(n.ix())) {
            if let Some(i) = map_of(Renaming::Type, n.kind()) {
                nodes.push((n.id(), i));
            }
        }
        let mut edges: Vec<(Id, usize)> = Vec::new();
        // Each stored edge's ends and relation after the maps, with the map that renames it: two edges that would
        // then be one are refused, naming the renamed one.
        let mut triples: BTreeMap<Triple<'_>, (Id, Option<usize>)> = BTreeMap::new();
        let mut reserved: BTreeMap<&str, usize> = BTreeMap::new();
        for e in l.graph.edges() {
            let ends = [e.from_ix(), e.to_ix()];
            let provenance =
                PROVENANCE_RELATIONS.contains(&e.kind()) && !reserved_edge(l, e.kind(), ends);
            let i = map_of(Renaming::Relation, e.kind()).filter(|_| !provenance);
            let kind = match i {
                Some(i) => {
                    // As `edge edit --relation`: an edge whose own `updated` or `updated_by` is not umx's stamp keeps
                    // it, so the rename is refused rather than overwrite it (F75).
                    if !stamp_or_none(&e.to_edge().fields) {
                        errors.push(problems::edge_reserved_field_in_use(e.id()));
                    }
                    edges.push((e.id(), i));
                    maps[i].to.as_str()
                }
                None => e.kind(),
            };
            if reserved_edge(l, kind, ends) {
                *reserved.entry(kind).or_default() += 1;
            }
            let key = (e.from_graph(), e.from(), kind, e.to_graph(), e.to());
            match triples.get(&key) {
                Some(&(other, j)) if i.is_some() || j.is_some() => {
                    let (m, id, other) = match i {
                        Some(i) => (&maps[i], e.id(), other),
                        None => (&maps[j.expect("one is renamed")], other, e.id()),
                    };
                    errors.push(problems::adopt_map_duplicate_edge(
                        &graph,
                        (&m.from, &m.to),
                        id,
                        other,
                    ));
                }
                Some(_) => {}
                None => {
                    triples.insert(key, (e.id(), i));
                }
            }
        }
        for (relation, count) in &reserved {
            if let Some(r) = ValidName::new(relation) {
                errors.push(problems::adopt_reserved_relation(&graph, &r, *count));
            }
        }
        let fold = adopt::fold(&own);
        for c in fold.clashes.iter().filter(|c| !c.keeps_own_kind()) {
            // The fix names the declaration the graph has now: before a --map that renamed its owner.
            let what = if c.at.relation {
                Renaming::Relation
            } else {
                Renaming::Type
            };
            let on = maps
                .iter()
                .find(|m| {
                    m.what == what
                        && m.to.as_str() == c.at.on
                        && declares(&l.meta.ontology, what, m.from.as_str(), &c.at.field)
                })
                .map_or(c.at.on.as_str(), |m| m.from.as_str());
            let (Some(on_name), Some(field)) = (ValidName::new(on), ValidName::new(&c.at.field))
            else {
                continue;
            };
            // While nodes or edges hold the field, the next lists them; once none does, it removes the declaration.
            let holders = match what {
                Renaming::Type => l
                    .graph
                    .nodes()
                    .filter(|n| n.kind() == on)
                    .any(|n| n.custom_field_names().any(|f| f == c.at.field)),
                Renaming::Relation => l
                    .graph
                    .edges()
                    .filter(|e| e.kind() == on)
                    .any(|e| e.to_edge().fields.contains_key(&c.at.field)),
            };
            let fix = match (what, holders) {
                (Renaming::Type, true) => Command::new("node list")
                    .word("--type")
                    .name(&on_name)
                    .word("--columns")
                    .name(&field),
                (Renaming::Relation, true) => Command::new("edge list")
                    .word("--type")
                    .name(&on_name)
                    .word("--columns")
                    .name(&field),
                (Renaming::Type, false) => Command::new("ontology remove field")
                    .name(&on_name)
                    .name(&field),
                (Renaming::Relation, false) => Command::new("ontology remove field")
                    .word("--relation")
                    .name(&on_name)
                    .name(&field),
            };
            let fix = fix.word("--graph").name(&graph);
            errors.push(problems::adopt_field_kind_clash(
                &graph,
                (on, c.at.relation),
                &field,
                (c.kind, c.std_kind),
                fix,
            ));
        }
        let mut hints = Vec::new();
        if dry_run {
            let mapped = |what: Renaming, n: &str| map_of(what, n).is_some();
            let types: BTreeSet<&str> = fold
                .custom
                .types
                .iter()
                .map(String::as_str)
                .chain(used_types.iter().copied())
                .filter(|t| !mapped(Renaming::Type, t))
                .collect();
            let relations: BTreeSet<&str> = fold
                .custom
                .relations
                .iter()
                .map(String::as_str)
                .chain(used_relations.iter().copied())
                .filter(|r| !mapped(Renaming::Relation, r))
                .collect();
            for (what, names) in [(Renaming::Type, types), (Renaming::Relation, relations)] {
                for (from, to) in adopt::suggestions(what, names.into_iter()) {
                    if let (Some(from), Some(to)) = (ValidName::new(&from), ValidName::new(to)) {
                        hints.push(problems::adopt_rename_suggested(
                            &graph,
                            what.as_str(),
                            &from,
                            &to,
                        ));
                    }
                }
            }
        }
        if let Err(mut e) = cap(&mut errors) {
            e.extend(hints);
            return Err(e);
        }
        let mut warnings = value_warnings(&graph, l, &maps, &map_of);
        warnings.extend(super::ontology::soft_kind_warnings(
            &fold.ontology,
            None,
            &graph,
        ));
        let (moved, tag_changes, tag_warnings) = plan_tags(&graph, l, &fields);
        warnings.extend(tag_warnings);
        let renamed: Vec<MapDone> = maps
            .iter()
            .enumerate()
            .map(|(i, m)| MapDone {
                what: m.what,
                from: m.from.as_str().to_string(),
                to: m.to.as_str().to_string(),
                nodes: nodes.iter().filter(|(_, j)| *j == i).count(),
                edges: edges.iter().filter(|(_, j)| *j == i).count(),
            })
            .collect();
        let mut value = empty(standard::STD_VERSION, false);
        value.folded = fold.folded;
        value.folded_fields = fold.folded_fields;
        value.extended = fold.extended;
        value.custom = fold.custom;
        value.renamed = renamed;
        let mut ontology = fold.ontology;
        for m in moved.iter().filter(|m| m.declaration_removed) {
            drop_declarations(&mut ontology, &m.field);
        }
        value.field_to_tag = moved;
        if dry_run {
            let mut notes = warnings;
            notes.extend(hints);
            return Ok(Done { value, notes });
        }
        let to = |(id, i): &(Id, usize)| (*id, maps[*i].to.as_str().to_string());
        // One change per node: its new type and the fields --field-to-tag moved.
        let mut changes = tag_changes;
        for (id, i) in &nodes {
            changes.entry(*id).or_default().kind = Some(maps[*i].to.as_str().to_string());
        }
        let changes: Vec<(Id, NodeChange)> = changes.into_iter().collect();
        let edge_renames: Vec<(Id, String)> = edges.iter().map(to).collect();
        let mut notes = self
            .adopt_write(ctx, &alias, (ontology, spines), &changes, &edge_renames)
            .map_err(|p| vec![p])?;
        notes.splice(0..0, warnings);
        value.nodes = changes.iter().map(|(id, _)| *id).collect();
        Ok(Done { value, notes })
    }

    /// The one write set: graph.json alone when no line changes, else graph.json with the renamed node and edge
    /// lines through the attributed write `node edit` and `edge edit` use. Memory is put back if the set fails.
    fn adopt_write(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        (ontology, spines): (umx_core::Ontology, Vec<umx_core::spine::SpineDecl>),
        nodes: &[(Id, NodeChange)],
        edges: &[(Id, String)],
    ) -> Result<Vec<Problem>, Problem> {
        let files = self.ports.files.clone();
        let l = self
            .graphs
            .get_mut(alias)
            .expect("selected graph is loaded");
        let before = (
            core::mem::replace(&mut l.meta.ontology, ontology),
            core::mem::replace(&mut l.meta.spines, spines),
        );
        if nodes.is_empty() && edges.is_empty() {
            return persist(
                files.as_ref(),
                alias,
                l,
                &[GraphFile::Graph],
                Touched::default(),
            )
            .inspect_err(|_| {
                (l.meta.ontology, l.meta.spines) = before;
            });
        }
        // Writes read the new vocabulary (text fields, secret checks), as a write after the change would.
        l.refresh_ontology();
        let old_edges: Vec<Edge> = edges
            .iter()
            .filter_map(|(id, _)| l.graph.edge(*id).map(|e| e.to_edge()))
            .collect();
        let allows = self.secret_allows();
        let graph = alias_name(alias);
        let by = ctx.actor_display().to_string();
        let mut secrets = Vec::new();
        let written = self.attributed_write_with(
            ctx,
            alias,
            &[GraphFile::Graph],
            |batch, session, _, now, at| {
                for (id, change) in nodes {
                    let Some(old) = batch.graph().node(*id).map(|n| n.to_node()) else {
                        continue;
                    };
                    let mut n = old.clone();
                    if let Some(kind) = &change.kind {
                        n.kind = kind.clone();
                    }
                    for (field, value) in &change.fields {
                        match value {
                            Some(v) => {
                                n.fields.insert(field.clone(), v.clone());
                            }
                            None => {
                                n.fields.remove(field);
                            }
                        }
                    }
                    n.updated = now.to_string();
                    n.record_edit(session);
                    secrets.extend(
                        super::secrets::node_write_secrets(
                            Some(&old),
                            &n,
                            at.ontology,
                            at.id,
                            &allows,
                        )
                        .into_iter()
                        .map(|hit| hit.warning(&graph, n.id, "Node")),
                    );
                    batch.insert_node(n).map_err(|e| problems::image_full(&e))?;
                }
                for (id, kind) in edges {
                    let Some(mut e) = batch.graph().edge(*id).map(|e| e.to_edge()) else {
                        continue;
                    };
                    e.kind = kind.clone();
                    e.fields.insert(
                        "updated".to_string(),
                        umx_core::Value::String(now.to_string()),
                    );
                    e.fields.insert(
                        "updated_by".to_string(),
                        umx_core::Value::String(by.clone()),
                    );
                    e.record_edit(session);
                    batch.insert_edge(e);
                }
                Ok(())
            },
        );
        let l = self
            .graphs
            .get_mut(alias)
            .expect("selected graph is loaded");
        let mut done = match written {
            Ok(done) => done,
            Err(p) => {
                (l.meta.ontology, l.meta.spines) = before;
                l.refresh_ontology();
                return Err(p);
            }
        };
        l.refresh_ontology();
        let l = &self.graphs[alias];
        for old in &old_edges {
            if old.has_external_end() {
                self.incoming.remove_edge(l.instance, old);
                if let Some(e) = l.graph.edge(old.id) {
                    self.incoming.add_edge(l.instance, e);
                }
            }
        }
        let mut notes = secrets;
        notes.append(&mut done.notes);
        for (id, _) in nodes {
            if let Some(n) = l.graph.node(*id) {
                notes.extend(key_warnings(&graph, &l.graph, &n.to_node()));
            }
        }
        Ok(notes)
    }
}

/// Whether the stored ontology `o` declares `field` on the type or relation `on`.
fn declares(o: &umx_core::Ontology, what: Renaming, on: &str, field: &str) -> bool {
    match what {
        Renaming::Type => o
            .type_decl(on)
            .is_some_and(|t| t.fields.iter().any(|f| f.name == field)),
        Renaming::Relation => o
            .relation_fields
            .get(on)
            .is_some_and(|fs| fs.iter().any(|f| f.name == field)),
    }
}

/// `adopt_value_kind`: per (type or relation after the maps, std field), the user nodes, or the stored edges, holding
/// the field with a value that is not std's kind, counted, with the first. Kept as written: a write warns
/// `field_kind_mismatch` the same way.
fn value_warnings(
    graph: &ValidName,
    l: &Loaded,
    maps: &[Map],
    map_of: &dyn Fn(Renaming, &str) -> Option<usize>,
) -> Vec<Problem> {
    let mut found: ValueMisses = BTreeMap::new();
    let mut miss = |key: (String, &'static str, bool), id: Id, kind| {
        found
            .entry(key)
            .and_modify(|(count, _, _)| *count += 1)
            .or_insert((1, id, kind));
    };
    for n in l.graph.nodes().filter(|n| !l.graph.is_provenance(n.ix())) {
        let kind = map_of(Renaming::Type, n.kind()).map_or(n.kind(), |i| maps[i].to.as_str());
        for name in n.custom_field_names() {
            let Some(s) = standard::fields_of_type(kind).find(|s| s.name == name) else {
                continue;
            };
            if !n
                .custom_field(name)
                .is_some_and(|v| umx_core::value::fits(&v, s.kind))
            {
                miss((kind.to_string(), s.name, false), n.id(), s.kind);
            }
        }
    }
    for e in l.graph.edges() {
        let kind = map_of(Renaming::Relation, e.kind()).map_or(e.kind(), |i| maps[i].to.as_str());
        let edge = e.to_edge();
        for (name, v) in &edge.fields {
            let Some(s) = standard::fields_of_relation(kind).find(|s| s.name == name) else {
                continue;
            };
            if !umx_core::value::fits(v, s.kind) {
                miss((kind.to_string(), s.name, true), e.id(), s.kind);
            }
        }
    }
    found
        .into_iter()
        .filter_map(|((on, field, edges), (count, first, std_kind))| {
            Some(problems::adopt_value_kind(
                graph,
                (&ValidName::new(&on)?, edges),
                &ValidName::new(field)?,
                std_kind,
                (count, first),
            ))
        })
        .collect()
}
