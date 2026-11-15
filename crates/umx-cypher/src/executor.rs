use alloc::collections::{BTreeMap, BTreeSet};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::cell::{Cell, RefCell};
use core::cmp::Ordering;
use core::ops::ControlFlow;

use crate::ast::{
    AggFn, Clause, CmpOp, Direction, Expr, Literal, MatchClause, NodePattern, OrderItem,
    PathPattern, Projection, Query, ReturnItem, UnwindClause, WithClause,
};
use crate::error::{CypherError, Limit};
use crate::provider::{GraphProvider, PropKey};
use crate::value::CypherValue;

/// The tabular result of executing a query.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CypherValue>>,
    /// Things the query ran through that the caller should hear about, once each.
    pub warnings: Vec<Warning>,
    /// Work units the query used (see [`Limits`]).
    pub work: u64,
}

/// A warning: the query ran, but its rows may not mean what the reader expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// An untyped directed step (`(a)-->(b)`) crossed edges of these symmetric relations, which appear in
    /// their stored direction only (D49 R2). Name the relation and match it undirected instead.
    SymmetricInDirectedStep { relations: Vec<String> },
}

/// A bound variable: a graph node or relationship (which keep their identity for pattern chaining),
/// or a scalar/computed value (produced by a `WITH` projection).
enum Binding<G: GraphProvider> {
    Node(G::NodeId),
    Edge(G::EdgeId),
    /// Shared, so cloning a row is O(1) however large a list it binds (security B1).
    Value(Rc<CypherValue>),
}

impl<G: GraphProvider> Clone for Binding<G> {
    fn clone(&self) -> Self {
        match self {
            Binding::Node(n) => Binding::Node(*n),
            Binding::Edge(e) => Binding::Edge(*e),
            Binding::Value(v) => Binding::Value(v.clone()),
        }
    }
}

/// A single binding row: maps variable names to their bindings.
///
/// Backed by a small `Vec` rather than a `BTreeMap`: rows hold only a handful of variables, so this
/// is cheaper to allocate and clone (which matters when a scan materializes one row per matched
/// node) and faster to look up at these sizes.
struct Row<G: GraphProvider> {
    entries: Vec<(String, Binding<G>)>,
}

impl<G: GraphProvider> Clone for Row<G> {
    fn clone(&self) -> Self {
        Row {
            entries: self.entries.clone(),
        }
    }
}

impl<G: GraphProvider> Row<G> {
    fn new() -> Self {
        Row {
            entries: Vec::new(),
        }
    }

    fn get(&self, key: &str) -> Option<&Binding<G>> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// Binds `key`, overwriting any existing binding for it (preserving map semantics).
    fn insert(&mut self, key: String, value: Binding<G>) {
        if let Some(slot) = self.entries.iter_mut().find(|(name, _)| *name == key) {
            slot.1 = value;
        } else {
            self.entries.push((key, value));
        }
    }

    fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(name, _)| name)
    }
}

/// A binding row paired with the node most recently matched in the path being expanded.
type Working<G> = Vec<(Row<G>, <G as GraphProvider>::NodeId)>;

/// A projected row: the new bindings carried to the next clause, plus the aligned output values.
struct Projected<G: GraphProvider> {
    bindings: Row<G>,
    values: Vec<CypherValue>,
}

/// One hop's edges, as `(edge, the node at the other end)`.
type Steps<G> = Vec<(<G as GraphProvider>::EdgeId, <G as GraphProvider>::NodeId)>;

/// A step's targets: the edge for a fixed-length step, none for a variable-length one.
type Targets<G> = Vec<(
    Option<<G as GraphProvider>::EdgeId>,
    <G as GraphProvider>::NodeId,
)>;

/// Output column names paired with the binding rows a `WITH` clause carries forward.
type WithOutput<G> = (Vec<String>, Vec<Row<G>>);

/// Output column names paired with the projected rows of a projection.
type ProjectedRows<G> = (Vec<String>, Vec<Projected<G>>);

/// The upper bound taken for a variable-length step written without one (`*`, `*1..`). Walks are enumerated, so an
/// open bound must be capped; LadybugDB caps at 30, the subset at 10 (s03 refuses an open bound instead).
pub const OPEN_UPPER_BOUND: u32 = 10;

/// The default work budget, in work units: one per node scanned, edge step read, binding row
/// made, and per property read its provider cost. Deterministic, so a query stops at the same point
/// on every machine. Set from the s03 bench (100k nodes, rich, release, Apple M-series): the most any
/// accepted query of the design and workshop 006 sets uses is 4.13M units (q5, 466 ms), and the
/// slowest rate is the degree query's, about 4,200 units per ms; so 10M units is at most about
/// 2.4 s, cypher-expert's target of about 2 s, with 2.4x headroom over q5.
pub const DEFAULT_MAX_WORK: u64 = 10_000_000;

/// The default cap on rows held at once: binding rows of a clause, walk frontiers and results,
/// cartesian products, and the rows kept for ORDER BY, DISTINCT and aggregation. It bounds memory:
/// a degree query at 100k nodes peaked at 379 MB of materialised rows in the s02 bench.
pub const DEFAULT_MAX_ROWS: usize = 1_000_000;

/// How deeply a list or map value may nest. Each `WITH [x] AS x` adds a level while every expression stays
/// shallow, so the parser's cap does not bound it; cloning, dropping, comparing and converting a value all
/// recurse once per level, and 5,000 levels aborted a 2 MiB stack (reviewer R115-2). Checked whenever a list
/// or map is built, so every value the executor holds is at most this deep. The same as the parser's cap, so
/// any list literal that parses can be built.
pub const MAX_VALUE_DEPTH: usize = crate::parser::MAX_DEPTH;

/// What one list or map element costs when it is built or copied. A held element is about 100 bytes, so at
/// one unit the budget let `WITH [x, x] AS x` (which doubles a value per clause) hold 432 MB before it
/// stopped; at four the budget allows about 2.5M element copies (reviewer R115-2, security B1).
pub const ELEMENT_COST: u64 = 4;

/// The execution error for a value nested past [`MAX_VALUE_DEPTH`].
pub const VALUE_TOO_DEEP: &str = "a list or map is nested too deep";

/// How often, in work units, the cancel hook is asked.
pub const CANCEL_EVERY: u64 = 1 << 16;

/// What a query may spend before it is stopped.
pub struct Limits<'a> {
    pub max_work: u64,
    pub max_rows: usize,
    /// Asked every [`CANCEL_EVERY`] units; `true` stops the query with [`CypherError::Cancelled`]. The
    /// caller builds it (a wall-clock backstop); the executor reads no clock.
    pub cancel: Option<&'a dyn Fn() -> bool>,
}

impl Default for Limits<'_> {
    fn default() -> Self {
        Limits {
            max_work: DEFAULT_MAX_WORK,
            max_rows: DEFAULT_MAX_ROWS,
            cancel: None,
        }
    }
}

/// Executes a parsed query against any [`GraphProvider`] with the default [`Limits`].
///
/// # Errors
///
/// As [`execute_limited`].
pub fn execute<G: GraphProvider>(graph: &G, query: &Query) -> Result<ResultSet, CypherError> {
    execute_limited(graph, query, &Limits::default())
}

/// Executes a parsed query, charging its work to `limits` as it goes: a query over budget stops
/// mid-walk with [`CypherError::TooLarge`] and returns no rows.
///
/// # Errors
///
/// Returns a [`CypherError::Execution`] for unknown relationship types, aggregates used in `WHERE`,
/// or `ORDER BY` expressions that cannot be resolved against the projection;
/// [`CypherError::TooLarge`] past a limit; [`CypherError::Cancelled`] when the hook says so.
pub fn execute_limited<G: GraphProvider>(
    graph: &G,
    query: &Query,
    limits: &Limits<'_>,
) -> Result<ResultSet, CypherError> {
    // D49 R1: a directed step naming a symmetric relation is refused before anything runs.
    for rel in query.rel_patterns() {
        if rel.direction != Direction::Both
            && let Some(relation) = rel.types.iter().find(|t| graph.is_symmetric(t))
        {
            return Err(CypherError::SymmetricDirection {
                relation: relation.clone(),
            });
        }
    }
    let symmetric_present = graph
        .relationship_types()
        .iter()
        .any(|r| graph.is_symmetric(r));
    let executor = Executor {
        graph,
        limits,
        work: Cell::new(0),
        values_wanted: Cell::new(true),
        symmetric_present,
        crossed_symmetric: RefCell::new(BTreeSet::new()),
        in_hoist: RefCell::new(BTreeMap::new()),
        pushdown: RefCell::new(BTreeMap::new()),
        pushdown_active: Cell::new(false),
        pushdown_exact: Cell::new(true),
        prop_keys: RefCell::new(Vec::new()),
    };
    executor.run(query)
}

struct Executor<'a, G: GraphProvider> {
    graph: &'a G,
    limits: &'a Limits<'a>,
    /// Work units charged so far.
    work: Cell<u64>,
    /// Whether the projection being made needs display values (see `shown`).
    values_wanted: Cell<bool>,
    /// Whether any relationship type of the graph is symmetric; when not, the D49 R2 check is skipped.
    symmetric_present: bool,
    /// The symmetric relations an untyped directed step crossed (D49 R2), for the warning.
    crossed_symmetric: RefCell<BTreeSet<String>>,
    /// Per-filter-pass memo of loop-invariant `IN` right-hand-side lists, keyed by the RHS
    /// expression's address, so `x IN <constant list>` is an O(log n) `BTreeSet` lookup instead of an
    /// O(list) scan re-evaluated per row. Populated for the duration of a single `filter_rows` pass.
    in_hoist: RefCell<BTreeMap<usize, Rc<HoistedIn>>>,
    /// Single-variable WHERE predicates from every non-optional MATCH clause, keyed by the variable
    /// they reference, so a predicate prunes the scan/expansion that first binds its variable — even
    /// when written on a later clause. Built once per query in `run`; the full WHERE is still applied
    /// afterwards, except the conjuncts the pushdown already proved (see `run_match`).
    pushdown: RefCell<BTreeMap<String, Vec<Expr>>>,
    /// Gates pushdown application to non-optional MATCH pattern evaluation only. It must stay off
    /// during `OPTIONAL MATCH` expansion and `EXISTS` subqueries: there, pruning a candidate that
    /// fails a single-variable predicate can change results (e.g. an `OPTIONAL MATCH ... WHERE t IS
    /// NULL` anti-join would null-fill every row and wrongly keep them all).
    pushdown_active: Cell<bool>,
    /// Set at each `run_match`, cleared when a pushed predicate's evaluation ends in an ordinary
    /// error (the node is kept so the full WHERE raises it): while set, every binding the MATCH made
    /// passed each of its pushed predicates as truthy.
    pushdown_exact: Cell<bool>,
    /// Each node property name read so far, with the key [`GraphProvider::property_key`] gave it: resolved once per
    /// query, then found by a linear scan (a query reads few names).
    prop_keys: RefCell<Vec<(String, Option<PropKey>)>>,
}

