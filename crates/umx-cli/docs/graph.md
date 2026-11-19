# Graphs

A graph is a folder holding three files: `graph.json` (id, alias, title, vocabulary), `nodes.jsonl` and
`edges.jsonl`, one record per line, sorted by id. The files are the truth; the daemon holds a working copy.
Saving an unchanged graph rewrites identical bytes, so `git diff` stays empty and merges stay clean.
A write changes only the node lines of the nodes it changed: a node line edited by hand keeps its bytes until
umx changes that node, then it is written in umx's form with its unknown keys kept.

## umx graph new <path> --alias <alias>

Creates the three files and loads the graph, and adds the line `.umx-tmp-*` to the folder's `.gitignore` (a
`.gitignore` already there is kept as it is, with the line added at the end unless it is already there). `<path>`
is an absolute folder. The alias is snake_case and is how
you name the graph in commands and addresses (`umx:<alias>/<id>`).

    umx graph new /Users/me/work/launch-graph --alias launch
    umx graph new /Users/me/work/blank --alias blank --empty      # no starter vocabulary
    umx graph new /Users/me/work/launch-graph --alias launch --title "Launch planning"
    umx graph new /Users/me/work/plain --alias plain --no-std     # not a std graph

A new graph uses std v1, umx's standard vocabulary (`person`, `meeting`, `works_for`, `starts_on`...): its
graph.json's ontology says `"std": 1`, and every read and write knows the std names without declaring them
(`umx docs get ontology`, section std). `--no-std` makes a graph without it, whose vocabulary is only what it
declares; `--empty` and `--no-std` are separate (an `--empty` graph still uses std unless `--no-std`). The reply's
`data.ontology.std` is the std version, or null.

The starter vocabulary: types `problem, actor, option, mechanism, criterion, question, decision, tension,
precedent, capability`; statuses `open, proposed, leaning, agreed, acknowledged, contested, ruled_out,
context`; relations `supports, undercuts, depends_on, blocks, conflicts_with, answers, part_of,
alternative_to, raises, owned_by, tests, possibly_same_as, criterion_for`. See `umx docs get ontology`.

`--embedding-model <model@dims>` records the model whose vectors the graph will hold (`+p<N>` when the model
uses a prompt prefix), for example `--embedding-model text-embedding-3-small@256`. It goes in graph.json's
`embedding` section with `text_form` (how umx turns a node into text: 2 since data fields are left out, 1 before)
and `policy` (`open`), and
`graph load` and `graph list` show it. Without the flag the graph records no model, and umx never picks one for
it. When a configured embedder uses a different model, `graph new` and `graph load` warn `embedder_mismatch`
naming both keys. The graph keeps its own model, and vectors from two models are never mixed.

    umx graph new /Users/me/work/launch-graph --alias launch --embedding-model text-embedding-3-small@256

`--secured` makes the graph secured: only an embedder that keeps its text on this machine may embed it. See
Embedding policy below.

    umx graph new /Users/me/work/client-x --alias client_x --secured

In a git repository, `graph new` also adds the graph to the repository's `Graphfile`, its committed list of graphs
(`umx docs get graphfile`), and the reply's `data.graphfile` names the line and the `undo` command. `--no-graphfile`
leaves the Graphfile alone: use it for a private graph or a folder git ignores. A secured graph is left out
(`graphfile_skipped_secured`) unless `--graphfile` is given. Outside a git repository nothing is added, and only
`--graphfile` says so, with the hint `graphfile_no_repo`.

    umx graph new /Users/me/work/repo/notes --alias notes --no-graphfile
    umx graph new /Users/me/work/repo/client-x --alias client_x --secured --graphfile

There are no graph templates. Every new graph starts from the starter vocabulary (or none), and you change it
as the work needs. The reply's `data.next` (and the `next:` lines with `--human`) lists commands, each runnable
as printed, that add a type, a relation and a status and change a graph setting:

    umx ontology add type experiment --graph launch
    umx ontology add relation mitigates --graph launch
    umx ontology add status parked --graph launch
    umx graph set launch --title "Launch planning"
    umx graph set launch --policy secured
    umx graph set launch --purpose "Decide how to launch in spring"
    umx graph set launch --conventions "Titles are short noun phrases"
    umx graph set launch --description "status:open=Not worked on yet"

A folder that already has `graph.json` is refused (`graph_exists`: load it instead). A folder whose
`nodes.jsonl` or `edges.jsonl` holds records but has no `graph.json` is refused too (`graph_orphan_records`), and
nothing is written: those records belong to a graph whose `graph.json` is missing. Restore that `graph.json` (for
example from git) and `umx graph load` the folder, or move the files away and run `graph new` again.

## umx graph load <path>

Loads an existing graph folder. Use `--as <alias>` when another loaded graph already has its alias.

    umx graph load /Users/me/work/launch-graph
    umx graph load /Users/me/work/copy-of-launch --as launch_2

A bare alias loads a graph this machine has loaded before (see The graph index below): `umx graph load launch`
loads the folder the index remembers for `launch`, under that alias (or `--as`), through the same checks. A folder
named like the alias in the current folder still wins: the index is asked only when that folder holds no graph.
When the index has the alias for two or more folders, umx picks none (`alias_ambiguous`, listing each folder with
the command that loads it).

    umx graph load launch

`--graphfile` loads every graph the current repository's `Graphfile` lists, or with `--set <name>` one set of them
(`umx docs get graphfile`), each under its Graphfile alias.

    umx graph load --graphfile --set acme

External edge ends name graph ids, not aliases or copies. Format 3 is written when an external end first
needs it and never downgraded. `own_graph_end` means a stored end named its own graph: it loads as local and
the next write removes that redundant graph key, without deleting the record or unknown fields.

## umx graph list

