//! The refusals and warnings of the query subset (cypher-expert's design input section 1.3), as
//! [`Problem`]s: a stable `query_*` code plus checked slots (umx's own words, names that pass
//! workshop 007 rule 2, counts). No text of the query and no free text of the fork reaches a slot
//! (req-41); a hint goes in the code's message template, and `next` is always a runnable umx command.
//! The message and next templates are rows of umx-protocol's catalogue.
//!
//! Most refusals come from a pre-check before anything runs: the character where the lexer stopped,
//! the token stream, then the parsed query. What is left of a parse failure is `query_syntax` with
//! its byte position and a reason from a fixed list.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::name::is_snake_case;
use umx_core::problem::{Command, Problem, Slot};
use umx_core::secrets::SECRET_SUSPECTED;
use umx_cypher::ast::{
    Clause, Expr, Literal, MapEntry, NodePattern, PathPattern, Projection, Query, RelPattern,
};
use umx_cypher::lexer::{Token, TokenKind};

pub const SYNTAX: &str = "query_syntax";
pub const WRITE_NOT_SUPPORTED: &str = "query_write_not_supported";
pub const CLAUSE_NOT_SUPPORTED: &str = "query_clause_not_supported";
pub const FEATURE_NOT_SUPPORTED: &str = "query_feature_not_supported";
pub const AMBIGUOUS_LABELS: &str = "query_ambiguous_labels";
/// A label or relationship type spelled `std` or `std:<name>` (ws008 T2b, req-99): std names are plain in Cypher.
pub const STD_QUALIFIER: &str = "query_std_qualifier";
pub const UNKNOWN_FUNCTION: &str = "query_unknown_function";
pub const ALIAS_SHADOWS_VARIABLE: &str = "query_alias_shadows_variable";
pub const ORDER_BY_ENTITY: &str = "query_order_by_entity";
pub const WITH_ORDER_NEEDS_LIMIT: &str = "query_with_order_needs_limit";
pub const UNBOUNDED: &str = "query_unbounded";
pub const BOUND_TOO_LARGE: &str = "query_bound_too_large";
pub const SYMMETRIC_DIRECTION: &str = "query_symmetric_direction";
pub const TOO_LARGE: &str = "query_too_large";
pub const TIMED_OUT: &str = "query_timed_out";
pub const INVALID: &str = "query_invalid";
pub const TRUNCATED: &str = "query_truncated";
pub const UNKNOWN_LABEL: &str = "query_unknown_label";
pub const UNKNOWN_RELATION: &str = "query_unknown_relation";
pub const UNKNOWN_PROPERTY: &str = "query_unknown_property";
pub const LABEL_UNUSED: &str = "query_label_unused";
pub const RELATION_UNUSED: &str = "query_relation_unused";
pub const PROPERTY_UNUSED: &str = "query_property_unused";
pub const PROVENANCE_HIDDEN: &str = "provenance_hidden";
pub const SYMMETRIC_IN_DIRECTED_STEP: &str = "query_symmetric_in_directed_step";
/// A `$name` the query reads that no `--param` gives.
pub const PARAM_MISSING: &str = "query_param_missing";
/// A variable read before any pattern or clause binds it.
pub const UNKNOWN_VARIABLE: &str = "query_unknown_variable";
/// A column named like the key umx's secret flags take (F241).
pub const COLUMN_RESERVED: &str = "query_column_reserved";

/// The upper bound a variable-length step may have, and on a similar_to step.
pub const MAX_HOPS: u32 = 10;
pub const MAX_SIMILAR_HOPS: u32 = 2;

/// The relation followed only when named (workshop 008 ruling 8).
pub(crate) const SIMILAR_TO: &str = "similar_to";

// The fixed reasons of query_syntax. Every slot word is rendered by `Fill::text`, which keeps letters, digits and
// ` ._/+~:=<>()_-` only; the code test fails on any other character.
pub const UNEXPECTED_CHARACTER: &str = "an unexpected character";
pub const UNCLOSED_STRING: &str = "a string that is not closed";
pub const BAD_NUMBER: &str = "a number that cannot be read";
pub const NOT_EQUAL: &str = "use <> for not-equal";
pub const BACKTICKS: &str = "names are snake_case (backticks are not needed)";
pub const SNAKE_CASE: &str = "names are snake_case";
pub const NESTING: &str = "nesting deeper than 64 levels";
pub const DOES_NOT_PARSE: &str = "the query does not continue as the grammar of the subset expects";
pub const STARTS_WITH: &str = "a query starts with MATCH or OPTIONAL MATCH or UNWIND or WITH";
/// query_invalid's reason for two columns with one name.
pub const SAME_COLUMN: &str = "two columns have the same name (give each its own alias)";
pub const PARAM_NAME: &str = "a parameter is a dollar sign then its name";

