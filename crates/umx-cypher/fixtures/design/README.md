# Cypher design fixture

cypher-expert's design graph (workstream 005 design input, section 1.1): 11 nodes, 19 edges, synthetic. It is a
umx graph folder (`graph.json`, `nodes.jsonl`, `edges.jsonl`), so umx-graph loads it as it is. `queries.json` holds
73 queries: 53 marked `conformance` with rows measured on LadybugDB 0.22.3 (`tools/lbug-conformance`), and probes
recorded with the `umx_error` or `umx_warning` that umx must give. An alias query supplies `alias_of`, the
LadybugDB twin used to check its expected rows: `type(r)` in umx has the same rows as `label(r)` in LadybugDB.
The checked `lbug_error` probes must still reject `type(r)` and month text in `date('2025-01')`; accepting either
fails conformance rather than merely recording a changed result. A query marked `provider_functions` calls the
date functions, which umx-query's provider answers (VENDORED row 28), so umx-cypher's own test graph skips it.

Ids are ULIDs `01JDCYPHER` + zeros + the short name, so they sort as the short names do and every `ORDER BY`,
`min` and `max` expectation holds. Crockford base32 has no `O`, so options are `N1`, `N2`:

| Short | Id ends | Short | Id ends |
|---|---|---|---|
| C1 criterion | `0C1` | P1, P2 problem | `0P1`, `0P2` |
| D1 to D3 decision | `0D1` to `0D3` | Q1 to Q3 question | `0Q1` to `0Q3` |
| O1, O2 option | `0N1`, `0N2` | E01 to E19 edge | `E01` to `E19` |

`evidence_kinds` is derived from each node's `evidence` (one entry per kind), as umx does.

The fixture holds no reversed pair of a symmetric relation (D49: umx keeps one of such a pair at load; F48).
`conflicts_with` E14 is stored lower id first. The conformance harness refuses a fixture that holds one.