Lists loaded graphs: alias, id, path, node and edge counts, `kept_lines` (see Check below), when a command last
used each one (`last_used`), the seconds until it unloads itself (`unloads_in_s`, none when idle unload is
off), the `embedding` section, `effective_policy` and `downgraded` (see Embedding policy below), and
`vectors_pending` (see `umx docs get embeddings`).

    umx graph list

`--all` also lists every graph this machine has loaded before and that is not loaded now (see The graph index
below).

    umx graph list --all

`--provenance` adds provenance nodes, stored provenance links and derived `authored_by` and `edited_by`
relations to loaded graphs' counts: every listed loaded graph when none is selected, or only the graphs
you name with `--graph` or `UMX_GRAPH` (the other rows keep content-only counts). It does not load a
graph or show a hidden one: a secured graph's provenance counts need that graph named.
Without the flag, counts exclude provenance. With `--all`, unloaded rows keep their remembered counts.
`graph guide <alias> --provenance` includes the named graph's provenance too (`umx docs get guide`).

`--graphfile` instead lists every graph the current repository's `Graphfile` names: loaded, not loaded or missing.

    umx graph list --graphfile

## umx graph scan [<folder>]

Finds the graph folders in a repository (all of the current one, from its top, when you name no folder; only the
folder you name otherwise) and lists each against its `Graphfile`: listed, new, missing, moved, id_mismatch,
unreadable, pending, outside_repo, duplicate_id or nested_repo. It loads nothing. `--add`
writes the new graphs and moved paths into the Graphfile, `--drop <alias>` removes one entry, `--rewrite` writes a
hand-edited file in umx's form, and `--depth <n>` looks deeper than 8 folders (at most 32). See
`umx docs get graphfile`.

    umx graph scan
    umx graph scan --add

## The graph index

The daemon remembers every graph loaded on this machine, also after it unloads, in this machine's graph registry
(see After a daemon restart). `umx graph list --all` shows them all, loaded or not, one row per folder:

- `alias`, `id`, `path`
- `state`: `loaded`; `idle` (it unloaded when idle, and comes back at daemon start or when a command names it);
  `not_reloaded` (it failed to come back at daemon start); `unloaded` (you unloaded it)
- `folder`: `present`; `missing` (renamed, moved or deleted: no `graph.json` there); `outside_roots` (outside the
  graph roots now, so umx does not look at it)
- `last_loaded` (its last load here, a reload at daemon start included), and `nodes`, `edges` and `model` as
  they were when it last loaded or unloaded
- `policy`, and `repo` (the folder holding `.git` above it)
- `reload`: the exact command that loads it again, `umx graph load <path> --as <alias>`

A graph that is not loaded and not open shows only its alias, id, path, state, folder, policy and reload, never
its counts, model or repo. Its policy is the strictest of what it was at its last load, this machine's policy
record and its `graph.json` now, and a folder umx cannot read counts as secured. Loaded graphs show what
`umx graph list` shows. The index keeps up to 64 graphs; past that, the oldest unloaded one is dropped.

## umx graph rename <alias> <new alias>

    umx graph rename launch launch_v2

