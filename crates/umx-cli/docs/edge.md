# Edges

An edge says how two nodes relate. It is stored once, in `edges.jsonl`; incoming edges are worked out when the
graph loads.

On disk an edge's relation is stored as `type` (the record's kind, as for nodes and as Cypher's type(r)); the CLI and JSON say `relation`.

## umx edge add <from> <relation> <to>

Read the arguments as a sentence: from, relation, to. umx prints the sentence back with both titles so you can
check the direction.

    umx edge add 01JC8XA4H7 blocks 01JC8XB2KQ
    # Approval delays blocks Launch date
    umx edge add umx:launch/01JC8XA4H7 depends_on umx:launch/01JC8XC9MN
    umx edge add 01JC8XA4H7 supports 01JC8XB2KQ --set confidence=0.7

- `--set k=v`, `--set-str k=v` and `--set-json k=<json>` set fields on the edge, typed as for `umx node add`.
- The relation is snake_case. An undeclared relation is accepted with `off_vocabulary_relation`. Relations umx
  derives itself are refused (`name_invalid`): `similar_to` (from the vectors), and `authored_by` and `edited_by`
  (from node provenance). Read those with `umx neighbours <node> --relation <r>`.
- `--set` fields follow the relation's declared fields (`umx ontology add field --relation <r> <name>:<kind>`).
  An undeclared one is kept with `off_vocabulary_edge_field`, and a value that does not fit its declared kind
  is kept with `field_kind_mismatch`. An edge's own keys, such as `from`, `to_selector` or `evidence`, are
  refused with `reserved_name` (`umx docs get ontology`).
- Adding the same edge again with no fields, or with every supplied field equal to its stored typed value,
  changes nothing and warns `edge_exists`. It writes nothing and adds no stamp, unless it gives evidence the
  edge does not hold yet: then the new pointers are appended, nothing else changes, the edge's
  `edited_by_session` records the session, and it warns `evidence_appended`.
- A supplied field missing from that edge, or holding a different value, refuses with `edge_exists_differs`.
  The error names the edge id and field names, never values. Add never merges or changes an existing edge;
  there is no `--merge`. Its `next` is `umx edge edit <id> --set <field>=<value>` for the first such field:
  edit the edge in place, keeping its id.
- Identity is the relation and each end's graph id and node id, within the storing graph only. Aliases, named
  copies and full own-graph addresses resolve to those identities. Stored passage selectors are part of the
  key: a missing selector is distinct from a passage selector. Add still refuses endpoint fragments, and
  `from_selector` and `to_selector` remain reserved fields.
- `--evidence <pointer>` (repeatable) and `--evidence-kind room|chat|screen|document|analyst` record where the claim
  that this relation holds came from, exactly as on `umx node add` (`umx docs get evidence`): paths relative to the
  graph's folder, `root:<name>/...`, http(s) and `umx:` addresses, each with a `#` selector. A `umx:` source that
  names no node in a loaded open graph warns `dangling_evidence`; one in a graph that is not loaded, or is secured,
  warns `evidence_unchecked`. Evidence never changes which edge it is: the same ends and relation are one edge.
  Each new pointer records the writing session as `by_session`, as on nodes, and a pointer that looks like a
  secret warns `secret_suspected` (a batch refuses it).
- A symmetric relation (`conflicts_with`, `alternative_to`, `possibly_same_as`, `same_as`, `not_same_as`, or one marked with
  `umx ontology add relation <r> --symmetric`) is one edge both ways: it is stored with the lower graph-id/node-id
  pair as `from` (the lower node id within one graph),
  and adding it the other way round follows the same field comparison and no-op or refusal. `umx edge list --from/--to` lists edges
  as stored. If a merge leaves both directions in edges.jsonl, the load keeps the lower-id one, keeps the other
  line in the file and warns `symmetric_duplicate`; `umx check` lists it, and you settle it as a `duplicate_id`.
- `not_same_as` is written only by `umx entity deny` (it needs a reason): `edge add`, `edge edit` and `edge delete`
  of it, and a batch line, are refused with `entity_link_invalid`, whose next is `umx entity deny` or
  `umx entity undeny`. `reason` is reserved on edges.
- Either end, or both, can be in another graph. Use `--graph <author>` to choose where the edge is stored;
  that graph alone is written. A loaded alias resolves to its graph id. For an unloaded target give its graph
  id and a complete node id, not an alias or prefix; `external_end_unchecked` means it was stored unchecked.