/// A hoisted `IN` list: its members' keys, and the members that are strings the provider reads as more than text
/// (dates), which `IN` matches through [`GraphProvider::compare_text`] as `=` does.
struct HoistedIn {
    set: BTreeSet<CypherValue>,
    texts: Vec<String>,
}

/// A value as `ORDER BY`, `min` and `max` order it: a string by the provider's [`GraphProvider::text_sort_key`] when
/// it gives one (umx: a date written other than canonically sorts at its instant, ws008 TM2), else itself, then by
/// itself; every other value by [`CypherValue::total_cmp`]. Made once per value sorted.
struct SortKey {
    value: CypherValue,
    text: Option<String>,
}

impl SortKey {
    fn cmp(&self, other: &SortKey) -> Ordering {
        match (&self.value, &other.value) {
            (CypherValue::Str(a), CypherValue::Str(b)) => self
                .text
                .as_deref()
                .unwrap_or(a)
                .cmp(other.text.as_deref().unwrap_or(b))
                .then_with(|| a.cmp(b)),
            _ => self.value.total_cmp(&other.value),
        }
    }
}

impl<G: GraphProvider> Executor<'_, G> {
    fn sort_key(&self, value: CypherValue) -> SortKey {
        let text = value.as_str().and_then(|s| self.graph.text_sort_key(s));
        SortKey { value, text }
    }

    /// Charges `units` of work; past the budget the query stops, and every [`CANCEL_EVERY`] units the
    /// cancel hook is asked.
    fn charge(&self, units: u64) -> Result<(), CypherError> {
        let before = self.work.get();
        let used = before.saturating_add(units);
        self.work.set(used);
        if used > self.limits.max_work {
            return Err(CypherError::TooLarge {
                cause: Limit::Work,
                used,
            });
        }
        if let Some(cancel) = self.limits.cancel
            && before / CANCEL_EVERY != used / CANCEL_EVERY
            && cancel()
        {
            return Err(CypherError::Cancelled { used });
        }
        Ok(())
    }

    /// One more row made, `held` rows now held at once: one unit of work, and the row cap.
    fn row_made(&self, held: usize) -> Result<(), CypherError> {
        self.charge(1)?;
        if held > self.limits.max_rows {
            return Err(CypherError::TooLarge {
                cause: Limit::Rows,
                used: self.work.get(),
            });
        }
        Ok(())
    }

    /// Node property `name` resolved once per query ([`GraphProvider::property_key`]): asked the first time the name
    /// is read and kept.
    fn prop_key(&self, name: &str) -> Option<PropKey> {
        let found = self
            .prop_keys
            .borrow()
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, key)| *key);
        found.unwrap_or_else(|| {
            let key = self.graph.property_key(name);
            self.prop_keys.borrow_mut().push((name.into(), key));
            key
        })
    }

    /// A node property, charged at its provider cost (a field decoded from a node's line costs more). A resolved
    /// name ([`Self::prop_key`]) is read by its key.
    fn property(&self, node: G::NodeId, name: &str) -> Result<CypherValue, CypherError> {
        match self.prop_key(name) {
            Some(key) => {
                self.charge(self.graph.key_cost(key))?;
                Ok(self.property_value(node, key))
            }
            None => {
                self.charge(self.graph.property_cost(name))?;
                Ok(self.graph.property(node, name))
            }
        }
    }

    /// The value of a resolved node property. Charges nothing: the caller has charged [`GraphProvider::key_cost`].
    fn property_value(&self, node: G::NodeId, key: PropKey) -> CypherValue {
        self.graph.property_at(node, key)
    }

    /// `expr` as a resolved node property read (`n.prop` with `n` bound to a node), for the reads that can ask
    /// [`GraphProvider::each_str`] instead of building the value. `None` takes the ordinary evaluation.
    fn node_key(&self, row: &Row<G>, expr: &Expr) -> Option<(G::NodeId, PropKey)> {
        let Expr::Property(var, name) = expr else {
            return None;
        };
        let Some(Binding::Node(node)) = row.get(var) else {
            return None;
        };
        Some((*node, self.prop_key(name)?))
    }

    /// `left IN n.prop`, `n.prop` resolved to `key` (ws005 s15): the provider's list answers without being built.
    /// Charged as the right side's read is, once. Every left value asks `each_str`, so a property it does not hold
    /// is read as before (a virtual node's unknown name is still noted). Kept out of `eval_expr` so that function's
    /// other arms build as before (the s15 ruler's custom-read row).
    #[inline(never)]
    fn in_node_list(
        &self,
        left: &CypherValue,
        node: G::NodeId,
        key: PropKey,
    ) -> Result<bool, CypherError> {
        self.charge(self.graph.key_cost(key))?;
        let mut found = false;
        let answered = self.graph.each_str(node, key, &mut |item| match left {
            CypherValue::Str(l) if str_equal(self.graph, l, item) => {
                found = true;
                ControlFlow::Break(())
            }
            CypherValue::Str(_) => ControlFlow::Continue(()),
            // Null is never in a list, and no other value equals a string.
            _ => ControlFlow::Break(()),
        });
        if !answered {
            found = compare_values(self.graph, left, CmpOp::In, &self.property_value(node, key));
        }
        Ok(found)
    }

    /// `size(n.prop)`, `n.prop` resolved to `key` (ws005 s15), charged once: the length of the provider's list,
    /// counted without building it, or `None` when the provider holds no list there (the caller reads the value).
    #[inline(never)]
    fn node_list_len(&self, node: G::NodeId, key: PropKey) -> Result<Option<i64>, CypherError> {
        self.charge(self.graph.key_cost(key))?;
        let mut count = 0usize;
        let answered = self.graph.each_str(node, key, &mut |_| {
            count += 1;
            ControlFlow::Continue(())
        });
        Ok(answered.then(|| i64::try_from(count).unwrap_or(i64::MAX)))
    }

    fn run(&self, query: &Query) -> Result<ResultSet, CypherError> {
        let mut rows: Vec<Row<G>> = vec![Row::new()];
        let mut scope: Vec<String> = Vec::new();

        // Build the pushdown map once from every non-optional MATCH's WHERE, so a single-variable
        // predicate prunes the scan/expansion that first binds its variable even when the WHERE is
        // written on a later clause. Without this, `MATCH (t)-[:R]->(c) MATCH (c)-..-(x) WHERE
        // t.name = ...` materializes the entire `(t)-[:R]->(c)` relation before the `t` filter runs.
        // The map is built afresh for each MATCH from the clauses at and after it (see `build_global_pushdown`), so a
        // predicate only ever prunes the binding its own name refers to.
        for (i, clause) in query.clauses.iter().enumerate() {
            if matches!(clause, Clause::Match(m) if !m.optional) {
                *self.pushdown.borrow_mut() = build_global_pushdown(&query.clauses[i..]);
            }
            match clause {
                Clause::Match(m) if m.optional => {
                    rows = self.run_optional_match(rows, m, &mut scope)?;
                }
                Clause::Match(m) => rows = self.run_match(rows, m, &mut scope)?,
                Clause::Unwind(u) => {
                    rows = self.run_unwind(rows, u)?;
                    if !scope.contains(&u.var) {
                        scope.push(u.var.clone());
                    }
                }
                Clause::With(w) => {
                    let (columns, rows_after) = self.run_with(&rows, w, &scope)?;
                    rows = rows_after;
                    scope = columns;
                }
            }
        }

        // Terminal RETURN.
        let (columns, projected) = self.project_rows(&query.result, &rows, &scope)?;
        let projected = self.finalize(&query.result, &columns, projected)?;
        let crossed = self.crossed_symmetric.take();
        let warnings = if crossed.is_empty() {
            Vec::new()
        } else {
            vec![Warning::SymmetricInDirectedStep {
                relations: crossed.into_iter().collect(),
            }]
        };
        Ok(ResultSet {
            columns,
            rows: projected.into_iter().map(|p| p.values).collect(),
            warnings,
            work: self.work.get(),
        })
    }

    fn run_match(
        &self,
        mut rows: Vec<Row<G>>,
        clause: &MatchClause,
        scope: &mut Vec<String>,
    ) -> Result<Vec<Row<G>>, CypherError> {
        // Variables that are constant across the whole pass: if the clause's input is a single row
        // (e.g. right after `WITH collect(...) AS used`), every variable it binds has the same value
        // in every row produced below, so `x IN <those vars>` right-hand sides can be hoisted.
        let constant_vars: BTreeSet<String> = if rows.len() == 1 {
            rows[0].keys().cloned().collect()
        } else {
            BTreeSet::new()
        };

        // Apply the (global) pushdown map only while evaluating this non-optional MATCH's patterns:
        // scans prune before expansion. Pushdown must stay off during `OPTIONAL MATCH` expansion and
        // `EXISTS` (see `pushdown_active`), so it is gated rather than always-on.
        let before = scope.len();
        self.pushdown_exact.set(true);
        self.pushdown_active.set(true);
        for pattern in &clause.patterns {
            rows = self.eval_pattern(rows, pattern)?;
            add_pattern_vars(scope, pattern);
        }
        self.pushdown_active.set(false);

        if let Some(predicate) = &clause.where_clause {
            // A conjunct of this WHERE that reads only a variable this MATCH bound was pushed down
            // (`build_global_pushdown` always takes this clause's own WHERE) and evaluated truthy
            // for that variable's binding in every row left, unless a pushed evaluation erred: it
            // is not evaluated again (ws003 s22). The rest runs in the written order.
            let proven = if self.pushdown_exact.get() {
                proven_conjuncts(predicate, &scope[before..])
            } else {
                Vec::new()
            };
            rows = self.filter_rows_hoisted(rows, predicate, &constant_vars, &proven)?;
        }
        Ok(rows)
    }

    /// Whether a candidate node satisfies the single-variable predicates pushed down for its
    /// variable. Best-effort: evaluation errors are ignored here (the full WHERE runs later and
    /// will surface them, so an error clears `pushdown_exact`), so a node is only pruned when a
    /// pushed predicate is definitively false.
    ///
    /// An ordinary evaluation error keeps the node (the full WHERE raises it later), but a stop (the
    /// budget, a cap or the cancel hook) ends the query here: swallowing it would answer a cancelled
    /// query with whatever rows survived (reviewer R115-3).
    fn node_passes_pushdown(
        &self,
        var: &Option<String>,
        node: G::NodeId,
    ) -> Result<bool, CypherError> {
        self.passes_pushdown(var, Binding::Node(node))
    }

    /// The same for an edge bound to a relationship variable (`WHERE s.score > 0.8`): the step keeps only edges
    /// whose pushed predicates are not definitively false, so a scan over many edges never holds the rest.
    fn edge_passes_pushdown(
        &self,
        var: &Option<String>,
        edge: G::EdgeId,
    ) -> Result<bool, CypherError> {
        self.passes_pushdown(var, Binding::Edge(edge))
    }

    fn passes_pushdown(
        &self,
        var: &Option<String>,
        binding: Binding<G>,
    ) -> Result<bool, CypherError> {
        if !self.pushdown_active.get() {
            return Ok(true);
        }
        let Some(name) = var else {
            return Ok(true);
        };
        let preds = match self.pushdown.borrow().get(name) {
            Some(preds) => preds.clone(),
            None => return Ok(true),
        };
        let mut row = Row::new();
        row.insert(name.clone(), binding);
        for predicate in &preds {
            match self.eval_expr(&row, predicate) {
                Ok(value) if !value.is_truthy() => return Ok(false),
                Err(e @ (CypherError::TooLarge { .. } | CypherError::Cancelled { .. })) => {
                    return Err(e);
                }
                Err(_) => self.pushdown_exact.set(false),
                Ok(_) => {}
            }
        }
        Ok(true)
    }

    /// Filters rows by a predicate, first hoisting any loop-invariant `IN` right-hand side (one
    /// whose variables are all constant across the pass) into a `BTreeSet` evaluated once. The
    /// `proven` conjuncts are taken as true without being evaluated.
    fn filter_rows_hoisted(
        &self,
        rows: Vec<Row<G>>,
        predicate: &Expr,
        constant_vars: &BTreeSet<String>,
        proven: &[*const Expr],
    ) -> Result<Vec<Row<G>>, CypherError> {
        if constant_vars.is_empty() || rows.is_empty() {
            return self.filter_rows_proven(rows, predicate, proven);
        }

        // The constant variables have identical values in every row, so evaluate hoistable RHS
        // lists against the first row.
        self.in_hoist.borrow_mut().clear();
        self.build_in_hoist(predicate, constant_vars, &rows[0])?;
        let result = self.filter_rows_proven(rows, predicate, proven);
        self.in_hoist.borrow_mut().clear();
        result
    }

    /// Walks a predicate and, for each `expr IN <list>` whose right-hand side references only
    /// constant variables, evaluates the list once and stores it as a `BTreeSet` keyed by the RHS
    /// expression's address.
    fn build_in_hoist(
        &self,
        expr: &Expr,
        constant_vars: &BTreeSet<String>,
        sample: &Row<G>,
    ) -> Result<(), CypherError> {
        match expr {
            Expr::Compare(left, CmpOp::In, right) => {
                self.build_in_hoist(left, constant_vars, sample)?;
                if references_only(right, constant_vars)
                    && let CypherValue::List(items) = self.eval_expr(sample, right)?
                {
                    let set: BTreeSet<CypherValue> = items.iter().map(value_key).collect();
                    let texts = items
                        .iter()
                        .filter_map(CypherValue::as_str)
                        .filter(|s| self.graph.compare_text(s, s).is_some())
                        .map(String::from)
                        .collect();
                    let key = core::ptr::from_ref::<Expr>(right) as usize;
                    self.in_hoist
                        .borrow_mut()
                        .insert(key, Rc::new(HoistedIn { set, texts }));
                }
            }
            Expr::Not(inner) | Expr::IsNull(inner, _) => {
                self.build_in_hoist(inner, constant_vars, sample)?;
            }
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
                self.build_in_hoist(a, constant_vars, sample)?;
                self.build_in_hoist(b, constant_vars, sample)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Runs `UNWIND <expr> AS var`: for each input row, evaluates the list and emits one output row
    /// per element with `var` bound to it. A null list yields no rows; a non-list is an error.
    fn run_unwind(
        &self,
        rows: Vec<Row<G>>,
        clause: &UnwindClause,
    ) -> Result<Vec<Row<G>>, CypherError> {
        let mut output = Vec::new();
        for row in rows {
            match self.eval_expr(&row, &clause.expr)? {
                CypherValue::List(items) => {
                    for item in items {
                        let mut new_row = row.clone();
                        new_row.insert(clause.var.clone(), Binding::Value(Rc::new(item)));
                        output.push(new_row);
                        self.row_made(output.len())?;
                    }
                }
                CypherValue::Null => {}
                _ => {
                    return Err(CypherError::execution("UNWIND expects a list"));
                }
            }
        }
        Ok(output)
    }

    /// Runs `OPTIONAL MATCH`: a left join. For each input row, the patterns (and the clause's
    /// `WHERE`) are matched independently; if nothing matches, the input row is kept with the
    /// clause's newly introduced variables bound to null.
    fn run_optional_match(
        &self,
        rows: Vec<Row<G>>,
        clause: &MatchClause,
        scope: &mut Vec<String>,
    ) -> Result<Vec<Row<G>>, CypherError> {
        // Variables introduced by this clause that are not already in scope; these are nulled out
        // for input rows that produce no match.
        let mut new_vars: Vec<String> = Vec::new();
        for pattern in &clause.patterns {
            add_pattern_vars(&mut new_vars, pattern);
        }
        new_vars.retain(|var| !scope.contains(var));

        let mut output = Vec::with_capacity(rows.len());
        for row in rows {
            let mut matched = vec![row.clone()];
            for pattern in &clause.patterns {
                matched = self.eval_pattern(matched, pattern)?;
            }
            if let Some(predicate) = &clause.where_clause {
                matched = self.filter_rows(matched, predicate)?;
            }

            if matched.is_empty() {
                let mut null_row = row;
                for var in &new_vars {
                    null_row.insert(var.clone(), Binding::Value(Rc::new(CypherValue::Null)));
                }
                output.push(null_row);
            } else {
                output.extend(matched);
            }
            self.row_made(output.len())?;
        }

        for pattern in &clause.patterns {
            add_pattern_vars(scope, pattern);
        }
        Ok(output)
    }

    fn run_with(
        &self,
        rows: &[Row<G>],
        clause: &WithClause,
        scope: &[String],
    ) -> Result<WithOutput<G>, CypherError> {
        let p = &clause.projection;
        self.values_wanted.set(p.distinct || !p.order_by.is_empty());
        let projected = self.project_rows(p, rows, scope);
        self.values_wanted.set(true);
        let (columns, projected) = projected?;
        let projected = self.finalize(&clause.projection, &columns, projected)?;

        // `WITH ... WHERE` filters the projected bindings.
        let mut new_rows = Vec::with_capacity(projected.len());
        for p in projected {
            if let Some(predicate) = &clause.where_clause
                && !self.eval_expr(&p.bindings, predicate)?.is_truthy()
            {
                continue;
            }
            new_rows.push(p.bindings);
        }
        Ok((columns, new_rows))
    }

    fn filter_rows(&self, rows: Vec<Row<G>>, predicate: &Expr) -> Result<Vec<Row<G>>, CypherError> {
        self.filter_rows_proven(rows, predicate, &[])
    }

    fn filter_rows_proven(
        &self,
        rows: Vec<Row<G>>,
        predicate: &Expr,
        proven: &[*const Expr],
    ) -> Result<Vec<Row<G>>, CypherError> {
        let mut filtered = Vec::with_capacity(rows.len());
        for row in rows {
            if self.holds(&row, predicate, proven)? {
                filtered.push(row);
            }
        }
        Ok(filtered)
    }

    /// Whether `expr` is truthy for `row`, taking each `proven` conjunct as true. Walks the AND
    /// chain as `eval_expr` does (left, then right only when left is truthy), so the conjuncts left
    /// run in the same order with the same errors; with nothing proven it is `eval_expr` itself.
    fn holds(
        &self,
        row: &Row<G>,
        expr: &Expr,
        proven: &[*const Expr],
    ) -> Result<bool, CypherError> {
        if proven.is_empty() {
            return Ok(self.eval_expr(row, expr)?.is_truthy());
        }
        if proven.contains(&core::ptr::from_ref(expr)) {
            return Ok(true);
        }
        match expr {
            Expr::And(a, b) => Ok(self.holds(row, a, proven)? && self.holds(row, b, proven)?),
            _ => Ok(self.eval_expr(row, expr)?.is_truthy()),
        }
    }

    // ---- Pattern matching ------------------------------------------------

    fn eval_pattern(
        &self,
        base: Vec<Row<G>>,
        pattern: &PathPattern,
    ) -> Result<Vec<Row<G>>, CypherError> {
        // Plan the traversal so it starts from an already-bound endpoint when possible: a
        // forward-written `(x)-[:R]->(d)` with `d` bound is executed as `(d)<-[:R]-(x)`, walking
        // through its incoming edges instead of scanning every node as `x`. Reversal yields the
        // identical set of variable bindings; it only changes traversal direction/cost.
        // A map entry that reads a variable this pattern binds (`(a)-->(b {id: a.id})`) cannot be
        // known while the path is matched, so it becomes a filter on the finished path; only what is
        // bound before the pattern starts (literals, earlier clauses, UNWIND) prunes and pushes down.
        let (pattern, deferred) = split_props(pattern, base.first());
        let planned = plan_pattern(&pattern, base.first(), |node: &NodePattern| {
            node.props.iter().any(|(key, _)| self.graph.indexed(key))
        });
        let pattern = &planned;

        let mut working: Working<G> = Vec::new();

        for row in base {
            for node in self.candidates_for_node(&row, &pattern.start, !pattern.rest.is_empty())? {
                let mut new_row = row.clone();
                if let Some(var) = &pattern.start.var {
                    new_row.insert(var.clone(), Binding::Node(node));
                }
                working.push((new_row, node));
                self.row_made(working.len())?;
            }
        }

        for (rel, node) in &pattern.rest {
            working = self.expand_step(working, rel, node)?;
        }

        let mut rows = Vec::with_capacity(working.len());
        'rows: for (mut row, _) in working {
            for (var, key, expr) in &deferred {
                let node = match row.get(var) {
                    Some(Binding::Node(node)) => *node,
                    _ => continue 'rows,
                };
                let want = self.eval_expr(&row, expr)?;
                if !compare_values(self.graph, &self.property(node, key)?, CmpOp::Eq, &want) {
                    continue 'rows;
                }
            }
            row.entries
                .retain(|(name, _)| !name.starts_with(HIDDEN_VAR));
            rows.push(row);
        }
        Ok(rows)
    }

    fn candidates_for_node(
        &self,
        row: &Row<G>,
        pattern: &NodePattern,
        step_start: bool,
    ) -> Result<Vec<G::NodeId>, CypherError> {
        let wanted = self.pattern_props(row, pattern)?;
        if let Some(var) = &pattern.var
            && let Some(binding) = row.get(var)
        {
            return Ok(match binding {
                Binding::Node(existing) if self.node_matches(*existing, pattern, &wanted)? => {
                    vec![*existing]
                }
                _ => Vec::new(),
            });
        }

        let candidates: Vec<G::NodeId> = match self.indexed_candidates(pattern, &wanted, step_start)
        {
            Some(nodes) => nodes
                .into_iter()
                .filter(|node| self.matches_labels(*node, &pattern.labels))
                .collect(),
            None if step_start => self.graph.scan_step_start(&pattern.labels),
            None => self.graph.scan(&pattern.labels),
        };
        // Every candidate read is work, whether it passes or not.
        self.charge((candidates.len() as u64).saturating_add(self.graph.take_expansion_work()))?;
        let mut kept = Vec::new();
        for node in candidates {
            if self.props_match(node, &wanted)? && self.node_passes_pushdown(&pattern.var, node)? {
                kept.push(node);
            }
        }
        Ok(kept)
    }

    /// A node pattern's property map, evaluated once against the row (`{id: x}` reads `x`; F1).
    fn pattern_props(
        &self,
        row: &Row<G>,
        pattern: &NodePattern,
    ) -> Result<Vec<(String, CypherValue)>, CypherError> {
        pattern
            .props
            .iter()
            .map(|(key, expr)| Ok((key.clone(), self.eval_expr(row, expr)?)))
            .collect()
    }

    /// Candidates from the provider's index instead of a scan: an indexed property in the node's map
    /// (`{id: 'x'}`), or a pushed-down `WHERE v.prop = <literal>` on an indexed property. `None`: scan.
    fn indexed_candidates(
        &self,
        pattern: &NodePattern,
        wanted: &[(String, CypherValue)],
        step_start: bool,
    ) -> Option<Vec<G::NodeId>> {
        let lookup = |prop: &str, value: &CypherValue| {
            if step_start {
                self.graph.lookup_step_start(prop, value)
            } else {
                self.graph.lookup(prop, value)
            }
        };
        for (key, value) in wanted {
            if self.graph.indexed(key)
                && let Some(nodes) = lookup(key, value)
            {
                return Some(nodes);
            }
        }
        if !self.pushdown_active.get() {
            return None;
        }
        let var = pattern.var.as_ref()?;
        let preds = self.pushdown.borrow().get(var).cloned()?;
        preds.iter().find_map(|pred| {
            let (prop, literal) = indexed_equality(pred, var)?;
            if !self.graph.indexed(prop) {
                return None;
            }
            lookup(prop, &literal_to_value(literal))
        })
    }

    fn expand_step(
        &self,
        working: Working<G>,
        rel: &crate::ast::RelPattern,
        node: &NodePattern,
    ) -> Result<Working<G>, CypherError> {
        let rel_types = self.resolve_rel_types(rel)?;
        if rel.var.is_some() && rel.length.is_some() {
            return Err(CypherError::execution(
                "a relationship variable on a variable-length step is not supported",
            ));
        }
        let mut next = Vec::new();

        for (row, current) in working {
            let targets = self.step_targets(
                current,
                rel_types.as_deref(),
                rel.direction,
                rel.length.as_ref(),
            )?;
            let wanted = self.pattern_props(&row, node)?;
            for (edge, target) in targets {
                if !self.node_matches(target, node, &wanted)? {
                    continue;
                }
                if !self.node_passes_pushdown(&node.var, target)? {
                    continue;
                }
                // If the target variable is already bound, it must be the same node.
                if let Some(var) = &node.var
                    && let Some(existing) = row.get(var)
                {
                    match existing {
                        Binding::Node(n) if *n == target => {}
                        _ => continue,
                    }
                }
                // A bound relationship variable must be this very edge (R-Q2).
                let edge_var = rel.var.as_ref().zip(edge);
                if let Some((var, edge)) = edge_var {
                    match row.get(var) {
                        Some(Binding::Edge(e)) if *e == edge => {}
                        Some(_) => continue,
                        None if !self.edge_passes_pushdown(&rel.var, edge)? => continue,
                        None => {}
                    }
                }
                let mut new_row = row.clone();
                if let Some((var, edge)) = edge_var {
                    new_row.insert(var.clone(), Binding::Edge(edge));
                }
                if let Some(var) = &node.var {
                    new_row.insert(var.clone(), Binding::Node(target));
                }
                next.push((new_row, target));
                self.row_made(next.len())?;
            }
        }

        Ok(next)
    }

    fn step_targets(
        &self,
        node: G::NodeId,
        rel_types: Option<&[String]>,
        direction: Direction,
        length: Option<&crate::ast::VarLength>,
    ) -> Result<Targets<G>, CypherError> {
        if let Some(var_length) = length {
            return Ok(self
                .var_length_targets(node, rel_types, direction, var_length)?
                .into_iter()
                .map(|target| (None, target))
                .collect());
        }
        // One target per edge: parallel edges and a self-loop matched undirected each give their own row
        // (walk semantics, as LadybugDB 0.22.3; R-Q3).
        Ok(self
            .step_once(node, rel_types, direction)?
            .into_iter()
            .map(|(edge, target)| (Some(edge), target))
            .collect())
    }

    /// One hop over every listed type at once (`None`: an untyped step), one provider call per direction.
    fn step_once(
        &self,
        node: G::NodeId,
        rel_types: Option<&[String]>,
        direction: Direction,
    ) -> Result<Steps<G>, CypherError> {
        let directed = |incoming: bool| {
            let edges = self.graph.expand_types(node, rel_types, incoming);
            if rel_types.is_none() && self.symmetric_present {
                for (edge, _) in &edges {
                    if self.graph.edge_is_symmetric(*edge) {
                        let label = self.graph.edge_label(*edge);
                        self.crossed_symmetric.borrow_mut().insert(label);
                    }
                }
            }
            edges
        };
        let steps = match direction {
            Direction::Outgoing => directed(false),
            Direction::Incoming => directed(true),
            // Undirected is the outgoing edges, then the incoming ones, with no dedupe: a self-loop is found
            // twice, once from each end.
            Direction::Both => {
                let mut targets = self.graph.expand_types(node, rel_types, false);
                targets.extend(self.graph.expand_types(node, rel_types, true));
                targets
            }
        };
        // Every step read is work.
        self.charge((steps.len() as u64).saturating_add(self.graph.take_expansion_work()))?;
        Ok(steps)
    }

    /// Every walk of `min..=max` steps from `start` gives one target, so a node reached by two walks is listed
    /// twice and an edge may repeat within a walk (LadybugDB 0.22.3's WALK semantics; R-Q3). The walk count is
    /// finite because every walk is at most `max` long; an open upper bound is capped at
    /// [`OPEN_UPPER_BOUND`] until s03 refuses it.
    fn var_length_targets(
        &self,
        start: G::NodeId,
        rel_types: Option<&[String]>,
        direction: Direction,
        var_length: &crate::ast::VarLength,
    ) -> Result<Vec<G::NodeId>, CypherError> {
        let max = var_length.max.unwrap_or(OPEN_UPPER_BOUND);
        let mut results = Vec::new();
        if var_length.min == 0 {
            results.push(start);
        }
        let mut frontier = vec![start];
        for depth in 1..=max {
            if frontier.is_empty() {
                break;
            }
            let mut next = Vec::new();
            for node in &frontier {
                // Each step is charged as it is read, and the frontier and results held count
                // against the row cap, so a walk that explodes stops part way, not after it is built.
                next.extend(
                    self.step_once(*node, rel_types, direction)?
                        .into_iter()
                        .map(|(_, target)| target),
                );
                let held = next.len() + frontier.len() + results.len();
                if held > self.limits.max_rows {
                    return Err(CypherError::TooLarge {
                        cause: Limit::Rows,
                        used: self.work.get(),
                    });
                }
            }
            if depth >= var_length.min {
                results.extend(next.iter().copied());
            }
            frontier = next;
        }
        Ok(results)
    }

    fn node_matches(
        &self,
        node: G::NodeId,
        pattern: &NodePattern,
        wanted: &[(String, CypherValue)],
    ) -> Result<bool, CypherError> {
        Ok(self.matches_labels(node, &pattern.labels) && self.props_match(node, wanted)?)
    }

    /// A node matches an empty label set, or any one of the listed labels (label disjunction).
    fn matches_labels(&self, node: G::NodeId, labels: &[String]) -> bool {
        labels.is_empty()
            || labels
                .iter()
                .any(|label| self.graph.matches_label(node, label))
    }

    /// Every `{key: value}` holds; null never matches (`{due: null}` matches nothing, as in openCypher).
    fn props_match(
        &self,
        node: G::NodeId,
        wanted: &[(String, CypherValue)],
    ) -> Result<bool, CypherError> {
        for (key, value) in wanted {
            if !compare_values(self.graph, &self.property(node, key)?, CmpOp::Eq, value) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The step's relationship types that the graph uses; `None` for an untyped step, which the provider
    /// answers over every type it lists.
    fn resolve_rel_types(
        &self,
        rel: &crate::ast::RelPattern,
    ) -> Result<Option<Vec<String>>, CypherError> {
        if rel.types.is_empty() {
            return Ok(None);
        }
        // Soft vocabulary (D7): a relation no edge uses matches nothing, not an error; the caller warns. A named-only
        // relation is known when named.
        let named_only = self.graph.named_only_types();
        Ok(Some(
            rel.types
                .iter()
                .filter(|name| self.graph.has_relationship_type(name) || named_only.contains(name))
                .cloned()
                .collect(),
        ))
    }

    // ---- Expression evaluation -------------------------------------------

    fn eval_expr(&self, row: &Row<G>, expr: &Expr) -> Result<CypherValue, CypherError> {
        match expr {
            Expr::Literal(literal) => Ok(literal_to_value(literal)),
            Expr::Var(name) => Ok(match row.get(name) {
                Some(binding) => self.binding_value(binding)?,
                None => CypherValue::Null,
            }),
            Expr::Property(var, prop) => Ok(match row.get(var) {
                Some(Binding::Node(node)) => self.property(*node, prop)?,
                Some(Binding::Edge(edge)) => {
                    self.charge(1)?;
                    self.graph.edge_property(*edge, prop)
                }
                _ => CypherValue::Null,
            }),
            Expr::Not(inner) => Ok(CypherValue::Bool(!self.eval_expr(row, inner)?.is_truthy())),
            Expr::And(a, b) => Ok(CypherValue::Bool(
                self.eval_expr(row, a)?.is_truthy() && self.eval_expr(row, b)?.is_truthy(),
            )),
            Expr::Or(a, b) => Ok(CypherValue::Bool(
                self.eval_expr(row, a)?.is_truthy() || self.eval_expr(row, b)?.is_truthy(),
            )),
            Expr::Compare(a, op, b) => {
                // Fast path: `x IN <hoisted constant list>` becomes an O(log n) BTreeSet membership; a string
                // the provider reads as more than text (a date) is also checked against the list's such strings,
                // so `IN` follows `=` on both paths. That scan runs only for such a string, and costs one unit per
                // string it checks, so the work budget and the cancel hook bound it (security B1).
                if *op == CmpOp::In {
                    let key = core::ptr::from_ref::<Expr>(b) as usize;
                    let hoisted = self.in_hoist.borrow().get(&key).cloned();
                    if let Some(hoisted) = hoisted {
                        let left = self.eval_expr(row, a)?;
                        if matches!(left, CypherValue::Null) {
                            return Ok(CypherValue::Bool(false));
                        }
                        if hoisted.set.contains(&value_key(&left)) {
                            return Ok(CypherValue::Bool(true));
                        }
                        let found = match left.as_str() {
                            Some(l)
                                if !hoisted.texts.is_empty()
                                    && self.graph.compare_text(l, l).is_some() =>
                            {
                                self.charge(hoisted.texts.len() as u64)?;
                                hoisted
                                    .texts
                                    .iter()
                                    .any(|t| self.graph.compare_text(l, t) == Some(Ordering::Equal))
                            }
                            _ => false,
                        };
                        return Ok(CypherValue::Bool(found));
                    }
                }
                let left = self.eval_expr(row, a)?;
                // `x IN n.prop` over the provider's list (ws005 s15), after the left side as today.
                if *op == CmpOp::In
                    && let Some((node, key)) = self.node_key(row, b)
                {
                    return Ok(CypherValue::Bool(self.in_node_list(&left, node, key)?));
                }
                let right = self.eval_expr(row, b)?;
                Ok(CypherValue::Bool(compare_values(
                    self.graph, &left, *op, &right,
                )))
            }
            Expr::List(items) => {
                // A list made costs its elements, so a huge literal meets the budget.
                self.charge(items.len() as u64 * ELEMENT_COST)?;
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval_expr(row, item)?);
                }
                nest(CypherValue::List(values))
            }
            Expr::IsNull(inner, negated) => {
                let is_null = matches!(self.eval_expr(row, inner)?, CypherValue::Null);
                Ok(CypherValue::Bool(is_null ^ *negated))
            }
            Expr::Exists {
                patterns,
                where_clause,
            } => {
                let mut rows = vec![row.clone()];
                for pattern in patterns {
                    rows = self.eval_pattern(rows, pattern)?;
                }
                if let Some(predicate) = where_clause {
                    rows = self.filter_rows(rows, predicate)?;
                }
                Ok(CypherValue::Bool(!rows.is_empty()))
            }
            Expr::Case {
                operand,
                branches,
                default,
            } => self.eval_case(row, operand.as_deref(), branches, default.as_deref()),
            Expr::Function { name, args } => self.eval_function(row, name, args),
            Expr::MapProjection { var, entries } => self.eval_map_projection(row, var, entries),
            Expr::Aggregate { .. } => Err(CypherError::execution(
                "aggregate functions are only allowed in WITH or RETURN projections",
            )),
        }
    }

    fn eval_map_projection(
        &self,
        row: &Row<G>,
        var: &str,
        entries: &[crate::ast::MapEntry],
    ) -> Result<CypherValue, CypherError> {
        // A map projection is defined over a node; on a null/non-node binding it yields null.
        let node = match row.get(var) {
            Some(Binding::Node(node)) => *node,
            _ => return Ok(CypherValue::Null),
        };

        let mut map = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                crate::ast::MapEntry::Property(prop) => {
                    map.push((prop.clone(), self.property(node, prop)?));
                }
                crate::ast::MapEntry::Literal(key, expr) => {
                    map.push((key.clone(), self.eval_expr(row, expr)?));
                }
            }
        }
        nest(CypherValue::Map(map))
    }

    fn eval_case(
        &self,
        row: &Row<G>,
        operand: Option<&Expr>,
        branches: &[(Expr, Expr)],
        default: Option<&Expr>,
    ) -> Result<CypherValue, CypherError> {
        if let Some(operand) = operand {
            // Simple form: compare the operand against each WHEN value.
            let subject = self.eval_expr(row, operand)?;
            for (when, then) in branches {
                let candidate = self.eval_expr(row, when)?;
                if values_equal(self.graph, &subject, &candidate) {
                    return self.eval_expr(row, then);
                }
            }
        } else {
            // Generic form: the first truthy WHEN condition wins.
            for (when, then) in branches {
                if self.eval_expr(row, when)?.is_truthy() {
                    return self.eval_expr(row, then);
                }
            }
        }
        match default {
            Some(default) => self.eval_expr(row, default),
            None => Ok(CypherValue::Null),
        }
    }

    fn eval_function(
        &self,
        row: &Row<G>,
        name: &str,
        args: &[Expr],
    ) -> Result<CypherValue, CypherError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "tolower" | "toupper" => {
                let arg = single_arg(name, args)?;
                Ok(match self.eval_expr(row, arg)? {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Str(s) => CypherValue::Str(if lower == "tolower" {
                        s.to_lowercase()
                    } else {
                        s.to_uppercase()
                    }),
                    _ => {
                        return Err(CypherError::execution(format!(
                            "`{name}` expects a string argument"
                        )));
                    }
                })
            }
            "size" => {
                let arg = single_arg(name, args)?;
                // `size(n.prop)` over the provider's list (ws005 s15), charged once on both branches.
                let value = match self.node_key(row, arg) {
                    Some((node, key)) => match self.node_list_len(node, key)? {
                        Some(len) => return Ok(CypherValue::Int(len)),
                        None => self.property_value(node, key),
                    },
                    None => self.eval_expr(row, arg)?,
                };
                Ok(match value {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Str(s) => {
                        CypherValue::Int(i64::try_from(s.chars().count()).unwrap_or(i64::MAX))
                    }
                    CypherValue::List(items) => {
                        CypherValue::Int(i64::try_from(items.len()).unwrap_or(i64::MAX))
                    }
                    _ => {
                        return Err(CypherError::execution(
                            "`size` expects a string or list argument",
                        ));
                    }
                })
            }
            "coalesce" => {
                if args.is_empty() {
                    return Err(CypherError::execution(
                        "`coalesce` requires at least one argument",
                    ));
                }
                for arg in args {
                    let value = self.eval_expr(row, arg)?;
                    if value != CypherValue::Null {
                        return Ok(value);
                    }
                }
                Ok(CypherValue::Null)
            }
            "labels" => {
                let arg = single_arg(name, args)?;
                Ok(match self.eval_expr(row, arg)? {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Node { label, .. } => {
                        CypherValue::List(vec![CypherValue::Str(label)])
                    }
                    _ => {
                        return Err(CypherError::execution("`labels` expects a node argument"));
                    }
                })
            }
            // `label(x)`: a node's type or a relationship's type, as one string (LadybugDB's form).
            "label" | "type" => {
                let arg = single_arg(name, args)?;
                Ok(match self.eval_expr(row, arg)? {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Node { label, .. } if name.eq_ignore_ascii_case("label") => {
                        CypherValue::Str(label)
                    }
                    CypherValue::Relationship { label, .. } => CypherValue::Str(label),
                    _ => {
                        return Err(CypherError::execution(
                            if name.eq_ignore_ascii_case("type") {
                                "`type` expects a relationship argument"
                            } else {
                                "`label` expects a node or relationship argument"
                            },
                        ));
                    }
                })
            }
            // A provider's own function (VENDORED row 28), asked only once every built-in has missed.
            _ if self.graph.has_function(&lower) => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval_expr(row, arg)?);
                }
                self.charge(1)?;
                self.graph
                    .function(&lower, &values)
                    .map_err(CypherError::execution)
            }
            _ => Err(CypherError::execution(format!("unknown function `{name}`"))),
        }
    }

    fn node_value(&self, node: G::NodeId) -> CypherValue {
        CypherValue::Node {
            id: self.graph.node_id(node),
            label: self.graph.label(node),
            name: self.graph.name(node),
        }
    }

    fn edge_value(&self, edge: G::EdgeId) -> CypherValue {
        CypherValue::Relationship {
            id: self.graph.edge_id(edge),
            label: self.graph.edge_label(edge),
        }
    }

    /// A projected row's display value, or null when the projection does not need one: a WITH with no
    /// DISTINCT and no ORDER BY only carries its (shared) bindings, so it never copies a bound list.
    fn shown(&self, binding: &Binding<G>) -> Result<CypherValue, CypherError> {
        if self.values_wanted.get() {
            self.binding_value(binding)
        } else {
            Ok(CypherValue::Null)
        }
    }

    /// A binding's value. Reading a bound list or map copies it, so it costs its elements (security B1).
    fn binding_value(&self, binding: &Binding<G>) -> Result<CypherValue, CypherError> {
        Ok(match binding {
            Binding::Node(node) => self.node_value(*node),
            Binding::Edge(edge) => self.edge_value(*edge),
            Binding::Value(value) => {
                self.charge(elements(value) * ELEMENT_COST)?;
                (**value).clone()
            }
        })
    }

    // ---- Projection ------------------------------------------------------

    /// Projects the current rows through a `WITH` or `RETURN` projection, returning the output
    /// column names and one [`Projected`] per output row (bindings + aligned display values).
    /// Does not apply DISTINCT / ORDER BY / SKIP / LIMIT — see [`finalize`].
    fn project_rows(
        &self,
        projection: &Projection,
        rows: &[Row<G>],
        scope: &[String],
    ) -> Result<ProjectedRows<G>, CypherError> {
        if projection.star {
            return self.project_star(rows, scope);
        }

        let items = &projection.items;
        let columns = projection.column_names();

        let projected = if projection.has_aggregate() {
            self.project_aggregated(items, &columns, rows)?
        } else {
            self.project_simple(items, &columns, rows)?
        };

        Ok((columns, projected))
    }

    fn project_star(
        &self,
        rows: &[Row<G>],
        scope: &[String],
    ) -> Result<ProjectedRows<G>, CypherError> {
        if scope.is_empty() {
            return Err(CypherError::execution(
                "`*` requires at least one variable in scope",
            ));
        }

        let columns = scope.to_vec();
        let mut projected = Vec::with_capacity(rows.len());
        for row in rows {
            let mut bindings = Row::new();
            let mut values = Vec::with_capacity(columns.len());
            for name in &columns {
                let binding = row
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| Binding::Value(Rc::new(CypherValue::Null)));
                values.push(self.shown(&binding)?);
                bindings.insert(name.clone(), binding);
            }
            projected.push(Projected { bindings, values });
        }
        Ok((columns, projected))
    }

    fn project_simple(
        &self,
        items: &[ReturnItem],
        columns: &[String],
        rows: &[Row<G>],
    ) -> Result<Vec<Projected<G>>, CypherError> {
        let mut output = Vec::with_capacity(rows.len());
        for row in rows {
            output.push(self.project_one(items, columns, row)?);
            self.row_made(output.len())?;
        }
        Ok(output)
    }

    /// Projects a single input row, preserving node identity for bare-variable items so the
    /// projected variable can still be used as a node by later pattern matching.
    fn project_one(
        &self,
        items: &[ReturnItem],
        columns: &[String],
        row: &Row<G>,
    ) -> Result<Projected<G>, CypherError> {
        let mut bindings = Row::new();
        let mut values = Vec::with_capacity(items.len());
        for (column, item) in columns.iter().zip(items) {
            let binding = self.project_binding(row, item)?;
            values.push(self.shown(&binding)?);
            bindings.insert(column.clone(), binding);
        }
        Ok(Projected { bindings, values })
    }

    fn project_binding(&self, row: &Row<G>, item: &ReturnItem) -> Result<Binding<G>, CypherError> {
        if let Expr::Var(name) = &item.expr {
            return Ok(row
                .get(name)
                .cloned()
                .unwrap_or_else(|| Binding::Value(Rc::new(CypherValue::Null))));
        }
        Ok(Binding::Value(Rc::new(self.eval_expr(row, &item.expr)?)))
    }

    fn project_aggregated(
        &self,
        items: &[ReturnItem],
        columns: &[String],
        rows: &[Row<G>],
    ) -> Result<Vec<Projected<G>>, CypherError> {
        // Group rows by the values of the non-aggregate (grouping) items.
        let mut group_order: Vec<Vec<CypherValue>> = Vec::new();
        let mut groups: BTreeMap<Vec<CypherValue>, Vec<usize>> = BTreeMap::new();

        for (index, row) in rows.iter().enumerate() {
            let mut key = Vec::new();
            for item in items {
                if !item.expr.contains_aggregate() {
                    key.push(self.eval_expr(row, &item.expr)?);
                }
            }
            // Grouped by value: 3 and 3.0 are one group (R-Q6); the group shows its first row's values.
            let key: Vec<CypherValue> = key.iter().map(value_key).collect();
            if !groups.contains_key(&key) {
                group_order.push(key.clone());
            }
            groups.entry(key).or_default().push(index);
        }

        // With no grouping keys and no input rows, aggregates still produce a single row.
        let grouping_keys = items
            .iter()
            .filter(|item| !item.expr.contains_aggregate())
            .count();
        if group_order.is_empty() && grouping_keys == 0 {
            group_order.push(Vec::new());
            groups.insert(Vec::new(), Vec::new());
        }

        let mut output = Vec::with_capacity(group_order.len());
        for key in group_order {
            let row_indices = &groups[&key];
            let group_rows: Vec<&Row<G>> = row_indices.iter().map(|index| &rows[*index]).collect();
            let representative = group_rows.first().copied();

            let mut bindings = Row::new();
            let mut values = Vec::with_capacity(items.len());
            for (column, item) in columns.iter().zip(items) {
                let binding = if item.expr.contains_aggregate() {
                    Binding::Value(Rc::new(self.eval_aggregate(&item.expr, &group_rows)?))
                } else if let Some(row) = representative {
                    // Preserve the grouping item's binding (e.g. node identity).
                    self.project_binding(row, item)?
                } else {
                    Binding::Value(Rc::new(CypherValue::Null))
                };
                values.push(self.shown(&binding)?);
                bindings.insert(column.clone(), binding);
            }
            output.push(Projected { bindings, values });
        }

        Ok(output)
    }

    /// Applies DISTINCT, ORDER BY, SKIP, and LIMIT to a set of projected rows. An ORDER BY item is a
    /// projected column, or an expression over the projected variables (`WITH b ORDER BY b.id`; F1).
    fn finalize(
        &self,
        projection: &Projection,
        columns: &[String],
        mut projected: Vec<Projected<G>>,
    ) -> Result<Vec<Projected<G>>, CypherError> {
        if projection.distinct {
            dedupe_projected(&mut projected);
        }

        if !projection.order_by.is_empty() {
            let mut keyed: Vec<(Vec<SortKey>, Projected<G>)> = Vec::with_capacity(projected.len());
            for p in projected {
                let mut keys = Vec::with_capacity(projection.order_by.len());
                for item in &projection.order_by {
                    keys.push(self.sort_key(
                        match resolve_order_column(item, &projection.items, columns) {
                            Ok(index) => p.values[index].clone(),
                            Err(error) => {
                                let mut vars = BTreeSet::new();
                                if !collect_vars(&item.expr, &mut vars)
                                    || !vars.iter().all(|v| columns.contains(v))
                                {
                                    return Err(error);
                                }
                                self.eval_expr(&p.bindings, &item.expr)?
                            }
                        },
                    ));
                }
                keyed.push((keys, p));
            }
            // A stable sort: rows with equal keys keep their order.
            keyed.sort_by(|(a, _), (b, _)| {
                for ((x, y), order_item) in a.iter().zip(b).zip(&projection.order_by) {
                    let ordering = x.cmp(y);
                    let ordering = if order_item.descending {
                        ordering.reverse()
                    } else {
                        ordering
                    };
                    if ordering != core::cmp::Ordering::Equal {
                        return ordering;
                    }
                }
                core::cmp::Ordering::Equal
            });
            projected = keyed.into_iter().map(|(_, p)| p).collect();
        }

        if let Some(skip) = projection.skip {
            if skip >= projected.len() {
                projected.clear();
            } else {
                projected.drain(0..skip);
            }
        }

        if let Some(limit) = projection.limit
            && projected.len() > limit
        {
            projected.truncate(limit);
        }

        Ok(projected)
    }

    fn eval_aggregate(&self, expr: &Expr, group: &[&Row<G>]) -> Result<CypherValue, CypherError> {
        let Expr::Aggregate {
            func,
            arg,
            distinct,
        } = expr
        else {
            return Err(CypherError::execution("expected an aggregate function"));
        };

        // count(*) does not evaluate an argument.
        if *func == AggFn::Count && arg.is_none() {
            return Ok(CypherValue::Int(
                i64::try_from(group.len()).unwrap_or(i64::MAX),
            ));
        }

        let arg_expr = arg
            .as_ref()
            .ok_or_else(|| CypherError::execution("aggregate function requires an argument"))?;

        let mut values = Vec::new();
        for row in group {
            let value = self.eval_expr(row, arg_expr)?;
            if value != CypherValue::Null {
                values.push(value);
            }
        }

        if *distinct {
            values = dedupe_values(values);
        }

        Ok(match func {
            AggFn::Count => CypherValue::Int(i64::try_from(values.len()).unwrap_or(i64::MAX)),
            AggFn::Collect => {
                // collect's list costs its elements once more: it is kept, possibly bound by a WITH.
                self.charge(values.len() as u64 * ELEMENT_COST)?;
                nest(CypherValue::List(values))?
            }
            AggFn::Min => values
                .into_iter()
                .map(|v| self.sort_key(v))
                .min_by(SortKey::cmp)
                .map_or(CypherValue::Null, |k| k.value),
            AggFn::Max => values
                .into_iter()
                .map(|v| self.sort_key(v))
                .max_by(SortKey::cmp)
                .map_or(CypherValue::Null, |k| k.value),
            // sum stays an integer over integers and becomes a float once a float is summed; avg is always
            // a float (LadybugDB's DOUBLE; R-Q6). Values are summed in row order.
            AggFn::Sum => {
                if values.iter().any(|v| matches!(v, CypherValue::Float(_))) {
                    CypherValue::Float(crate::value::Float(
                        values.iter().filter_map(CypherValue::as_f64).sum(),
                    ))
                } else {
                    CypherValue::Int(values.iter().filter_map(CypherValue::as_int).sum())
                }
            }
            AggFn::Avg => {
                let numbers: Vec<f64> = values.iter().filter_map(CypherValue::as_f64).collect();
                if numbers.is_empty() {
                    CypherValue::Null
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    let count = numbers.len() as f64;
                    CypherValue::Float(crate::value::Float(numbers.iter().sum::<f64>() / count))
                }
            }
        })
    }
}