pub(crate) fn syntax(position: usize, reason: &'static str) -> Problem {
    Problem::new(SYNTAX)
        .with("position", Slot::Count(position as u64))
        .with("reason", Slot::Word(reason))
}

/// A feature outside the subset: a fixed hint naming the umx verb to use instead (umx path for paths), and a next
/// that runs as written (the docs topic for that verb, else the query docs).
fn feature(what: &'static str) -> Problem {
    let (instead, topic) = match what {
        "path variables" | "shortest path" => (
            "For a path between two nodes use umx path <from> <to>.",
            "explore",
        ),
        _ => ("See the subset in umx docs get query.", "query"),
    };
    Problem::new(FEATURE_NOT_SUPPORTED)
        .with("feature", Slot::Word(what))
        .with("instead", Slot::Word(instead))
        .with("fix", Slot::Command(Command::new("docs get").word(topic)))
}

/// A name from the query in a slot only if it passes rule 2; otherwise umx's word "a name".
pub fn name_slot(name: &str) -> Slot {
    Slot::name_or(name, "a name")
}

/// `query_std_qualifier`, naming the plain name to write when the query gave one after the qualifier.
fn std_qualifier(plain: Option<&str>) -> Problem {
    Problem::new(STD_QUALIFIER).with("name", plain.map_or(Slot::Word("a name"), name_slot))
}

/// Whether a label or relationship type is the std qualifier: `std` itself or `std:<name>`.
fn is_std(name: &str) -> bool {
    name == umx_core::standard::STD || name.starts_with(umx_core::standard::QUALIFIER)
}

/// The lexer stopped at `position`: classified by the character there, never echoed.
pub(crate) fn lex_failure(text: &str, position: usize, message: &str) -> Problem {
    let at = text.get(position..).and_then(|t| t.chars().next());
    // n:`std:person`: a backticked label or type after its `:` (the lexer has no backticks, so this never parsed).
    if at == Some('`')
        && text[..position].trim_end().ends_with(':')
        && let Some(quoted) = text[position + 1..].split('`').next()
        && let Some(rest) = quoted
            .strip_prefix(umx_core::standard::QUALIFIER)
            .or_else(|| (quoted == umx_core::standard::STD).then_some(""))
    {
        return std_qualifier((!rest.is_empty()).then_some(rest));
    }
    match at {
        Some('`') => syntax(position, BACKTICKS),
        Some('!') => syntax(position, NOT_EQUAL),
        Some('+' | '/' | '%') => feature("arithmetic"),
        Some('~') => feature("regular expressions (=~)"),
        Some('$') => syntax(position, PARAM_NAME),
        _ if message.starts_with("unterminated") => syntax(position, UNCLOSED_STRING),
        Some(c) if c.is_ascii_digit() => syntax(position, BAD_NUMBER),
        _ => syntax(position, UNEXPECTED_CHARACTER),
    }
}

/// The parser stopped at `position` with one of its own messages: nesting first, then what the tokens show
/// (a write, a clause or a feature outside the subset), else a syntax error at that byte.
pub(crate) fn parse_failure(position: usize, message: &str, tokens: &[Token]) -> Problem {
    if message == umx_cypher::NESTING_TOO_DEEP {
        return syntax(position, NESTING);
    }
    if message.starts_with("variable-length bound is too large") {
        return Problem::new(BOUND_TOO_LARGE).with("max", Slot::Count(u64::from(MAX_HOPS)));
    }
    // A RETURN-only query (cypher-expert F4): LadybugDB answers it; the subset starts with a reading clause.
    if ident(tokens.first()).is_some_and(|w| w.eq_ignore_ascii_case("RETURN")) {
        return syntax(0, STARTS_WITH);
    }
    self::tokens(tokens).unwrap_or_else(|| syntax(position, DOES_NOT_PARSE))
}

fn ident(t: Option<&Token>) -> Option<&str> {
    match t.map(|t| &t.kind) {
        Some(TokenKind::Ident(s)) => Some(s),
        _ => None,
    }
}

fn is(t: Option<&Token>, word: &str) -> bool {
    ident(t).is_some_and(|s| s.eq_ignore_ascii_case(word))
}

fn kind(t: Option<&Token>) -> Option<&TokenKind> {
    t.map(|t| &t.kind)
}

