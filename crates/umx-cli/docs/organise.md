# Organising: which graph, and when to make another

Before you add anything, know which graph it belongs in. This topic is about choosing: when to extend a graph,
when to make a new one, how several graphs share one daemon, and who may change what. For the commands see
`umx docs get graph` and `umx docs get scope`; for links between graphs, `umx docs get linking`.

## The model

- A graph is a folder: `graph.json` (its id, alias, title and vocabulary), `nodes.jsonl` and `edges.jsonl`, plus
  `embeddings/` and `similar/` when it has vectors. The files are the truth; the daemon holds a working copy.
- The graph id (in graph.json) is the graph's identity. The alias is this machine's name for it in commands and
  addresses, `umx:<alias>/<node id>`.
- A node is named by its id, a unique prefix of 6 or more characters, or its address. Never by title.
- The vocabulary is soft: a new type, status or relation is accepted with a warning whose `next` declares it.
  Common things use the same names in every graph (below).

## Find the right graph first

    umx graph list                         # what is loaded, with counts and paths
    umx graph guide <alias>                # what a graph is for, its words and what they mean
    umx scope list                         # saved sets of graphs

Read the guide before writing to a graph you did not make: its purpose and conventions say what belongs in it.
If no loaded graph fits, ask the human which graph to use or whether to make one. Don't make a graph for client or
private material without the human's yes, and ask where its folder should live: git history keeps everything.

## Extend or make a new graph

Extend the graph when the new material has the same audience, secrecy, owner and lifecycle. Make a new graph when
one of these differs:

- **Audience**: who may read it. A client's graph and your notes about the client are two graphs.
- **Secrecy**: whether it is read only when named, and where its text may go. Those are per-graph settings
  (`visibility` and `embedder`; `--secured` sets both).
- **Owner**: who decides what goes in it. One writer per graph (below); two independent writers want two graphs
  that link.
- **Lifecycle**: a record that must not change (what a workshop said) and a living understanding. Keep the record
  frozen in its own graph; put corrections in the living graph, linked to what they correct.
- **Size**: a graph written all day that grows without bound gets slower to write. Split along one of the seams
  above before size forces it.

Don't split by type alone (a graph of people, a graph of decisions): cross-graph joins then need every endpoint
and link owner in scope. Don't make a graph per document or meeting when they share an audience and owner:
make the meeting a node.

When you make one, shape it straight away: declare the types, statuses and relations it needs, set its title,
purpose and conventions. `graph new` prints the commands.

    umx graph new /Users/me/graphs/launch --alias launch --embedding-model embeddinggemma-2@256+p1
    umx ontology add type risk --graph launch
    umx graph set launch --purpose "Decide how to launch in spring"

## Common names

People, organisations and other common things use the same names in every graph, so graphs line up. These are
std v1, the standard vocabulary built into umx. A std graph (any `umx graph new` without `--no-std`) knows them
with no declaration (`umx docs get ontology`, section std); on a graph without std, declare these names exactly
with `umx ontology add`. Either way, invent no synonyms:

- Types: `person`, `organisation`, `team`, `role`, `project`, `meeting`, `event`, `segment`, `decision`, `action`,
  `question`, `risk`, `topic`, `document`, `transcript`, `place`, `system`.
- Relations: `member_of`, `works_for`, `part_of`, `attended`, `owns`, `decides`, `answers`, `raised_in`, `raised_by`,
  `mentions`, `sourced_from`, `supersedes`, `held_at`, `precedes`, `instance_of`.
- Directions: `works_for` person to organisation; `member_of` person to team and team to organisation; `owns` owner
  to thing (never `owned_by`); `raised_by` a point to who said it; `raised_in` a point to the meeting; `held_at` a
  meeting or event to a place; `precedes` one thing before another of the same type, where there is no clock or
  to show tracks (a cycle is allowed); `instance_of` an instance to what it is one of (a deployment to its
  product), across graphs if need be, with no declaration. It never makes the two one entity (`umx docs get
  entity`).
- A `meeting` is any gathering with attendees; an `event` is something that happened that is not a gathering.
- Dates (kind date): `starts_on` and `ends_on`, the one span pair, on any node or edge that has a span in the
  world (meetings, events, projects, a `works_for` link); `due_on` on projects, actions, decisions and risks. A
  part of a meeting is a `segment`, `part_of` its meeting, event or segment, with a `step` (an int from 1, the
  order among siblings under the same parent only; two siblings may share one). Write the precision the source gives
  (`2025`, `2025-01`, a day, or a date-time with its offset); how to write and compare dates: `umx docs get time`.
  umx warns `anchor_undated`, `time_no_offset` and `time_as_text` when a write leaves a time out or writes one as text.
  Person fields: `aliases`, `job_title`, `org`. The title is the name; there is no name field.

