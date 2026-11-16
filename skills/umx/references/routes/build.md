# /umx build: make and shape a graph

Job: create a graph, change its vocabulary or settings, or add and change nodes and edges by hand. The
daemon must be running (`/umx start`).

## A new graph

    umx graph new /absolute/folder --alias my_graph

The folder must be under the daemon's roots (usage 003). **usage 019, usage 053**: there are no templates.
`graph new` returns `data.next`, commands that add a type, a relation and a status and set a graph setting,
each runnable as printed. Shape the graph first: run or adapt them for the types, statuses and relations
this problem needs, and set the title.

**usage 057**: if the graph will use similarity search, record its embedding model at creation with
`--embedding-model <model@dims>` (`+p<N>` for a prompt-prefixed model). Use the model's own name, never a
deployment's. umx never picks a model for a graph. On `embedder_mismatch`, keep working: the graph keeps its
model and vectors are never mixed (usage 017). **usage 073**: dims are 1 to 8192; a wider key is refused, and a
pulled graph.json recording one fails `graph load` with `graph_invalid`. **usage 082**: never add a `similar_to`
edge or relation by hand (`name_invalid`); umx keeps them, read with `umx neighbours <node> --relation similar_to`.

An existing folder is `umx graph load` (`/umx explore`), not `graph new`.

## Vocabulary and settings

- See it: `umx ontology show`.
- **usage 007**: the vocabulary is soft. A write with an undeclared name warns `off_vocabulary_type` (or
  status, relation, field) with the `umx ontology add ...` that declares it. If the name is right for this
  graph, run that `next`; don't contort content to fit the starter vocabulary, and don't leave many such
  warnings unresolved (`umx docs get ontology`).
- **usage 141**: `umx ontology remove type|status|relation <name>` and `remove field <type> <name>` take an
  entry out. While nodes or edges use it, it refuses (`ontology_in_use`, with a next that lists them); change or
  delete those first. Built-ins (`part_of`, `open`, derived and provenance names, `std:` entries) never go.
- **usage 103**: one field on many types is one call: `umx ontology add field risk,issue impact:int`, or
  `--all-types`; all or nothing, each culprit its own error (`type_unknown`, `field_conflict`). An `ontology add`
  that matches what is declared is ok with `"unchanged": true` and writes nothing, so setup can re-run; one that
  differs (another kind, a type missing a given field, a plain add of a symmetric relation) is refused.
- **usage 129**: edge fields have a vocabulary too. `edge add --set` with a field the relation does not declare
  warns `off_vocabulary_edge_field`; its next, `umx ontology add field --relation <r> <name>:<kind>`, declares it
  (typed from then on; `ontology show` lists `relation_fields`). An edge's own keys are `reserved_name`.
- **usage 189**: change an edge's custom fields in place, keeping its id: `umx edge edit <edge> --set valid_to=<date>`
  ends a membership, `--set-str why=<text>` fixes a reason, `--unset <field>` removes one (a batch line takes `sets`
  and `unsets`). Never delete and re-add an edge to change a field.
- **usage 165**: declare date fields as kind `date` and write every date-time with its offset or `Z`; umx stores
  universal time, and a bare date-time is taken as universal time (`time_no_offset` warns; usage 178). `starts_on` and `ends_on`
  are the one span pair (never `valid_from`/`valid_to`). Date the meeting (and its `segment` nodes, with a
  `step`), link points to it with `raised_in`, and never put a time in a string field (`umx docs get time`).
- **usage 176**: use the std v1 names (`umx docs get organise`). Work that moves takes the lifecycle statuses
  `active`, `stalled`, `delivered` and `withdrawn` on any type: declare them, then `umx node status <node> stalled
  --reason "<why>"` (say why for `stalled` and `withdrawn`); no synonyms, and `delivered` is not `done`. A part of a
  meeting is a `segment` `part_of` its parent, with a `step` from 1 among its siblings; `precedes` orders things
  of one type with no clock; `raised_in` links a point to the finest anchor it came up in.