const WRITES: [(&str, &str); 6] = [
    ("CREATE", "CREATE"),
    ("MERGE", "MERGE"),
    ("SET", "SET"),
    ("DELETE", "DELETE"),
    ("DETACH", "DETACH DELETE"),
    ("REMOVE", "REMOVE"),
];
const CLAUSES: [&str; 10] = [
    "CALL", "LOAD", "COPY", "ATTACH", "INSTALL", "EXPORT", "IMPORT", "USE", "DROP", "ALTER",
];
const DDL_AFTER_CREATE: [&str; 6] = ["NODE", "REL", "TABLE", "INDEX", "MACRO", "SEQUENCE"];
const KEYWORDS: [&str; 14] = [
    "RETURN", "WITH", "WHERE", "AND", "OR", "NOT", "IN", "WHEN", "THEN", "ELSE", "DISTINCT", "BY",
    "SKIP", "LIMIT",
];

/// Why a query that did not parse was refused, from its tokens: a write, a clause outside the subset, or a
/// feature the grammar does not have. Run only on a parse failure, so a keyword used as a name is never refused.
fn tokens(tokens: &[Token]) -> Option<Problem> {
    let at = |i: usize| tokens.get(i);
    if let Some(p) = std_in_tokens(tokens) {
        return Some(p);
    }
    // Shortest path first: it is often written with a path variable too (`p = shortestPath(...)`).
    if tokens.iter().any(|t| {
        matches!(&t.kind, TokenKind::Ident(w)
            if ["SHORTESTPATH", "ALLSHORTESTPATHS", "SHORTEST"].iter().any(|s| w.eq_ignore_ascii_case(s)))
    }) {
        return Some(feature("shortest path"));
    }
    // `[` opening a relationship pattern (after `-` or `<-`), as opposed to a list literal.
    let mut rel_brackets: Vec<bool> = Vec::new();
    for i in 0..tokens.len() {
        let prev = i.checked_sub(1).and_then(at);
        let next = at(i + 1);
        // A keyword in clause position: not a property (`n.set`), map key (`{set: 1}`) or label.
        let clause_word = !matches!(
            kind(prev),
            Some(TokenKind::Dot | TokenKind::Colon | TokenKind::Pipe)
        ) && !matches!(kind(next), Some(TokenKind::Colon));
        match &tokens[i].kind {
            TokenKind::Ident(word) if clause_word => {
                let upper = word.to_ascii_uppercase();
                if upper == "CREATE"
                    && ident(next)
                        .is_some_and(|n| DDL_AFTER_CREATE.iter().any(|d| n.eq_ignore_ascii_case(d)))
                {
                    return Some(
                        Problem::new(CLAUSE_NOT_SUPPORTED).with("clause", Slot::Word("DDL")),
                    );
                }
                if let Some((_, clause)) = WRITES.iter().find(|(w, _)| *w == upper) {
                    // An edge write: CREATE or MERGE of a pattern with an arrow, or DELETE, DETACH, SET or
                    // REMOVE of a variable bound inside `[...]`.
                    let target = if upper == "DETACH" {
                        ident(at(i + 2))
                    } else {
                        ident(next)
                    };
                    let relationship = if matches!(upper.as_str(), "CREATE" | "MERGE") {
                        tokens[i..]
                            .iter()
                            .any(|t| matches!(t.kind, TokenKind::Minus))
                    } else {
                        target.is_some_and(|v| {
                            tokens.windows(2).any(|w| {
                                matches!(w[0].kind, TokenKind::LBracket)
                                    && matches!(&w[1].kind, TokenKind::Ident(s) if s == v)
                            })
                        })
                    };
                    return Some(
                        Problem::new(WRITE_NOT_SUPPORTED)
                            .with("clause", Slot::Word(clause))
                            .with(
                                "topic",
                                Slot::Word(if relationship { "edge" } else { "node" }),
                            ),
                    );
                }
                if let Some(clause) = CLAUSES.iter().find(|c| **c == upper) {
                    return Some(
                        Problem::new(CLAUSE_NOT_SUPPORTED).with("clause", Slot::Word(clause)),
                    );
                }
                match upper.as_str() {
                    "UNION" => return Some(feature("UNION")),
                    "MATCH"
                        if ident(next).is_some()
                            && matches!(kind(at(i + 2)), Some(TokenKind::Eq)) =>
                    {
                        return Some(feature("path variables"));
                    }
                    "NODES" | "LENGTH" | "RELATIONSHIPS"
                        if matches!(kind(next), Some(TokenKind::LParen)) =>
                    {
                        return Some(feature("path variables"));
                    }
                    "ALL" | "ANY" | "NONE" | "SINGLE"
                        if matches!(kind(next), Some(TokenKind::LParen))
                            && ident(at(i + 2)).is_some()
                            && is(at(i + 3), "IN") =>
                    {
                        return Some(feature("list predicates (all or any or none or single)"));
                    }
                    _ => {}
                }
            }
            TokenKind::LBracket => {
                let rel = matches!(kind(prev), Some(TokenKind::Minus));
                rel_brackets.push(rel);
                if !rel && ident(next).is_some() && is(at(i + 2), "IN") {
                    return Some(feature("list comprehensions"));
                }
            }
            TokenKind::RBracket => {
                rel_brackets.pop();
            }
            TokenKind::Star => {
                let in_rel = rel_brackets.last().copied().unwrap_or(false);
                let projection_star =
                    is(prev, "RETURN") || is(prev, "WITH") || is(prev, "DISTINCT");
                let count_star = matches!(kind(prev), Some(TokenKind::LParen));
                if !in_rel && !projection_star && !count_star {
                    return Some(feature("arithmetic"));
                }
            }
            TokenKind::Minus => {
                // `n.x - 1`, `3 - 1`: a minus after a value, not after an arrow end or an operator.
                let after_value = matches!(
                    kind(prev),
                    Some(TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::Str(_))
                ) || (ident(prev)
                    .is_some_and(|w| !KEYWORDS.iter().any(|k| w.eq_ignore_ascii_case(k)))
                    && matches!(kind(i.checked_sub(2).and_then(at)), Some(TokenKind::Dot)));
                if after_value {
                    return Some(feature("arithmetic"));
                }
            }
            TokenKind::LParen => {
                // `(n:a:b)`: two labels joined by `:` mean AND in openCypher and OR in LadybugDB.
                let (mut j, mut names) = (i + 1, Vec::new());
                if ident(at(j)).is_some() && matches!(kind(at(j + 1)), Some(TokenKind::Colon)) {
                    j += 1;
                }
                while matches!(kind(at(j)), Some(TokenKind::Colon)) {
                    match ident(at(j + 1)) {
                        Some(label) => names.push(label),
                        None => break,
                    }
                    j += 2;
                }
                if names.len() >= 2 {
                    return Some(
                        Problem::new(AMBIGUOUS_LABELS)
                            .with("a", name_slot(names[0]))
                            .with("b", name_slot(names[1])),
                    );
                }
            }
            _ => {}
        }
    }
    None
}

