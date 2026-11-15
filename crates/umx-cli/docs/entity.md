# Entities: one thing across graphs

The same person, organisation, system or anything else often appears in several graphs. Give each of those
nodes the same entity key and umx knows they are one entity. There is no central graph and no registry: the key
lives on each node, in its own graph, and travels with that graph's files.

Two nodes can also be joined by a link: `umx entity link` stores one `same_as` edge between them, for nodes that
already exist with different keys (or none). `umx entity show` lists an entity's members across the loaded graphs
and says how each one joined. When two nodes are NOT one thing, though a key or a link says so, `umx entity deny`
stores one `not_same_as` edge with a reason, and no key or link joins them while it counts. Seeing an entity's edges
from every graph at once (neighbours that follow members) comes later.

## Related, not the same

Three std relations look alike and mean different things. Only `same_as` joins:

- `same_as`: the two nodes are one thing (`umx entity link`). They are one entity.
- `instance_of`: one thing is an instance of another, such as a deployment of a product or a copy of a template.
  It points from the instance to what it is one of, may cross graphs, and is written like any edge, with no
  declaration: `umx edge add <deployment> instance_of umx:<graph>/<product> --graph <graph>`. The two stay two
  entities, however many such edges there are. A key the two share still joins them, so do not give an instance
  its product's key.
- `part_of`: one thing is inside another (a segment of a meeting, a team of an organisation). It never joins.

A node that bundles two things is not an instance of either: split it, or link its parts with `part_of`, and remove
any key it shares with one of them.

## The key

`entity_key` is a standard field on a node of any type: a list of `scheme:value` keys.

- `email:<address>` and `upn:<name>`: a person's email address or user principal name. Compared without case
  (stored lowercase) and trimmed. Allowed: whoever holds a graph is trusted with what is in it.
- `id:<umx id>`: a 26-character umx id, when the thing has no natural key. Use the id of one of its nodes, for
  example the node in the first graph that recorded it.
- A public identifier where one exists: `domain:example.com`, `abn:51824753556`, `url:https://example.com`.
  `domain:` is compared without case and loses its trailing dots (`domain:Example.COM.` is stored
  `domain:example.com`); `url:` lowercases its scheme and host and keeps the rest as given, user info included
  (`url:HTTPS://Ann@Example.COM/Path` is stored `url:https://Ann@example.com/Path`).
- Any other snake_case scheme is accepted.

Two nodes are the same entity when they have the same type and share at least one key. A node can hold several
keys (an email and a UPN, a domain and an ABN). The same key on nodes of different types does not match.

Keys compare exactly apart from that: `name:Acme` and `name:acme` are different keys, so write a natural key in
lower case wherever its source ignores case. A key stored in an older form (a hand edit, or a domain written before
umx lowercased domains) loads with `entity_key_not_canonical`, is still found, and is rewritten on the node's next
write.

A key whose value is the node's own title (compared without case, runs of spaces as one) is stored with the hint
`entity_key_is_title`: a name is often shared, so it is a weak identity. Some things really are known by their name;
prefer an email, a domain or an id where one exists.

Refused with `entity_key_invalid`, writing nothing: a key that is not `scheme:value` with a snake_case scheme, has
no value, has a value over 256 bytes, holds a control, bidi or zero-width character, has whitespace inside an
`id:`, `email:` or `upn:` value, or is an `id:` key that is not a umx id; and a node with more than 32 keys. A
stored key like that (a hand edit) is ignored when the graph loads. A key that looks like a secret (an API key,
say) is kept but warns `secret_suspected`, naming the field, never the key.

## umx entity key <node> add|remove <key>

    umx entity key 01JC8XA4H7 add email:Jo.Smith@Example.com     # stored as email:jo.smith@example.com
    umx entity key umx:crm/01JC8XA4H7 add upn:jsmith@corp.example
    umx entity key 01JC8XA4H7 remove email:jo.smith@example.com

Adds a key to the node's `entity_key` list, or takes one out. The key is checked and put in its stored form
first; adding a key the node already has, or removing one it does not have, changes nothing (`changed` is
empty). Removing the last key removes the field. The reply is the node, as from `umx node edit`.

`entity_key` is also a field like any other, so these work too:

    umx node add --type person --title "Jo Smith" --set entity_key=email:jo@example.com,upn:jsmith@corp.example
    umx node edit 01JC8XA4H7 --set entity_key=email:jo@example.com     # replaces the whole list
    umx node edit 01JC8XA4H7 --unset entity_key

and a batch line's `sets` take it the same way (`{"field":"entity_key","value":["email:jo@example.com"]}`). Every
key is checked, whichever way it arrives.

