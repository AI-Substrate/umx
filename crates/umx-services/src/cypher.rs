//! `umx query` (req-27, workstream 005 s04): a read-only Cypher query over the loaded graphs, beside walk and search
//! (blueprint 001 d-12). Stateless: it runs on the request's thread under the shared read lock, never on the writer.
//!
//! - Scope (d-20, SEC-5): `--graph a,b` (or one name, or `UMX_GRAPH`) names the graphs, whatever their policy; with
//!   none named, the open graphs only, and the secured ones are counted in `secured_graphs_skipped`, never named. The
//!   policies are read again when the query finishes; a graph no longer open is dropped and the query runs again.
//! - Each graph's symmetric relations come from its own ontology (D49 R3).
//! - The 10 s backstop is a cancel hook over the clock port: `query_timed_out`, no rows.
//! - At most [`QUERY_PERMITS`] queries run at once (graph-storage-expert F2: a query may hold hundreds of MB);
//!   another is refused at once with `query_busy`. No wait here: services has no sleep (architect).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::traversal::ReadSpace;
use umx_core::Value;
use umx_core::name::is_snake_case;
use umx_core::problem::{Problem, Slot};
use umx_core::value::{SetValue, typed_value};
use umx_query::{GraphView, Param, Part, RunOptions};

struct Crossings<'a> {
    space: ReadSpace<'a>,
    skipped: core::cell::RefCell<alloc::collections::BTreeSet<(usize, Id)>>,
    unresolved_work: core::cell::Cell<u64>,
}

impl umx_query::ExternalSteps for Crossings<'_> {
    fn steps(
        &self,
        at: umx_query::At<umx_query::ViewNode>,
        incoming: bool,
        relations: Option<&[String]>,
    ) -> Vec<umx_query::ExternalStep> {
        let (alias, node) = at.in_parts(&self.space.aliases);
        let part = self
            .space
            .aliases
            .iter()
            .position(|a| a == alias)
            .expect("selected");
        let loaded = self
            .space
            .store
            .loaded(&self.space.aliases[part])
            .expect("selected");
        let id = match node {
            umx_query::ViewNode::Stored(ix) => loaded.graph.node_at(ix).id(),
            umx_query::ViewNode::Graph => loaded.meta.id,
        };
        self.space
            .external_steps(self.space.at(part, id), crate::Direction::Both, None, false)
            .into_iter()
            .filter(|hop| (hop.edge.way == crate::Way::In) == incoming)
            .filter(|hop| relations.is_none_or(|names| names.contains(&hop.edge.relation)))
            .filter_map(|hop| {
                if self.space.state(hop.to).is_some() {
                    self.unresolved_work
                        .set(self.unresolved_work.get().saturating_add(1));
                    self.skipped
                        .borrow_mut()
                        .insert((hop.owner, hop.edge.id.expect("stored")));
                    return None;
                }
                let target = hop.to.part?;
                let loaded = self.space.store.loaded(&self.space.aliases[target])?;
                let node = if hop.to.node == hop.to.graph {
                    umx_query::ViewNode::Graph
                } else {
                    umx_query::ViewNode::Stored(loaded.graph.node_ix(hop.to.node)?)
                };
                let owner = self.space.store.loaded(&self.space.aliases[hop.owner])?;
                Some(umx_query::ExternalStep {
                    edge: umx_query::At::new(
                        hop.owner as u32,
                        owner.graph.edge(hop.edge.id?)?.ix(),
                    ),
                    node: umx_query::At::new(target as u32, node),
                })
            })
            .collect()
    }

    fn take_unresolved_work(&self) -> u64 {
        self.unresolved_work.replace(0)
    }

    fn graph_nodes(&self) -> Vec<umx_query::At<umx_query::ViewNode>> {
        self.space
            .aliases
            .iter()
            .enumerate()
            .filter_map(|(part, alias)| {
                let id = self.space.store.loaded(alias).expect("selected").meta.id;
                let hops = self.space.external_steps(
                    self.space.at(part, id),
                    crate::Direction::Both,
                    None,
                    false,
                );
                self.unresolved_work
                    .set(self.unresolved_work.get().saturating_add(hops.len() as u64));
                hops.iter()
                    .any(|hop| self.space.state(hop.to).is_none())
                    .then(|| umx_query::At::new(part as u32, umx_query::ViewNode::Graph))
            })
            .collect()
    }
}