fn resolve_order_column(
    order_item: &OrderItem,
    items: &[ReturnItem],
    columns: &[String],
) -> Result<usize, CypherError> {
    // Match by identical projection expression first.
    if let Some(index) = items.iter().position(|item| item.expr == order_item.expr) {
        return Ok(index);
    }

    // Otherwise, a bare variable in ORDER BY may name a projected column or alias.
    if let Expr::Var(name) = &order_item.expr
        && let Some(index) = columns.iter().position(|column| column == name)
    {
        return Ok(index);
    }

    Err(CypherError::execution(format!(
        "ORDER BY expression `{}` must also appear in the projection",
        order_item.expr.display_name()
    )))
}

/// Builds the pushdown map from every non-optional MATCH clause's `WHERE`: single-variable,
/// aggregate/EXISTS-free conjuncts keyed by the variable they reference.
///
/// Only non-optional MATCH predicates are safe to push into candidate generation: they are inner
/// selections, so pruning a candidate that fails a single-variable predicate can never drop a row
/// the final WHERE would have kept. `OPTIONAL MATCH` (outer join) and `WITH ... WHERE` are skipped.
///
/// `clauses` start at the MATCH being run. A predicate written later only applies while its name still means the
/// same binding: a WITH ends every binding (`MATCH (n) WITH count(n) AS c MATCH (n) WHERE n.x = 1` counts every
/// n), and an UNWIND that rebinds a name makes a new one (`MATCH ()-[s]->() UNWIND [1] AS s MATCH (n) WHERE s = 1`
/// keeps every s; reviewer R134-2).
fn build_global_pushdown(clauses: &[Clause]) -> BTreeMap<String, Vec<Expr>> {
    let mut map: BTreeMap<String, Vec<Expr>> = BTreeMap::new();
    let mut rebound: BTreeSet<&str> = BTreeSet::new();
    for clause in clauses {
        match clause {
            Clause::With(_) => break,
            Clause::Unwind(u) => {
                rebound.insert(u.var.as_str());
            }
            Clause::Match(m) if !m.optional => {
                let Some(predicate) = &m.where_clause else {
                    continue;
                };
                let mut conjuncts = Vec::new();
                collect_conjuncts(predicate, &mut conjuncts);
                for conjunct in conjuncts {
                    if let Some(var) = single_var(conjunct)
                        && !rebound.contains(var.as_str())
                    {
                        map.entry(var).or_default().push(conjunct.clone());
                    }
                }
            }
            Clause::Match(_) => {}
        }
    }
    map
}

