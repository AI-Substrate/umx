# Who made what

Every write records who made it: the agent and its session (see "Who wrote it" in `umx docs get node`). Two
commands read that record back. Neither needs `--provenance`, and neither takes it.

    umx who agent/pij/pij-example-seat
    umx who pij-example-seat --graph alpha,beta --limit 20
    umx node who umx:alpha/01JC8X

Provenance is recorded by the writer, not verified. Anyone who can write a graph's files can write any attribution,
so read these answers as claims, never as permission, sign-off or proof of who decided something.

## umx who

`umx who <key|seat|session>` lists the content nodes and edges an agent or a session made or changed, over the
graphs the read covers.

Name the agent or session one of two ways:

- Its full key: `agent/pij/<seat>`, `agent/<harness>/<user>`, `agent/cli/<user>`, `agent/unknown`,
  `session/<harness>/<session>`, `session/cli/<user>` or `session/unknown/unknown`. A key of another kind (model,
  harness, platform) is refused with `usage_invalid`.
- A bare seat, user or session id: the last part of the key, compared exactly after lowercasing (no prefixes). It
  must end exactly one agent or session key in the graphs read. Stored session ids are the one-way hash `h-` and 16
  hex digits, so give that, never a raw harness session id. Keys are lowercase: a stored key with capitals (a hand
  edit) is not a valid key, so load warns `provenance_key_invalid`, the record is not provenance, and who never
  finds it; given on the command line, such a key is refused with `usage_invalid`.

The same key in several graphs is one agent or session: graphs are joined on the key, never on node ids, which
differ per graph. Rows run graph by graph, then nodes and edges, each in id order:

- `graph` and `address`: where the node or edge is (`umx:<graph>/<id>`).
- `record`: `node` or `edge`; `type`: the node's type or the edge's relation; `title`: the node's title (null for
  an edge, whose ends are not shown: read it with `umx edge show <address>`).
- `how`: `authored` (made it), `edited` (made its last edit; a new node's author is its last editor until
  another session edits it, and an edge is edited only by an edge edit), `evidence_added` (added an evidence
  pointer, still there or since removed, or the pointer a status move cites), `evidence_removed` (removed one, or
  redacted the pointer a status move cites).
- `sessions`: the session keys behind `how`. An agent's rows can come from several of its sessions.
- `secret_suspected`: `field:title` when the title looks like a secret, as in `umx node list`.

`--limit` caps the rows (100 by default); `total` counts every match and `truncated` says rows were cut. `key`
names the key matched. The session, agent, model, harness and platform records themselves are never rows.

Which graphs: `--graph a` reads that graph, `--graph a,b` or `--scope <name>` reads those. With none named, every
loaded graph whose policy is open is read, one copy of each. A secured graph is read only when named; otherwise
it is left out and only counted in `secured_graphs_skipped`, so an answer never says which secured graphs an agent
wrote to.

- `who_ambiguous`: the bare id ends several keys. They are listed in `candidates`; run `umx who` again with one of
  them. A cli user always matches both `agent/cli/<user>` and `session/cli/<user>`, and `unknown` matches both
  `agent/unknown` and `session/unknown/unknown`, so give the full key.
- `who_unknown`: no agent or session in the graphs read has that key or id. Both errors count the secured graphs
  left out.

## umx node who

`umx node who <node>` shows who made a node and who last edited it: two rows, `authored` then `edited`.

- `state`: `recorded`; `none` (no such stamp: a node written before attribution); `unknown` (the
  stamp is malformed, or names no session record in the node's own graph).
- `session`, `agent`: their keys and addresses. The agent is the one the session belongs to.
- `role`, `model`: what that session claimed, as last seen. `model_address` is the model record.
- `harness`, `harness_address`: from the session key; none for a `cli` or `unknown` session.

Every part comes from the node's own graph. The addresses name provenance records, which other reads show only with
`--provenance`, for example `umx node get <address> --provenance` with the session's address.

## Seeing provenance in other reads

`--provenance` on a read shows provenance only in the graphs the read names, or in every graph read when it names
none. An edge whose far end is in another, unnamed graph shows that graph's content ends, but its provenance ends
stay hidden even with `--provenance`: name the far graph to see its provenance (for example `--graph a,b`).