use crate::graph_store::{GRAPH_WIDE, GraphStore};
use crate::{Ctx, Done, Outcome, problems};

use umx_core::Id;
use umx_core::ports::ModelKey;

/// One value of a query row. Services' own type: the daemon names no umx-query type (E-2).
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Cell>),
    Map(Vec<(String, Cell)>),
    Node(NodeCell),
    Edge(EdgeCell),
    Similar(SimilarCell),
    Provenance(ProvenanceCell),
}

/// A whole similar_to edge in a row (workshop 008 section 9): derived from the graph's lists, so it has no id or
/// address. `score` is in ten-thousandths.
#[derive(Clone, Debug, PartialEq)]
pub struct SimilarCell {
    pub graph: String,
    pub from: Id,
    pub to: Id,
    pub score: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProvenanceCell {
    pub graph: String,
    pub kind: String,
    pub from: Id,
    pub to: Id,
}

/// The similar_to lists of one graph a query read: the model they were made with, k, the floor (ten-thousandths) and
/// whether that floor is calibrated for the model, where the graph's floor comes from and its calibration run.
#[derive(Clone, Debug, PartialEq)]
pub struct QuerySimilarity {
    pub graph: String,
    pub key: ModelKey,
    pub k: usize,
    pub floor: i32,
    pub floor_calibrated: bool,
    pub floor_source: umx_core::similarity::FloorSource,
    pub floor_run: Option<&'static str>,
}

/// A whole node in a row: umx's node shape (cypher-expert section 1.2). `title_flagged`: umx's secret check flags the
/// title and no allow covers it, the rule of a read (F241); never set on the graph node, whose title is checked as a
/// string at its row's column.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeCell {
    pub id: Id,
    pub address: String,
    pub graph: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub title_flagged: bool,
}

impl NodeCell {
    /// The virtual node of a graph in scope (`MATCH (g:graph)`), not a stored node: its title is the graph's.
    pub fn is_graph(&self) -> bool {
        self.kind == "graph"
            && self
                .address
                .strip_prefix("umx:")
                .is_some_and(|alias| alias == self.graph)
    }
}

/// A whole edge in a row: umx's edge shape, with its selectors when set.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeCell {
    pub id: Id,
    pub address: String,
    pub graph: String,
    pub kind: String,
    pub from: Id,
    pub to: Id,
    pub from_selector: Option<Value>,
    pub to_selector: Option<Value>,
}

impl From<umx_query::Cell> for Cell {
    fn from(c: umx_query::Cell) -> Cell {
        use umx_query::Cell as Q;
        match c {
            Q::Null => Cell::Null,
            Q::Bool(b) => Cell::Bool(b),
            Q::Int(i) => Cell::Int(i),
            Q::Float(f) => Cell::Float(f),
            Q::Str(s) => Cell::Str(s),
            Q::List(items) => Cell::List(items.into_iter().map(Cell::from).collect()),
            Q::Map(entries) => Cell::Map(entries.into_iter().map(|(k, v)| (k, v.into())).collect()),
            Q::Node(n) => Cell::Node(NodeCell {
                id: n.id,
                address: n.address,
                graph: n.graph,
                kind: n.kind,
                title: n.title,
                status: n.status,
                title_flagged: false,
            }),
            Q::Edge(e) => Cell::Edge(EdgeCell {
                id: e.id,
                address: e.address,
                graph: e.graph,
                kind: e.kind,
                from: e.from,
                to: e.to,
                from_selector: e.from_selector,
                to_selector: e.to_selector,
            }),
            Q::Similar(e) => Cell::Similar(SimilarCell {
                graph: e.graph,
                from: e.from,
                to: e.to,
                score: e.score,
            }),
            Q::Provenance(e) => Cell::Provenance(ProvenanceCell {
                graph: e.graph,
                kind: e.kind,
                from: e.from,
                to: e.to,
            }),
        }
    }
}

