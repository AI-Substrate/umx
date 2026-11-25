# Nodes

`umx node get umx:<alias>` reads the graph's virtual node: id (the graph id), address, type `graph`, title
and alias. It is not stored, and ordinary node lists, search and counts never include it.
The type `graph` is reserved; a stored node using it is kept but warns `reserved_type`.
The graph's own id is also reserved. A stored node using it is kept aside, not loaded, with
`node_id_is_graph_id`; `umx check` lists it. Fix the line to a new id or add a replacement and re-point edges.
Unrelated writes keep the original line unchanged.

A node is one thing in the graph. Every node has `id`, `type`, `title`, `status` (default `open`), `note`,
`evidence`, `status_history`, `created`, `updated` and `created_by`, plus any custom fields you set.

`node get`, `node list`, `node find` and `node history` hide provenance records by default. Add
`--provenance` to that read to include them. It does not change graph selection or enable any write.
Id prefixes and suggestions use the same visibility, so an opt-in can make a prefix ambiguous.

## umx node add --type <type> --title <title>

    umx node add --type problem --title "Approval delays"
    umx node add --type option --title "Pre-approve vendors" --status proposed --note "From the 3 Nov call"
    umx node add --type risk --title "Launch slips" --set likelihood=3 --set owner=finance
    umx node add --type option --title "Weekly review" --parent 01JC8XA4H7     # adds a part_of edge
    umx node add --type problem --title "Approval delays" --evidence 'calls/3-nov.vtt#seq=120-140' --evidence-kind room

- `--set k=v` types the value: by the field's declared kind, else `3` is int, `0.7` float, `true` bool,
  anything else a string. Only a field declared as kind `date` checks a date: a year (`2025`), a month
  (`2025-01`), a day, or a date-time stored in universal time; write the month or year the source gives, never
  an invented day (`umx docs get time`). `--set-str k=v` forces a string; `--set-json k='["a","b"]'` sets any
  value exactly.
- `--evidence <pointer>` (repeatable) and `--evidence-kind <kind>` store evidence on the new node in the same
  write, checked as for `umx node note` (see `umx docs get evidence`). A refused pointer adds no node.
- `--tag <tag>` (repeatable, or a comma list) gives a std graph's node std tags, stored lowercased, deduplicated
  and sorted (see Tags in `umx docs get ontology`).
- An undeclared type, status or field is accepted with an `off_vocabulary_*` warning whose `next` declares it.
- A title close to an existing node's adds a `near_duplicate` hint naming it and its type. The node is still
  added; if it was a duplicate, use the existing one instead. A node of the same type is near from a title score of
  0.85, a node of another type only from 0.95. The words the two titles do not share must be within a typo of
  each other (joined, split, plural and misspelled words count), and never differ in a number: "Data Science team"
  is no near duplicate of "Geoscience Data team", nor "Phase 1 plan" of "Phase 2 plan", nor "Unit A" of "Unit B"
  (a word under four letters must match exactly, or be the same letters joined or split).
- Related nodes (req-123): when an embedder may embed the graph and its model has a floor, node add embeds the new
  node's text once, after the write, and `data.related` lists up to 3 existing nodes of the same graph whose vectors
  score at or above that floor: `id`, `address`, `title` and `score` (cosine), best first. A model with a query
  prompt (`+p1`, the local model) is cut at its search floor, which `embedders.<name>.search_floor` overrides; one
  without (the Azure models) at its similar_to floor, which `embedders.<name>.similar_floor` overrides (`umx docs get
  config`). With neither a calibrated floor nor that key set, the check is skipped. `data.related_floor` gives the
  `value`, its `source` (`calibrated` or `config`) and the `use` (`search` or `similar_to`). They are close in
  meaning, which is not the same thing as a duplicate: read them and link the new node to the ones it is about. This
  is separate from `near_duplicate`, which compares title words. Nodes whose vectors are not made yet are not
  compared. `data.related` is `null` when the check did not run; then a `related_skipped` hint says why (turned off,
  no embedder, no floor, no vectors yet, a text that looks like a secret with a hosted embedder, the cap, the time
  bound, or the embedder failed) and its `next` is the command that turns it on or shows what is missing.