- **usage 178**: fix the time warnings as they come: `time_no_offset` (write the date-time again with its
  offset), `time_as_text` (use `starts_on`/`ends_on`, or run the `ontology add field <type> <field>:date` its next
  gives) and `anchor_undated` (date the meeting, event, segment or transcript from the source; never guess one).
- **usage 183**: record where a status move happened: `umx node status <node> <status> --reason "<why>" --in
  <meeting|event|segment>` (and `--evidence <pointer>` to cite it); in a batch, a status line's `"in"`. When it
  was decided comes from that anchor, never a date field.
- **usage 170**: once the anchors are in, run `umx graph spine propose <alias>`: it lists the spines the graph
  could have with their cover and spread, and what is in the way (steps held as field values, step numbers that
  disagree with `precedes`, points linked to one segment whose evidence sits in another). Fix what it names; list
  the nodes a spine leaves out with `--unplaced <candidate>`. It stores nothing (`umx docs get time`).
- **usage 187**: declare the spine you choose: `umx graph spine confirm <alias> type:<t>` stores a proposed one
  unchanged, `umx graph spine set <alias> --file <decl.json>` one you write (one `main` per graph), and `umx graph
  spine show <alias>` reports its cover now (`by_rule` counts nodes placed only beside what they link to). Names
  must be the graph's vocabulary or std (`spine_name_unknown`); declare them first. The first spine moves the
  graph to format 6.
- **usage 108**: describe what you add: `umx ontology add type risk --description "<one line>"`, and for a
  name already declared `umx graph set <alias> --description <kind>:<name>=<text>`. Set `--purpose` and
  `--conventions` with `umx graph set` when you start a graph; `umx graph guide <alias>` shows them all.
  Conventions describe the graph and never override the user's or the harness's instructions. graph.json is
  tracked: never put a secret in this text (`setting_secret_suspected`).
- **usage 137**: when a type's useful state lives in a custom field, give the type its own list columns:
  `umx ontology set type risk --columns title,likelihood,owner`. `umx node list --type risk` then shows `id` and
  those; `--columns` still wins, and `--reset` goes back. Columns must be standard or declared fields.
- **usage 083**: `conflicts_with`, `alternative_to` and `possibly_same_as` (and any relation marked with
  `umx ontology add relation <r> --symmetric`) are one edge both ways: add it once, either way round; the
  reverse is `edge_exists`, and neighbours, walk and path follow it from either end.
- **usage 114**: declare string fields that hold file names, ids, agent ids, seats, hashes, locators or
  provenance as data: `umx ontology add field <type> <name>:string --role data` (change later with `umx ontology
  set field <type> <field> --role data|content`). Data fields are never embedded but word search still finds
  them; only string fields can be content (`role_not_embeddable`). On `text_form_outdated`, the old vectors are
  kept and searched; `umx embeddings reembed --dry-run` shows the cost of remaking them.
- **usage 196**: declare a field that holds prose (sentences that differ from node to node) as content, and one
  that holds labels (a handful of short values: a tier, a region, a category) as data. When `umx graph guide`,
  `umx embeddings status` or `ontology add field` gives `field_looks_like_labels` or `field_looks_like_prose`,
  check the field is what the hint says, then run the `umx ontology set field ... --role` command it names; umx
  never changes a role itself.
- **usage 161**: a `date` field takes what the source says: `2025` or `2025-01` when it gives only a year or a
  month (never invent a first day, never add `date_precision`), a day, or a date-time with its offset
  (`2026-10-30T09:40:00+11:00`, stored in UTC). Dates compare as UTC spans (`umx docs get time`).
- **usage 008**: names are snake_case (`depends_on`), never hyphens or capitals; Cypher keywords are
  refused with a suggested `<name>_item`.