/// How many queries may run at once in one daemon.
pub const QUERY_PERMITS: usize = 2;

/// The wall-clock backstop of one query (cypher-expert's section 4).
pub const QUERY_BACKSTOP_MS: u64 = 10_000;

/// `umx query '<cypher>' [--param name=value ...] [--limit N] [--offset N]`.
#[derive(Clone, Debug, PartialEq)]
pub struct CypherRequest {
    pub text: String,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// `$name` values in the order given, as `--param` (raw, typed as `--set` types it), `--param-str` and
    /// `--param-json` give them.
    pub params: Vec<(String, SetValue)>,
    /// `--along` (D71 step 12): the query sees only the nodes the spine places in range, and the edges between
    /// them (the induced subgraph). One graph only.
    pub along: Option<crate::AlongRequest>,
}

/// A query's rows in the 007 list shape: the columns in RETURN order, the rows of the window asked for, how many
/// rows the query made (`total`), and whether rows past the window were cut.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    /// F241: for each row, `field:<column>` for each column whose cell holds a value umx's secret check flags, in
    /// column order (see [`row_flags`]); empty for a row with none.
    pub flags: Vec<Vec<String>>,
    pub total: usize,
    pub truncated: bool,
    /// Work units used (deterministic: the same query on the same graphs costs the same).
    pub work: u64,
    /// Present when the query names similar_to: each graph in scope that holds similar_to lists, in scope order.
    pub similarity: Option<Vec<QuerySimilarity>>,
    /// `--graph` named copies of one graph (d-21): whole nodes and edges then say their copy.
    pub copies: bool,
    /// The spine range the query saw (`--along`), with what it left out of the graph (cypher-expert C3).
    pub along: Option<crate::AlongInfo>,
}

/// The permits of the query gate: a count, taken by compare-exchange and given back by the guard's Drop, so a
/// panic or an early return frees it.
pub(crate) struct QueryPermits {
    held: AtomicUsize,
    max: usize,
}

impl QueryPermits {
    pub(crate) fn new(max: usize) -> QueryPermits {
        QueryPermits {
            held: AtomicUsize::new(0),
            max,
        }
    }

    pub(crate) fn try_acquire(&self) -> Option<QueryPermit<'_>> {
        let mut held = self.held.load(Ordering::Acquire);
        loop {
            if held >= self.max {
                return None;
            }
            match self.held.compare_exchange_weak(
                held,
                held + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(QueryPermit(self)),
                Err(now) => held = now,
            }
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }
}

/// One running query's permit; dropping it gives the permit back.
pub struct QueryPermit<'a>(&'a QueryPermits);