## Finding the nodes that share a key

    umx node list --where entity_key=email:jo@example.com --scope people   # a saved scope
    umx node list --where entity_key=email:jo@example.com --graph crm,meetings
    umx node list --type person --where entity_key=email:Jo@Example.com    # the value is normalised too

`--where entity_key=<key>` matches a node when any of its keys equals the key (`!=` when none does), with the key
put in stored form first. Across `--graph a,b` or `--scope` the rows carry their graph
and address. With no `--graph`, `--scope` or address while several graphs are loaded, a key lookup covers every
loaded open graph, as `entity show` does; any other `node list` still asks which graph. A secured graph is searched
only when the read names it. When the key is held under another type in a covered graph (an organisation in one
graph, a team in another), the lookup warns `entity_type_mismatch`: those nodes never match.

Look an entity up this way before you add it to another graph, and copy its key onto the new node. That is what
keeps keys the same across graphs without a central list.

## umx entity link <a> <b>

    umx entity link umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M          # stored in crm, a's graph
    umx entity link umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M --graph meetings
    umx entity link umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M --dry-run    # check it, write nothing

Says that two nodes, usually in two graphs, are the same entity. It stores one `same_as` edge, a std symmetric
relation like `possibly_same_as`: stored once with the lower end first, whichever order you give, and never
warned off-vocabulary. The edge goes in the graph `--graph` names, or else in the first node's graph (`UMX_GRAPH` only resolves a bare
id, never moves the link). It touches
neither node and writes no key: set keys with `umx entity key`.

- Linking two nodes already linked changes nothing: `"unchanged": true`.
- `--dry-run` checks the link exactly as a real one (every refusal and warning, the graph it would go in and its
  sentence) and writes nothing: `"dry_run": true`, and `"edge": null` when it would add one.
- Refused with `entity_link_invalid`, writing nothing: a node linked to itself, or ends that name two copies of
  one graph (the same graph loaded twice).