- The node is written first, and the check never undoes or fails the write. The reply then waits for the check, at
  most 5 s; past that it comes back without `related`, with a `related_skipped` hint. A search waiting on the same
  embedder always goes first.
- `--related` / `--no-related` turn the check on or off for this call. Without either, the graph's `related`
  setting decides (`umx graph set <alias> --related off`), else this machine's (`umx config set related off`);
  it is on by default. Turn it off when adds must be fast: embedding with a local model takes a moment.
- `--locator <path|url|root:...>` makes the node a resource: a file, folder or URL, added once per graph
  (`resource_exists` returns the one already there). `--title` is then optional. See `umx docs get resource`.
- `--raised-in <anchor>` adds a `raised_in` edge from the new node to the anchor (the segment, meeting or event it
  came up in) in the same write: `data.raised_in_edge` names it. The anchor is a node of this graph or an address
  in another (`umx:<graph>/<id>`), resolved exactly as `umx edge add` resolves an end, so it is refused or warned
  about as edge add would be, and a refusal writes nothing. An anchor of a type `raised_in` does not expect warns
  `relation_endpoint_unexpected` and is stored. See `umx docs get time`.
- `--on-key resolve` with `--set entity_key=<scheme>:<value>`: when a node of this type in the graph already holds
  one of the keys, the add resolves to that node instead of adding one, and only appends or fills (new evidence,
  absent fields and keys, an empty note, a first parent, a new raised_in anchor). The reply then carries
  `resolved: true` and `changed`. A held title, status, note, parent or field is kept and named by the
  `on_key_field_held` hint; a key two nodes hold is `on_key_ambiguous`. Not with `--locator`. See
  `umx docs get batch` (Found by key).
- Adding or changing more than a handful of nodes? Use `umx batch` (`umx docs get batch`): one call, one write,
  all or nothing.

## umx node get <node>

    umx node get 01JC8XA4H7
    umx node get umx:launch/01JC8XA4H7Q1V9K2M3N4P5R6S7
    umx node get 01JC8XA4H7 --columns id,title,evidence_kinds
    umx node get 01JC8XA4H7 --columns all