impl Drop for QueryPermit<'_> {
    fn drop(&mut self) {
        self.0.held.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The `$name` values: each name snake_case and given once; `--param` typed as `--set` types a value, `--param-str`
/// always a string, `--param-json` a scalar or a flat list of scalars (workshop 007 rule 5). A nested list or a
/// map is refused: a parameter becomes part of the query, and its nesting must stay under the parser's cap.
fn params(given: &[(String, SetValue)]) -> Result<BTreeMap<String, Param>, Problem> {
    fn scalar(v: Value) -> Option<Param> {
        Some(match v {
            Value::Null => Param::Null,
            Value::Bool(b) => Param::Bool(b),
            Value::Number(n) => match n.as_i64() {
                Some(i) => Param::Int(i),
                None => Param::Float(n.as_f64().unwrap_or(f64::NAN)),
            },
            Value::String(s) => Param::Str(s),
            Value::Array(_) | Value::Object(_) => return None,
        })
    }
    let mut out = BTreeMap::new();
    for (name, value) in given {
        if !is_snake_case(name) {
            return Err(problems::usage_invalid(
                "a --param name is snake_case",
                "query",
            ));
        }
        let (value, _) = typed_value(value, None);
        let param = match value {
            Value::Array(items) => items
                .into_iter()
                .map(scalar)
                .collect::<Option<Vec<_>>>()
                .map(Param::List),
            other => scalar(other),
        }
        .ok_or_else(|| {
            problems::usage_invalid(
                "a --param-json value is a scalar or a flat list of scalars",
                "query",
            )
        })?;
        if out.insert(name.clone(), param).is_some() {
            return Err(problems::usage_invalid(
                "--param names a parameter twice",
                "query",
            ));
        }
    }
    Ok(out)
}

/// Runs one query (see the module doc). The caller holds the shared read lock.
pub(crate) fn query(
    store: &GraphStore,
    permits: &QueryPermits,
    ctx: &Ctx,
    req: &CypherRequest,
) -> Outcome<QueryRows> {
    let params = params(&req.params)?;
    if req.along.is_some() {
        crate::graph_store::along_one_graph(ctx)?;
        // R407-1: a refusal the text alone makes comes before the range is built (plan 216: before any graph work).
        umx_query::preflight(&req.text)?;
    }
    let scope = store.graphs_in_scope(ctx, None, "query")?;
    if req.along.is_some() && scope.aliases.len() != 1 {
        return Err(problems::usage_invalid(
            "--along reads one graph; name it with --graph",
            "time",
        ));
    }
    let include_named = matches!(
        ctx.scope,
        Some(crate::Scope::All {
            include_named: true
        })
    );
    let Some(_permit) = permits.try_acquire() else {
        return Err(problems::query_busy());
    };
    let clock = store.clock();
    // One read of the clock: the query's now (current_date(), current_timestamp() on every row and pass) and the
    // start of its backstop.
    let now_ms = clock.now_ms();
    let deadline = now_ms.saturating_add(QUERY_BACKSTOP_MS);
    let cancel = || clock.now_ms() >= deadline;
    let options = RunOptions {
        limit: req.limit.unwrap_or(umx_query::DEFAULT_LIMIT),
        offset: req.offset.unwrap_or(0),
        params,
        cancel: Some(&cancel),
        now_ms: Some(now_ms),
        ..RunOptions::default()
    };
    let mut aliases = scope.aliases;
    let mut skipped = scope.skipped;
    let copies = !scope.twice.is_empty();
    // --along: the range is built once, in services, outside the query's work units (cypher-expert C4). It, its reply
    // object, its hints and its refusals are the ranged graph's, so each is given only while that graph is still
    // read at finish (reviewer B2): a refusal is returned only if the graph is still readable now, else the graph
    // leaves the query as a graph secured mid-run does, and a range whose graph a later pass dropped is not replied.
    let ranged = req.along.as_ref().map(|_| aliases[0].clone());
    let mut kept = match (&req.along, &ranged) {
        (Some(a), Some(alias)) => match store.along(ctx, alias, a) {
            Ok(k) => Some(k),
            Err(p) => {
                let readable = if include_named {
                    store
                        .graphs_in_scope(ctx, None, "query")?
                        .aliases
                        .contains(alias)
                } else {
                    scope.named || store.shown_unnamed(alias)
                };
                if readable {
                    return Err(p);
                }
                aliases.retain(|a| a != alias);
                if !include_named {
                    skipped += 1;
                }
                None
            }
        },
        _ => None,
    };
    for pass in 0..2 {
        let mut parts = Vec::with_capacity(aliases.len());
        for alias in &aliases {
            let l = store.loaded(alias).ok_or_else(|| store.not_loaded(alias))?;
            parts.push(Part {
                alias: alias.as_str(),
                graph: &l.graph,
                symmetric: l
                    .meta
                    .ontology
                    .symmetric_kinds()
                    .into_iter()
                    .map(ToString::to_string)
                    .collect(),
            });
        }
        let targets = aliases
            .iter()
            .map(|alias| {
                let loaded = store.loaded(alias).expect("selected");
                umx_query::GraphTarget {
                    id: loaded.meta.id,
                    title: &loaded.meta.title,
                }
            })
            .collect();
        let crossings = Crossings {
            space: ReadSpace::from_aliases(store, ctx, aliases.clone(), scope.named),
            skipped: core::cell::RefCell::new(alloc::collections::BTreeSet::new()),
            unresolved_work: core::cell::Cell::new(0),
        };
        let view = GraphView::new(parts)
            .with_provenance(ctx.include_provenance)
            .with_external(
                targets,
                &crossings,
                aliases.iter().any(|alias| store.has_external_edges(alias)),
            );
        let view = match (&kept, aliases.first()) {
            (Some(k), Some(alias)) => {
                let l = store.loaded(alias).expect("selected");
                view.with_mask(0, k.set(&l.graph))
            }
            _ => view,
        };
        let rows = umx_query::run(&view, &req.text, &options)?;
        let similar = rows
            .similar_missing
            .as_deref()
            .map(|missing| similar_to_read(store, &aliases, missing));
        // d-20 at finish: a graph secured (or unreadable) while the query ran leaves an unnamed query's answer.
        // Each pass drops at least one graph, so this ends.
        if include_named {
            let readable = store.graphs_in_scope(ctx, None, "query")?;
            let before = aliases.len();
            aliases.retain(|a| readable.aliases.contains(a));
            if aliases.len() < before {
                continue;
            }
        } else if !scope.named {
            let before = aliases.len();
            aliases.retain(|a| store.shown_unnamed(a));
            if aliases.len() < before {
                if pass == 1 {
                    return Err(problems::usage_invalid(
                        "graph policy changed during the read (retry)",
                        "query",
                    ));
                }
                skipped += before - aliases.len();
                continue;
            }
        }
        // d-21: the other copies are named only while still open; one secured meanwhile is counted with the secured
        // graphs, never named (reviewer R132-2).
        let others: Vec<String> = scope
            .copies_skipped
            .iter()
            .filter(|c| store.shown_unnamed(c))
            .cloned()
            .collect();
        let mut warnings = Vec::new();
        let secured = skipped + (scope.copies_skipped.len() - others.len());
        if secured > 0 {
            warnings.push(problems::secured_graphs_skipped(secured));
        }
        if !others.is_empty() {
            warnings.push(problems::copies_skipped(&others));
        }
        if include_named {
            warnings.extend(store.named_graphs_read(&aliases));
        }
        if copies {
            warnings.push(problems::graph_twice(&scope.twice));
        }
        if let Some(saved) = &scope.saved {
            warnings.extend(store.saved_warnings(saved, &aliases)?);
        }
        warnings.extend(store.stale_notes(&aliases));
        warnings.extend(
            rows.warnings
                .into_iter()
                .map(|warning| declared_vocabulary_warning(warning, store, &aliases, ctx)),
        );
        if !crossings.skipped.borrow().is_empty() {
            warnings.push(problems::external_edges_skipped(
                crossings.skipped.borrow().len(),
            ));
        }
        let similarity = similar.map(|(lists, incomplete)| {
            warnings.extend(incomplete);
            lists
        });
        let along = kept
            .take()
            .filter(|_| aliases.first() == ranged.as_ref())
            .map(|mut k| {
                if let Some(alias) = aliases.first() {
                    k.count_graph(&store.loaded(alias).expect("selected").graph);
                }
                k.finish(GRAPH_WIDE, GRAPH_WIDE, &mut warnings)
            });
        let mut cells: Vec<Vec<Cell>> = rows
            .rows
            .into_iter()
            .map(|r| r.into_iter().map(Cell::from).collect())
            .collect();
        let flags = row_flags(store, &rows.columns, &mut cells);
        return Ok(Done {
            value: QueryRows {
                columns: rows.columns,
                rows: cells,
                flags,
                total: rows.total,
                truncated: rows.truncated,
                work: rows.work,
                similarity,
                copies,
                along,
            },
            notes: warnings,
        });
    }
    unreachable!("a stable pass returns and a second policy change refuses")
}

/// F241 (plan 216): each row's flagged columns, named `field:<column>`, and each node cell's own flag. A row's flags
/// are the authority (security): they come from the values umx made, never from a key the query built, so a map that
/// copies a node cell's shape is judged by its strings. A column is flagged when anything inside its cell, at any
/// depth, is: a node's title by the read rule (`title_flagged`, its graph's allows), a string the check flags
/// (`field_suspect(<column>, text)`: a list's items, a map's keys and values), an edge's text-quote selectors, and
/// the graph node's title (graph titles have no allows). One allows read per query at most, at the first suspect
/// title.
fn row_flags(store: &GraphStore, columns: &[String], rows: &mut [Vec<Cell>]) -> Vec<Vec<String>> {
    let allows = core::cell::OnceCell::new();
    rows.iter_mut()
        .map(|row| {
            columns
                .iter()
                .zip(row.iter_mut())
                .filter_map(|(column, cell)| {
                    flag_cell(store, &allows, column, cell)
                        .then(|| umx_core::place::named_place(column))
                })
                .collect()
        })
        .collect()
}

/// Whether `cell` holds a flagged value; marks each stored node cell inside it whose title is flagged. Visits every
/// part, so a nested node's own flag is set even once the column is known to be flagged.
fn flag_cell(
    store: &GraphStore,
    allows: &core::cell::OnceCell<umx_core::secret_allows::SecretAllows>,
    column: &str,
    cell: &mut Cell,
) -> bool {
    let text = |s: &str| umx_core::secrets::field_suspect(column, s).is_some();
    match cell {
        Cell::Null | Cell::Bool(_) | Cell::Int(_) | Cell::Float(_) => false,
        Cell::Similar(_) | Cell::Provenance(_) => false,
        Cell::Str(s) => text(s),
        Cell::List(items) => items
            .iter_mut()
            .fold(false, |any, c| flag_cell(store, allows, column, c) | any),
        Cell::Map(entries) => entries.iter_mut().fold(false, |any, (k, v)| {
            let key = text(k);
            flag_cell(store, allows, column, v) | key | any
        }),
        Cell::Node(n) if n.is_graph() => text(&n.title),
        Cell::Node(n) => {
            n.title_flagged = match store.loaded(&n.graph) {
                Some(l) => store.title_flagged(allows, l.meta.id, n.id, &n.title),
                // Security N1 on #407: a graph not loaded now has no allows to read, so the title rule's scan alone
                // decides and the cell fails closed.
                None => umx_core::secrets::field_suspect("title", &n.title).is_some(),
            };
            n.title_flagged
        }
        Cell::Edge(e) => [&e.from_selector, &e.to_selector]
            .into_iter()
            .flatten()
            .any(|v| json_flagged(v, &text)),
    }
}

/// Whether any string in `v`, a key or a value at any depth, is one `text` flags.
fn json_flagged(v: &Value, text: &impl Fn(&str) -> bool) -> bool {
    match v {
        Value::String(s) => text(s),
        Value::Array(items) => items.iter().any(|x| json_flagged(x, text)),
        Value::Object(map) => map.iter().any(|(k, x)| text(k) || json_flagged(x, text)),
        _ => false,
    }
}

fn declared_vocabulary_warning(
    mut warning: Problem,
    store: &GraphStore,
    aliases: &[String],
    ctx: &Ctx,
) -> Problem {
    use umx_query::check;
    let unused = match warning.code {
        check::UNKNOWN_LABEL => check::LABEL_UNUSED,
        check::UNKNOWN_RELATION => check::RELATION_UNUSED,
        check::UNKNOWN_PROPERTY => check::PROPERTY_UNUSED,
        _ => return warning,
    };
    let Some((_, Slot::Name(name))) = warning.slots.iter().find(|(key, _)| *key == "name") else {
        return warning;
    };
    let selected = || {
        aliases
            .iter()
            .map(|alias| store.loaded(alias).expect("selected"))
    };
    // Virtual endpoints can lack a property that stored nodes use. That warning does not mean unused.
    if warning.code == check::UNKNOWN_PROPERTY
        && selected().any(|l| {
            ctx.read_scope(&l.graph)
                .nodes()
                .any(|n| n.custom_field_names().any(|f| f == name.as_str()))
                || ctx
                    .read_scope(&l.graph)
                    .edges()
                    .any(|e| e.field(name.as_str()).is_some())
        })
    {
        return warning;
    }
    if selected().any(|l| {
        let o = &l.meta.ontology;
        match warning.code {
            check::UNKNOWN_LABEL => o.type_decl(name.as_str()).is_some(),
            check::UNKNOWN_RELATION => o.has_relation(name.as_str()),
            check::UNKNOWN_PROPERTY => {
                o.types
                    .iter()
                    .any(|t| t.fields.iter().any(|f| f.name == name.as_str()))
                    || o.relation_fields
                        .values()
                        .any(|fields| fields.iter().any(|f| f.name == name.as_str()))
            }
            _ => unreachable!("only vocabulary warnings"),
        }
    }) {
        warning.code = unused;
    }
    warning
}

/// For a query that named similar_to: the lists each graph in scope holds (model, k, floor), and
/// `similar_to_incomplete` for each graph whose lists are stale (being rebuilt, or a list lost a node) or where a
/// similar_to step read from or reached a node with no whole-node vector (`missing`, per graph in `aliases` order).
fn similar_to_read(
    store: &GraphStore,
    aliases: &[String],
    missing: &[bool],
) -> (Vec<QuerySimilarity>, Vec<Problem>) {
    let (mut lists, mut warnings) = (Vec::new(), Vec::new());
    for (alias, &missing) in aliases.iter().zip(missing) {
        let Some(l) = store.loaded(alias) else {
            continue;
        };
        let graph = &l.graph;
        if let Some(key) = graph.similar_space().and_then(|s| ModelKey::parse(s).ok()) {
            let (k, floor) = graph.similar_settings();
            let raw = l
                .meta
                .embedding
                .as_ref()
                .and_then(|e| e.similarity.as_ref());
            let (settings, _) = umx_core::similar::Settings::of(&key, raw, store.floors());
            lists.push(QuerySimilarity {
                graph: alias.clone(),
                key,
                k,
                floor,
                // Lists made with another floor are stale and being rebuilt; their floor is not the calibrated one.
                floor_calibrated: settings.calibrated() && settings.floor == floor,
                floor_source: settings.source,
                floor_run: settings.run,
            });
        }
        let stale = store.similar_stale(alias) || graph.similar_stale().next().is_some();
        if stale || missing {
            warnings.push(problems::similar_to_incomplete(
                &crate::graph_store::alias_name(alias),
            ));
        }
    }
    (lists, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use umx_core::ports::GraphDir;
    use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};

    /// A store with no graph loaded.
    fn empty_store() -> GraphStore {
        let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
        GraphStore::new(crate::Ports {
            files: files.clone(),
            vector_cache: files,
            vector_cache_root: GraphDir::parse("/work/cache").unwrap(),
            clock: Arc::new(FakeClock::default()),
            ids: Arc::new(FakeIds::new()),
            state: Arc::new(FakeLocalState::new()),
            allows: Arc::new(FakeLocalState::new()),
            lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
            scopes: Arc::new(FakeLocalState::new()),
            registry: Arc::new(FakeLocalState::new()),
            sources: Arc::new(umx_fakes::FakeSourceReader::open(
                GraphDir::parse("/").unwrap(),
            )),
        })
    }

    fn node(graph: &str, title: &str) -> Cell {
        Cell::Node(NodeCell {
            id: Id(7),
            address: alloc::format!("umx:{graph}/{}", Id(7).to_text()),
            graph: graph.into(),
            kind: "problem".into(),
            title: title.into(),
            status: "open".into(),
            title_flagged: false,
        })
    }

    /// Security N1 on #407: a node cell whose graph is not loaded when the flags are made fails closed, by the title
    /// rule's scan alone (no allows to read); a plain title stays unflagged.
    #[test]
    fn a_node_cell_of_a_graph_not_loaded_is_judged_by_its_title_alone() {
        let store = empty_store();
        let columns = ["n".to_string(), "p".to_string()];
        let token = alloc::format!("AKIA{}", "Q7Z3X9K2Q7Z3GONE");
        let mut rows = alloc::vec![alloc::vec![
            node("gone", &alloc::format!("Cache {token}")),
            node("gone", "Plain title"),
        ]];
        assert_eq!(row_flags(&store, &columns, &mut rows), [["field:n"]]);
        let flagged = |c: &Cell| matches!(c, Cell::Node(n) if n.title_flagged);
        assert!(flagged(&rows[0][0]));
        assert!(!flagged(&rows[0][1]));
    }
}