/// The prefix of the variable an anonymous node gets while its deferred map entries are checked; no
/// Cypher name can start with a space, and the binding is dropped when the pattern is done.
const HIDDEN_VAR: &str = " node";

/// Splits a pattern's property maps: an entry whose value reads only variables bound before the
/// pattern starts (in `sample`, the first input row) stays in the map, where it prunes candidates
/// and may use the index; every other entry is returned as `(variable, key, value)` to check on
/// the finished path, and its node gets a hidden variable if it has none.
fn split_props<G: GraphProvider>(
    pattern: &PathPattern,
    sample: Option<&Row<G>>,
) -> (PathPattern, Vec<(String, String, Expr)>) {
    let bound = |expr: &Expr| {
        let mut vars = BTreeSet::new();
        collect_vars(expr, &mut vars)
            && vars
                .iter()
                .all(|v| sample.is_some_and(|row| row.get(v).is_some()))
    };
    let mut deferred = Vec::new();
    let mut hidden = 0;
    let mut split = |node: &NodePattern| {
        let mut node = node.clone();
        let (keep, later): (Vec<_>, Vec<_>) = node.props.into_iter().partition(|(_, e)| bound(e));
        node.props = keep;
        if !later.is_empty() {
            let var = node.var.get_or_insert_with(|| {
                hidden += 1;
                format!("{HIDDEN_VAR}{hidden}")
            });
            deferred.extend(later.into_iter().map(|(k, e)| (var.clone(), k, e)));
        }
        node
    };
    let start = split(&pattern.start);
    let rest = pattern
        .rest
        .iter()
        .map(|(rel, node)| (rel.clone(), split(node)))
        .collect();
    (PathPattern { start, rest }, deferred)
}