`--columns a,b,c` returns only those fields: any field name, plus `graph` and `address`. A field the node does
not have comes back as null. `--columns all` returns node list's usual columns (`id, type, title, status,
updated`) plus every custom field the node has, sorted by name. Give `all` alone: `all` beside other names is
refused (`usage_invalid`).

`secret_suspected` names what umx's own secret check flags among the values the reply shows, and is left out
when nothing is flagged. It holds names only, never a value: `field:<name>` (title, note, a string field or
`locator`), `key:<i>` (the i-th entry of the node's `entity_key` array), `evidence:<i>`, `status_reason:<i>`
(the i-th status move's reason), `status_evidence:<i>` (the i-th status move's evidence pointer) and
`evidence_removed:<i>`, each position counted from 1 (`evidence:1` is the first pointer), as `check --lint` and a
redact's candidates count them; a graph's own node (`umx:<alias>`) flags its title. A custom field is flagged by its
name (`field:zeta`), where `check --lint`, a redact's candidates and an edge's flags give its number (`field 2`). The
check is the one a write warns with; a field allowed with `umx embeddings allow` is not flagged while it is still
embedded (made `data`, it is flagged again). `secret_suspected` is a reserved name: a write cannot set it, and a
stored field of that name is never shown. The values themselves are still in the reply: never copy a flagged value
into a message, log or another graph (`umx docs get safety`).

## umx node list

    umx node list --type problem
    umx node list --status open --limit 20
    umx node list --where 'likelihood>=3'
    umx node list --within 01JC8XA4H7          # everything under it by part_of
    umx node list --tag needs_review --no-tag blocked   # std tags, from the tag index

`--where '<field><op><value>'` (quote it: `<` and `>` mean redirection to a shell) with `= != < <= > >=`. A node without the field never matches, even for `!=`.
Quotes around the value are optional: one matching pair, `'...'` or `"..."`, is dropped, so
`--where "updated>='2026-11-12'"` finds what `--where 'updated>=2026-11-12'` does.
Two dates compare as spans of UTC time: `due_on<2025-02` is wholly before February, `due_on=2025-01` is anywhere
in January (table in `umx docs get time`). `created` and `updated` compare the same way: `updated>=2026-11-12` is
from the start of that UTC day, `updated=2026-11-12` anywhere in it, and a full date-time works too. On a date field
(declared, `created` or `updated`), `< <= > >=` with a value that is not a date is refused with `where_invalid`;
`=` and `!=` still compare it as text, so a stored non-date stays findable.
On a list field, a value with no comma looks for one element: `=` matches when any element equals it and `!=`
when none does. A value with commas (`tags=red,blue`) is compared to the whole list. `entity_key=<key>` finds the
nodes holding a key (see `umx docs get entity`); with no graph named while several are loaded, that lookup covers
every loaded open graph.
Lists are sorted by id; `--limit` (default 100) and `--offset` page through them. Rows have `id, type, title,
status, updated`; `--columns id,title,owner` picks others, as for `umx node get`. With `--type <t>` and no
`--columns`, a type given its own columns by `umx ontology set type <t> --columns` shows `id` and those (see
`umx docs get ontology`). `--columns all` shows the usual columns plus every custom field any node on the page has,
sorted by name, null where a node lacks one (as `umx edge list --columns all`); it overrides a type's own columns.
A custom field named like a column umx computes (`copy`, `graph`, `address`, a standard field) is never shown:
the computed column wins.
`--columns id,spine:<name>` adds where the graph's declared spine `<name>` places each node, by the rule `umx graph
spine show` uses: `{"band": n, "by": "field", "anchor": false}`. `band` counts along the spine in its own order (an
anchor's rank; for any other node, the rank of the anchor whose band it sits in; -1 before every anchor), and `by` is
how it was placed (`field`, `link`, `evidence`, `rule` or `not_placed`, whose `band` is null). The rows stay sorted
by id: order by `band` to read them along the spine. The cell is null where the row's graph declares no spine of
that name (`umx graph spine show <alias>` lists them).
Each row carries `secret_suspected` as `umx node get` does, for the columns that row returns only; the column is
added when some row on the page has it. Ask for it by name (`--columns id,note,secret_suspected`) and every row
has it, `[]` when nothing is flagged.
`--tag <a>[,<b>]` keeps nodes holding a tag of each `--tag` (repeated flags are AND, a comma is OR,
`'origin:*'` is a facet) and `--no-tag` leaves out nodes holding any of its tags; both come from the graph's tag
index, never a scan, and need a std graph (see Tags in `umx docs get ontology`).
`--graph a,b` or `--scope both` lists across graphs, with graph/address provenance and one combined page.
Without either, the single-graph behavior is unchanged. See `umx docs get scope`.
`--along <spine>` with `--after <end>` and `--before <end>` keeps only the nodes a spine places in that range,
before paging; both ends are inclusive, and the reply's `along` and hints count what it left out (one graph only;
`umx docs get time`):

    umx node list --along day --after 01JC8XA4H7 --before 01JC8XB2K9
    umx node list --along date_field:due_on --before 2026-12-31 --type task

## umx node find <words>

Finds nodes by approximate title (typos and word order are fine), best match first. A title holding every word
you give (whole words, any order; case, hyphens and punctuation do not matter) is found however long it is. It
scores at least 0.5, below a close match, and shorter such titles rank first. An exact title scores 1.0 and comes
first. Typos are judged against the whole title, so a short query with a typo can still miss a long title: give
fewer words, spelled as in the title. A query of one to three words that is not held and scores under 0.8 must
have each word within a typo of a title word (or join title words): a short name does not find a different name
that only shares letters, and numbers match exactly. find reads titles only, never notes or other fields, and does not stem
(`advocate` does not find `advocates`). When nothing matches its hint `find_no_match` says use `umx search` for words
in notes or another form of a word.

    umx node find "aproval delays"
    umx node find "month-end approval"
    # finds "Approval delays at month end for the regional finance team and the shared services desk" (15 words)
    umx node find "launch" --type decision
    umx node find "launch" --within 01JC8XA4H7   # only nodes under it by part_of
    umx node find "launch" --tag needs_review    # only matches holding that std tag (--no-tag leaves out)
    umx node find "launch" --along day --after 01JC8XA4H7   # only nodes the spine places from that segment on

## umx node edit <node>

Changes the title, type, note or custom fields. `--note` replaces the note; to add to it, use `umx node note`.
The result lists the fields that changed; an edit that changes nothing writes nothing.

    umx node edit 01JC8XA4H7 --title "Approval delays at month end"
    umx node edit 01JC8XA4H7 --type risk --set likelihood=4 --unset owner
    umx node edit 01JC8XA4H7 --note "Replaces the whole note"
    umx node edit 01JC8XA4H7 --tag stage:review --untag needs_review

Status is not edited here: `--set status=...` is refused with `use_status_verb`, so every move is recorded.

`--tag <tag>` adds std tags and `--untag <tag>` removes them (`--untag origin:*` removes a facet), each repeatable;
removing the last drops `tags`. See Tags in `umx docs get ontology`.

`--locator <x>` re-points a resource node at a new source (or makes a plain node a resource), and
`--unset locator` makes it plain again; the node keeps its id and edges, and `locator_changed` counts what cited
the old source. `locator` and `locator_*` are never custom fields (`--set locator=...` is `locator_field_invalid`).
See `umx docs get resource`.

## umx node note <node> --append <text>

Adds a paragraph to the note and, in the same write, any evidence pointers (see `umx docs get evidence`).

    umx node note 01JC8XA4H7 --append "Finance confirmed the backlog on 4 Nov"
    umx node note 01JC8XA4H7 --append "Raised in the call" --evidence 'calls/3-nov.vtt#seq=120-140' --evidence-kind room
    umx node note 01JC8XA4H7 --append "See section 2" --evidence 'docs/brief.md#L10-L24'

## umx node evidence <node> list|add|remove|refresh

One node's evidence pointers, one at a time (req-126; usage 144). `list` shows each live pointer with its
position (`at`) and its text in canonical form; `--removed` lists the trail instead. `add` stores one pointer,
checked as for `node note` (no note text). `remove` needs `--reason` (1 to 500 characters) and moves the pointer,
unchanged, to the node's `evidence_removed` trail with when, why and by whom (the pointer nested under `removed`,
the removing session in the entry's own `by_session`; see `umx docs get evidence`); nothing is erased.

    umx node evidence 01JC8XA4H7 list
    umx node evidence 01JC8XA4H7 add 'docs/brief.md#L10-L24' --evidence-kind document
    umx node evidence 01JC8XA4H7 remove 'calls/3-nov.vtt#seq=120-140' --reason "a question, not an answer"
    umx node evidence 01JC8XA4H7 remove 'calls/3-nov.vtt#seq=120-140' --reason "duplicate" --at 2
    umx node evidence 01JC8XA4H7 list --removed
    umx node evidence 01JC8XA4H7 remove 'notes/creds.md#L3' --redact --reason "the path names a credential"

- A pointer matches on its source and selectors in canonical form; its kind is ignored. Copy it from `list`. A
  `umx:` source matches without its `?repo=` hint, and its id may be a prefix of the stored id (as text; nothing is
  looked up).
- Two live pointers that match refuse with `evidence_ambiguous`, listing their positions; `--at <n>` picks one.
- Nothing matching: `evidence_not_found`, with the node's pointers as candidates. Removing one that is already in
  the trail reports `already_removed` and changes nothing, so a clean-up can run twice.
- The action may come first: `umx node evidence remove <node> <pointer> --reason <why>` is the same.
- `remove --redact` (F183, usage 175) is for a pointer that should never have been written. It removes every
  pointer in this graph with the same source (the named node's, every other node's, live or in a trail, and on
  edges). Each one becomes a trail entry with only `kind`, `redacted: true`, when, who and the new reason. The reply
  gives counts, never the pointer or ids, and warns `redacted_in_files_only`: git history, other graphs and copies
  still hold it. It is refused with `evidence_redact_blocked`, changing nothing, while the source or a selector's
  text would remain anywhere in the graph's files (see `umx docs get evidence`). The candidates name each record
  and a place, never the text or a custom field's name: edit those, then redact again. `--reason` may not hold any
  part of the pointer or look like a secret. `--at` does not apply.
- `refresh '<pointer>'`, `refresh --at <n>` or `refresh --all` (usage 220) hashes the named pointer's source again,
  or every pointer's, and keeps what it holds now: the way to accept a `source_changed` from `umx check --sources`.
  One of the three is required: a shared graph may hold pointers you did not write, so a refresh never reads every
  file unasked. The reply lists each pointer (`changed`, `read`) and `hashed`, every local source read. See
  `umx docs get evidence`.

    umx node evidence 01JC8XA4H7 refresh --at 2
    umx node evidence 01JC8XA4H7 refresh --all

## umx node status <node> <status> --reason <why>

Moves a node to a status and records the move on the node: when, who, from, to, the reason and a label.

    umx node status 01JC8XA4H7 agreed --reason "Finance and ops signed off"
    umx node status 01JC8XA4H7 contested --reason "New data from the pilot" --label in_contention

- Without `--reason` the move still happens, with a `reason_missing` hint.
- The label is `resolved`, `in_contention` or `back_on_table`. Without `--label` it is inferred: open or
  contested to agreed is `resolved`, agreed to contested is `in_contention`, ruled_out to open or proposed is
  `back_on_table`; other moves have none.
- A status not in the ontology is accepted with an `off_vocabulary_status` warning.
- `--in <anchor>` says where the move happened: the meeting, event or segment it was decided or said in. The
  anchor is a node of this graph or an address in another (`umx:<graph>/<id>`), resolved exactly as
  `umx edge add` resolves an end, so it is refused or warned about as edge add would be, and a refusal makes no
  move. The move stores it as `in`, `umx:<graph id>/<node id>`. An anchor of a type `raised_in` does not expect
  warns `relation_endpoint_unexpected`.
- `--evidence <pointer>` (with `--evidence-kind`, as on `node add`) stores one pointer on the move, in the evidence
  grammar (`umx docs get evidence`), stamped with the adding session's `by_session` as node evidence is. It is
  scanned as node evidence is: a pointer that looks like a secret warns `secret_suspected` (a batch refuses it),
  and `node get` flags it as `status_evidence:<i>`. `--evidence-kind` is checked even without `--evidence`, as
  node add checks it: a kind umx does not know refuses `usage_invalid` and makes no move.
- A move's evidence lives on the move only and shows in `status_history`: it is not node evidence, so a node's
  `evidence_kinds`, `umx node evidence <node> list` (and its checks) and provenance reads do not count it. A
  `remove --redact` of its source does scrub it, keeping the move (`umx docs get evidence`).
- In a batch, a `node status` line takes `in`, `evidence` and `evidence_kind`; a batch's `--raised-in` never sets
  a move's `in`. `node get` shows each move's `in` and `evidence` in `status_history`.

    umx node status 01JC8XA4H7 agreed --reason "Signed off in the review" --in umx:launch/01JC8XB2K9 --evidence "calls/review.vtt#seq=40-52" --evidence-kind room

## umx node history <node>

Shows the node's creation and every status move, oldest first. With more than `--limit` rows (default 100) it
keeps the newest and says `truncated`. Edits other than status moves are not listed yet.

    umx node history 01JC8XA4H7
    umx node history umx:launch/01JC8XA4H7Q1V9K2M3N4P5R6S7 --limit 5

## Who wrote it

Every write records who made it: `created_by` on a new node, and `by` on each status move and history row. That
is `--by <name>` when the command has it, else `UMX_AUTHOR`, else your OS user. Agents: set `UMX_AUTHOR` once to
your seat or role name rather than passing `--by` each time.

    umx node add --type problem --title "Approval delays" --by pm-001

umx also attributes every successful write (`node add`, `node edit`, `node note`, `node status`, `edge add` and
`umx batch`) to the session and agent that made it, without you setting anything. It is recorded in the graph's own
files, in the same write as the change: a node records who authored it and who last edited it, and a new evidence
pointer records the session that added it. `umx node get` shows these as `authored_by_session` and
`edited_by_session` on the node and `by_session` on each new evidence pointer: each is the id of a session record
in the same graph, never a raw harness session id. Editing a node keeps its author and replaces only its last
editor; an edit that changes nothing records nothing. A node written before attribution existed keeps no author:
umx never invents one. The session and agent records, and the links between them, are not attributed themselves.

The CLI works out who you are once per write command, in this order, and the reply's `data.actor_tier` says which
it used:

- `pij`: your pij seat, from `pij-rs`, given at most 2 s;
- `harness`: your agent harness's session, from its environment;
- `cli`: your login name;
- `unknown`: none of these. The write still happens.

Session ids are never stored, shown or sent to the daemon as they are. The CLI checks a session id's shape and
keeps only a one-way hash of it (`h-` and 16 hex digits), for display; a session id of the wrong shape is recorded
as unknown. Every other captured value is checked and scanned for anything that looks like a secret, and one that
fails is recorded as unknown. Read commands never look you up.

A session record holds what is known about that session: its role, model and effort, and a link to its agent
record. When known, the agent key gives the pij seat and the agent links to the harness record: session to agent
to harness, with no direct link from a session to its harness. These describe the session, not each write: a write
whose capture misses a value keeps the last known one, and a new known value replaces it. So read a session's model
as the model of this session as last seen, never the model of a particular write.

This is a record of what the writer claimed, not proof. Anyone who can write the files can write any attribution,
so never treat it as permission, sign-off or evidence of who decided something. The session and agent records
themselves are hidden by default: lists, counts, get, find, search, walks and query leave them out.
Use `--provenance` on a read to include them. Search finds them through exact matches only; `node find`
matches their titles. They are never embedded, searched by BM25 or included in `similar_to`, even with the flag.
`authored_by` and `edited_by` are read-only node-to-session relations; follow `session_of` to the agent.
For example, `umx neighbours <node> --relation authored_by --provenance` reads the node's author.
`umx node who <node>` shows who made a node and who last edited it (session, agent, role, model and harness), and
`umx who <key|seat|session>` lists what an agent or session made, without `--provenance` (`umx docs get who`).
`created_by` and `by` above are unchanged.

    umx node who 01JC8X

- `actor_invalid` (warning): a field of the claimed identity was unsafe or malformed, so the write was recorded as
  unknown. The message names the field, never its value.
- `session_agent_conflict` (warning): this session is already linked to another agent. The first link is kept.
- `reserved_name`: `prov_key`, `authored_by_session` and `edited_by_session` are umx's; never `--set` them.
- `reserved_type` (warning): a node you add or edit uses a type umx keeps for provenance (`platform`, `agent`,
  `model`, `harness`, `session`). It stays an ordinary node, visible like any other; pick another type unless you
  mean it.
- `reserved_relation`: `session_of`, `runs_on`, `uses_model` and `registered_in` are kept by umx between
  provenance records; an edge with one is refused and nothing changes.
- `derived_relation`: `authored_by` and `edited_by` are worked out from the session keys and cannot be stored; an
  edge with one is refused and nothing changes.
- `provenance_key_invalid` (warning, on load and in `umx check`): a provenance value in the files is malformed, for
  example after a hand edit or a bad merge. Its value is kept and not used: a node line keeps its bytes while umx
  does not change that node, and an edge keeps the value but may have its spacing rewritten at the next edge write.
  Fix it from git history, or leave it; never invent a value.
- `duplicate_prov_key` (warning, on load and in `umx check`): two provenance records hold the same key, as after a
  merge where both branches recorded one session. Both are kept and the lower id is the one in use.

## Naming a node

A full id, a unique prefix of 6 or more characters, or an address `umx:<alias>/<id>`. Never a title. An unknown
id gives `node_not_found` with the nearest ids; a short prefix that matches several gives `id_ambiguous`.
