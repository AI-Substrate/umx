//! Abstract syntax tree for the supported subset of Cypher.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A complete parsed query: a sequence of reading clauses (`MATCH` / `WITH`) terminated by a
/// `RETURN` projection.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// The `MATCH` / `WITH` clauses, in order. Always at least one.
    pub clauses: Vec<Clause>,
    /// The terminal `RETURN` projection.
    pub result: Projection,
}

impl Query {
    /// Replaces every `$name` with its value from `params` (a literal, or a list of literals), so a parameter is a
    /// literal from here on: it pushes down, uses the id index and is hoisted exactly as a literal written in the
    /// text would be.
    ///
    /// # Errors
    ///
    /// The first parameter the query uses that `params` does not give.
    pub fn bind_params(
        &mut self,
        params: &alloc::collections::BTreeMap<String, Expr>,
    ) -> Result<(), String> {
        // An unaliased RETURN column that reads a parameter keeps its written name (`$x`), never the value's: equal
        // values would make equal column names, and the column header would carry the value (reviewer R127-2).
        for item in &mut self.result.items {
            if item.alias.is_none() && item.expr.reads_param() {
                item.alias = Some(item.expr.display_name());
            }
        }
        let mut missing = None;
        self.each_expr_mut(&mut |e| {
            if let Expr::Literal(Literal::Param(name)) = e {
                match params.get(name.as_str()) {
                    Some(value) => *e = value.clone(),
                    None => {
                        missing.get_or_insert_with(|| name.clone());
                    }
                }
            }
        });
        missing.map_or(Ok(()), Err)
    }

    /// Calls `f` on every expression of the query, outer ones before the ones inside them.
    fn each_expr_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        for clause in &mut self.clauses {
            match clause {
                Clause::Match(m) => {
                    for p in &mut m.patterns {
                        p.each_expr_mut(f);
                    }
                    if let Some(w) = &mut m.where_clause {
                        w.each_mut(f);
                    }
                }
                Clause::Unwind(u) => u.expr.each_mut(f),
                Clause::With(w) => {
                    w.projection.each_expr_mut(f);
                    if let Some(e) = &mut w.where_clause {
                        e.each_mut(f);
                    }
                }
            }
        }
        self.result.each_expr_mut(f);
    }

    /// Every relationship pattern in the query, in `MATCH`, `OPTIONAL MATCH` and `EXISTS` subqueries
    /// alike, in source order.
    #[must_use]
    pub fn rel_patterns(&self) -> Vec<&RelPattern> {
        let mut out = Vec::new();
        for clause in &self.clauses {
            match clause {
                Clause::Match(m) => {
                    for pattern in &m.patterns {
                        pattern.collect_rel_patterns(&mut out);
                    }
                    if let Some(w) = &m.where_clause {
                        w.collect_rel_patterns(&mut out);
                    }
                }
                Clause::Unwind(u) => u.expr.collect_rel_patterns(&mut out),
                Clause::With(w) => {
                    w.projection.collect_rel_patterns(&mut out);
                    if let Some(e) = &w.where_clause {
                        e.collect_rel_patterns(&mut out);
                    }
                }
            }
        }
        self.result.collect_rel_patterns(&mut out);
        out
    }
}

impl Projection {
    fn collect_rel_patterns<'a>(&'a self, out: &mut Vec<&'a RelPattern>) {
        for item in &self.items {
            item.expr.collect_rel_patterns(out);
        }
        for item in &self.order_by {
            item.expr.collect_rel_patterns(out);
        }
    }
}

impl PathPattern {
    fn collect_rel_patterns<'a>(&'a self, out: &mut Vec<&'a RelPattern>) {
        let node = |n: &'a NodePattern, out: &mut Vec<&'a RelPattern>| {
            for (_, e) in &n.props {
                e.collect_rel_patterns(out);
            }
        };
        node(&self.start, out);
        for (rel, n) in &self.rest {
            out.push(rel);
            node(n, out);
        }
    }
}

/// A reading clause in the query pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    Match(MatchClause),
    Unwind(UnwindClause),
    With(WithClause),
}

/// An `UNWIND <expr> AS <var>` clause: expands a list value into one row per element.
#[derive(Debug, Clone, PartialEq)]
pub struct UnwindClause {
    pub expr: Expr,
    pub var: String,
}

/// A `MATCH` clause: comma-separated path patterns with an optional `WHERE` filter.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchClause {
    /// `true` for `OPTIONAL MATCH` (left-join: rows with no match are kept with the clause's new
    /// variables bound to null).
    pub optional: bool,
    pub patterns: Vec<PathPattern>,
    pub where_clause: Option<Expr>,
}