/// `var.prop = <literal>` (either way round): the property and literal a pushed-down predicate fixes.
fn indexed_equality<'a>(pred: &'a Expr, var: &str) -> Option<(&'a str, &'a Literal)> {
    let Expr::Compare(left, CmpOp::Eq, right) = pred else {
        return None;
    };
    match (left.as_ref(), right.as_ref()) {
        (Expr::Property(v, prop), Expr::Literal(lit))
        | (Expr::Literal(lit), Expr::Property(v, prop))
            if v == var =>
        {
            Some((prop.as_str(), lit))
        }
        _ => None,
    }
}

/// Flattens a conjunction (`a AND b AND ...`) into its individual conjuncts.
fn collect_conjuncts<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::And(a, b) = expr {
        collect_conjuncts(a, out);
        collect_conjuncts(b, out);
    } else {
        out.push(expr);
    }
}

/// The conjuncts of a MATCH's own WHERE that `build_global_pushdown` pushed for a variable in
/// `bound` (the variables that MATCH's patterns bound), by address in the WHERE's tree.
fn proven_conjuncts(predicate: &Expr, bound: &[String]) -> Vec<*const Expr> {
    let mut conjuncts = Vec::new();
    collect_conjuncts(predicate, &mut conjuncts);
    conjuncts
        .into_iter()
        .filter(|c| single_var(c).is_some_and(|v| bound.contains(&v)))
        .map(core::ptr::from_ref)
        .collect()
}