- A loaded target must hold the node. An open author may link to a secured target, with `link_into_secured`;
  the warning carries only the supplied address. Do not copy secured text into the open graph.
- The first external end raises the author's file format to 3 in the same atomic write. Older umx builds
  refuse that graph until upgraded; removing external links never lowers the format.
- Many edges, or edges to nodes you are adding now: `umx batch` (`umx docs get batch`).
- A wrong edge: fix it with `umx edge edit` or `umx edge delete` below, never by editing edges.jsonl by hand.

To change fields on an existing edge, use `umx edge edit <edge> --set <field>=<value>` and `--unset <field>`
below: the edge keeps its id. Adding the edge again never changes its fields.

## umx edge edit <edge> --relation <relation> --reverse --set <field>=<value> --unset <field>

Retypes an edge (`--relation`), swaps its ends (`--reverse`), sets custom fields (`--set`, `--set-str`,
`--set-json`, repeatable) and removes them (`--unset`, repeatable), in any mix. Give at least one. The edge keeps its
id. Name it by its full id or address, as `umx edge list` shows it; edge ids take no prefix.

    umx edge list --from 01JC8XA4H7
    umx edge edit 01JC8XE5RT2W9V8B3N4M5K6J7H --reverse
    umx edge edit umx:launch/01JC8XE5RT2W9V8B3N4M5K6J7H --relation supports
    umx edge edit 01JC8XE5RT2W9V8B3N4M5K6J7H --relation depends_on --reverse
    umx edge edit 01JC8XE5RT2W9V8B3N4M5K6J7H --set valid_to=2026-11-30    # end a membership
    umx edge edit 01JC8XE5RT2W9V8B3N4M5K6J7H --set-str why="Named in the kickoff notes" --unset confidence

- The reply has the edge as it is now, its `sentence` (read it to check the direction) and `changed`: `type`,
  `direction`, then each field set to a new value or removed, or none when the edit changed nothing (then nothing is
  written).
- `--set` follows `edge add --set`. On a field the relation declares (`umx ontology add field --relation`) its
  raw value is read as that kind. On one it does not declare, the value is written and warns
  `off_vocabulary_edge_field`, and a raw `--set` value is guessed: a whole number is an integer, a number with a
  decimal point is a number, `true` and `false` are booleans, and anything else, `null` included, is text. To
  keep a value's exact type, use `--set-json` (JSON: `null`, a list, `"true"` as text) or `--set-str` (always
  text); copy a value from `umx edge show` with `--set-json`.
  A date or time field is read as `edge add` reads it (a time with no offset warns `time_no_offset`). A value that
  looks like a secret warns `secret_suspected` (a batch refuses it). Setting a field to the value it already holds
  changes nothing.
- With `--relation`, the fields are checked against the new relation; the ones the edit does not name that the new
  relation does not declare warn `off_vocabulary_edge_field`.
- The names `edge add --set` refuses (such as `type`, `from`, `to`, `created`, `updated`, `updated_by`, `evidence`
  and the provenance fields) are refused with `reserved_name`, except `--unset evidence` on an older edge whose own
  `evidence` field is not an evidence list (see `umx edge show`).
- An edit that changes the edge stamps it with `updated` (the time) and `updated_by`: display text naming the writer
  the CLI resolved for this command (your pij seat, else your login name). It is never the daemon's environment and
  never a `--by` label. Like all attribution it is recorded by the writer, not verified.
- The same edit replaces the edge line's `edited_by_session` with the session that made it: the id of a session
  record in the same graph, never a raw harness session id. Follow that session's `session_of` link to its agent
  (`agent/pij/<seat>` for a pij seat). The edge keeps its `authored_by_session`; an edit that changes nothing records
  nothing. See `umx docs get node`, Who wrote it, for how the writer is worked out and why it is never proof.
- The new relation follows `edge add`'s rules: snake_case, an undeclared one warns `off_vocabulary_relation`, and
  a relation umx derives (`similar_to`, `authored_by`, `edited_by`) is refused with `name_invalid`.
- An edge whose id is on more than one line of edges.jsonl (as after a git merge) can be edited: the edited line
  stays the one in use, the earlier lines are kept, and the reply warns `duplicate_id`.