/// A query that did not parse, with `std` as a label or relationship type (cypher-expert, ws008 T2b): in a node
/// pattern's label list (`(n:std:person)`), a relationship pattern's type list (`-[:std:member_of]-`), or a label
/// predicate after WHERE, AND, OR or NOT (`WHERE n:std:person`). Only those places: `std` as a variable, a map key
/// or value, or in a list comprehension is a name like any other. Run only on a parse failure.
fn std_in_tokens(tokens: &[Token]) -> Option<Problem> {
    let at = |i: usize| tokens.get(i);
    for (i, t) in tokens.iter().enumerate() {
        let opens = match &t.kind {
            TokenKind::LParen | TokenKind::LBracket => true,
            TokenKind::Ident(w) => ["WHERE", "AND", "OR", "NOT"]
                .iter()
                .any(|k| w.eq_ignore_ascii_case(k)),
            _ => false,
        };
        if !opens {
            continue;
        }
        // An optional variable, then `:` and names joined by `:` or `|`.
        let mut j = i + 1;
        if ident(at(j)).is_some() && matches!(kind(at(j + 1)), Some(TokenKind::Colon)) {
            j += 1;
        } else if !matches!(kind(at(j)), Some(TokenKind::Colon)) {
            continue;
        }
        while matches!(kind(at(j)), Some(TokenKind::Colon | TokenKind::Pipe)) {
            let Some(name) = ident(at(j + 1)) else {
                break;
            };
            if name == umx_core::standard::STD {
                let plain = matches!(kind(at(j + 2)), Some(TokenKind::Colon))
                    .then(|| ident(at(j + 3)))
                    .flatten();
                return Some(std_qualifier(plain));
            }
            j += 2;
        }
    }
    None
}

// ---- Variables in scope --------------------------------------------------------------------------------

/// A variable the query reads before a pattern, UNWIND or WITH binds it (cypher-expert F1): it used to read as
/// null, so `RETURN nope.id` gave a null per row and `count(zz)` gave 0, where LadybugDB refuses. Scope follows the
/// clauses: a MATCH binds its patterns' variables for its maps and WHERE; UNWIND binds its variable; WITH keeps only
/// what it projects (its ORDER BY also sees what came before); EXISTS sees the outer scope plus its own patterns.
pub(crate) fn unknown_variable(query: &Query) -> Option<Problem> {
    let mut scope: BTreeSet<&str> = BTreeSet::new();
    for clause in &query.clauses {
        match clause {
            Clause::Match(m) => {
                let bound = with_patterns(&scope, &m.patterns);
                if !patterns_bound(&m.patterns, &bound)
                    || m.where_clause
                        .as_ref()
                        .is_some_and(|w| !bound_expr(w, &bound))
                {
                    return Some(Problem::new(UNKNOWN_VARIABLE));
                }
                scope = bound;
            }
            Clause::Unwind(u) => {
                if !bound_expr(&u.expr, &scope) {
                    return Some(Problem::new(UNKNOWN_VARIABLE));
                }
                scope.insert(u.var.as_str());
            }
            Clause::With(w) => {
                let next = match projected(&w.projection, &scope) {
                    Ok(next) => next,
                    Err(p) => return Some(p),
                };
                if w.where_clause
                    .as_ref()
                    .is_some_and(|e| !bound_expr(e, &next))
                {
                    return Some(Problem::new(UNKNOWN_VARIABLE));
                }
                scope = next;
            }
        }
    }
    projected(&query.result, &scope).err()
}