/// Returns the sole variable an expression references if it is pushable (references exactly one
/// variable and contains no aggregate or EXISTS subquery); otherwise `None`.
fn single_var(expr: &Expr) -> Option<String> {
    let mut vars = BTreeSet::new();
    if !collect_vars(expr, &mut vars) {
        return None;
    }
    if vars.len() == 1 {
        vars.into_iter().next()
    } else {
        None
    }
}

/// Collects the variables referenced by an expression. Returns `false` if the expression contains
/// an aggregate or EXISTS subquery (not eligible for pushdown).
fn collect_vars(expr: &Expr, out: &mut BTreeSet<String>) -> bool {
    match expr {
        Expr::Var(name) => {
            out.insert(name.clone());
            true
        }
        Expr::Property(var, _) => {
            out.insert(var.clone());
            true
        }
        Expr::Literal(_) => true,
        Expr::List(items) => items.iter().all(|item| collect_vars(item, out)),
        Expr::Not(inner) | Expr::IsNull(inner, _) => collect_vars(inner, out),
        Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
            let left = collect_vars(a, out);
            let right = collect_vars(b, out);
            left && right
        }
        Expr::Function { args, .. } => args.iter().all(|arg| collect_vars(arg, out)),
        Expr::MapProjection { var, entries } => {
            out.insert(var.clone());
            entries.iter().all(|entry| match entry {
                crate::ast::MapEntry::Property(_) => true,
                crate::ast::MapEntry::Literal(_, expr) => collect_vars(expr, out),
            })
        }
        Expr::Case {
            operand,
            branches,
            default,
        } => {
            let mut ok = operand.as_ref().is_none_or(|o| collect_vars(o, out));
            for (when, then) in branches {
                ok = collect_vars(when, out) && ok;
                ok = collect_vars(then, out) && ok;
            }
            ok = default.as_ref().is_none_or(|d| collect_vars(d, out)) && ok;
            ok
        }
        Expr::Aggregate { .. } | Expr::Exists { .. } => false,
    }
}

