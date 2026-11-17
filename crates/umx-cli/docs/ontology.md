# Ontology: the graph's vocabulary

Each graph declares its node types (with the custom fields each carries), statuses and relations. The
vocabulary is soft: off-vocabulary writes are accepted with a warning naming the `umx ontology add` command
that would declare the name. Change the vocabulary as the work needs.

## umx ontology show

Lists types (with their fields, kinds and roles, and default list `columns` when set), statuses, relations, the symmetric relations and
`relation_fields` (the fields declared on each relation's edges), with the purpose, conventions and descriptions
that `umx graph guide` shows. Each field shows `role` (`content` or `data`, as umx acts on it) and `role_default`
(true when no role is stated, so the field is data).

    umx ontology show
    umx ontology show --graph launch

## umx ontology add ...

    umx ontology add type risk --field likelihood:float --field owner:string
    umx ontology add status parked
    umx ontology add relation mitigates
    umx ontology add relation rivals --symmetric   # a rivals b means b rivals a; declares it too if new
    umx ontology add field risk impact:int        # declares the type too if it is new
    umx ontology add field --relation works_for starts_on:date   # a field on the relation's edges
    umx ontology add field risk,issue,task impact:int   # several declared types, one write
    umx ontology add field --all-types owner:string     # every type the graph declares, one write
    umx ontology add status parked --description "Waiting on someone else"
    umx ontology add field doc file:string --role data
    umx ontology add field doc summary:string --role content   # prose: searched by meaning

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

## umx ontology set type <type> --columns | --reset

    umx ontology set type risk --columns title,likelihood,owner   # node list --type risk shows these
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
provenance types and relations, `same_as` (`umx entity link`), the standard fields and any `std:` entry. Removing something not declared is ok
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
`edited_by` (`derived_relation`) are refused on `edge add`.

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