- **usage 053**: graph settings change with `umx graph set <alias> --<setting> <value>` (`--title`, and
  `--policy secured` for a graph that holds client or private text; lowering a policy is the human's, usage 059).
  **usage 051**: a secured graph's text never goes to a hosted model, whatever the config, env or flags say; give
  the machine a local embedder instance for it (`embedder_not_local_for_secured` until there is one).
  An unknown setting is refused with `setting_unknown`, whose message lists the settings.
- **usage 128** (supersedes 112): a graph has two settings, `--visibility open|named` (named: read only when named)
  and `--embedder any|trusted|local` (which instances may get its text); secured is named + local. Trusted now means
  configured in the machine config: any instance there, hosted ones included, may embed a trusted graph with nothing
  to confirm. Before `umx graph set <alias> --embedder trusted` (which stays local until the human runs
  `umx graph confirm-policy <alias>`), tell the graph's owning seat. `embedder_route_changed` after a config set means
  named graphs will send text to the new endpoint after a restart: tell the human. Secret holds still apply.

## Nodes and edges

1. **usage 004**: look before you add: `umx node find "<words>"`. Reuse a match. If `node add` hints
   `near_duplicate`, check the candidate with its `next` and reuse it unless it is genuinely different.
   **usage 156**: near_duplicate names the candidate's type; another type hints only at near-identical wording, a
   shared scaffold with a different word or number never does; in a batch, `near_duplicate_line` names the
   earlier line that adds the near node: keep one of the two lines if they are the same thing.
2. **usage 005**: a node is something someone could agree with, argue against or be blocked by (an option,
   decision, criterion, open question, tension, constraint, owned deliverable). Not a passing remark, a
   restatement or a topic nobody took a position on.
3. Add: `umx node add --type <type> --title "<title>"`. **usage 006**: keep the id it prints and name nodes
   by id, a 6+ character prefix or `umx:<alias>/<id>`, never by title (§ C2).
4. **usage 009**: relate with `umx edge add <from> <relation> <to>`. It echoes a sentence; check it says
   what you meant. If the relation overstates the claim, pick a gentler one or add one. **usage 135**: put
   where the relationship was asserted on the edge (`--evidence <pointer> --evidence-kind <kind>`), never in a
   custom `why` field; `umx edge show <edge id>` reads it back.
5. Change: `umx node edit <node> ...` for title, type and fields. **usage 010**: when an argument moves,
   move the status with `umx node status <node> <status> --reason "<why>"`; contested means the argument is
   live, and a contradicted fact goes to contested with both values in the note.
6. **usage 110**: `export UMX_AUTHOR=<your seat or role>` once, so `created_by` and history's `by` say who
   wrote; `--by <name>` is for a single write on someone else's behalf (`umx docs get node`).
7. **usage 113**: a wrong edge (wrong relation or the wrong way round): find its id with
   `umx edge list --from <node>`, then `umx edge edit <edge> --relation <r>`, `--reverse`, or both; the id
   stays and `updated_by` records who. Remove one with `umx edge delete <edge>`. Never edit edges.jsonl by
   hand. `edge_would_duplicate` means the fixed edge already exists: delete this one instead.
   **usage 162**: a cross-graph edge is fixed the same way from the graph that stores it (`--graph` or its
   address there); the far graph need not be loaded, the reply shows only the stored far address, and an edit
   never moves an end (delete and add instead).
8. **usage 144**: stale or wrong evidence (a question line cited as an answer, a moved selector): see the node's
   pointers with `umx node evidence <node> list`, then `umx node evidence <node> remove '<pointer>' --reason
   "<why>"`, copying the pointer from list. It moves to the node's `evidence_removed` trail; nothing is erased.
   `evidence_ambiguous` lists positions: add `--at <n>`. `already_removed` means it is done. Many at once: batch
   `{"op":"evidence remove",...}` lines with one `umx batch --reason "<why>"`. Never hand-edit nodes.jsonl.
   **usage 175**: a pointer that must not stay (a credential in a path, private text in a quote): `umx node
   evidence <node> remove '<pointer>' --redact --reason "<why>"` removes every pointer with that source in the
   graph and keeps only `(redacted)` trail entries. Keep any part of the pointer out of the reason.
   `evidence_redact_blocked` names each place that still holds the text: edit those, then redact again. Read
   `redacted_in_files_only` as a to-do: git history and other graphs still hold it.

**usage 153**: re-adding an edge is `edge_exists` only with no fields or when every supplied field already
matches its stored typed value. A missing or different field refuses with `edge_exists_differs`, naming the
edge id and field names, never values; its next is `umx edge edit <id> --set <field>=<value>`. Aliases and named copies resolve to endpoint identities, symmetric
ends may swap, and only the storing graph's edges are compared. Add never merges. Read `umx docs get edge`:
to change an existing edge's fields, use `umx edge edit <edge> --set <field>=<value>` / `--unset <field>`
(usage 189); the edge keeps its id. Do not retry a refused batch as single writes.

**usage 104** (supersedes usage 079): more than a handful of adds or edits? Write one JSON op per line to a file (or pipe them) and
run `umx batch --file ops.jsonl --dry-run`, then `umx batch --file ops.jsonl`: one call, one write, all or
nothing. Give a node `"ref":"q1"` when a later line points at it, and point with the `_ref` twin (`parent_ref`,
`from_ref`, `to_ref`, `node_ref`); a real node goes in the plain field. Never loop over `umx node add`, never
shell-quote JSON into arguments. If any line fails nothing changes; fix the lines the errors name and rerun
(`umx docs get batch`).

Refs are lower-case words starting with a letter, at most 32 characters of `a-z 0-9 _ -`.
`estate`, `contract` and `workshop` work as refs in their `_ref` fields; their spelling is never
looked up as an id.

**usage 131**: a malformed ref now names its first bad character and 1-based position, or the
32-character limit. `brd_exA` points at `A` at position 7; unsafe characters use `U+XXXX`, never
raw control text. Fix that ref in the batch file; the grammar is unchanged.

**usage 147** supersedes usage 136 and carries usage 104: batch `node note` appends (where `node edit` replaces), and can
carry up to 32 evidence objects with per-pointer `kind`. Batch `node status` records the usual
reason/label/history. Use an earlier `node_ref` or the normal id/address field. Dry and real batches
run the same pre-commit secret scan; a hit rolls everything back. Singles warn and store.
Embedded fields keep `umx embeddings allow`; status reasons, evidence and edge fields have no
allow yet. Redact/replace a flagged value. Only a person-checked non-secret may be written as a
single change; never automatically retry a batch refusal that way. `umx docs get batch` gives
the exact line shapes and recovery. No URL is fetched and the embedding hold surface is unchanged.

**usage 191**: a batch `node add` takes evidence as pointer strings or `{"pointer":...,"kind":...}` entries, as
`node note` does (not both a per-entry kind and `evidence_kind`). Custom fields go in `sets`; a top-level `why`
fails saying so. A ref in a plain field (`"node":"q1"`) fails `node_not_found` with the hint
`batch_use_ref_field`: use `node_ref`. The CLI refuses a ref that is not lowercase `a-z 0-9 _ -` before
sending. A type error names the op and the field path (`evidence[0].pointer`). Setting one field twice in a
line's `sets` warns `duplicate_field`: only the last value is kept.

For an edge-direction check, add `--echo-sentences` to that dry or real batch.
It returns the first 100 edge-op sentences and `sentences_omitted`, using each
line's titles and stored direction. Each sentence is at most 1024 characters.
Do not enable it by default. Far secured/unloaded endpoints show id-only
addresses, never titles or aliases. New unminted dry-run edge ids remain null;
correlate by line, not a synthetic id.

For umx-to-umx ports, batch node and edge adds accept full `id` values and node adds accept a `created`
timestamp. A ref binds to the explicit id; dry runs show explicit ids and leave minted ids null.
For non-ULID source ids, let umx mint ids and keep the source value in a `source_id` custom field.
Never read a time from an id: use `created`. `id_taken` includes tombstones and kept lines, and
`id_in_future` refuses ids over 60 seconds ahead. `id_in_other_graph` checks only Open or explicitly
named loaded graphs with a different graph id. Other secured graphs are skipped silently.
Batch edges accept exactly one of `relation` or `type`;
edges.jsonl always stores `type`.

**usage 167**: a later batch (a trailing links batch, say) can use refs an earlier applied batch bound. Save
that batch's JSON reply (`umx batch --file nodes.jsonl > nodes.receipt.json`), then run
`umx batch --file links.jsonl --refs-from nodes.receipt.json`, dry run first; repeat `--refs-from` once per
earlier batch. Only an applied receipt is read (a dry run's is `batch_refs_from_dry_run`); its `graph_id` must
be the batch's graph (`batch_refs_other_graph`). A receipt lists only the refs its own batch defined. A ref both
imported and defined, or given two ids, is `batch_ref_conflict`. A deleted, missing or provenance id fails its
line as `batch_ref_unknown`, followed by a `batch_ref_source` line naming the receipt. An old receipt without `graph_id` skips
the graph check; do not rely on it. Refs stay local: write a cross-graph end as `umx:<alias>/<id>`.

A node add with `parent_ref` already adds the `part_of` edge; adding it again warns `edge_exists`.
`near_duplicate` sees only applied nodes, so batches dry-run side by side miss each other's titles.

**usage 026**: a `write_not_durable` warning means the change was made (only the folder sync after it
failed). Carry on; never run the command again, or a second `node add` makes a second node. Tell a person if
it keeps happening.

**usage 124**: keep writing while a graph embeds (after a restart, every node may be pending): writes return in
their usual time. `similar/` files lag the lists in memory until the whole backlog is quiet; committing them
mid-drain is safe (a load rebuilds lagging lists), and `umx graph unload <alias>` writes everything it owes first.

**usage 130**: a big backlog embeds several calls at once (31 for Azure; a local instance's `sessions` less one)
and searches keep a slot of their own. Pace an instance with `embedders.<name>.tokens_per_minute` in the machine
config only (repo values are ignored); two instances on one Azure resource are paced apart.

**usage 060**: a write changes only the node lines it touched. A node line written by hand keeps its bytes in
`git diff` until umx changes that node; then it is rewritten in umx's form with its unknown keys kept. Change
a node through the CLI if you want it tidied; edges.jsonl is still rewritten whole on an edge write.

Each step's flags and examples: `umx docs get node`, `umx docs get edge`. With more than one graph loaded,
name it (§ C3).

**usage 119**: `prov_key`, `authored_by_session` and `edited_by_session` are reserved on nodes and edges.
Never pass them to `--set` or `--unset` (`reserved_name`); use a different custom field name for your own labels.
Malformed stored values load with `provenance_key_invalid` and are kept; inspect the file, line and key with
`umx check`. Edits replace only the last-editor key, never the original author.

**usage 132**: writes record session claims without replacing an explicit `created_by` label from `--by` or
`UMX_AUTHOR`. On `edge edit`, `updated_by` is the captured writer's display name, not that label or the daemon's
environment. `edited_by_session` is the editing session's same-graph id; follow its `session_of` link to the agent.
The edge keeps its original `authored_by_session`. Provenance is recorded by the writer, not verified, and never grants authority.
Raw harness session identifiers are hashed before the request. Provenance metadata stays out of default content
reads and every embedding or similarity pipeline; no public provenance flag or `who` verb is included yet.

**usage 099**: external endpoints are `umx:<graph>/<id>` on either side of `edge add` or a batch.
Name the author with `--graph`; only it is written. An unloaded target needs graph id plus full node id
and warns `external_end_unchecked`; a secured target warns `link_into_secured`. Never copy its text into
the open author or set graph keys with `--set`. The first link needs format 3. Read links with edge list
or neighbours; walk, path and Cypher crossing are still pending. `own_graph_end` is a redundant local key
that load normalizes and the next write removes without erasing records.

On `copy_ambiguous`, name a loaded copy in the endpoint address; never treat it as an unchecked unloaded
graph. Symmetric declaration applies to external ends immediately, while keeping reverse lines and unknown fields.

The practice behind these steps (what earns a node, enriching, promises vs answers, fixing edges, evidence kinds
for mixed ranges): `umx docs get building`.