/// Whether an expression references only variables in `allowed` (and no aggregates), so it is
/// constant across a pass in which those variables are constant.
fn references_only(expr: &Expr, allowed: &BTreeSet<String>) -> bool {
    match expr {
        Expr::Var(name) => allowed.contains(name),
        Expr::Property(var, _) => allowed.contains(var),
        Expr::Literal(_) => true,
        Expr::List(items) => items.iter().all(|item| references_only(item, allowed)),
        Expr::Not(inner) | Expr::IsNull(inner, _) => references_only(inner, allowed),
        Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
            references_only(a, allowed) && references_only(b, allowed)
        }
        Expr::Function { args, .. } => args.iter().all(|arg| references_only(arg, allowed)),
        Expr::MapProjection { var, entries } => {
            allowed.contains(var)
                && entries.iter().all(|entry| match entry {
                    crate::ast::MapEntry::Property(_) => true,
                    crate::ast::MapEntry::Literal(_, expr) => references_only(expr, allowed),
                })
        }
        Expr::Case {
            operand,
            branches,
            default,
        } => {
            operand.as_ref().is_none_or(|o| references_only(o, allowed))
                && branches
                    .iter()
                    .all(|(w, t)| references_only(w, allowed) && references_only(t, allowed))
                && default.as_ref().is_none_or(|d| references_only(d, allowed))
        }
        // Aggregates and existential subqueries are not treated as loop-invariant.
        Expr::Aggregate { .. } | Expr::Exists { .. } => false,
    }
}

/// Chooses a traversal plan for a linear path: if the written start endpoint is not yet bound but
/// the end endpoint is, return a reversed copy so matching starts from the bound node. Otherwise
/// the pattern is returned unchanged. Reversal preserves the set of variable bindings exactly.
fn plan_pattern<G: GraphProvider>(
    pattern: &PathPattern,
    sample: Option<&Row<G>>,
    indexed: impl Fn(&NodePattern) -> bool,
) -> PathPattern {
    if should_reverse(pattern, sample, indexed) {
        reverse_path(pattern)
    } else {
        pattern.clone()
    }
}