/// A `WITH` clause: a projection that rebinds variables for the rest of the pipeline, with an
/// optional trailing `WHERE` filter applied after projection.
#[derive(Debug, Clone, PartialEq)]
pub struct WithClause {
    pub projection: Projection,
    pub where_clause: Option<Expr>,
}

/// A projection shared by `WITH` and `RETURN`: the items (or `*`), with `DISTINCT` and the
/// `ORDER BY` / `SKIP` / `LIMIT` modifiers.
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub distinct: bool,
    /// `*` — project every variable in scope. When set, `items` is empty.
    pub star: bool,
    pub items: Vec<ReturnItem>,
    pub order_by: Vec<OrderItem>,
    pub skip: Option<usize>,
    pub limit: Option<usize>,
}

impl Projection {
    /// The output column names, in order.
    #[must_use]
    pub fn column_names(&self) -> Vec<String> {
        self.items.iter().map(ReturnItem::column_name).collect()
    }

    /// Whether any projected item contains an aggregate function call.
    #[must_use]
    pub fn has_aggregate(&self) -> bool {
        self.items.iter().any(|item| item.expr.contains_aggregate())
    }
}

/// A path pattern: a starting node followed by zero or more relationship/node hops.
#[derive(Debug, Clone, PartialEq)]
pub struct PathPattern {
    pub start: NodePattern,
    pub rest: Vec<(RelPattern, NodePattern)>,
}

/// A node pattern such as `(c:Class {name: 'Foo'})` or `(c:Class|Module)`.
#[derive(Debug, Clone, PartialEq)]
pub struct NodePattern {
    pub var: Option<String>,
    /// Labels in a disjunction: a node matches if it has **any** of these labels. Empty means
    /// "any node".
    pub labels: Vec<String>,
    /// `{key: value}`: each value is an expression over the variables bound so far (`{id: x}`), not
    /// only a literal.
    pub props: Vec<(String, Expr)>,
}

/// A relationship pattern such as `-[:INHERITS*1..3]->`.
#[derive(Debug, Clone, PartialEq)]
pub struct RelPattern {
    pub var: Option<String>,
    /// Relationship types; empty means "any type".
    pub types: Vec<String>,
    pub direction: Direction,
    /// Variable-length specification, if the pattern used `*`.
    pub length: Option<VarLength>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// `-[]->`
    Outgoing,
    /// `<-[]-`
    Incoming,
    /// `-[]-`
    Both,
}

/// A variable-length relationship bound, from `*`, `*n`, `*min..`, `*..max`, or `*min..max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarLength {
    pub min: u32,
    pub max: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    /// `$name`, until [`Query::bind_params`] replaces it with the caller's value. One left unbound reads null.
    Param(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnItem {
    pub expr: Expr,
    pub alias: Option<String>,
}

impl ReturnItem {
    /// The output column name: the explicit alias, or a name derived from the expression.
    #[must_use]
    pub fn column_name(&self) -> String {
        self.alias
            .clone()
            .unwrap_or_else(|| self.expr.display_name())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Var(String),
    Property(String, String),
    Literal(Literal),
    /// A list literal such as `['a', 'b']`.
    List(Vec<Expr>),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Compare(Box<Expr>, CmpOp, Box<Expr>),
    /// `expr IS NULL` / `expr IS NOT NULL`; the bool is `true` when negated (`IS NOT NULL`).
    IsNull(Box<Expr>, bool),
    /// An existential subquery predicate `EXISTS { [MATCH] patterns [WHERE ...] }`. Evaluates to a
    /// boolean: whether the patterns match at least once given the current bindings. `NOT EXISTS`
    /// is expressed as [`Expr::Not`] wrapping this.
    Exists {
        patterns: Vec<PathPattern>,
        where_clause: Option<Box<Expr>>,
    },
    /// A `CASE` expression. `operand` is `Some` for the simple form (`CASE x WHEN v THEN ...`),
    /// `None` for the generic form (`CASE WHEN cond THEN ...`). Each branch is `(when, then)`.
    Case {
        operand: Option<Box<Expr>>,
        branches: Vec<(Expr, Expr)>,
        default: Option<Box<Expr>>,
    },
    /// A scalar function call such as `toLower(c.name)` or `coalesce(a, b)`.
    Function {
        name: String,
        args: Vec<Expr>,
    },
    /// A map projection `var { .prop, key: expr }` producing a map value.
    MapProjection {
        var: String,
        entries: Vec<MapEntry>,
    },
    Aggregate {
        func: AggFn,
        arg: Option<Box<Expr>>,
        distinct: bool,
    },
}

impl PathPattern {
    fn each_expr_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        for node in core::iter::once(&mut self.start).chain(self.rest.iter_mut().map(|(_, n)| n)) {
            for (_, e) in &mut node.props {
                e.each_mut(f);
            }
        }
    }
}

impl Projection {
    fn each_expr_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        for item in &mut self.items {
            item.expr.each_mut(f);
        }
        for item in &mut self.order_by {
            item.expr.each_mut(f);
        }
    }
}

