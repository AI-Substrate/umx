# Vendored: cypher-parser 0.8.1

| | |
|---|---|
| Upstream | cypher-parser 0.8.1, <https://github.com/Shopify/cypher-parser> (git a7b822fbece9ee2c3f2b57ecfd4e90a2ca215383, from the crate's `.cargo_vcs_info.json`) |
| Source of truth | the crates.io tarball `cypher-parser-0.8.1.crate`, sha256 `6ada60f869dbe5818d0ee40b06ad988627cc0d692d1ed42d692353bf0af8ddb6` |
| Licence | MIT, "Copyright (c) 2025-present, Shopify Inc." (`LICENSE.md`, kept verbatim) |
| Vendored | 2026-11-10, workstream 005 s01 (plan 041), by the ws005 coder |
| Why | Workshop 006 (the Cypher spike): umx runs a read-only openCypher subset over its own graph (D18, no LadybugDB in v1). 0.8.1 has no dependencies and a clean `GraphProvider` seam, but gives silent wrong answers (relationship variables, path semantics, parallel edges, floats) that umx must fix, and it needs `no_std` for umx's pure core. A fork is the only way to do both. |

Not copied: `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `README.md`, upstream's `Cargo.lock`, `.github/`. Fixes may be
offered upstream later; that needs Shopify's CLA, and is the project's decision, never a blocker.

## Reading the fork

The first commit that adds this crate is the pristine copy: `src/`, `tests/` and `LICENSE.md` byte-identical to the
tarball, with only the manifest ours. So the whole fork is one diff:

```sh
first=$(git log --diff-filter=A --format=%h -- crates/umx-cypher/LICENSE.md | tail -1)
git diff "$first" -- crates/umx-cypher/src crates/umx-cypher/tests
```

Every change after that is its own commit and gets a row below.

## Changes

| # | Change | PR |
|---|---|---|
| 1 | Package renamed `umx-cypher`; `license = "MIT"` stated in the crate, not taken from the workspace. Library renamed `cypher_parser` to `umx_cypher` in a second commit (upstream's tests and doc example follow). | ws005 s01 |
| 2 | `#![no_std]` + `alloc`: `Rc` from `alloc`, `Cell`/`RefCell`/`fmt` from `core`, `core::error::Error`. `HashMap`/`HashSet` become `BTreeMap`/`BTreeSet` (no new dependency), so `GraphProvider::NodeId` is `Copy + Ord` (was `Copy + Eq + Hash`) and `CypherValue` also derives `PartialOrd + Ord` (structural; only a set and map key, `ORDER BY` still uses `total_cmp`). Behaviour unchanged: upstream already emitted rows in insertion order and used the hash collections only for membership and lookup. Upstream's 73 tests and doc test pass unchanged. | ws005 s01 |
| 3 | Added `fixtures/design` (cypher-expert's design graph and `queries.json`) and `tests/design.rs`: the 0.8.1 baseline against LadybugDB 0.22.3's rows, and a determinism test. `tests/rows` compares cells (floats within 1e-9), shared with tools/lbug-conformance. | ws005 s01 |
| 4 | Walk semantics (R-Q3): variable length enumerates walks (no shared visited set); no per-step target dedupe, so parallel edges are separate rows; undirected is outgoing then incoming, so a self-loop is found twice. An open upper bound is capped at `OPEN_UPPER_BOUND` (10). | ws005 s02 |
| 5 | Relationship variables (R-Q2): `GraphProvider` gains `EdgeId`, `expand`/`expand_in` return `(edge, other end)` and `expand_in` is required (the `rel_sources` reverse-build fallback and upstream's `tests/expand_in.rs`, which tested only that fallback, are gone); `edge_property`, `edge_label`, `edge_id`; `label(x)`; `CypherValue::Relationship`. Upstream's test providers adapted. | ws005 s02 |
| 6 | Floats (R-Q6): float literals (`1..3` stays a range), `CypherValue::Float(Float)` with total-order `Eq`/`Ord`/`Hash`, numeric comparison across integer and float, `sum` float once a float is summed, `avg` always a float. | ws005 s02 |
| 7 | One provider call per hop: `GraphProvider::expand_types(node, types, incoming)` (default: per type), so an untyped step is one pass over a node's adjacency. | ws005 s02 |
| 8 | Id pushdown: `GraphProvider::indexed`/`lookup`; a property-map or pushed-down `WHERE v.p = <literal>` on an indexed property replaces the scan, and the planner starts a path at such an endpoint. | ws005 s02 |
| 9 | cypher-expert's F1: `ORDER BY` over projected variables (`WITH b ORDER BY b.id`); property-map values are expressions (`{id: x}`), so `NodePattern.props` holds `Expr` (two upstream parser assertions follow). | ws005 s02 |
| 10 | D49: `GraphProvider::is_symmetric`; a directed step naming a symmetric relation is refused (`CypherError::SymmetricDirection`); an untyped directed step crossing one warns (`ResultSet.warnings`, `Warning`). | ws005 s02 |
| 11 | `fixtures/design` is a umx graph folder with ULIDs (README has the legend); `queries.json` gains the D49 refusal probe. | ws005 s02 |

| 12 | round 1: one numeric-normalised key (an integral float and its integer share it) for a hoisted `IN` list, DISTINCT, grouping and `collect`/`count(DISTINCT)`; a property-map entry that reads a variable the same pattern binds is checked on the finished path (anonymous nodes get a hidden variable), so only values bound before the pattern starts push down. | ws005 s02 |

| 13 | Expression nesting capped at `MAX_DEPTH` (64) in the parser: parentheses, `NOT`, `AND`/`OR` chains, lists, `CASE`, `EXISTS`, function arguments, map projections; past it the syntax error "nesting too deep" (security F1: a stack overflow aborts the daemon). | ws005 s03 |
| 14 | Work budget and row cap charged inside the loops: `execute_limited(graph, query, &Limits)`, `CypherError::TooLarge { cause, used }` and `Cancelled { used }`, `ResultSet.work`, `GraphProvider::property_cost`; default 10M units and 1M rows held, cancel hook asked every 65,536 units (security F2). | ws005 s03 |
| 15 | Soft vocabulary: an unknown relationship type matches nothing instead of erroring (upstream's test follows); the caller warns. | ws005 s03 |
| 16 | Lists and maps compare through the numeric key, so `[3] = [3.0]` on every path (reviewer r2). | ws005 s03 |
| 17 | `AND`/`OR` chains bound the height of the tree they build (`Expr::height`), not only the calls still open, so a parenthesised chain inside a longer chain cannot pass `MAX_DEPTH` (reviewer R115-1). | ws005 s03 |
| 18 | Values held by rows are shared (`Binding::Value(Rc<CypherValue>)`), so a row clone is O(1); building or copying a list or map costs `ELEMENT_COST` (4) per element before it is made; a WITH without DISTINCT or ORDER BY carries no display copies; a list or map deeper than `MAX_VALUE_DEPTH` (64) is refused when built (security B1, reviewer R115-2). | ws005 s03 |
| 19 | A pushed-down predicate's stop (`TooLarge`, `Cancelled`) ends the query instead of rejecting the node; other errors still wait for the full `WHERE` (reviewer R115-3). | ws005 s03 |
| 20 | `$name` parameters: the lexer's `Param` token, `Literal::Param`, and `Query::bind_params`, which replaces each with the caller's literal before the query runs (so a parameter pushes down and uses the id index as a literal does), returning the first name left unbound. | ws005 s04 |
| 21 | `GraphProvider::edge_is_symmetric(edge)` (default: the relation's `is_symmetric`): the untyped-directed warning asks it per crossed edge, so a provider over several graphs answers from the edge's own graph (reviewer R127-1). | ws005 follow-ups |
| 22 | `bind_params` takes expressions (a literal or a list of literals), and an unaliased RETURN column that reads a parameter keeps its written name (`$x`), never the value's (reviewer R127-2). | ws005 follow-ups |
| 23 | `GraphProvider::has_relationship_type(name)` (default: `relationship_types` contains it): a typed step asks it per name on every match instead of listing every relation, which an EXISTS row did once per row (q2 at 100k, ws005 follow-ups). | ws005 follow-ups |
| 24 | Dates by instant (ws008 TM2, a umx departure from openCypher's text compare): `GraphProvider::compare_text(a, b)` (default `None`, text as before) answers two strings the provider reads as more than text; `=`, `<>`, `<`, `<=`, `>`, `>=`, `IN` (as written, and hoisted through a side list of such strings), the simple `CASE` and both property-map checks use it. A hoisted `IN` scans that side list only when the left string is such a string, and charges one work unit per item scanned (security B1). `GraphProvider::text_sort_key(s)` (default `None`) gives a string the text `ORDER BY`, `min` and `max` sort it by (then by itself), asked once per value sorted. The hook calls themselves are free; DISTINCT, grouping, collect keys and lists keep identity. umx answers both from `umx_core`. | ws008 TM2 |
| 25 | A MATCH's WHERE skips the conjuncts its pushdown already proved: a conjunct of its own WHERE that reads only a variable its patterns bound evaluated truthy for that variable's binding in every row left, so it is taken as true by address while the rest run in the written order with `AND`'s short-circuit (`run_match`, `holds`). Any ordinary error in a pushed evaluation clears `pushdown_exact`, and the whole WHERE runs as before. OPTIONAL MATCH, EXISTS and WITH ... WHERE are unchanged. Rows, order and errors are the same, and the work falls (review A4). | ws003 s22 |
| 26 | Property names resolved once per query: `PropKey`, and `GraphProvider::property_key(name)` (default `None`), `property_at(node, key)` (default null, never reached) and `key_cost(key)` (default 1, must equal `property_cost` of the key's name). `Executor::property`, which every node property read goes through, asks `property_key` the first time a name is read and keeps the answer for the query; a key is charged `key_cost` and read with `property_at`, a `None` keeps `property_cost` and `property`. Edge properties are unchanged. Rows, order, warnings and work are the same. | ws005 s14 |
| 27 | List membership and size from the provider's list: `GraphProvider::each_str(node, key, visit)` (default `false`) visits a node property's string items in list order without building the list. `x IN n.prop` (not hoisted) and `size(n.prop)` on a resolved node property charge `key_cost` once, in today's order, and ask `each_str`; a `false` reads the value with `property_at` and answers as before, with no second charge. Membership compares with `str_equal`, the one rule `=` uses for two strings (`values_equal`), so dates match by instant on both. Edge properties, list predicates and the hoisted `IN` are unchanged. Rows, order, errors, warnings and work are the same. | ws005 s15 |
| 28 | Provider functions: `GraphProvider::has_function(name)` (default `false`) and `function(name, args)` (default an error, never reached). `eval_function` asks `has_function` with the lower-cased name only after every built-in arm has missed, so a provider cannot shadow one; then it evaluates the arguments, charges one work unit and calls `function`, whose `Err` is an execution error. An unclaimed name keeps the unknown-function error. The executor stays free of clocks and calendars: umx answers the date functions (`date`, `timestamp`, `current_date`, `current_timestamp`, `date_part`, `date_trunc`, `date_diff`) in umx-query over `umx_core::time`. Existing queries' rows, order, errors, warnings and work are the same. | D71 step 10 |

## Baseline on the design fixture

44 conformance queries run on umx (w07 waits for s06). Of the first 35, upstream 0.8.1 answered 22 as LadybugDB 0.22.3 does and 13
not (s01: a02, a03, f01, m07, m08, m09, m10, m11, m12, m13, u01, w05, z15). Since s02 the fork answers all of them (44 at s03);
`tests/design.rs` (`KNOWN_WRONG`, now empty) fails if one comes back.