fn should_reverse<G: GraphProvider>(
    pattern: &PathPattern,
    sample: Option<&Row<G>>,
    indexed: impl Fn(&NodePattern) -> bool,
) -> bool {
    let Some((_, end)) = pattern.rest.last() else {
        return false; // single node: nothing to reverse
    };
    let start_bound = is_node_bound(&pattern.start.var, sample);
    let end_bound = is_node_bound(&end.var, sample);
    // Start from a bound endpoint, else from one the provider can look up by index (`(a)-->(b {id: 'x'})`).
    (!start_bound && end_bound)
        || (!start_bound && !end_bound && !indexed(&pattern.start) && indexed(end))
}

fn is_node_bound<G: GraphProvider>(var: &Option<String>, sample: Option<&Row<G>>) -> bool {
    match (var, sample) {
        (Some(name), Some(row)) => matches!(row.get(name), Some(Binding::Node(_))),
        _ => false,
    }
}

/// Reverses a linear path: walks the node/relationship chain end-to-start and flips each
/// relationship's direction. The resulting pattern matches the same variable bindings.
fn reverse_path(pattern: &PathPattern) -> PathPattern {
    if pattern.rest.is_empty() {
        return pattern.clone();
    }

    let mut nodes: Vec<&NodePattern> = Vec::with_capacity(pattern.rest.len() + 1);
    nodes.push(&pattern.start);
    for (_, node) in &pattern.rest {
        nodes.push(node);
    }
    let rels: Vec<&crate::ast::RelPattern> = pattern.rest.iter().map(|(rel, _)| rel).collect();

    let start = nodes[nodes.len() - 1].clone();
    let mut rest = Vec::with_capacity(rels.len());
    for i in (0..rels.len()).rev() {
        rest.push((reverse_rel(rels[i]), nodes[i].clone()));
    }
    PathPattern { start, rest }
}

fn reverse_rel(rel: &crate::ast::RelPattern) -> crate::ast::RelPattern {
    let mut reversed = rel.clone();
    reversed.direction = match rel.direction {
        Direction::Outgoing => Direction::Incoming,
        Direction::Incoming => Direction::Outgoing,
        Direction::Both => Direction::Both,
    };
    reversed
}

/// Adds the node variables introduced by a pattern to the scope, in declaration order.
fn add_pattern_vars(scope: &mut Vec<String>, pattern: &PathPattern) {
    let mut push = |var: &Option<String>| {
        if let Some(name) = var
            && !scope.contains(name)
        {
            scope.push(name.clone());
        }
    };
    push(&pattern.start.var);
    for (rel, node) in &pattern.rest {
        push(&rel.var);
        push(&node.var);
    }
}

/// Returns the single argument of a scalar function, or an error if the arity is not exactly one.
fn single_arg<'a>(name: &str, args: &'a [Expr]) -> Result<&'a Expr, CypherError> {
    match args {
        [arg] => Ok(arg),
        _ => Err(CypherError::execution(format!(
            "`{name}` expects exactly one argument"
        ))),
    }
}

fn literal_to_value(literal: &Literal) -> CypherValue {
    match literal {
        Literal::Str(value) => CypherValue::Str(value.clone()),
        Literal::Int(value) => CypherValue::Int(*value),
        Literal::Float(value) => CypherValue::Float(crate::value::Float(*value)),
        Literal::Bool(value) => CypherValue::Bool(*value),
        Literal::Null | Literal::Param(_) => CypherValue::Null,
    }
}

/// `left op right`. Two strings the provider reads as more than text (umx: dates) compare through
/// [`GraphProvider::compare_text`], for every operator built on `=` or order (ws008 TM2, a umx departure).
fn compare_values<G: GraphProvider>(
    graph: &G,
    left: &CypherValue,
    op: CmpOp,
    right: &CypherValue,
) -> bool {
    if matches!(left, CypherValue::Null) || matches!(right, CypherValue::Null) {
        return false;
    }

    match op {
        CmpOp::Eq => values_equal(graph, left, right),
        CmpOp::Neq => !values_equal(graph, left, right),
        CmpOp::Lt | CmpOp::Lte | CmpOp::Gt | CmpOp::Gte => {
            if !same_type(left, right) {
                return false;
            }
            let ordering = match (left, right) {
                (CypherValue::Str(a), CypherValue::Str(b)) => {
                    graph.compare_text(a, b).unwrap_or_else(|| a.cmp(b))
                }
                _ => left.total_cmp(right),
            };
            match op {
                CmpOp::Lt => ordering.is_lt(),
                CmpOp::Lte => ordering.is_le(),
                CmpOp::Gt => ordering.is_gt(),
                CmpOp::Gte => ordering.is_ge(),
                _ => unreachable!(),
            }
        }
        CmpOp::Contains => string_op(left, right, |haystack, needle| haystack.contains(needle)),
        CmpOp::StartsWith => {
            string_op(left, right, |haystack, needle| haystack.starts_with(needle))
        }
        CmpOp::EndsWith => string_op(left, right, |haystack, needle| haystack.ends_with(needle)),
        CmpOp::In => match right {
            CypherValue::List(items) => items.iter().any(|item| values_equal(graph, left, item)),
            _ => false,
        },
    }
}

/// `=`. Lists and maps keep identity element by element (a date inside a list is matched as text).
fn values_equal<G: GraphProvider>(graph: &G, left: &CypherValue, right: &CypherValue) -> bool {
    match (left, right) {
        // Lists and maps compare element by element with the same numeric rule as the set keys, so
        // `[3] = [3.0]` holds on every path, hoisted or not (R-Q6; reviewer's r2 follow-up).
        (CypherValue::List(_), CypherValue::List(_))
        | (CypherValue::Map(_), CypherValue::Map(_)) => value_key(left) == value_key(right),
        (CypherValue::Str(a), CypherValue::Str(b)) => str_equal(graph, a, b),
        _ => match (left.as_f64(), right.as_f64()) {
            // Numbers compare by value across integer and float (`3 = 3.0`; R-Q6).
            (Some(_), Some(_)) => left.total_cmp(right).is_eq(),
            _ => same_type(left, right) && left == right,
        },
    }
}

/// `=` for two strings: by instant when the provider reads both as more than text (ws008 TM2), else as text. The one
/// rule `values_equal` and `IN` over a provider's list ([`GraphProvider::each_str`]) share.
fn str_equal<G: GraphProvider>(graph: &G, a: &str, b: &str) -> bool {
    graph.compare_text(a, b).map_or(a == b, Ordering::is_eq)
}

fn same_type(left: &CypherValue, right: &CypherValue) -> bool {
    matches!(
        (left, right),
        (CypherValue::Bool(_), CypherValue::Bool(_))
            | (
                CypherValue::Int(_) | CypherValue::Float(_),
                CypherValue::Int(_) | CypherValue::Float(_)
            )
            | (CypherValue::Str(_), CypherValue::Str(_))
            | (CypherValue::Node { .. }, CypherValue::Node { .. })
            | (
                CypherValue::Relationship { .. },
                CypherValue::Relationship { .. }
            )
            | (CypherValue::List(_), CypherValue::List(_))
            | (CypherValue::Map(_), CypherValue::Map(_))
    )
}

fn string_op(left: &CypherValue, right: &CypherValue, op: impl Fn(&str, &str) -> bool) -> bool {
    match (left.as_str(), right.as_str()) {
        (Some(haystack), Some(needle)) => op(haystack, needle),
        _ => false,
    }
}

/// The key under which equal values meet in a set: DISTINCT, grouping, `collect(DISTINCT)`,
/// `count(DISTINCT)` and a hoisted `IN` list. An integral float and its integer share one key (`3`
/// and `3.0` are equal; R-Q6), in lists and maps too; every other value is its own key. The first
/// value seen is the one shown.
fn value_key(value: &CypherValue) -> CypherValue {
    match value {
        // Integral when it survives the round trip through i64 (inside i64's range; NaN fails the range).
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        CypherValue::Float(f)
            if (-9.2e18..=9.2e18).contains(&f.0) && (f.0 as i64) as f64 == f.0 =>
        {
            CypherValue::Int(f.0 as i64)
        }
        CypherValue::List(items) => CypherValue::List(items.iter().map(value_key).collect()),
        CypherValue::Map(entries) => CypherValue::Map(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), value_key(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A list or map just built from values that are each at most [`MAX_VALUE_DEPTH`] deep, refused if it
/// is one level too many.
fn nest(value: CypherValue) -> Result<CypherValue, CypherError> {
    if depth(&value) > MAX_VALUE_DEPTH {
        return Err(CypherError::execution(VALUE_TOO_DEEP));
    }
    Ok(value)
}

/// How many list or map levels a value has (a scalar has none). Only called on values whose parts are
/// within [`MAX_VALUE_DEPTH`], so the recursion is shallow.
fn depth(value: &CypherValue) -> usize {
    match value {
        CypherValue::List(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        CypherValue::Map(entries) => 1 + entries.iter().map(|(_, v)| depth(v)).max().unwrap_or(0),
        _ => 0,
    }
}

/// The elements a list or map holds, nested ones included (scalars hold none).
fn elements(value: &CypherValue) -> u64 {
    match value {
        CypherValue::List(items) => items.iter().map(|v| 1 + elements(v)).sum(),
        CypherValue::Map(entries) => entries.iter().map(|(_, v)| 1 + elements(v)).sum(),
        _ => 0,
    }
}

fn dedupe_projected<G: GraphProvider>(output: &mut Vec<Projected<G>>) {
    // Order-preserving dedup by projected values. `CypherValue` is `Ord`, so a `BTreeSet`
    // gives O(n log n) dedup (the old linear scan was O(n^2)).
    let mut seen: BTreeSet<Vec<CypherValue>> = BTreeSet::new();
    output.retain(|projected| seen.insert(projected.values.iter().map(value_key).collect()));
}

fn dedupe_values(values: Vec<CypherValue>) -> Vec<CypherValue> {
    // Order-preserving dedup backed by a `BTreeSet` (was an O(n^2) `contains` scan).
    let mut seen: BTreeSet<CypherValue> = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value_key(value)))
        .collect()
}