- An edit that would make the edge the same as another (same from, relation and to) is refused with
  `edge_would_duplicate`, naming the other edge; nothing changes. Delete one of the two instead.
- A symmetric relation has no direction: `--reverse` on one changes nothing but the relation and warns
  `edge_reverse_symmetric`, and a retype to one stores the lower id as `from`. When that swaps the stored ends,
  `changed` holds `direction` and the hint `edge_direction_normalised` names the relation and the ends as stored.
- An edge with an end in another graph is edited and deleted by the graph that stores it, the same way (F122).
  Its far end stays the address it stores: never loaded, read or named, so the reply, its `sentence` included,
  shows that address whether or not the far node exists, is loaded or is in a secured graph. An edit never moves
  an end: `--reverse` swaps the two ends whole, and a symmetric relation puts the lower (graph id, node id) first, so
  the far end can become `from` (`edge_direction_normalised` says so).
- Refused, changing nothing: `edge_not_found` (no edge has that id), `edge_derived` (a `similar_to`, `authored_by` or
  `edited_by` edge, which umx keeps itself) and
  `edge_reserved_field_in_use` (the edge has its own `updated` or `updated_by` field from before umx reserved
  those names; rename the field in edges.jsonl first; `umx check` lists such edges).

## umx edge delete <edge>

Removes one edge by its full id or address. The reply has the edge as it was, its `sentence` and
`"deleted": true`; nothing else records the delete, so keep the reply if you need it.

    umx edge delete 01JC8XE5RT2W9V8B3N4M5K6J7H

The same refusals as `edge edit` apply: `edge_not_found`, `edge_derived`. A delete is also
refused with `edge_repeated` when the id is on more than one line of edges.jsonl: the next load would bring an
earlier line back. `umx check` lists the lines (`duplicate_id`); keep one line for the id in edges.jsonl, then
delete. Many edits and deletes at once, all or nothing: `umx batch` with `edge edit` and `edge delete` lines.

    umx edge add 01JC8XA4H7 blocks umx:ops/01JC8XB2KQ --graph launch
    umx edge add umx:ops/01JC8XB2KQ supports 01JC8XA4H7 --graph launch

    umx edge add 01JC8XA4H7 feeds 01JC8XB2KQ --evidence 'root:repo/src/x.rs#L10-L20' --evidence-kind document

To link a whole graph, use its virtual graph node, `umx:<alias>`:

    umx edge add 01JC8XA4H7 records umx:ops --graph launch

It stores the target graph id as both `to_graph` and `to`, without writing a node into ops.
`umx node get umx:ops` shows id, address, type `graph`, title and alias. The graph node is read-only and
absent from ordinary lists and counts. A same-graph target is refused; use your hierarchy root.

## umx edge show <edge>

The whole edge: the edge list row (ends, titles, sentence), its custom fields and its full `evidence` list. Give the
full edge id from `umx edge list`, or its address `umx:<graph>/<edge id>`; a prefix is not accepted
(`edge_not_found`).

    umx edge show 01JC8XE1RM4TQ7W2N5V8B3K6YP

An older edge whose own custom `evidence` field is not an evidence list keeps it as written and is never read as
evidence: the load and `edge show` warn `evidence_field_kept`, `edge show` gives the value as `evidence_field_kept`
(its `evidence` list stays empty), and `umx check` lists it. Evidence given to such an edge, by `edge add` or a batch,
is refused with `evidence_field_invalid` and nothing changes. Move the field first, then add the evidence:
`umx edge edit <edge> --set-json why=<the value> --unset evidence`, with the value as `umx edge show` gives it
(`evidence_field_kept`): `--set-json` keeps its JSON type, where a raw `--set` would guess one.