/// F241 (cypher-expert R1): a final column named [`SECRET_SUSPECTED`], the key umx's secret flags take on each row, is
/// refused before any graph work: the row's flags would replace the column. Matched exactly, case and all, on the
/// columns the query returns: an alias, a bare variable (`WITH 1 AS secret_suspected RETURN secret_suspected`), and
/// every variable `RETURN *` returns. Run after [`unknown_variable`], so every projection here is bound.
pub(crate) fn reserved_column(query: &Query) -> Option<Problem> {
    let reserved = if query.result.star {
        let mut scope: BTreeSet<&str> = BTreeSet::new();
        for clause in &query.clauses {
            match clause {
                Clause::Match(m) => scope = with_patterns(&scope, &m.patterns),
                Clause::Unwind(u) => {
                    scope.insert(u.var.as_str());
                }
                Clause::With(w) => scope = projected(&w.projection, &scope).unwrap_or_default(),
            }
        }
        scope.contains(SECRET_SUSPECTED)
    } else {
        query
            .result
            .items
            .iter()
            .any(|item| item.column_name() == SECRET_SUSPECTED)
    };
    reserved.then(|| Problem::new(COLUMN_RESERVED).with("name", name_slot(SECRET_SUSPECTED)))
}

/// The names a projection leaves in scope, or the refusal when an item or its ORDER BY reads an unbound name.
fn projected<'q>(
    p: &'q Projection,
    scope: &BTreeSet<&'q str>,
) -> Result<BTreeSet<&'q str>, Problem> {
    if p.items.iter().any(|i| !bound_expr(&i.expr, scope)) {
        return Err(Problem::new(UNKNOWN_VARIABLE));
    }
    let mut next: BTreeSet<&str> = if p.star {
        scope.clone()
    } else {
        BTreeSet::new()
    };
    for item in &p.items {
        match (&item.alias, &item.expr) {
            (Some(a), _) => {
                next.insert(a.as_str());
            }
            (None, Expr::Var(v)) => {
                next.insert(v.as_str());
            }
            _ => {}
        }
    }
    let mut ordering = scope.clone();
    ordering.extend(next.iter().copied());
    if p.order_by.iter().any(|o| !bound_expr(&o.expr, &ordering)) {
        return Err(Problem::new(UNKNOWN_VARIABLE));
    }
    Ok(next)
}

fn with_patterns<'q>(scope: &BTreeSet<&'q str>, patterns: &'q [PathPattern]) -> BTreeSet<&'q str> {
    let mut bound = scope.clone();
    for p in patterns {
        let nodes = core::iter::once(&p.start).chain(p.rest.iter().map(|(_, n)| n));
        bound.extend(nodes.filter_map(|n| n.var.as_deref()));
        bound.extend(p.rest.iter().filter_map(|(r, _)| r.var.as_deref()));
    }
    bound
}

fn patterns_bound(patterns: &[PathPattern], bound: &BTreeSet<&str>) -> bool {
    patterns.iter().all(|p| {
        core::iter::once(&p.start)
            .chain(p.rest.iter().map(|(_, n)| n))
            .all(|n| n.props.iter().all(|(_, e)| bound_expr(e, bound)))
    })
}

