# Ontology: the graph's vocabulary

Each graph declares its node types (with the custom fields each carries), statuses and relations. The
vocabulary is soft: off-vocabulary writes are accepted with a warning naming the `umx ontology add` command
that would declare the name. Change the vocabulary as the work needs.

## umx ontology show

Lists types (with their fields, kinds, roles and declared `values`, and default list `columns` when set), statuses, relations, the symmetric relations and
`relation_fields` (the fields declared on each relation's edges), with the purpose, conventions and descriptions
that `umx graph guide` shows. Each field shows `role` (`content` or `data`, as umx acts on it) and `role_default`
(true when no role is stated, so the field is data). For a std graph it is the vocabulary umx acts on, std v1's
entries first, each marked (see std below).

    umx ontology show
    umx ontology show --graph launch
    umx ontology show --std        # std v1 alone, whatever graph

## umx ontology add ...

    umx ontology add type risk
    umx ontology add status parked
    umx ontology add relation mitigates
    umx ontology add relation rivals --symmetric   # a rivals b means b rivals a; declares it too if new
    umx ontology add field risk impact:string --values low,medium,high
    umx ontology add field --relation works_for starts_on:date   # a field on the relation's edges
    umx ontology add field risk,issue,task score:int   # several declared types (std's count), one write
    umx ontology add field --all-types reference:string   # every type graph.json declares (not std's), one write
    umx ontology add status parked --description "Waiting on someone else"
    umx ontology add field doc file:string --role data
    umx ontology add field doc summary:string --role content   # prose: searched by meaning
    umx ontology add relation works_for --from person --to team   # expected end types (soft), see std below
    umx ontology add tag origin:double_diamond   # a std tag (std graphs); see Tags below
    umx ontology add tag phase:*                 # every value of a facet

One field on several types (a comma list, or several types before the field) or on `--all-types` is one write,
all or nothing: if any type is not declared (`type_unknown`) or already has the field with another kind
(`field_conflict`) or role (`field_role_conflict`), nothing changes and each culprit is its own error. Types that
already have the field with the same kind and role are listed under `unchanged`. `--role` applies to every type.
Only the one-type form declares a new type.

Adding what is already there, the same way, is ok and writes nothing: the reply has `"unchanged": true`, so a setup
script can run again. That holds for a type whose given `--field`s it already has with the same kinds, a status, a
relation with the same `--symmetric` flag, and a field with the same kind and role. A declaration that differs is
refused and changes nothing: a field with another kind (`field_conflict`), a field with another role
(`field_role_conflict`: change a role only with `ontology set field`), a type re-declared with a field it lacks
(`field_missing`, whose next adds it), or a plain `add relation` of a symmetric relation such as `conflicts_with`
(`already_declared`). No `--role` means data, so re-declaring a field stated content without `--role content` is
`field_role_conflict`, and re-declaring a field stated data without `--role` is `unchanged`.

`--description <text>` on any `ontology add` gives the new name one line on what it means, shown by `umx graph
guide` (`umx docs get guide`). Re-declaring a name with the description it has, or none, is
`unchanged`; with another description it is `already_declared`, whose next is the
`umx graph set <alias> --description <kind>:<name>=<text>` that changes it. On several types a field's
description goes to each type that gets the field. A description that looks
like a secret is written and warned `setting_secret_suspected` (graph.json is tracked).

Field kinds: `string, int, float, bool, date, list` (`number` means float). A value that does not fit its
field's kind is stored as given with `field_kind_mismatch`. An undeclared field takes the kind of its first
value in the graph. Kind `date` takes a year (`2025`), a month (`2025-01`), a day or a date-time, and dates
compare by instant: `umx docs get time`.

## Declared string values

`umx ontology add field <type> <field>:string --values low,medium,high` declares an ordered list of labels.
Each label is snake_case (`[a-z][a-z0-9_]*`), at most 64 characters; there are at most 32, with no repeats.
Only string fields take values. An invalid declaration is `field_values_invalid`, writes nothing and gives a
corrected declaration in `next` when a snake_case form exists. The graph keeps the order in graph.json and
`ontology show` and `graph guide` show it.

The vocabulary is soft, not validation of node content. Any string is stored. A string outside the list warns
`field_value_off_vocabulary` with the declared labels as candidates; matching is exact and case sensitive
(`High` is not `high`). A non-string value still warns `field_kind_mismatch`. When the new string is a label,
the warning's `next` adds that one label. Otherwise its `next` edits the node
with a `<value>` placeholder; stored text never enters a message or command.
An add command longer than the 300-byte command cap uses `<values>` in place of its label.

    umx ontology set field risk impact --values low,medium,high,severe
    umx ontology set field task priority --clear-values

`ontology set field --values` replaces the graph's list; `--clear-values` makes a custom field unrestricted.
Give one of those flags, not both; either may be combined with `--role` in one write. Adding the same field and
list again, or a subset, is unchanged; adding labels appends only the missing ones, without reordering.
Appending beyond 32 is `field_values_invalid`, whose next sets a shorter list. An add without
`--values` leaves an existing list alone. On a std field, the graph may add labels but never remove std's:
std's labels stay first, followed by the graph's additions; clearing removes only the additions. There is no
graph format bump: older umx keeps the key but does not check values.
Only graph-added labels are stored, never copies of std's labels.

std v1 gives risks `likelihood` and `impact`, both strings with `low,medium,high`. Existing std 1 graphs gain them
when umx is updated, with no adoption or migration. Neither field is required. A graph may keep its own
non-string rating kind: `std_field_conflict_stored` warns and the graph's kind is used, including on load and
adoption. On other types, fields named `likelihood` or `impact` are custom and may have any kind.
Link a risk's owner as
`umx edge add <person|team|organisation> owns <risk>`, not an `owner` string. Link a mitigating node with
`umx edge add <node> mitigates <risk>`; a non-risk target is still stored with `relation_endpoint_unexpected`.

## umx ontology set type <type> --columns | --reset

    umx ontology set type risk --columns title,likelihood,impact   # node list --type risk shows these
    umx ontology set type risk --reset                            # back to the usual columns

Gives a declared type its own default `node list` columns. `umx node list --type risk` then shows `id` and these
columns instead of `id, type, title, status, updated`; an explicit `--columns` (including `--columns all`) always wins, and a list without
`--type` keeps the usual columns. Each column must be a standard node field (`title`, `status`, `updated`,
`evidence_kinds`...) or a field the type declares: another name is refused with `field_missing`, whose next
declares it, and an undeclared type with `type_unknown`. Give `--columns` or `--reset`, not both (`usage_invalid`).
Setting the same list again, or resetting a type with none, writes nothing (`"unchanged": true`). A type takes at most 32
columns, and the provenance fields (`prov_key`, `authored_by_session`, `edited_by_session`) are never columns: both
are refused with `usage_invalid`, and a provenance key found in graph.json is skipped and reported; node list reads at most 32 from graph.json.

`umx ontology show` lists a type's `columns` when it has some; graph.json keeps them under the type and writes the
key only when set. A column naming a field the type does not declare (a hand edit) is skipped by node list and
reported on load as `type_column_unknown`. Across graphs, a type's columns are used only when every graph in scope
that declares it gives the same list.

## Field roles: content or data

Every declared field has a role. Content fields are embedded (a field vector and a line in the whole-node text);
data fields never are. A node's title and note are always embedded. A field with no stated role is data, whatever
its kind (PRD 002 search-04): declare a string field that holds prose (sentences that differ from node to node)
with `--role content` so search by meaning finds it. Leave as data a field that holds labels, names, file names,
ids, agent ids, seats, hashes, locators or provenance: their text means nothing to an embedding and only adds
noise to similarity. `int, float, bool, date, list` are always data and cannot be content (`role_not_embeddable`).
Word search, exact match and `--where` treat both roles the same.

    umx ontology add field doc summary:string --role content
    umx ontology add field doc file:string             # data, the default
    umx ontology set field doc summary --role data     # change it later

Declaring a string field with no `--role` (with `ontology add field`, or `ontology add type --field`) gives the
hint `field_data_by_default`: the field is data, so search by meaning never sees it, and its next is the
`umx ontology set field <type> <field> --role content` that makes it content. Run it only when the field holds
prose. A re-declare that changes nothing gives no hint. A graph made before this default keeps the vectors its
role-less string fields have (still searched) until `umx embeddings reembed`; see `umx docs get embeddings`.

`ontology set field` changes a declared field's role (`field_not_declared` if the type does not declare it). The
nodes of that type with text in the field whose vectors that leaves out of date then have pending vectors, and the
background worker remakes them: the whole-node vector, plus the field's own vector when it becomes content; a data
field's vectors are dropped. The reply's `pending` and the `role_changed` warning say how many. A node whose vectors
already match its new texts (stating content a field that a graph made before this default embedded) is not
pending, and nothing is embedded for it. `prov_key, authored_by_session,
edited_by_session, entity_key` and the legacy identity keys `person_key, org_key, team_key` (older graphs may
still carry them), and provenance nodes, are never embedded, whatever is declared.
A role needs graph.json format 4, so an older umx refuses the graph instead of embedding its data fields.

umx never changes a role by itself, but it points out a string field whose values say it is in the wrong group,
with the `umx ontology set field` command that moves it, as a hint on `ontology add field` (for the field it
adds), `graph guide` and `embeddings status` (open graphs only). `field_looks_like_labels`: a content field
whose values are labels (at least 6 values, at most 12 distinct, each used twice on average, none over 40
characters); it says how many values it holds, each a field vector and an embedder call that data would save.
`field_looks_like_prose`: a data field whose values are prose (at least 3 values, more than half of 8 words or
more, at least 4 in 5 different). Hints carry counts only, never a value. Numbers, dates, booleans and lists,
and the fields umx never embeds, are never pointed out. Act on a hint only when the field is what it says;
the role stays as declared until the command runs.

## Edge fields

Edges carry custom fields too (`umx edge add a supports b --set confidence=0.7`), with the same soft vocabulary
as nodes. A field the relation does not declare is written with `off_vocabulary_edge_field`, whose `next` is the
`umx ontology add field --relation <relation> <name>:<kind>` that declares it. A declared field types the value
by its kind; a value that does not fit is stored as given with `field_kind_mismatch`. Retyping an edge with
`umx edge edit <edge> --relation <r>` warns the same way for each of its fields the new relation does not declare. Declaring a field on a new
relation declares the relation too. Declaring the same field and kind again is ok with `"unchanged": true`; another
kind is `relation_field_conflict`. The derived relations `similar_to`, `authored_by` and `edited_by` take no declared fields
(`name_invalid`: umx makes or derives those edges itself).

## umx ontology remove ...

    umx ontology remove type risk
    umx ontology remove status parked
    umx ontology remove relation mitigates          # its symmetric mark and description go too
    umx ontology remove field risk impact
    umx ontology remove field --relation works_for starts_on   # a field declared on a relation's edges

A removal is refused while anything uses the entry (`ontology_in_use`, with how many nodes or edges, and a `next`
that lists them): a type while a node has it, a status while a node is in it, a relation while an edge has it, a
field while a node of that type holds it or the type's default columns name it (`umx ontology set type <t>`), a
relation's field while an edge of the relation holds it. Change or
delete those first, then remove. Removing a type takes its field declarations and description with it; removing a
relation takes its symmetric mark, description and declared fields. Only records in the graph as loaded count as in use: lines umx
keeps in the files but does not use (a duplicate id's other lines, a dangling edge) never block a removal, and
once it is done they warn off-vocabulary when the graph loads. umx refuses to remove what it relies on (`ontology_builtin`):
`part_of`, the default status `open`, the derived relations (`similar_to`, `authored_by`, `edited_by`), the
provenance types and relations, `same_as` (`umx entity link`), the standard fields and any `std:` entry. In a std
graph a field std gives the type or relation is refused with `reserved_name` (see Custom fields on std types). Removing something not declared is ok
with `"unchanged": true` and writes nothing, so setup scripts can re-run.

## Names

Every name is snake_case (`[a-z][a-z0-9_]*`), and openCypher keywords such as `match`, `group` or `order` are
refused (`name_invalid`; the `next` suggests `group_item`). Standard field names (`id, type, title, status,
note, evidence, evidence_kinds, status_history, created, updated, created_by`, and for edges
`from, to, from_graph, to_graph, from_selector, to_selector, evidence_removed, std, updated, updated_by`, the last two
being the stamp `umx edge edit` writes) cannot
be custom fields (`reserved_name`). The provenance fields `prov_key`, `authored_by_session` and
`edited_by_session` are also reserved on nodes and edges: `--set` on any of them is refused with
`reserved_name`. Use a different custom field name; do not set provenance by hand: umx records who wrote what
itself (`umx docs get node`, Who wrote it). The types `platform`, `agent`, `model`, `harness` and `session` are
umx's for provenance (a node of yours with one stays content and warns `reserved_type`); the relations
`session_of`, `runs_on`, `uses_model` and `registered_in` (`reserved_relation`) and `authored_by` and
`edited_by` (`derived_relation`) are refused on `edge add`, and so is declaring any of those relations, in every
graph: `umx ontology add relation authored_by` is `derived_relation` and `umx ontology add relation session_of` is
`reserved_relation`. In a std graph `umx ontology add type session` (or another provenance type) is
`reserved_name` too; a graph without std may declare `model`, `agent` or `platform` as its own types, as before.
Each refusal's `next` names the docs to read.

`std` is reserved as a type, relation and field name in every graph, std or not (`reserved_name`), so the
qualifier `std:` is never ambiguous: `umx ontology add type std`, a relation or field named `std`, `--type std`
and `--set std=...` are refused.

## Symmetric relations

`conflicts_with`, `alternative_to`, `possibly_same_as`, `same_as` and `not_same_as` are symmetric in every graph: `a conflicts_with b`
is the same edge as `b conflicts_with a`. `same_as` (`umx entity link`) and `not_same_as` (`umx entity deny`) are
std: every graph knows them without declaring them. `umx ontology add relation <r> --symmetric` makes another relation symmetric for
this graph, and `umx ontology show` lists them all under `symmetric`. A symmetric edge is stored once, with the
lower graph-id/node-id pair as `from` (node id within one graph), and local `neighbours`, `walk` and `path`
follow it from either end. Marking a relation whose edges
already go both ways makes each pair one edge at once (`symmetric_duplicate` names the line kept in the file).
This includes external ends: declaration and reload give the same facts, without needing a reload. The
incoming index changes only after the declaration commits, and the retained reverse line keeps its unknown
fields and any own-graph normalization. A failed write changes none of them.
`part_of` and `similar_to` are directed by what they mean and cannot be marked (`symmetric_refused`).

## std: the standard vocabulary

A graph whose ontology says `"std": 1` uses std v1, the vocabulary built into umx (`umx docs get organise` lists
it): `umx graph new` writes it unless `--no-std`. Its vocabulary is std v1's types, statuses and relations, with
the fields std places on them (`job_title` and `org` on `person`, `starts_on`, `ends_on` and `due_on` on the dated
types, `step` on `segment`, `likelihood` and `impact` on `risk` (strings, `low,medium,high`), `starts_on` and `ends_on` on `works_for` and `member_of` edges), then the graph's own
declarations. So on a std graph `umx node add --type person --set job_title=...` warns nothing, word search finds
`job_title` text, and a Cypher `MATCH (p:person)` knows the label, all with no `ontology add`.

The graph adds to std and never removes or narrows it: declaring a std name again merges into std's entry.
graph.json holds only the graph's own declarations and `"std": 1`; std's entries are never written into it, and a
graph without `std` is written byte for byte as before.

### std's names in a std graph

- A std type or relation declared by name alone (`umx ontology add type person`, `umx ontology add relation
  works_for`) is std's already: `"unchanged": true`, nothing written. With fields or a description it is the
  graph's own declaration, merged into std's.
- A field std places on this type or relation keeps std's kind: `umx ontology add field person job_title:int`
  is refused with `std_field_conflict` (its next is `umx ontology show --std`). The soft risk ratings are the
  exception: a non-string declaration warns and keeps the graph's kind.
- A provenance type cannot be declared (`umx ontology add type session` is `reserved_name`); a user node of one
  warns `reserved_type`. A user edge of a provenance relation is refused (`reserved_relation`), of a derived one
  too (`derived_relation`).
- An edge that an older umx stored with `authored_by` or `edited_by`, or with a provenance relation between nodes
  that are not provenance records, loads with the warning `reserved_relation_stored` (its next shows the edge).
  Nothing is dropped.

A graph without std takes all of these names as its own custom names, the provenance types included. It reserves
only `std` itself and what every graph reserves (the provenance and derived relations above).

`std:<name>` is accepted wherever a flag or word takes a type or relation name (`--type std:person`,
`umx edge add <a> std:works_for <b>`, `umx ontology add field std:person nickname:string`) and is stored plain,
as `person`. A `std:` before a name std does not have is refused by the name rules (`name_invalid`).

### Custom fields on std types

A std graph adds its own fields to a std type or relation: `umx ontology add field person shoe_size:int` writes
only `shoe_size` under `person` in graph.json, never std's fields. `umx ontology show` lists std's fields first,
marked `"std": true`, then the graph's own, marked `"std": false`. The graph's field then works like any other:
`node add` and `node edit --set`, batch, `--where` and `--columns`, word search (a string field), roles and
embedding (`umx ontology set field person shoe_size --role ...`), and Cypher (`p.shoe_size`). Several types take a
field in one call, std's included (`umx ontology add field risk,team score:int`); `--all-types` means only the
types graph.json declares, never std's.

- A field std places on this type or relation keeps std's kind (`std_field_conflict` otherwise), except for the
  soft risk ratings described above. A name placed only elsewhere does not constrain a custom field. On a type or
  relation std already gives the field to (`job_title` on `person`, `tags` on any node, `starts_on` on
  `works_for` edges), the same kind and role is std's already: `"unchanged": true`, nothing written. With
  `--description` it is the graph's own declaration, std's kind and the description. Another role is
  `field_role_conflict`; change a role with `umx ontology set field`. On a type std does not give it to
  (`umx ontology add field topic due_on:date`) it is the graph's own declaration and kind, also for a field std
  places on other types only: `umx ontology add field transcript starts_on:date` declares it on `transcript`
  (std places `starts_on` on `meeting`, `event`, `project` and `segment`), and the same again is then unchanged.
- A reserved key (`id`, `title`, `status`, `note`, the provenance keys such as `prov_key`, the locator fields,
  `entity_key`, `std`) is never a custom field name (`reserved_name`), in any graph.
- `umx ontology remove field person shoe_size` removes the extension. A field std gives the type cannot be
  removed (`umx ontology remove field person aliases` is `reserved_name`). Removing the graph's own declaration
  of a field std places on the type (a role from `umx ontology set field`, or a description) brings std's back,
  even while nodes hold the field; when that changes the role, it is a role change like `ontology set field`'s
  (`role_changed`, the nodes pending until remade). A field std allows on any node but does not place on the type
  (`starts_on` on a `widget`) is the graph's own once declared, and is in use while a node holds it.
- A graph.json edited by hand to give a placed std field another kind still loads and warns
  `std_field_conflict_stored`. Std's kind wins except for risk ratings, which keep the graph's kind.

### Adopting std: umx ontology adopt-std

A graph made without std (`--no-std`, or before std existed) moves onto std v1 with `umx ontology adopt-std`, in
one write set:

    umx ontology adopt-std --dry-run --graph launch                    # the plan; writes nothing
    umx ontology adopt-std --graph launch                              # std: 1, the graph's own std copies folded in
    umx ontology adopt-std --map individual=person --map knows_of=mentions --graph launch
    umx ontology adopt-std --field-to-tag origin --graph launch       # origin values become origin:<value> tags

- graph.json gets `"std": 1`. A type, relation or status std has is dropped from the graph's own lists, and so is a
  field std places on that type or relation with std's kind and role and no description. What std does not supply
  stays: a field std lacks (an extension), a field with another role or a description (an override), and every name
  std does not have. The reply lists them as `folded`, `extended` and `custom`. Roles and kinds read as before, so
  no embedded text changes; std's string fields (`job_title`, `org`) become searchable by words.
- With no `--map`, only graph.json is written: every node and edge line is left as it is. A graph built to std's
  names adopts with no line change.
- `--map <from>=<to>` (repeat it) renames a type or a relation on the lines that have it, and nowhere else. Ids,
  ends and `umx:<graph>/<id>` addresses stay, so links from other graphs keep working; each renamed line is stamped
  as `umx node edit --type` and `umx edge edit --relation` stamp it (a node's `updated`, an edge's `updated` and
  `updated_by`, and `edited_by_session`); an edge whose own `updated` or `updated_by` is not umx's stamp is
  refused (`edge_reserved_field_in_use`), as edge edit refuses it. The
  declaration moves with it (merged into `<to>`'s when the graph declares both), with its fields, roles,
  description and columns, and so do the relation's symmetric mark, end types and description and the spines
  that name it. `<to>` may be `std:person`. umx never renames on its own: `--dry-run` only suggests a rename for a
  name a small slip from std's (`organization`, hint `adopt_rename_suggested`, whose next is the dry run with that
  `--map`). Other graphs are never read or written: one whose own `--from`/`--to` end types name the old type warns
  `relation_endpoint_unexpected` there until it is changed.
- A conflict blocks the write and is listed with its fix, with or without `--dry-run`: a field declared with another
  kind than std's where std places it (`adopt_field_kind_clash`, next `umx ontology remove field ...`;
  risk rating overrides only warn and keep their kind); stored edges of a relation a std
  graph keeps for umx, `authored_by`, `edited_by`, `similar_to` or a provenance relation between user nodes
  (`adopt_reserved_relation`: rename them with `--map authored_by=written_by`); and a `--map` that names nothing of
  the graph (`adopt_map_unknown`), a name that is both a type and a relation (`adopt_map_ambiguous`), merges two
  declarations that differ (`adopt_map_merge`), renames a symmetric relation to one that is not
  (`adopt_map_symmetry`), would make two edges one (`adopt_map_duplicate_edge`), chains or swaps names
  (`--map a=b --map b=c`, `usage_invalid`), or targets a name other writes refuse: a relation named `std`,
  `not_same_as`, `similar_to` or another derived or provenance relation, a type named `std`, `graph` or a provenance
  type. `not_same_as` edges are `umx entity deny`'s and are never renamed. While nodes or edges hold a field of
  another kind, `adopt_field_kind_clash`'s next lists them: unset it there (`--unset <field>`), then remove the
  declaration.
- Nodes or edges that hold a std field with a value of another kind (`aliases` as a string, a `starts_on` on a
  `works_for` edge that is not a date) warn `adopt_value_kind` with how many and the first; the values are kept as
  written.
- A `--map` over many lines is one write: memory holds about one more image of the graph while it runs. Run
  `--dry-run` first to see how many lines it renames.
- `--field-to-tag <field>` (repeat it) turns a custom field of nodes into std tags, in the same write set: each text
  value, or each text in a list value, becomes `<field>:<value>` in stored form, with tag_invalid's snake_case fix
  applied (`Double Diamond` becomes `origin:double_diamond`), merged into the node's tags. Ids and addresses stay;
  each changed line is stamped as a node edit stamps it. An empty list leaves the node with the moved values. A value
  with no snake_case form (`123`), one whose snake_case form is longer than a tag (it is never cut), one that is not
  text (a number, an object, a nested list), one that holds anything secret-shaped (a string or an object key at any
  depth, as stored or upper cased), and every value of a node that would then hold more than 64 tags stay in the
  field: `adopt_tag_value_invalid` counts them per reason and lists the first 20 node ids and values as candidates,
  text cut to 80 characters, anything else by its shape only (`<object>`, `<list>`, `<not text>`) and a secret as
  `<masked>`; its next lists the field. When no node keeps a non-empty value the field's declarations and default
  columns go too. The reply's `field_to_tag` says, per
  field, how many nodes moved values (`nodes`), the tags they gained (`tags`), the nodes that keep a value (`kept`),
  how many distinct values became a tag another value also became (`collapsed`: `A B` and `a-b` are one tag), and
  whether the declaration went (`declaration_removed`). Edge values and relation declarations are never touched. It
  refuses (`adopt_field_to_tag_refused`) a std field or `tags`, a field only edges have, one no node type declares or
  holds, and one a declared spine reads. It also works on a graph that already uses std, alone: there `--map` is
  `usage_invalid`.
- A graph that already uses std is `already_std` (with no `--field-to-tag`): `"unchanged": true`, nothing written, so
  a script can re-run.
- It works on a secured graph named with `--graph` like any write, and its reply names nothing outside that graph.

### Expected end types

Each std relation expects types at its ends: `works_for` from `person` to `organisation`; `member_of` from
`person` or `team` to `team` or `organisation`; `owns` from `person`, `team` or `organisation` to any node;
`mitigates` from any node to `risk`;
`raised_by` from `question`, `risk`, `decision` or `action` to `person`, `team` or `organisation`; `raised_in` to
`segment`, `meeting`, `event`, `transcript` or `phase`; `held_at` from `meeting` or `event` to `place` or
`system`; `precedes` between two nodes of one type. The expectation is soft: in a std graph an edge outside it
is stored, with the warning `relation_endpoint_unexpected` naming the end's type and the types expected.
`umx ontology add relation works_for --from person --to team` widens them for this graph (repeat `--from` and
`--to` for more): graph.json keeps the added types under `relation_endpoints`, written only once one is added.
On a relation of the graph's own the same flags set its expected end types. `--symmetric` takes neither.

### Tags

In a std graph every node may carry std tags in its `tags` list: role data, never embedded or word-indexed. A tag
is `<value>` or `<facet>:<value>`, each part snake_case (`[a-z][a-z0-9_]`, at most 64 characters), for example
`origin:double_diamond` or `needs_review`. Tags are stored lowercased, deduplicated and sorted, so their order
never makes a git diff; a node holds at most 64 (`tag_limit`). A tag that is not snake_case once lowercased is
refused with `tag_invalid`; nothing is written. Its `next` repeats the node or the graph, the type and every tag
given (for node add the title is a placeholder), the refused tag in snake_case form (`origin:Double-Diamond` becomes
`origin:double_diamond`); other flags are not repeated, so add them back. With `--scope` or a graph list it names
the first std graph read. A `--set tags=` value or a batch line has no command to repeat, so its `next` is
`umx docs get ontology`, as it is for a tag with no snake_case form, one that looks like a secret, or a next too
long to print as written.

    umx node add --type risk --title "Launch slips" --tag origin:double_diamond --tag needs_review
    umx node edit 01JC8XA4H7 --tag stage:review --untag needs_review   # --untag origin:* drops a facet
    umx node list --tag origin:double_diamond --tag needs_review       # both (repeated flags are AND)
    umx node list --tag needs_review,blocked                           # either (a comma is OR)
    umx node list --tag 'origin:*' --no-tag blocked                    # a facet, leaving out blocked

`--tag` on node add (repeat it, or give a comma list), `--tag` and `--untag` on node edit, and `--set tags=a,b`
all write the same stored form; give tags one way per call. Batch lines take `tags` (node add) and `tag` and
`untag` (node edit), see `umx docs get batch`. A hand-edited `tags` value loads with `tag_not_canonical` (or
`tag_unusable` for an entry umx cannot read), is found as its stored form, and the node's next write stores it so.

Tags are free-form until a facet has a declared tag: `umx ontology add tag <facet>:<value>` or
`umx ontology add tag <facet>:*` (every value) adds it to graph.json's `tags`, shown by `umx ontology show`. Then a
tag in that facet that is not declared warns `tag_off_vocabulary` (never refused), whose `next` declares it. A tag
in another facet, or with no facet, never warns. A declared tag names its facet; adding one already declared is
`unchanged`.

`--tag` and `--no-tag` filter `umx node list` from one inverted tag index per graph, so they never scan nodes;
with `--graph a,b` or `--scope` a graph without std is skipped and `tag_filter_not_std` counts it. The same flags,
read the same way, filter the other reads: `umx search` narrows every leg to the tagged nodes before it ranks (so
`--limit 1` gives the best tagged hit, never an empty page), `umx node find` keeps its tagged matches, `umx walk`
lists only the tagged nodes it reaches (the walk goes on through the others, as with `--within`) and
`umx neighbours` lists only the tagged far ends. A row whose node the read cannot show (an unnamed secured graph,
one not loaded, a missing node) is never tested under `--tag` or `--no-tag`: it is left out and
`tag_filter_untested` counts it, naming nothing. A walk counts only the rows it reached to fill its page. A
graph without std is skipped and counted in `tag_filter_not_std` whenever the read may cover several graphs (a
list, a scope, `--all`, or no graph named while several are loaded, even when a walk or neighbours starts in it);
only a read of that graph alone is `tag_needs_std`. In
`umx query`, `n.tags` in a std graph reads the stored form, so `'needs_review' IN n.tags` matches a hand-edited
`Needs_Review` too; it reads every node it is asked about, as any property does. In a graph
without std `tags` stays an ordinary custom field: nothing normalises it, and `--tag`, `--untag`, `--no-tag` and
`ontology add tag` are refused with `tag_needs_std`, whose `next` is `umx ontology adopt-std --dry-run`.

`umx ontology show` on a std graph marks what std gives: top-level `std` (the version, null when not std), `std:
true` on each std type, on each field std places and on each std relation field, and the std names among
`statuses` and `relations` in `std_statuses` and `std_relations`. `std_any_fields` lists the std fields any node
(or edge) has with no declaration (`tags`, `entity_key`, `starts_on`, `ends_on`). The hint `std_entries_shown`
says the std entries come from umx, not graph.json. Entries with `std: false` are the graph's own.
`umx ontology show --std` prints std v1 alone in the same shape, for any graph or none.

A graph.json whose `std` is newer than this umx knows (`"std": 2` read by a std v1 umx) still loads, with the
warning `std_newer` naming both versions: it is read with std v1's names, and names only the newer std has are
off-vocabulary until umx is updated. `umx ontology set field <type> <field> --role ...` takes a std field the graph has not declared (`person`
`job_title`): it writes the graph's own declaration of that one field, with std's kind. `umx graph guide` lists a std entry only when the graph declares or uses it,
marked `std`, and never asks for a description of a std entry graph.json does not declare.