`secret_suspected` names what umx's own secret check (the one an edge write warns with) flags among the edge's
evidence and string fields, left out when nothing is: `edge:<id>/evidence:<i>` (the i-th pointer, counted from 1),
`edge:<id>/field <n>` (the n-th custom field as the edge's line holds it, counted from 1, never its name, where a
node's flags name a field by its name) and `edge:<id>/field:<name>` for a field umx names itself, by the key the
reply shows it under (a `not_same_as` edge's `reason`; an older edge's prose `evidence` as
`field:evidence_field_kept`). Only the stored fields the reply shows are flagged: a computed column (`sentence`,
`from_title`) hides a stored field of the same name. `umx check --lint` and a redact's candidates name the same places
alike. It holds names only, never a value; the value is still in the reply, so never copy a flagged value elsewhere
(`umx docs get safety`). `secret_suspected` is a reserved edge field: a write cannot set it, and a stored field of that
name is never shown.

## umx edge list

Provenance nodes and their incident edges are hidden by default. `--provenance` includes stored
provenance links and the read-only `authored_by` and `edited_by` relations from each node to its session.
Derived rows have `id: null`, `derived: true` and no address; they cannot be resolved, edited or deleted.
They stay inside one graph. The flag never enables a write or widens the graph scope. It shows provenance only
in the graphs the read names (every graph read when it names none): an edge's far end in another, unnamed graph
shows that graph's content but never its provenance, so name the far graph to see its provenance (`--graph a,b`);
otherwise that link is omitted. `umx who` and `umx node who` read attribution with no flag (`umx docs get who`).
`similar_to` still belongs to the similarity read verbs, not edge list.

    umx edge list --from 01JC8XA4H7
    umx edge list --to 01JC8XB2KQ --type blocks
    umx edge list --type part_of --limit 50
    umx edge list --from 01JC8XA4H7 --columns all
    umx edge list --from 01JC8XA4H7 --columns id,type,to,to_title,confidence

Each row has `id, type, from, to, graph, address, from_graph, to_graph, sentence, evidence_count, evidence_kinds`,
and visible ends have
`from_title` and `to_title`. `from_graph` and `to_graph` are graph ids, including local ends. An external
edge is listed at its local end; `--from` and `--to` also include links stored by other loaded graphs.
`--limit` (default 100) and `--offset` page through the rows.
`--where <field>=<value>` keeps the edges whose field holds exactly that value (repeat it for more fields; only
`=`): `umx edge list --where auto_added_by=risk_register` lists the edges umx added for that hub (`umx docs get hubs`).
`--graph a,b` or `--scope both` lists across graphs, adding `graph` and `address`, with global pagination.
See `umx docs get scope`.
`--along <spine>` with `--after <end>` and `--before <end>` keeps only the edges whose two ends the spine places in
that range (both ends inclusive), before paging; an edge with an end in another graph has an end no spine of this
graph places, so it is never in range: it is counted as not placed, though `umx graph spine show --unplaced`
lists only this graph's nodes. The reply's `along` and hints count the edges left out (one graph only;
`umx docs get time`):

    umx edge list --along day --after 01JC8XA4H7 --before 01JC8XA4H7

An edge's custom fields (set with `--set` on `edge add`, such as `confidence`) are not in the rows unless you
ask. `--columns all` adds every custom field found on the page as a column, after the usual ones, sorted by
name; a row whose edge lacks one has null. Give `all` alone: `all` beside other names is refused (`usage_invalid`). `--columns a,b,c` shows exactly those
columns, as node list does: any row column above, `created`, `created_by` or a custom field, null where the edge
lacks it. Across graphs, `graph` and `address` are always added. A custom field named like a row column above
(`to_title`, `from_state`...) is never shown: the row column wins, and is null where the row leaves it out on
purpose (a hidden end's title, a visible end's state).
Each row carries `secret_suspected` as `umx edge show` does, for the fields that row returns only (a row shows no
evidence); the column is added when some row on the page has it. Ask for it by name (`--columns
id,confidence,secret_suspected`) and every row has it, `[]` when nothing is flagged.

An unloaded or unnamed secured end has `from_state` or `to_state: "graph_not_loaded"`, with no title or
existence claim. A visible loaded end with no node is `edge_broken`, also listed by `umx check`. Incoming
links owned by an unnamed secured graph are omitted entirely. Target and owner policies are checked at
read time. Copies resolve to the primary unless the scope names another copy.
When copies are loaded but have no primary, a graph-id write refuses with `copy_ambiguous`, not
`external_end_unchecked`: name a loaded copy in the endpoint address. A visible ambiguous read end has
`from_state` or `to_state: "copy_ambiguous"` and `from_candidates` or `to_candidates` naming visible copies.
Use a scope naming the copy to resolve it. Hidden secured copies are never candidates.

`own_graph_end` on load means an explicit graph key named the author's own graph. It is local in memory;
the next write removes that redundant key while keeping the record and unknown fields. Never set
`from_graph` or `to_graph` with `--set`; use endpoint addresses.