impl Expr {
    /// Whether a `$name` appears in this expression.
    fn reads_param(&mut self) -> bool {
        let mut found = false;
        self.each_mut(&mut |e| found |= matches!(e, Expr::Literal(Literal::Param(_))));
        found
    }

    /// Calls `f` on this expression, then on every expression inside it (the parser bounds the depth).
    fn each_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        f(self);
        match self {
            Expr::Var(_) | Expr::Property(..) | Expr::Literal(_) => {}
            Expr::List(items) | Expr::Function { args: items, .. } => {
                items.iter_mut().for_each(|x| x.each_mut(f));
            }
            Expr::Not(x) | Expr::IsNull(x, _) => x.each_mut(f),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
                a.each_mut(f);
                b.each_mut(f);
            }
            Expr::Exists {
                patterns,
                where_clause,
            } => {
                for p in patterns {
                    p.each_expr_mut(f);
                }
                if let Some(w) = where_clause {
                    w.each_mut(f);
                }
            }
            Expr::Case {
                operand,
                branches,
                default,
            } => {
                for x in operand.iter_mut().chain(default.iter_mut()) {
                    x.each_mut(f);
                }
                for (w, t) in branches {
                    w.each_mut(f);
                    t.each_mut(f);
                }
            }
            Expr::MapProjection { entries, .. } => {
                for entry in entries {
                    if let MapEntry::Literal(_, x) = entry {
                        x.each_mut(f);
                    }
                }
            }
            Expr::Aggregate { arg, .. } => {
                if let Some(x) = arg {
                    x.each_mut(f);
                }
            }
        }
    }

    /// How many levels the built tree has (a leaf is 1). The parser bounds it (see
    /// `parser::MAX_DEPTH`), so this recursion, evaluation and drop stay shallow; it is called only on
    /// trees whose parts already passed that bound.
    pub fn height(&self) -> usize {
        let most =
            |items: &mut dyn Iterator<Item = &Expr>| items.map(Expr::height).max().unwrap_or(0);
        1 + match self {
            Expr::Var(_) | Expr::Property(..) | Expr::Literal(_) => 0,
            Expr::List(items) | Expr::Function { args: items, .. } => most(&mut items.iter()),
            Expr::Not(e) | Expr::IsNull(e, _) => e.height(),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => a.height().max(b.height()),
            Expr::Exists {
                patterns,
                where_clause,
            } => {
                let props = patterns.iter().flat_map(|p| {
                    core::iter::once(&p.start)
                        .chain(p.rest.iter().map(|(_, n)| n))
                        .flat_map(|n| n.props.iter().map(|(_, e)| e))
                });
                most(&mut props.chain(where_clause.as_deref()))
            }
            Expr::Case {
                operand,
                branches,
                default,
            } => most(
                &mut operand
                    .as_deref()
                    .into_iter()
                    .chain(default.as_deref())
                    .chain(branches.iter().flat_map(|(w, t)| [w, t])),
            ),
            Expr::MapProjection { entries, .. } => {
                most(&mut entries.iter().filter_map(|e| match e {
                    MapEntry::Literal(_, e) => Some(e),
                    MapEntry::Property(_) => None,
                }))
            }
            Expr::Aggregate { arg, .. } => most(&mut arg.as_deref().into_iter()),
        }
    }

    /// The relationship patterns inside this expression's `EXISTS` subqueries.
    fn collect_rel_patterns<'a>(&'a self, out: &mut Vec<&'a RelPattern>) {
        match self {
            Expr::Var(_) | Expr::Property(..) | Expr::Literal(_) => {}
            Expr::List(items) | Expr::Function { args: items, .. } => {
                for item in items {
                    item.collect_rel_patterns(out);
                }
            }
            Expr::Not(e) | Expr::IsNull(e, _) => e.collect_rel_patterns(out),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
                a.collect_rel_patterns(out);
                b.collect_rel_patterns(out);
            }
            Expr::Exists {
                patterns,
                where_clause,
            } => {
                for pattern in patterns {
                    pattern.collect_rel_patterns(out);
                }
                if let Some(w) = where_clause {
                    w.collect_rel_patterns(out);
                }
            }
            Expr::Case {
                operand,
                branches,
                default,
            } => {
                for e in operand.iter().chain(default.iter()) {
                    e.collect_rel_patterns(out);
                }
                for (when, then) in branches {
                    when.collect_rel_patterns(out);
                    then.collect_rel_patterns(out);
                }
            }
            Expr::MapProjection { entries, .. } => {
                for entry in entries {
                    if let MapEntry::Literal(_, e) = entry {
                        e.collect_rel_patterns(out);
                    }
                }
            }
            Expr::Aggregate { arg, .. } => {
                if let Some(e) = arg {
                    e.collect_rel_patterns(out);
                }
            }
        }
    }

    /// Whether this expression tree contains an aggregate function call.
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub fn contains_aggregate(&self) -> bool {
        match self {
            Expr::Aggregate { .. } => true,
            Expr::Not(inner) | Expr::IsNull(inner, _) => inner.contains_aggregate(),
            Expr::And(a, b) | Expr::Or(a, b) => a.contains_aggregate() || b.contains_aggregate(),
            Expr::Compare(a, _, b) => a.contains_aggregate() || b.contains_aggregate(),
            Expr::List(items) => items.iter().any(Expr::contains_aggregate),
            Expr::Function { args, .. } => args.iter().any(Expr::contains_aggregate),
            Expr::MapProjection { entries, .. } => entries.iter().any(|entry| match entry {
                MapEntry::Property(_) => false,
                MapEntry::Literal(_, expr) => expr.contains_aggregate(),
            }),
            Expr::Case {
                operand,
                branches,
                default,
            } => {
                operand.as_ref().is_some_and(|o| o.contains_aggregate())
                    || branches
                        .iter()
                        .any(|(w, t)| w.contains_aggregate() || t.contains_aggregate())
                    || default.as_ref().is_some_and(|d| d.contains_aggregate())
            }
            // An EXISTS subquery is an opaque boolean; aggregates inside it do not affect the
            // outer projection's grouping.
            Expr::Var(_) | Expr::Property(..) | Expr::Literal(_) | Expr::Exists { .. } => false,
        }
    }

    /// A human-readable name for the expression, used as a default column header.
    #[must_use]
    pub fn display_name(&self) -> String {
        match self {
            Expr::Var(v) => v.clone(),
            Expr::Property(v, p) => format!("{v}.{p}"),
            Expr::Literal(lit) => match lit {
                Literal::Str(s) => format!("'{s}'"),
                Literal::Int(i) => i.to_string(),
                Literal::Float(f) => format!("{f:?}"),
                Literal::Bool(b) => b.to_string(),
                Literal::Null => "null".to_string(),
                Literal::Param(name) => format!("${name}"),
            },
            Expr::List(items) => {
                let inner: Vec<String> = items.iter().map(Expr::display_name).collect();
                format!("[{}]", inner.join(", "))
            }
            Expr::IsNull(inner, negated) => {
                let not = if *negated { " NOT" } else { "" };
                format!("{} IS{not} NULL", inner.display_name())
            }
            Expr::Exists { .. } => "EXISTS { ... }".to_string(),
            Expr::Case { .. } => "CASE ... END".to_string(),
            Expr::MapProjection { var, .. } => format!("{var} {{ ... }}"),
            Expr::Function { name, args } => {
                let inner: Vec<String> = args.iter().map(Expr::display_name).collect();
                format!("{name}({})", inner.join(", "))
            }
            Expr::Not(inner) => format!("NOT {}", inner.display_name()),
            Expr::And(a, b) => format!("{} AND {}", a.display_name(), b.display_name()),
            Expr::Or(a, b) => format!("{} OR {}", a.display_name(), b.display_name()),
            Expr::Compare(a, op, b) => {
                format!("{} {} {}", a.display_name(), op.as_str(), b.display_name())
            }
            Expr::Aggregate {
                func,
                arg,
                distinct,
            } => {
                let inner = match arg {
                    Some(expr) => expr.display_name(),
                    None => "*".to_string(),
                };
                let distinct = if *distinct { "DISTINCT " } else { "" };
                format!("{}({distinct}{inner})", func.as_str())
            }
        }
    }
}

/// An entry in a map projection.
#[derive(Debug, Clone, PartialEq)]
pub enum MapEntry {
    /// `.prop` — take property `prop` of the projected node, under key `prop`.
    Property(String),
    /// `key: expr` — an explicit key with a computed value.
    Literal(String, Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    Contains,
    StartsWith,
    EndsWith,
    In,
}

impl CmpOp {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Neq => "<>",
            CmpOp::Lt => "<",
            CmpOp::Lte => "<=",
            CmpOp::Gt => ">",
            CmpOp::Gte => ">=",
            CmpOp::Contains => "CONTAINS",
            CmpOp::StartsWith => "STARTS WITH",
            CmpOp::EndsWith => "ENDS WITH",
            CmpOp::In => "IN",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFn {
    Count,
    Collect,
    Min,
    Max,
    Sum,
    Avg,
}

impl AggFn {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AggFn::Count => "count",
            AggFn::Collect => "collect",
            AggFn::Min => "min",
            AggFn::Max => "max",
            AggFn::Sum => "sum",
            AggFn::Avg => "avg",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderItem {
    pub expr: Expr,
    pub descending: bool,
}