- Two nodes of different types still link, and warn `entity_type_mismatch`: check you meant it.
- A link that would put two nodes of one graph in one entity (one end's entity already has another member in the
  other end's graph) still stores, and warns `entity_link_overlap`, naming that member. Before the link, each end's
  entity is read over every loaded open graph plus the graphs the request names.
- A link stored in an open graph that points into a secured graph warns `link_into_secured`, as any edge does.

The reply is the edge and its sentence, as from `umx edge add`. `umx edge list --type same_as` lists the links a
graph stores.

## umx entity unlink <a> <b>

    umx entity unlink umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M

Removes the `same_as` edge between the two nodes, in either order: from the graph `--graph` names, or else from
the first node's graph or, failing that, the second's. `"deleted": true` with the edge it removed; when no link is
stored, `"unchanged": true` and nothing changes. This is how a wrong link is fixed: at its source.

## umx entity deny <a> <b> --reason <text>

    umx entity deny umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M --reason "two people named Jo Smith"
    umx entity undeny umx:crm/01JC8XA4H7 umx:meetings/01JD2B9Q0M

Says that two nodes are NOT the same entity, whatever keys or links say. It stores one `not_same_as` edge (a std
symmetric relation, stored once with the lower end first) holding `reason`, and the writer and session, as any edge
does. It goes in the graph `--graph` names, or else in the first node's graph, and that graph must hold one of the
two nodes. A denial beats keys and links: `umx entity show` builds the entity by joining keys (by type and key,
holders by graph id then node id) and then links (by the storing graph's id, then edge id), and skips any join that
would put a denied pair in one entity, warning `entity_join_denied` with what it skipped and the denial. Every read
skips the same joins.

- `--reason` is required: 1 to 500 characters, no control, bidi or zero-width characters. A reason that looks like a
  secret warns `secret_suspected`. The reason is data: never embedded or searched, shown only where the edge is.
- A far node must be one you can read: one in a graph that is not loaded, or that the request does not name when it
  is secured, is refused exactly as a node that does not exist (`node_not_found`), by full id or by prefix alike. A
  secured node named by its address is stored as that address only, with `link_into_secured`.
- Refused with `entity_link_invalid`, writing nothing: a node with itself, two copies of one graph, a graph that holds
  neither node, and a pair this graph links with `same_as` (its next unlinks them first). `umx entity link` on a
  denied pair is refused the same way, with next `umx entity undeny`. An `edge add`, `edge edit` or `edge delete` of
  `not_same_as`, a batch line included, is refused with next `umx entity deny` or `umx entity undeny`.
- A repeat (either order) is `"unchanged": true` and keeps the first reason: to change it, undeny and deny again.
- A `possibly_same_as` between the pair in the same graph is replaced in the same write (`replaced` names it). A
  proposal stored in another graph stays, but a read never lists it while the denial counts.
- A denial counts only while the read covers its graph and both its nodes. Otherwise it is ignored, and never named.
  It never hides a node: each stays visible as its own entity.

`umx entity undeny <a> <b>` removes the denial, looking where `entity unlink` looks: `--graph`, else the first
node's graph, then the second's. `"deleted": true`, or `"unchanged": true` when none is stored. `edge add`,
`edge edit` and `edge delete` never write a `not_same_as` edge: deny and undeny do.

## umx entity show <node>

    umx entity show umx:crm/01JC8XA4H7
    umx entity show umx:crm/01JC8XA4H7 --scope people
    umx entity show 01JC8XA4H7 --graph crm,meetings

The entity a node belongs to: every member, by graph, and how each one joined.

- Members (`members`, the node you asked about first): nodes of the same type sharing an `entity_key`, and nodes
  joined by a `same_as` edge, followed onward from each member (so A keyed like B, and B linked to C, is one
  entity). Each member lists its keys and `matched`: `{"by":"asked"}`, `{"by":"entity_key","key":...}` (a key it
  shares with another member) or `{"by":"same_as","edge":...}` (the link and the graph storing it).
- Proposed (`proposed`): nodes a `possibly_same_as` edge proposes for a member, with the edge. They are not
  members, and their own proposals are not followed.
- `graphs`: the graphs the read covered. With `--scope` or `--graph a,b`, those, and the node must be in one of
  them (`graph_mismatch` if its address names another). Otherwise every loaded open graph, plus the node's own. A secured graph is covered only when the read names it (its address, `--graph`
  or a scope), and nothing from a secured graph that was not named is ever shown. Two copies of one graph never
  group: one copy per graph is covered.

- `entity`: the entity's address, which is the address of its lead member: the member with the lowest graph id,
  then node id, among the members shown. `entity_id` is the same member as `umx:<graph id>/<node id>`, which does
  not change when aliases do. Both are member addresses, so `umx entity show`, `umx node get` and the rest take
  them as they are. A node that is never shown (in a graph the read does not cover) never leads.
- `keys`: the keys two or more members hold, sorted.
- `secret_suspected`, left out when nothing is flagged: the names of shown values umx's secret check flags, never
  the values. On a member, `field:title` and `key:<i>` (the i-th entry of its `entity_key`, counted from 1); on a
  proposed node, `field:title`; at the top, `key:<i>` into `keys`. A `matched` key is one of its member's keys, so the
  member's flags cover it. See `umx docs get node` and `umx docs get safety`.

Nothing merged is stored: the entity is built when you ask, from the keys and links in the covered graphs. So the
answer depends on what is loaded, which is why `graphs` is in the reply; `entity` and `entity_id` depend on it too.
A link to a node in a graph that is not covered still joins the members on either side of it, but that node is
never shown.

A key held by two or more nodes of one type in a covered graph joins none of them in that graph (it still joins in
the others), and `entity show` warns `entity_key_ignored`.

## Warnings

- `duplicate_key`: two nodes of the same type in one graph hold the same key. Both are kept; the lower id is the
  one a key lookup finds. Raised when the graph loads and when a write makes it. Fix it at the source: remove the
  key from the node that should not have it (`umx entity key <node> remove <key>`).
- A shared mailbox or role address (a team inbox) works as a key but makes everyone who lists it one entity.
  Use a personal address.
- `entity_graph_twice` (entity show): the entity holds two or more nodes of one graph. A real entity has one node
  per graph, so a key or a link probably joined two different things. It names the graph, the two lowest nodes and
  what joined the second (a key's scheme, never its value, or the link's id); its next shows that node's entity.
- `entity_key_ignored` (entity show): a key held by several nodes of one type in a graph joined none of them there.
  Give each thing its own key.
- `entity_link_overlap` (entity link): the link is stored, but it put two nodes of one graph in one entity.
- `entity_types_mixed` (entity show): the members hold more than one type, as after a link between two types.
- `entity_type_mismatch`: a link joined two types, or (entity show, the key lookup and `umx check`) the same key is
  held under two types in the covered graphs, so those nodes never match. It names the two types and their graphs,
  only graphs the read covered; its next retypes one node: `umx node edit <node> --type <type>`.
- `entity_key_is_title` (a hint, on a write): the key's value is the node's own title.
- `entity_join_denied` (entity show): a key or link join was skipped because a denial covers the pair. It names the
  key's scheme or the link's id and the denial's id; its next is `umx entity undeny`.

## Don't

- Don't put a key on a node just because the titles look alike. A shared key says "this is the same thing".
- Don't declare `entity_key` with `umx ontology add field`: its kind is fixed (`reserved_name`).
- Don't expect `entity_key` text in search or embeddings: keys are data, never embedded or word indexed.
- Don't link two nodes because their titles look alike: propose it with a `possibly_same_as` edge and let a
  person confirm it. Linking says "this is the same thing".