Work that moves (an engagement, a project, an action) uses the four lifecycle statuses, on any type that fits.
Move between them freely with `umx node status <node> <status> --reason "<why>"`; history keeps the path.

| Status | Standing | Means |
| --- | --- | --- |
| `active` | unsettled | work on it is under way |
| `stalled` | unsettled | started, but not moving and not given up (blocked, waiting or parked); say why |
| `delivered` | settled | done, with its outcome handed over |
| `withdrawn` | out of play | given up or cancelled before delivery, by either side; say why |

One name each: `delivered` is not `done`, and don't add `complete` or `cancelled`. The built-in `open` stays. Keep a
free-text outcome in the status move's reason or a note, never in another field. A graph that already declares one
of these names keeps its own meaning.

Domain words (`tension`, `mechanism`, `clarification`) stay custom: declare them with `umx ontology add`.

## One writer per graph

The daemon applies writes one at a time, so files never conflict, but it cannot stop two agents adding the same
thing twice. Each graph has one writer at a time. Helpers read and propose (for example batch files); the writer
applies. If another seat owns a graph, send it your changes rather than writing them yourself.

Before changing a graph's settings (`umx graph set`, its embedding model, `umx embeddings reembed`), tell the
graph's owner. Those change `graph.json` or every vector file.

## Several graphs on one daemon

Single-graph commands use an address or `--graph <alias>` (they must agree), else `UMX_GRAPH`, else the only loaded
graph. With several loaded and none named, they fail with `graph_ambiguous`; `next` repeats the command with
`--graph`. Working in one graph? Set it once:

    export UMX_GRAPH=launch

Reads that accept `--graph a,b` or a saved `--scope` are node list, edge list, search, query, neighbours, walk and
path. Crossings need the edge's owner and both endpoint graphs in scope. Search/query with nothing selected
read loaded open graphs; `--all` makes that explicit and overrides `UMX_GRAPH`. Lists keep their single-graph
default. Writes, `node get` and `node find` still select one graph.

    umx search approval --graph launch,ops
    umx scope save both --graph launch,ops

### Secured graphs

A graph's `visibility` is `open` or `named`, and its `embedder` is `any`, `trusted` or `local`. A secured graph
(`--secured`, or `umx graph set <alias> --policy secured`) is named plus local: its text never goes to an embedder
that is not local, and it is left out of search and query unless named; replies only count it in
`secured_graphs_skipped`. A client graph whose text may go to the hosted embedders this machine's config defines
can be named plus trusted:

    umx graph set client_x --visibility named
    umx graph set client_x --embedder trusted

Name a graph to read it. Never copy its text into a graph with a looser setting. Raising either setting is fine;
lowering either needs the human at their own terminal (`umx graph confirm-policy`): never run that yourself, ask.
See `umx docs get graph`, Embedding policy.

### Copies

Two folders with one graph id (two worktrees) are copies. Load the second with `--as <other alias>`. Reads with no
graph named read one copy and list the others in `copies_skipped`. Name a copy with `--graph` to read it.

## Loaded and unloaded

The daemon remembers the graphs you load. A graph unused for 30 minutes unloads itself, and the next command that
names it (`--graph <alias>`, an address, or a saved scope it belongs to) loads it again first, with the hint `graph_loaded_on_demand`: read it and carry
on. After a restart the daemon reloads every remembered graph before it answers. A command that names no graph
never loads one, so name your graph.

An explicit named command's `graph_not_loaded` error means someone unloaded the graph (`umx graph unload`
stays unloaded until loaded again) or this daemon never loaded it; the message says which. Run its `next`.
A graph missing after a restart is named by
`umx doctor` (`doctor_graph_not_reloaded`) with the command that loads it. Don't keep your own list of folders to
reload.
A crossing's `state: "graph_not_loaded"` can also mean a hidden secured graph; never infer that it is absent.

    umx graph load /Users/me/graphs/launch

Never edit a graph's files by hand while it is loaded; the daemon's next write may put its own copy back. Change
edges with `umx edge edit` and `umx edge delete` (`umx docs get edge`), cross-graph edges included, from the graph
that stores them. If a file must be edited for a change no verb makes, unload the graph, edit it keeping
ids and order, load it, and run `umx check --graph <alias>`.

## Don't

- Don't read graph files directly to answer a question: use umx, so you see what the daemon sees.
- Don't create a graph to avoid deciding where something belongs; ask.
- Don't put email addresses, phone numbers or credentials in any graph.
- Don't start, stop or reconfigure a daemon other people share without asking.