/// Whether every variable `e` reads is in `bound` (the parser bounds the depth of `e`).
fn bound_expr(e: &Expr, bound: &BTreeSet<&str>) -> bool {
    let all = |items: &mut dyn Iterator<Item = &Expr>| {
        let mut ok = true;
        for x in items {
            ok &= bound_expr(x, bound);
        }
        ok
    };
    match e {
        Expr::Var(v) | Expr::Property(v, _) | Expr::MapProjection { var: v, .. }
            if !bound.contains(v.as_str()) =>
        {
            false
        }
        Expr::Var(_) | Expr::Property(..) | Expr::Literal(_) => true,
        Expr::List(items) | Expr::Function { args: items, .. } => all(&mut items.iter()),
        Expr::Not(x) | Expr::IsNull(x, _) => bound_expr(x, bound),
        Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
            bound_expr(a, bound) && bound_expr(b, bound)
        }
        Expr::Exists {
            patterns,
            where_clause,
        } => {
            let inner = with_patterns(bound, patterns);
            patterns_bound(patterns, &inner)
                && where_clause
                    .as_deref()
                    .is_none_or(|w| bound_expr(w, &inner))
        }
        Expr::Case {
            operand,
            branches,
            default,
        } => all(&mut operand
            .as_deref()
            .into_iter()
            .chain(default.as_deref())
            .chain(branches.iter().flat_map(|(w, t)| [w, t]))),
        Expr::MapProjection { entries, .. } => all(&mut entries.iter().filter_map(|x| match x {
            MapEntry::Literal(_, e) => Some(e),
            MapEntry::Property(_) => None,
        })),
        Expr::Aggregate { arg, .. } => all(&mut arg.as_deref().into_iter()),
    }
}

// ---- The parsed query ------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Entity {
    Node,
    Relationship,
    Relationships,
}

/// Everything in a query the AST checks and the warnings look at, gathered in one walk.
#[derive(Default)]
pub(crate) struct Gathered<'q> {
    pub nodes: Vec<&'q NodePattern>,
    pub rels: Vec<&'q RelPattern>,
    pub exprs: Vec<&'q Expr>,
    /// Each WITH (`true`) or RETURN with the node and relationship variables in scope where it runs: a name
    /// a WITH or UNWIND rebinds to a value is no longer an entity, and EXISTS variables stay inside it
    /// (reviewer R115-4).
    pub projections: Vec<(&'q Projection, bool, BTreeSet<&'q str>)>,
    scope: BTreeMap<&'q str, Entity>,
    type_hint: Option<&'static str>,
}