Renames a graph for good: graph.json's `alias` becomes the new one and keeps the old one in `former_aliases`, so
every `umx:<old>/<id>` address, evidence pointer and edge end keeps working on any machine that loads the graph.
`umx resolve umx:<old>/<id>` answers `status: redirected` with `redirected_from` and the address under the new alias;
every other read or write that names the old alias (`--graph`, an address, an edge end, a verb's alias) uses the
graph and adds one hint, `alias_redirected`. New evidence written through the old alias is stored under the new one;
pointers already stored are never rewritten. Edges and saved scopes key on the graph id, so they need nothing. The
registry row, saved-scope hints and the Graphfile entry and sets (`umx docs get graphfile`) follow. A Graphfile
that already lists another graph under the new alias is left as it is with `graphfile_not_updated` naming the file;
the rename itself stands.

- A chain resolves in one step: after `a -> b -> c`, both `umx:a/` and `umx:b/` read graph `c`.
- Renaming back to a former alias is allowed and takes it off the list.
- At most 16 former aliases are kept; past that the oldest go and the reply lists them (`dropped`).
- A graph loaded `--as` another alias keeps both old names: graph.json's own alias and the one it was loaded as.
- Refused with `alias_in_use` when the new alias is loaded, in this machine's graph index for another folder, or a
  former alias of another loaded graph. The live alias always wins over a former one: a name loaded as some
  graph's alias, or kept in the index for an unloaded graph, never redirects.
- Two loaded graphs that once had the same alias: naming it is `former_alias_ambiguous` (its candidates are the
  open ones only); use a live alias. An evidence pointer through it is kept with `dangling_evidence` saying so. An
  `UMX_GRAPH` default is read only when it is the graph used, so `--graph` or an address overrides such a default.
- The hint's next is the command as given with each former alias read as the live one: the rest of a `--graph`
  list, a `--scope` and the author graph are kept.
- `umx graph confirm-policy <old>` confirms the graph that name is now; its `policy_lowered` names the live alias.
- A secured graph reached by a former alias is read only where its live alias would be (named); in an open
  graph's evidence it reads as not loaded, as before.
- graph.json format 7: an older umx refuses a renamed graph (`graph_format_newer`) rather than lose its
  redirects. A graph never renamed or moved keeps its format.

## umx graph move <alias> <folder>

    umx graph move launch /work/repo/graphs/launch

Moves the graph's folder in one rename (never a copy, and never over anything made at the new path meanwhile). The
graph stays loaded through it, under the same alias: its vectors, similar_to lists, queued work, copy order,
primary mark and registry row are kept, and only its folder changes. The new folder must not exist (a file there
counts), its parent must, it must not be inside the folder that moves, and both must be inside the roots
(`graph_move_refused` says which, and nothing moves, graph.json included). It stays inside the graph's git repo:
a folder in another repo is `graph_move_cross_repo`, since every `root:repo` pointer's base would change; move it
with `git mv` (or copy it), `umx graph load` it there, and re-cite its `root:repo` pointers. graph.json records
the repo-relative folder it left in `former_paths` (format 7), written before the move so a move cut short never
forgets it; a folder whose name is not a repo path (`a:b`, `~archive`) cannot be recorded, so its move is refused
before anything is written. Commit the move with git as any other change.

The reply's `stale` lists this graph's `root:repo/<old folder>/...` pointers (node evidence, status-move evidence
and edge evidence, places counted from 1) with `evidence_root_stale`, each compared in the locator normal form
(`./` and a doubled `/` do not hide one) and listed once however many former folders it is in; re-cite each under
the new folder. `umx check`
keeps listing them as rows of kind `evidence_root_stale` until they are. Pointers relative to the graph folder move
with it and are not stale. Other graphs citing the old folder are not checked (`other_graphs_checked: false`).
The registry row, saved-scope hints and the Graphfile entry's `path` follow. A folder moved by hand (`git mv`) and
then loaded heals the same way: the load says `moved_from` and lists the stale pointers once (`stale_recorded:
false`, since a load writes no graph.json; `graph move` is the way that records them). It is a move only when no
other folder this machine knew the graph at still holds it; otherwise it is a copy (see Copies of one graph).

## umx graph forget <alias>

Removes a graph that is not loaded from the index: it is no longer listed, and never comes back at daemon start or
on demand. Its files are untouched. A loaded graph is refused (`graph_forget_loaded`): unload it first. An alias the
index does not know is `graph_not_known`. If the index file cannot be written, nothing is forgotten
(`graph_forget_not_saved`); `umx doctor` says why. Other registry updates that cannot be written (a graph's index
details, the no-primary mark of its copies) warn `registry_update_not_saved` and never stop a command; at daemon
start they show in `umx daemon status`. A registry that becomes unreadable after the daemon read it at start, or
before an unload records a graph's details, warns `registry_unreadable` naming the step that was skipped.

    umx graph unload launch
    umx graph forget launch

## umx graph set <alias> --<setting> <value>

Changes a loaded graph's settings in `graph.json`. Give one or more settings; every name is checked before
anything changes. The flag is the setting's name exactly, in snake_case.

    umx graph set launch --title "Launch planning"
    umx graph set launch --visibility named
    umx graph set launch --embedder trusted
    umx graph set launch --related off

| setting | what it is |
|---|---|
| `title` | the graph's title, shown by `umx graph list` |
| `policy` | `open` or `secured`: sets both settings below (see Embedding policy below) |
| `visibility` | `open`, or `named`: left out of unscoped search and multi-graph reads, read only when named |
| `embedder` | `any`, `trusted` (any instance in the machine config, local or hosted) or `local` |
| `purpose` | what the graph is for, shown first by `umx graph guide`; an empty value clears it |
| `conventions` | how the graph is used (naming, notes, when to add an edge), shown by `umx graph guide` |
| `description` | one line on what a type, status, relation or field means: `<kind>:<name>=<text>` |
| `related` | `on` or `off`: whether node add and batch look for related nodes for each node they add (`umx docs get node`), over this machine's `related` setting (on unless set off); never set, the machine's decides. A call's `--related` or `--no-related` wins |

    umx graph set launch --purpose "Decide how to launch in spring"
    umx graph set launch --description "type:risk=Something that may go wrong"
    umx graph set launch --description "field:risk.likelihood=0 to 1, how likely it is"

`--description` takes `<kind>:<name>=<text>`, the kind `type`, `status`, `relation` or `field`, and a field named
`<type>.<field>`. An empty text clears it. A type or field must be declared first (`description_undeclared` names
the `umx ontology add ... --description` that declares it with one); a status or relation need not be (warned
`off_vocabulary_*`). See `umx docs get guide` for what the guide shows and the size limits.

An unknown setting is refused (`setting_unknown`), and its message lists the settings. The vocabulary is not a
setting: add to it with `umx ontology add` (`umx docs get ontology`).

## umx graph guide <alias>

What a graph is for and what its words mean, for an agent new to it: the purpose and conventions, then every
type (with its fields), status and relation with its description, how many nodes or edges use it, and one
example. Run it first on a graph you have not worked in. See `umx docs get guide`.

    umx graph guide launch

The guide ends with the graph's spine line: the best spine `umx graph spine propose` offers and its cover, or
`none offered`, with the command that lists them all. It is a hint, never a warning.

## umx graph spine propose <alias>

Lists the spines the graph could have: the orders its nodes could be laid out along (`umx docs get time`). It
reads only and stores nothing.

    umx graph spine propose demo
    umx graph spine propose demo --unplaced type:segment --limit 50

Each candidate has a unique `name` (`type:<t>` for an anchor type, else the built-in's own), a `source`
(`anchor_type`: a type with few nodes that many others link to; or `builtin`: `recorded` by `created`, `status` by
the latest status move, `date_field:<f>` for each declared date field, `source` by evidence position), a `clock`
(`calendar`, `local` or `order`), and for an anchor type the `decl` it proposes (`name`, `anchors`, `order`,
`place`, `main` always false). The candidate `type:segment` proposes the spine named `segment`: use the candidate
name with `--unplaced`, the spine name in your conventions. Only a date orders nodes that are not anchors; a `step`
counts inside its parent, and with no field the anchors follow the declared chain. `cover` counts every node once,
by its strongest placement: `by_field`, then `by_link`, then `by_evidence`, then `by_rule`, else `not_placed`. A
node is placed by rule when nothing else places it and it links (either way) to nodes placed by field, link or
evidence: it sits in the middle of their bands (the lower median), each neighbour counted once however many links
join them. A rule placement never places another node, no node an anchor sits inside (every meeting a segment is
`part_of`) is placed by rule, and `spread` counts placements by data only: a rule placement is hollow. `spread` gives the
`bands`, the `largest_band_share` and the `distinct_values`. A candidate is `offered` unless `why_not` says
`no_cover`, `no_order` (anchors with no date, step or chain) or `no_spread` (fewer than 2 bands, or one band holds
0.95 of the placed nodes). Offered candidates come first, then the most placed.

`in_the_way` lists what stands between the graph and a good spine, each with a `count`, up to 5 `examples` (ids)
and a `next` command: `steps_as_field_values` (a text or int field whose few values sit on most nodes: make them
nodes), `order_disagrees` (a chain such as `precedes` against the `step` numbers, worked out per `part_of`
parent) and `link_evidence_disagree` (a node linked to one anchor whose evidence sits in another's). `hints` counts
the candidates not offered and the nodes the best one does not place.

`--unplaced <candidate>` adds `unplaced`: the ids that candidate does not place, a page at a time with `--limit`
(100 when not given) and `--offset`, and `total`. `--by-rule <candidate>` adds `by_rule`, the same way, for the ids
it places only by rule; give one of the two. `hints.by_rule` counts the best offered candidate's rule placements
with the `--by-rule` command that lists them (null when it has none), and the guide's and check's spine line carry
the same command as `by_rule_next`. Name the candidate exactly as listed (`type:segment`,
`recorded`, `date_field:due_on`); an unknown name is `usage_invalid`, with the candidates listed.

## umx graph spine confirm <alias> <candidate>

Declares a proposed spine: proposes again and stores the candidate's `decl` in graph.json's `spines` exactly as
`propose` lists it (`main` false). Name an anchor-type candidate as listed (`type:segment`); an unknown name is
`usage_invalid` with the candidates listed, and a built-in (`recorded`, `status`, `date_field:<f>`, `source`) is
`spine_builtin_candidate`: every graph has those, so there is nothing to confirm (write a declaration and `set` it).
The declaration is checked as `set` checks one.

    umx graph spine confirm demo type:segment

## umx graph spine set <alias> --file <decl.json>

Declares a spine you write, or replaces the declared spine of the same name. `--file` holds one JSON object (`-`
reads stdin; at most 64 KiB), the shape `propose` lists as `decl`:

    {"name": "day", "title": "segment by starts_on", "main": true,
     "anchors": {"type": "segment", "inside": "part_of"},
     "order": ["starts_on", "step", "precedes"], "place": ["raised_in", "evidence"]}

    umx graph spine set demo --file day-spine.json

- `name` is a snake_case name, unique among the graph's spines; `title` is one line of 1 to 500 characters with no
  leading or trailing space, stored as given (a title that looks like a secret warns `setting_secret_suspected`).
- `anchors.type` is a type of the graph (its ontology) or std; `anchors.inside`, if given, a relation.
- `order` holds fields of the anchor type (its declared fields, or std ones such as `starts_on` and `step`) or
  relations (a chain such as `precedes`), first that applies; `place` holds relations, strongest first, and
  `evidence` for evidence position.
- A name that is neither the graph's vocabulary nor std is refused, `spine_name_unknown`, naming the part
  (`anchors.type`, `anchors.inside`, `order` or `place`). Its next is the command that declares it: `umx ontology
  add type`, `umx ontology add relation`, or for an `order` name no edge uses, `umx ontology add field <type>
  <name>:date`. `propose` may offer a candidate built from a type or relation the graph has not declared; run that
  next, then confirm again. Another shape, or a key it does not have, is `usage_invalid`.
- At most 16 `order` entries and 16 `place` entries, and at most 32 spines in a graph: past a cap it is
  `usage_invalid`.
- A graph has at most one `main` spine: the builder's primary reading. Declaring a second is `spine_main_taken`,
  whose next shows the main one; set that one with `"main": false` first.
- `recorded` and `status` are the built-in spines `--along` reads (`umx docs get time`), so a declaration may not
  take either name (`spine_name_reserved`, which suggests `<name>_spine`; `date_field:<f>` is not a snake_case
  name). One stored before umx reserved them still loads; `umx check` warns `spine_name_reserved_stored`.
- A refusal writes nothing. The same declaration again is `unchanged`; a changed one is `replaced`.

The first declared spine raises graph.json to format 6 (the reply's `format`), which an older umx refuses to load
(`graph_format_newer`) rather than drop the spines. A graph that declares none keeps its format. A graph.json whose
stored spines umx could not have written (a hand edit: a name that is not a vocabulary name, two of one name, two
`main` spines, a padded title or an entry past a cap) is refused at load, as any graph.json that does not read is.

## umx graph spine show <alias> [<name>]

Places the graph on each declared spine (or the one named) as it is now, with the rule `propose` uses: each with
its `decl`, `clock`, `cover` (`by_field`, `by_link`, `by_evidence`, `by_rule`, `not_placed`), `spread`, and
`offered` and `why_not` as `propose` judges a candidate, and `by_rule_next` (the command that lists the nodes it
places only by rule, or null). It reads only. With none declared, `spines` is empty and `next` is `umx graph spine
propose <alias>`. A name not declared is `spine_not_found`, with the declared ones as candidates. `--by-rule
<name>` shows that spine and adds `by_rule`: the ids it places only by rule, a page at a time with `--limit` (100
when not given) and `--offset`. `--unplaced <name>` adds `unplaced` the same way, for the ids it does not place;
give one of the two. These are the lists the `--along` hints name (`umx docs get time`).

    umx graph spine show demo
    umx graph spine show demo day
    umx graph spine show demo --by-rule day
    umx graph spine show demo --unplaced day

## umx graph spine remove <alias> <name>

Drops one declared spine (`spine_not_found` if there is none of that name). The format stays: a write never lowers
it.

    umx graph spine remove demo day

## Embedding policy

A graph has two settings (D53). `visibility` is `open` or `named` (left out of unscoped search and multi-graph reads;
read only when named). `embedder` is `any`, `trusted` or `local`: which embedder instances may get its text
(`umx docs get embeddings`). `policy` in graph.json's `embedding` section is the short form: `open` is open + any,
`secured` is named + local. A graph with no section, or no `policy` key, is `open`. Any other pair is written as
`"policy": "secured"` with `visibility` and `embedder` beside it, so an older umx reads it as secured; a file with
those keys in any other form is refused at load (`graph_invalid`). This happens when an older umx sets `policy` to
`open` and keeps the keys. Fix it by removing both keys or by setting `policy` back to `secured`. A graph's `embedding` shows the file's words;
`effective_policy` shows the pair umx acts on (`open`, `secured`, or such as `named+trusted`), with
`effective_visibility` and `effective_embedder`.
Tightening an open graph leaves the vector files it wrote while open in its folder: `secured_shards_remain` (see
`umx docs get embeddings`, Vectors left from when a graph was open).

graph.json can change under you: a pull, a merge, a hand edit, an agent. So this machine also keeps a record,
`$UMX_HOME/state/graph-policies.json`, of the last policy umx recorded here for each graph, by graph id and by folder
(so every copy of a secured graph, such as a git worktree, stays secured). umx acts on the stricter of the two,
shown as `effective_policy`. Each setting is compared on its own: loosening either (named to open, local to trusted,
trusted to any) is a lowering.

- Raising the policy takes effect at once and is recorded: `graph new --secured`, `graph set <alias> --policy
  secured`, or loading a graph whose graph.json says `secured`.
- Lowering it is one umx call, by any caller, with no terminal step: `graph set <alias> --embedder trusted` (or
  `--visibility open`, `--policy open`) writes graph.json and lowers this machine's record in the same call. The
  reply warns `policy_lowered`: what loosened (from and to, the stricter of graph.json and the record before and
  after), each embedder instance of this machine that may now get the graph's text and could not before (its
  name, local or hosted, and a hosted one's endpoint host), and that the change is recorded, with when. Lowered to
  `open`, it also says new vectors now go in the graph folder itself. It is a warning, never a block: lowering a
  graph is its owner's decision, so lower one only when its owner asked for it. Once lowered, the daemon embeds the
  graph's nodes with the newly allowed instance by itself.
- A graph.json lowered outside umx (the value changed to `open`, the `policy` key deleted, or the whole
  `embedding` section deleted, as by a pull) never loosens the graph by itself: it stays at the recorded policy
  and `graph load` and `graph set` warn `policy_downgrade_unconfirmed`. Its owner may confirm it, after the graph
  is loaded with its new graph.json, with one call that warns as above:

      umx graph confirm-policy client_x

- Every downgrade is recorded, and `graph list` and `graph load` show `downgraded` (when) until the graph is
  raised again. Confirming a raise or an unchanged policy changes nothing more.

The record is checked like `config.toml`: a plain file owned by you with mode 600, in a folder with mode 700.
If it is refused, does not parse, or is missing while its folder exists (deleted), every graph counts as secured
(`local_state_unreadable`, naming the file and why) and umx never rewrites it; fix or remove it by hand. An empty
`state/` folder with no file is what a crash during the very first write leaves: if this machine never confirmed a
policy, remove the empty folder (`rmdir ~/.umx/state`, or under `$UMX_HOME`); otherwise restore the file.

If recording a secured policy fails (`local_state_not_saved`), the daemon keeps the graph secured, retries with
every later write of the record, and refuses `graph set --policy open` (`policy_not_recorded`) until the secured
policy is recorded. If graph.json took a lower policy but the record could not (`policy_not_lowered`), the graph
stays at its recorded policy until `umx graph confirm-policy <alias>` records it. `local_state_not_durable` means
the record was written but its folder sync failed. The record protects only this machine: a fresh clone
elsewhere trusts its graph.json until its first secured load or confirm there.

A secured graph with a configured embedder that sends text off the machine warns `embedder_not_local_for_secured`,
and none of its text is embedded.

## umx graph unload <alias>

Drops a graph from the daemon's memory. Every write already reached its files; first it writes the `similar/` files
it still owes while vectors arrive (`umx docs get embeddings`), and warns `write_failed` or `write_not_durable` if
that did not fully land (the next load then rebuilds the lists). Load it again with `umx graph load`. An unloaded
graph does not come back when the daemon restarts, and naming it does not load it (`graph_not_loaded`, whose next is
the `umx graph load` command for its folder). Unloading a graph that did not come back after a restart (see below)
makes the daemon forget it.

    umx graph unload launch

## After a daemon restart

Graphs you loaded come back when the daemon starts again (`umx daemon restart`, stop then start, `just dev`, the
service). The daemon reloads them in the order you loaded them, under the same aliases, through the same checks as
`umx graph load` (roots, the folder still holding the same graph, the embedding policy). It remembers them in
this machine's graph registry, `~/.umx/registry/graphs.toml` (or under `$UMX_HOME`): one row per folder with its
graph id, alias, folder and whether it is loaded, plus what `graph list --all` shows of it. It is never written
into a graph or repo folder, and is trusted like the policy record (mode 600 in a 700 folder). The policy it shows
is only shown: umx never acts on it. At most 64 rows; past that, unloaded rows are
forgotten first, and a load that cannot be remembered warns `registry_not_saved`.

A graph that does not come back (its folder is gone, outside the roots, written by a newer umx, or now holding
another graph: `graph_id_changed`) stays unloaded and never stops the daemon from starting. `umx daemon status`
lists it under `registry.not_reloaded` with the reason's code, and `umx doctor` reports `doctor_graph_not_reloaded`
with the command that loads it. If the registry itself cannot be used (refused, does not parse, newer), nothing
reloads, umx never rewrites it, and loads warn `registry_unreadable`; `umx doctor` names the file.

    umx daemon status
    umx doctor

## Loading on demand

A command that names a graph (`--graph <alias>`, an address `umx:<alias>/<id>`, `--graph a,b`, or a saved scope's
members with `--scope <name>`) which unloaded
when idle, or did not come back after a restart, loads it first and then runs, with a `graph_loaded_on_demand`
hint naming the folder. If that load fails, you get the load's own error (such as `graph_not_found`). A command
that names no graph never loads one, and a graph you unloaded is never loaded on demand. With `--graph a,b` or a
scope, every graph in it that unloaded when idle loads, one `graph_loaded_on_demand` each; if one of them was
unloaded on purpose, the command is refused (`graph_not_loaded` or `scope_graph_not_loaded`) and nothing loads. If
one graph's own load fails partway through, the graphs loaded before it stay loaded and you get that load's error.

## Idle unload

A graph no command has used for 30 minutes unloads itself. Change that with `umx daemon start --idle-unload
<duration>` (`90s`, `30m`, `2h`, from `1s` to `720h`; `0` means never). The daemon sleeps until the next graph is due, so an idle
daemon still uses no CPU. `umx graph list` does not count as using a graph. A graph that unloaded when idle is
still remembered: the next command that names it loads it again (see Loading on demand), and a restart reloads it.

    umx graph list
    umx graph list --human

## Choosing a graph

An address `umx:<alias>/<id>` names its own graph. Otherwise `--graph <alias>` picks one, else `UMX_GRAPH`, else
the only loaded graph. With several loaded and none named, the error's `next` repeats your command with
`--graph`. An address and a `--graph` that disagree are refused (`graph_mismatch`). When every loaded graph is a
copy of one graph, the command takes its primary (see Copies below). Give `--graph` once: a second one is refused
(`usage_invalid`), never read as the last one given; several graphs go in one `--graph a,b` (`umx docs get scope`).

One folder is one loaded graph, however its path is spelled (symlinks, `/tmp` and `/private/tmp`): loading it
again replaces the earlier load, under the new alias.

## Copies of one graph

A graph's own id is reserved for its virtual graph node. A stored node using that id is not loaded:
`node_id_is_graph_id` names its first line, and `umx check` lists every line using it. Those lines, and local
edges naming the missing stored node, are kept byte for byte through unrelated writes. Change the line
by hand to a new id, or add a replacement with `umx node add` and re-point the edges. Reload after fixing it.
A node in another graph may use the same id: graph-qualified identities keep those nodes distinct.

The same graph (the same graph id, as in two worktrees) can be loaded twice under two aliases:
`umx graph load <path> --as <alias>`. Each load is a copy. A command that names no graph reads one copy of each
graph, its primary: the copy loaded first (a reload of a folder keeps its place). The others are left out and named
in `copies_skipped`, so counts, search hits and walks are never doubled. Secured graphs are left out first, so a
secured graph's copies are only counted. Query, search, node list and edge list read several graphs:
naming copies with `--graph a,b` reads each, with `query_graph_twice` and copy provenance.
Saved selections use `--scope <name>` (`umx docs get scope`); they resolve graph ids to primary copies.
Walk, path and neighbours also accept graph lists and saved scopes, crossing external links while keeping
similar_to local (`umx docs get explore`). With no graph named, single-graph reads take the primary
when the loaded graphs are copies of one open graph. Name one copy with `--graph`. When the primary
is unloaded and one copy is left, that copy is primary; with two or more left none is, and a command that names
no graph is refused with `copy_ambiguous`, whose next is `umx graph primary <alias>` (below); naming a copy with
`--graph` also works. Both survive a restart: the daemon reloads graphs in the order they were loaded, so the
primary stays the primary, and a graph with no primary still has none while two or more of its copies come back.
A no-primary state clears when one copy is left or `umx graph primary` names one; if only one copy comes back after
a restart, it is primary again, and `umx daemon status` warns `primary_restored` naming it. A secured graph's copies
are never named in `copies_skipped`; they are counted with the secured graphs.

## umx graph primary <alias>

Makes the named copy the primary of its graph: commands that name no graph read it from now on, and it is still
the primary after a daemon restart (its row in this machine's graph index records it as loaded and moves ahead of
the other copies').

    umx graph primary design_b

The copy must be loaded or remembered: a remembered copy that unloaded after being idle loads again first, as for
any command that names a graph. A copy you unloaded with `umx graph unload`, or never loaded, is `graph_not_loaded`.
The reply is `alias`, `graph_id` and `unchanged` (true when it already was the primary, or is the only copy). It
never names the other copies. A secured copy may be made primary, but then reads that name no graph skip that graph
id entirely, and the reply warns `primary_is_secured`; name a copy with `--graph` to read it. A graph index that
cannot be written warns `registry_update_not_saved` and the choice holds until the daemon stops; run the same
`umx graph primary <alias>` again once the index can be written, and it is saved (still `unchanged: true`).

## Where graphs may live: roots

The daemon reads and writes graph folders only inside its roots: your home folder and your temp folder by
default. To use other folders, set the `roots` setting (`umx config set roots <folders>`, colon-separated
absolute folders), or `UMX_ROOTS` for one daemon, then restart the daemon. A folder outside the roots, or a
symlink leading out of them, is refused with `path_outside_roots`, whose message names the current roots. An
invalid `roots` setting closes every folder (`config_invalid`); see `umx docs get config`.

    umx config set roots /Users/me/work:/tmp/graphs
    umx daemon stop
    UMX_ROOTS=/Users/me/work:/tmp/graphs umx daemon start      # in another window
    umx graph new /tmp/graphs/scratch --alias scratch

## Errors

`graph_exists` (use `umx graph load`), `graph_not_found` (use `umx graph new`), `graph_invalid` (names the file
and line that does not parse), `graph_format_newer` (a newer umx wrote the graph; nothing is loaded),
`graph_too_large` (see Size limits),
`alias_invalid`, `alias_in_use`, `path_outside_roots`, `write_failed` (nothing changed), `write_not_durable` (a
warning: the change was made but the folder sync after it failed), `graph_not_loaded` (says why: unloaded, or the daemon started and has not loaded it; its next loads it when the
daemon remembers the folder), `graph_id_changed` (a remembered folder now holds another graph), `registry_unreadable`
and `registry_not_saved` (warnings about this machine's graph registry), `graph_loaded_on_demand` (a hint: the
named graph was loaded for the command), `setting_unknown`,
`description_too_long`, `description_undeclared` and the warning `setting_secret_suspected` (from `umx graph set`,
see `umx docs get guide`),
`embedder_mismatch` (a warning: the configured embedder's model differs from the graph's; nothing is mixed, new
vectors wait as pending, and its next, `umx embeddings reembed --graph <alias> --dry-run`, shows what re-embedding
would cost),
`recovery_conflict` (see below), `graph_files_changed` and the warning `stale_files` (see Files changed on disk),
and for the embedding policy `policy_lowered`, `policy_not_lowered`, `policy_downgrade_unconfirmed`,
`local_state_unreadable`, `local_state_not_saved`, `local_state_not_durable` (warnings) and `policy_not_recorded`
(see Embedding policy).

## Size limits

Each kind of graph file has a cap: `graph.json` 1 MiB, `.gitignore` 64 KiB, a vector shard
(`embeddings/<model@dims>/<xx>.tsv`) 64 MiB, `nodes.jsonl` and `edges.jsonl` 4 GiB (4294967295 bytes). A file over
its cap is refused with `graph_too_large`, naming the file, its size and the cap; umx checks the size before
reading and never reads past the cap, so nothing is loaded. A write that would take a file over its cap is
refused the same way and changes nothing. A file that big is almost always a bad merge or something copied in by
mistake: find where it came from (`git log -p -- <file>`) rather than trimming it.

## Writes and crashes

Every file is written whole: to a temp file in the same folder, synced, then renamed over the old one. A change
to several files (a node with its parent edge, a new graph) is all or nothing: umx also writes a small marker,
`.umx-tmp-set.json`, before the renames and removes it after. If the daemon dies part way, the next command on
that folder finishes the renames. Temp files and the marker start with `.umx-tmp-`; the `.gitignore` line keeps
them out of git, and they never show as graph files.

If a file changed between the crash and the next command (a `git pull`, a checkout, a hand edit), umx will not
write over it. Every command on that folder fails with `recovery_conflict`, naming the files, and changes
nothing; `umx check` lists them for a loaded graph. To settle it, choose one side:

- keep the files as they are now: delete `.umx-tmp-set.json` and the other `.umx-tmp-` files in the folder;
- or finish the cut-off write: put the named files back as they were before the change (for example with
  `git checkout`), and run the command again.

## Files changed on disk

A loaded graph is a copy in memory of its files as umx loaded them. When those files change behind umx (a `git
checkout` or `git pull`, a merge, a hand edit, another tool, or only a `touch`), the copy is out of date, and
writing it would put the old copy back over the new files. So umx refuses:

- every write to that graph fails with `graph_files_changed`, naming the graph and the files that changed; nothing
  is written and the graph in memory is as it was;
- every read of it still answers from memory, with the warning `stale_files` naming the same files.

umx never reloads on its own: which copy you meant to work on is your call. To work on the files as they are now,
unload the graph and load it again (the `next` of both codes runs the two as one line, joined by `&&`):

    umx graph unload launch
    umx graph load ~/work/launch --as launch

What umx compares is each of `graph.json`, `nodes.jsonl` and `edges.jsonl` as it read or last wrote it (device,
inode, size and times), not its bytes, so a touch that changes no byte is refused too: umx errs on the side of not
writing. It looks before a write starts and again just before the write commits; a change that lands in the
microseconds between that last look and the commit is not seen. Vector and similar_to files are written only
while those three are unchanged; their own shards are not compared.

## Check

`umx check` checks persisted records, not a result view. It keeps malformed-provenance diagnostics
and does not accept `--provenance` (`usage_invalid`).

A graph's files can hold things that load but need a person: lines out of id order or blank lines in
`nodes.jsonl` or `edges.jsonl` (as after a git merge that appended both sides), the same node or edge id on two
lines (as after a git merge that kept both sides of an edit), an edge whose `from` or `to` names a node that is
not in the graph (as after a merge where one branch deleted the node), or a key under `ontology` or `embedding` in
graph.json that this umx does not know. `umx graph load` warns about each (`unsorted_file`, `duplicate_id`,
`dangling_edge`, `unknown_key`, `provenance_key_invalid`, `duplicate_prov_key`, `reserved_type`; at most 20, then
`load_warnings_not_shown` with the count) and loads the graph.
Nothing is dropped:

- An out-of-order file loads as usual; the next write of that file puts it in id order without blank lines, so
  that write's diff moves lines. `unsorted_file` names the first line out of place.

- For a repeated id the last line is the one in use; every earlier line is kept and written back unchanged by
  each write. To settle it, keep the line you want, delete the others and load the graph again.
- A dangling edge is kept in the file, unchanged, but is not an edge of the loaded graph: `edge list` and walks
  do not see it. It joins the graph at the next load that finds its node.
- Unknown keys are kept as they are.
- Two resource nodes with the same locator (as after a merge where both branches added the same file) both load
  and stay: `duplicate_locator` names the higher id, and the lower id is the one umx treats as that resource. To
  settle it, move what you want onto the lower id's node and remove the other. A resource node whose stored source
  is not in the form umx writes (a hand edit such as `./docs//a.md`) loads with `locator_not_normalised` and is
  matched as it normalises. A graph with a resource node is written at the locator format (graph format 5), which
  an older umx refuses with `graph_format_newer`.
- A custom field named `locator` whose value is not a locator (one written before umx reserved the name) loads as
  that custom field with `locator_field_kept`, and every write keeps it as it is; rename it by hand in
  nodes.jsonl. No write sets `locator` or a `locator_*` name as a custom field: `--set locator=...` is
  `locator_field_invalid`, and nothing changes.

`graph load` and `graph list` show `kept_lines`: lines in the files that are not in the graph (earlier lines of
repeated ids and dangling edges), so `nodes`, `edges` and `kept_lines` add up to every record line on disk.

    umx check --graph demo

lists what the files hold now, one row per finding: `kind` (`unsorted_file`, `duplicate_id`, `dangling_edge`, `unknown_key`,
`provenance_key_invalid` for a malformed provenance value, kept and not used, `duplicate_prov_key` for
two provenance records with one key, both kept with the lower id in use,
`duplicate_locator`, `locator_not_normalised`, `locator_field_kept`,
`edge_reserved_field_in_use` for an edge with its own `updated` or `updated_by` field, which `umx edge edit` will
not overwrite (rename it in edges.jsonl),
`date_not_a_date` for a node's or edge's date field whose stored value is not a date in years 0001 to 9999 (subject
`<id> <field>`; it loads as written and compares as text; also a warning, whose next sets a date),
`recovery_conflict` for a cut-off write that cannot be finished, see Writes and crashes, or
`gitignore_line_missing` when git, reading the folder's `.gitignore`, would not hide every `.umx-tmp-` file: no line
hides them, or a later `!` line un-hides some, read as git reads it with `core.ignorecase` either false or true;
that one also comes back as a warning), `file`, `subject` (the id, the key's path, or the file's name) and
`lines`.

`data.spines` gives each checked graph's spine line, as `umx graph guide` shows it; it never changes the status or
the exit code.

### umx check --lint

    umx check --graph demo --lint
    umx check --graph demo --lint --kind orphans --offset 100

`--lint` (F139) also reads the content of every checked graph for what a reader of a shared graph would trip on. It
reads only and never refuses: each kind it finds gives one hint (with the count, the first id and a next that
fixes it), and `data.lint.rows` lists every finding: `graph`, `kind`, `id` (the node's, an edge's, or null for
graph.json) and `place`, named as a read's `secret_suspected` names it (`field:title`, `field:note`, `key:<i>`,
`evidence:<i>`, `status_reason:<i>`, `status_evidence:<i>`, `evidence_removed:<i>`, `edge:<id>/evidence:<i>`,
`edge:<id>/field <n>`, `graph:title`, `graph:purpose`, `graph:conventions` or `graph:descriptions`). A list entry is
named by its position, counted from 1 (`evidence:1` is the first pointer). A custom field is named by its place among
the record's custom fields as its line holds them (a hand edit's order too), counted from 1 (`field 2`), never by its
name, since a name may hold a term. A redact's candidates and an edge's `secret_suspected` number the same entry and
field alike; a node's `secret_suspected` counts list entries alike but names a custom field by its name
(`field:zeta`).
The kinds:

- `orphans` (`lint_orphans`): nodes with no edge at all. A `similar_to` or provenance link does not count, and the
  std anchor types (meeting, event, segment, transcript) are left out: an anchor may stand alone.
- `proposed_keys` (`lint_proposed_keys`): a usable `entity_key` (one the entity index would read) on a node whose
  status is `proposed`.
- `terms` (`lint_terms`): a place holding a word from this machine's lint terms file (below), matched in any case
  with the redact's case fold (`ß` matches `ss`). Every text a read shows is scanned: titles, notes, custom fields
  and their names, keys, evidence, status reasons and pointers (with a redacted pointer's reason), the removal trail,
  edge evidence and fields, and graph.json's title, purpose, conventions and descriptions, as the check read them.
  The hint's first place is the row's id and place (`<id> field 2`).
- `double_negatives` (`lint_double_negatives`): a title that says "lacks no" (or "lack no", "lacking no").
- `raised_in_evidence` (`lint_raised_in_evidence`): a `raised_in` link whose node's evidence and anchor's evidence
  share a source and unit (`seq` or `lines`) but never overlap, one row per link. The row adds `anchor` and `ranges`
  (`unit`, then the nearest pair: the node's pointer and the anchor's).
- `sensitivity_label`: umx has no sensitivity label field, so it never runs.

`data.lint.skipped` names each kind that did not run, and why. Each graph lists at most 100 rows per kind;
`lint_truncated` gives how many more and the next page, `--kind <kind> --offset <n>` (`--kind` runs one kind; both
need `--lint`). Only the graphs the check reads are linted: a secured graph is linted only when named.

The lint terms file is `$UMX_HOME/lint/lint-terms.json`, written by hand and read fresh on each `--lint`:

    {"umx_lint_terms": 1, "terms": [{"term": "<word>"}, {"term": "<word>", "graphs": ["<graph id>"]}]}

A term without `graphs` applies to every graph. Each term is 2 to 100 characters with no leading or trailing
space, and there are at most 256 terms. The file must be the owner's only: 0600, in a 0700 folder. A file that does
not parse, is newer, breaks a limit, or that others can read skips the term scan with one hint,
`lint_terms_unusable`, naming the file. With no file there is no term scan and no hint. A hit names the node and
the place, never the term or the text. The file is a plain local file any process of yours can read: a list of
words to keep out of graphs, not a place for credentials (`secret_suspected` covers those). It is never in a graph
folder or in config, and never sent to the daemon over the socket.

A link to another graph is read only when that graph is named, unless it is open: a row `graph_not_loaded`
(subject `<edge> <end> umx:<graph id>/<node>`) is the same for a graph that is not loaded and one whose visibility
keeps it out of unnamed reads, and `link_targets_unnamed` (information) counts the links whose graphs the check did
not name. Name them all in one check:

    umx check --graph acme,acme_d1,acme_d2
    umx check --scope acme_all

`--graph a,b` or `--scope <saved scope>` checks every graph in it, and every one counts as named, so links between
them resolve (or show `edge_broken` when the node is gone). Each row then has `graph`, and `data.graphs` gives each
graph's own `total`. A listed graph that is not loaded is refused, as one `--graph x` is. `umx doctor` runs this check
over every loaded graph, so links between loaded graphs never count as problems.