impl<'q> Gathered<'q> {
    pub fn of(query: &'q Query) -> Gathered<'q> {
        let mut g = Gathered::default();
        for clause in &query.clauses {
            match clause {
                Clause::Match(m) => {
                    g.bind_patterns(&m.patterns);
                    for p in &m.patterns {
                        g.pattern(p);
                    }
                    if let Some(w) = &m.where_clause {
                        g.expr(w);
                    }
                }
                Clause::Unwind(u) => {
                    g.expr(&u.expr);
                    g.scope.remove(u.var.as_str());
                }
                Clause::With(w) => {
                    g.projection(&w.projection, true);
                    let before = g.scope.clone();
                    if !w.projection.star {
                        g.scope.clear();
                    }
                    for item in &w.projection.items {
                        let name = item.alias.as_deref().or(match &item.expr {
                            Expr::Var(v) => Some(v.as_str()),
                            _ => None,
                        });
                        if let Some(name) = name {
                            if let Expr::Var(v) = &item.expr
                                && let Some(entity) = before.get(v.as_str())
                            {
                                g.scope.insert(name, *entity);
                            } else {
                                g.scope.remove(name);
                            }
                        }
                    }
                    if let Some(e) = &w.where_clause {
                        g.expr(e);
                    }
                }
            }
        }
        g.projection(&query.result, false);
        g
    }

    fn bind_patterns(&mut self, patterns: &'q [PathPattern]) {
        for p in patterns {
            for n in core::iter::once(&p.start).chain(p.rest.iter().map(|(_, n)| n)) {
                if let Some(v) = n.var.as_deref() {
                    self.scope.entry(v).or_insert(Entity::Node);
                }
            }
            for (r, _) in &p.rest {
                if let Some(v) = r.var.as_deref() {
                    self.scope.entry(v).or_insert(if r.length.is_some() {
                        Entity::Relationships
                    } else {
                        Entity::Relationship
                    });
                }
            }
        }
    }

    fn pattern(&mut self, p: &'q PathPattern) {
        self.node(&p.start);
        for (rel, node) in &p.rest {
            self.rels.push(rel);
            self.node(node);
        }
    }

    fn node(&mut self, n: &'q NodePattern) {
        self.nodes.push(n);
        for (_, e) in &n.props {
            self.expr(e);
        }
    }

    fn projection(&mut self, p: &'q Projection, with: bool) {
        self.projections
            .push((p, with, self.scope.keys().copied().collect()));
        for item in &p.items {
            self.expr(&item.expr);
        }
        let before = self.scope.clone();
        for item in &p.items {
            if let Some(alias) = item.alias.as_deref() {
                if let Expr::Var(v) = &item.expr
                    && let Some(entity) = before.get(v.as_str())
                {
                    self.scope.insert(alias, *entity);
                } else {
                    self.scope.remove(alias);
                }
            }
        }
        for item in &p.order_by {
            self.expr(&item.expr);
        }
        self.scope = before;
    }

    fn expr(&mut self, e: &'q Expr) {
        self.exprs.push(e);
        if let Expr::Function { name, args } = e
            && name.eq_ignore_ascii_case("type")
        {
            let hint = match args.as_slice() {
                [Expr::Literal(Literal::Null)] => None,
                [Expr::Var(v)] => match self.scope.get(v.as_str()) {
                    Some(Entity::Relationship) => None,
                    Some(Entity::Relationships) => {
                        Some("use label(r) for the relation of an edge per hop")
                    }
                    Some(Entity::Node) => Some("use label(n) for the type of a node"),
                    None => Some("use type(r) with one single-hop relationship variable"),
                },
                _ => Some("use type(r) with one single-hop relationship variable"),
            };
            self.type_hint = self.type_hint.or(hint);
        }
        match e {
            Expr::Var(_) | Expr::Property(..) | Expr::Literal(_) => {}
            Expr::List(items) | Expr::Function { args: items, .. } => {
                items.iter().for_each(|x| self.expr(x))
            }
            Expr::Not(x) | Expr::IsNull(x, _) => self.expr(x),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Exists {
                patterns,
                where_clause,
            } => {
                let before = self.scope.clone();
                self.bind_patterns(patterns);
                for p in patterns {
                    self.pattern(p);
                }
                if let Some(w) = where_clause {
                    self.expr(w);
                }
                self.scope = before;
            }
            Expr::Case {
                operand,
                branches,
                default,
            } => {
                for x in operand.iter().chain(default.iter()) {
                    self.expr(x);
                }
                for (w, t) in branches {
                    self.expr(w);
                    self.expr(t);
                }
            }
            Expr::MapProjection { entries, .. } => {
                for entry in entries {
                    if let MapEntry::Literal(_, x) = entry {
                        self.expr(x);
                    }
                }
            }
            Expr::Aggregate { arg, .. } => {
                if let Some(x) = arg {
                    self.expr(x);
                }
            }
        }
    }

    /// Every property name the query reads.
    pub fn properties(&self) -> BTreeSet<&'q str> {
        let mut out = BTreeSet::new();
        for n in &self.nodes {
            out.extend(n.props.iter().map(|(k, _)| k.as_str()));
        }
        for e in &self.exprs {
            match e {
                Expr::Property(_, p) => {
                    out.insert(p.as_str());
                }
                Expr::MapProjection { entries, .. } => {
                    for entry in entries {
                        if let MapEntry::Property(p) = entry {
                            out.insert(p.as_str());
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// Where `name` first appears as a token, for a syntax position.
fn position_of(tokens: &[Token], name: &str) -> usize {
    tokens
        .iter()
        .find(|t| matches!(&t.kind, TokenKind::Ident(s) if s == name))
        .map_or(0, |t| t.position)
}

/// The executor's own functions; the date functions are [`crate::dates::NAMES`], which the provider answers.
const FUNCTIONS: [&str; 6] = ["tolower", "toupper", "size", "coalesce", "label", "type"];

/// The refusals the parsed query shows, in a fixed order.
pub(crate) fn query(
    g: &Gathered<'_>,
    tokens: &[Token],
    symmetric: &dyn Fn(&str) -> bool,
) -> Option<Problem> {
    // std names are plain in Cypher (ws008 T2b): `(n:std|person)` would widen to every node. WHERE label predicates
    // (`n:std`) are not in the subset; if they join it, check their labels here too.
    for names in g
        .nodes
        .iter()
        .map(|n| &n.labels)
        .chain(g.rels.iter().map(|r| &r.types))
    {
        if let Some(i) = names.iter().position(|n| is_std(n)) {
            let plain = names[i]
                .strip_prefix(umx_core::standard::QUALIFIER)
                .or_else(|| names.get(i + 1).map(String::as_str));
            return Some(std_qualifier(plain));
        }
    }
    // Names of the graph's vocabulary are snake_case (workshop 007 rule 2).
    let vocabulary = g
        .nodes
        .iter()
        .flat_map(|n| n.labels.iter())
        .chain(g.rels.iter().flat_map(|r| r.types.iter()))
        .map(String::as_str)
        .chain(g.properties());
    for name in vocabulary {
        if !is_snake_case(name) {
            return Some(syntax(position_of(tokens, name), SNAKE_CASE));
        }
    }
    for e in &g.exprs {
        if let Expr::Function { name, .. } = e {
            let lower = name.to_ascii_lowercase();
            if !FUNCTIONS.contains(&lower.as_str())
                && !crate::dates::NAMES.contains(&lower.as_str())
            {
                let hint = match lower.as_str() {
                    "labels" => "use label(n) for the type of a node",
                    "id" => "use n.id for the id of a node",
                    "now" | "datetime" | "localdatetime" => {
                        "use current_timestamp() or current_date() for now and timestamp(x) for a date-time"
                    }
                    "datediff" | "date_sub" | "age" => {
                        "use date_diff with a unit and two dates for the gap between them"
                    }
                    _ => "that function is not in the query subset",
                };
                return Some(Problem::new(UNKNOWN_FUNCTION).with("hint", Slot::Word(hint)));
            }
        }
    }
    if let Some(hint) = g.type_hint {
        return Some(Problem::new(UNKNOWN_FUNCTION).with("hint", Slot::Word(hint)));
    }
    for rel in &g.rels {
        if let Some(length) = &rel.length {
            if rel
                .types
                .iter()
                .any(|kind| matches!(kind.as_str(), "authored_by" | "edited_by"))
            {
                return Some(feature("variable-length provenance relations"));
            }
            let cap = if rel.types.iter().any(|t| t == SIMILAR_TO) {
                MAX_SIMILAR_HOPS
            } else {
                MAX_HOPS
            };
            match length.max {
                None => {
                    return Some(Problem::new(UNBOUNDED).with("max", Slot::Count(u64::from(cap))));
                }
                Some(max) if max > cap => {
                    return Some(
                        Problem::new(BOUND_TOO_LARGE).with("max", Slot::Count(u64::from(cap))),
                    );
                }
                _ => {}
            }
            if rel.var.is_some() {
                return Some(feature("a relationship variable on a variable-length step"));
            }
        }
        if rel.direction != umx_cypher::ast::Direction::Both
            && let Some(r) = rel.types.iter().find(|t| symmetric(t))
        {
            return Some(Problem::new(SYMMETRIC_DIRECTION).with("relation", name_slot(r)));
        }
    }
    for (p, with, entities) in &g.projections {
        // Rows are objects keyed by column: two columns with one name would lose one.
        let names = p.column_names();
        if names
            .iter()
            .enumerate()
            .any(|(i, n)| names[..i].contains(n))
        {
            return Some(Problem::new(INVALID).with("reason", Slot::Word(SAME_COLUMN)));
        }
        for item in &p.items {
            if let Some(alias) = &item.alias
                && entities.contains(alias.as_str())
                && item.expr != Expr::Var(alias.clone())
            {
                return Some(Problem::new(ALIAS_SHADOWS_VARIABLE));
            }
        }
        for order in &p.order_by {
            if let Expr::Var(v) = &order.expr {
                let column = p.items.iter().find(|i| i.column_name() == *v);
                let entity = match column {
                    Some(item) => {
                        matches!(&item.expr, Expr::Var(x) if entities.contains(x.as_str()))
                    }
                    None => entities.contains(v.as_str()),
                };
                if entity {
                    return Some(Problem::new(ORDER_BY_ENTITY));
                }
            }
        }
        if *with && !p.order_by.is_empty() && p.skip.is_none() && p.limit.is_none() {
            return Some(Problem::new(WITH_ORDER_NEEDS_LIMIT).with("max_rows", Slot::Count(1000)));
        }
    }
    None
}

/// An executor message that is not a refusal above, as a reason from a fixed list.
pub(crate) fn invalid(message: &str) -> Problem {
    let reason = if message == umx_cypher::VALUE_TOO_DEEP {
        "a list or map is nested too deep (at most 64 levels)"
    } else if message.starts_with("UNWIND expects") {
        "UNWIND needs a list"
    } else if message.starts_with("aggregate functions are only allowed") {
        "an aggregate is allowed only in WITH or RETURN"
    } else if message.starts_with("ORDER BY expression") {
        "ORDER BY must name a returned column or a property of a returned variable"
    } else if message.contains("requires at least one variable in scope") {
        "RETURN * needs a variable in scope"
    } else if message.contains("expects exactly")
        || message.contains("requires at least one argument")
    {
        "a function got the wrong number of arguments"
    } else if message.contains("has no unit") {
        "a date function got a unit it does not have (umx docs get time lists them)"
    } else if message.contains("expects a") {
        "a function got a value of the wrong kind"
    } else {
        "the query cannot run as written"
    };
    Problem::new(INVALID).with("reason", Slot::Word(reason))
}

/// `true` when `name` is a field every node or every edge has, or a similar_to edge's `score`.
pub(crate) fn standard_field(name: &str) -> bool {
    matches!(
        name,
        "id" | "type"
            | "address"
            | "graph"
            | "title"
            | "status"
            | "note"
            | "evidence"
            | "evidence_kinds"
            | "status_history"
            | "created"
            | "updated"
            | "created_by"
            | "authored_by_session"
            | "edited_by_session"
            | "from"
            | "to"
            | "score"
            | "derived"
    ) || (name != "locator" && umx_core::locator::LOCATOR_NAMES.contains(&name))
}
