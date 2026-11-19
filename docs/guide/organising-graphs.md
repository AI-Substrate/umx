# Organising graphs

How to decide what goes in a graph, where its folder lives, how graphs share a machine and a repository, and
how they point at each other. For the commands themselves see `umx docs get graph`, `umx docs get scope` and
`umx docs get edge`; this page is about the choices.

## What a graph is on disk

A graph is one folder:

    launch/
      .gitignore          # one line, .umx-tmp-*, so umx's temp files never reach git
      graph.json          # id, alias, title, vocabulary, embedding settings
      nodes.jsonl         # one node per line, sorted by id
      edges.jsonl         # one edge per line, sorted by id
      embeddings/         # vectors, when an embedder is set up: <model@dims>/<shard>.tsv
      similar/            # similar_to lists, kept beside the vectors: <model@dims>/<shard>.tsv

Every write goes to a temp file in the folder, is synced, then renamed over the old file, so a file is never
half written. A change to several files (a node and its `part_of` edge, a new graph) is all or nothing, even
through a crash. Saving a graph that did not change writes the same bytes, so `git diff` stays empty.

The graph id in `graph.json` is the graph's identity. The alias (`--alias launch`) is only how this machine
names it in commands and addresses, `umx:launch/<node id>`. Two machines may load the same graph under
different aliases.

## When to make a new graph

Start with one graph per piece of work and split only along a real seam. Make a new graph when one of these
differs between the two parts:

- **Audience.** Who may read it. A client's graph and your team's internal notes about that client are two
  graphs, because one may be shared and the other may not.
- **Secrecy.** Whether it is read only when named, and where its text may go. Those are per-graph settings, so
  secret material and open material belong in separate graphs.
- **Owner and writer.** Who decides what goes in it. A graph should have one writer at a time (see below), so
  two teams writing independently want two graphs that link to each other.
- **Lifecycle.** A record that must stay as it was (what was said in a workshop) and a living understanding
  that keeps changing. Freeze the record in its own graph and put later corrections in the living graph, linked
  to what they correct.
- **Size.** Reads stay fast at 100,000 nodes, but every write rewrites the files it changes, so a graph that is
  written to all day and grows without bound gets slower to write. See
  [loading and performance](loading-and-performance.md).

Keep things in one graph when they share all of these. Two graphs about the same thing for no reason make every
question a cross-graph question.

Don't split by node type ("a graph of people, a graph of decisions"). Types are what the vocabulary is for, and
splitting them breaks every walk and query that joins them.

## A worked layout

An invented consulting engagement with two workshops, a general graph and a team's internal view:

    engagement-graphs/            # a git repository, local or private
      workshop_day1/              # frozen: what the room said on day 1
      workshop_day2/              # frozen: what the room said on day 2
      engagement/                 # living: later calls, clarifications, decisions, people
      sources/                    # sources that travel with the graphs; others are cited through roots
      _ops/                       # the scripts that build batches, kept with the graphs

`engagement` links to nodes in the workshop graphs rather than copying them, so the workshop graphs stay exactly
as the room left them. A correction is a node in `engagement` that points at the node it corrects.

## One repository or many

- **Graphs next to the work.** A graph about one codebase or one document set can live in that repository, in a
  folder such as `graphs/<alias>/`. It is versioned with what it describes.
- **A repository of graphs.** Several related graphs that change together, like the layout above, can share one
  repository, with the scripts that build them.
- **Client and private graphs.** Keep them in a local-only or private repository, never in one that is pushed
  somewhere public, and mark them secured (below). Git history keeps everything ever committed, so decide before
  the first commit.

The daemon only reads and writes folders inside its roots: your home folder and your temp folder by default.
Put graphs elsewhere with `umx config set roots <folders>` and `umx daemon restart`.

## Git and merges

Commit the graph files together with `embeddings/` and `similar/`. Teammates who pull then load vectors without
embedding anything again. Vectors are keyed by `model@dims`, not by Azure deployment or endpoint, so two people on
different Azure resources with the same model share them.

Because lines are sorted by id and a write changes only the lines it must, most merges are clean. When two
branches touch the same graph, git can still leave things umx has to show you:

- lines out of order or blank lines (both sides appended), which the next write puts back in order;
- the same id on two lines (both sides edited a node), where the last line wins until you keep one;
- an edge whose node the other branch deleted, which stays in the file but out of the graph until its node is
  back.

After a merge, run:

    umx check --graph <alias>

It lists each finding with its file and lines. Nothing is dropped silently, and `umx graph list` counts lines on
disk that are not in the graph as `kept_lines`. To settle one, unload the graph, fix the file by hand, load it
again and check again.

Never edit `similar/` by hand. After a merge that joined two branches' lists, umx rebuilds them in the background,
or run `umx similarity rebuild --graph <alias>`.

## One writer per graph

The daemon applies writes one at a time, so the files never conflict. It cannot stop two agents from adding the
same meeting to one graph twice. Pick one writer per graph: one agent or person who adds and changes nodes.
Helpers read, propose changes (for example as batch files), and the writer applies them.

Before changing a graph's settings (its embedding model, its policy, a re-embed), tell the person or agent who
owns it. Those changes rewrite `graph.json` or many vector files and can surprise someone mid-session.

## Secured, named and trusted graphs

A graph has two settings:

- `visibility`: `open`, or `named`. A named graph is left out of `umx search`, `umx query` and other multi-graph
  reads when no graph is named; the reply only counts how many were skipped, never which. Name it with `--graph`
  or a saved scope to read it.
- `embedder`: where its text may go. `any`; `trusted`, any embedder instance in this machine's config, local or
  hosted; or `local`, never off the machine.

A graph that is not fully open (anything but `open` and `any`) keeps its vectors and `similar_to` lists in a
private cache under `~/.umx/vectors/`, never in the folder, so they are never committed and each machine embeds it
again.

`secured` is the short form for named plus local:

    umx graph new ~/client-graphs/acme --alias acme --secured
    umx graph set acme --policy secured             # an existing graph

An open graph that may use this machine's hosted embedder but should be read only by name:

    umx graph set beta --visibility named --embedder trusted

Every instance in the machine config counts as trusted, so adding or repointing a hosted embedder there decides
where `trusted` graphs' text goes; umx warns `embedder_route_changed` when a config change moves it.

Raising either setting takes effect at once. Lowering either (named to open, local to trusted, trusted to any) is
one `umx graph set` call, by anyone: the reply warns `policy_lowered`, naming what loosened and which embedders of
this machine may now get the graph's text, and the machine records the change with its time. Lowering a graph is
its owner's decision. The machine remembers the stricter setting even if `graph.json` is edited or pulled, so a
pulled or edited lower setting waits until `umx graph confirm-policy <alias>` confirms it.

Whatever the policy: no email addresses or phone numbers in graphs, and no credentials. umx warns
`secret_suspected` when text looks like a key or token and holds that field back from hosted embedders.

## Several graphs on one daemon

One daemon serves every graph you load. Commands choose a graph like this:

1. an address in the command, `umx:<alias>/<id>`, names its own graph;
2. otherwise `--graph <alias>`;
3. otherwise the `UMX_GRAPH` environment variable;
4. otherwise the only loaded graph.

With several loaded and none named, a command fails with `graph_ambiguous`, and its `next` repeats the command with
`--graph`. Agents working in one graph should `export UMX_GRAPH=<alias>`.

Some reads take several graphs: `umx node list`, `umx edge list`, `umx search` and `umx query` accept
`--graph a,b` or a saved scope:

    umx scope save engagement_all --graph engagement,workshop_day1,workshop_day2
    umx search approval --scope engagement_all

Search and query with nothing named read every loaded open graph. `umx node find`, writes, `neighbours`, `walk`
and `path` take one graph.

### Copies of one graph

Two folders with the same graph id (two git worktrees of one repository) are copies. Load the second under its own
alias:

    umx graph load ~/wt/feature/engagement --as engagement_feature

Reads that name no graph read the first-loaded copy only and list the others in `copies_skipped`, so counts are
never doubled. Name a copy with `--graph` to read it. Saved scopes always resolve to the primary copy.

## Linking across graphs

An edge can join nodes in two graphs. It is stored in one graph, the author, named with `--graph`; only the
author's files change:

    umx edge add <engagement node> clarifies umx:workshop_day1/<node id> --graph engagement

Link across graphs when the thing already lives in another graph and that graph is where it belongs: a correction
pointing at the workshop node it corrects, a decision pointing at the client's requirement. Don't copy the node
into your graph to avoid the link; the copy drifts.

Don't link when you mean "these look alike". Similarity is a hint for a person or agent to check, not a fact about
the world; `similar_to` edges are made by umx inside each graph and are never written across graphs.

What reads do with links:

- `umx edge list --from/--to` and `umx neighbours` show cross-graph links at either end, including links stored
  by other loaded graphs. A target in an unloaded or hidden graph shows its ids and `graph_not_loaded`, never its
  title.
- `umx walk`, `umx path` and `umx query` follow a link into another graph when the graph that stores it and both
  of its ends are in the read. With no graph selected (no `--graph`, saved scope or `UMX_GRAPH`), the read is
  every loaded open graph plus any graph your addresses name; with `--graph a,b` or a saved scope, it is exactly
  those. `UMX_GRAPH` selects too: a walk from a bare id, or a query, then reads only that graph (an address in the
  command wins over it, and `umx query --all` ignores it). A named or secured graph is in the read only when you
  name it, with `--graph`, a saved scope, an address or `UMX_GRAPH`; so `UMX_GRAPH` set to a secured graph reads it
  on every bare read. A far end outside the read shows in walk and path as `graph_not_loaded`, and in
  query as no row plus `external_edges_skipped`; neither means the node is absent.

      umx walk umx:engagement/<id> --depth 3 --graph engagement,workshop_day1
      umx query 'MATCH (c)-[:clarifies]->(w) RETURN c.title AS correction, w.title AS original, w.graph AS g' --graph engagement,workshop_day1

- The first cross-graph link raises the author's file format, and older umx builds refuse that graph until they
  are upgraded.
- An open graph may link into a secured one (it warns `link_into_secured`). Never copy the secured node's text
  into the open graph.

A whole graph can be a link target too: `umx:<alias>` names a graph's virtual node, so a correction can point at
a frozen workshop graph as a whole (`umx edge add <id> records umx:workshop_day1 --graph engagement`).

Planned: a shared directory graph for people, so each graph links to one person node instead of keeping its own
copy.

## Evidence and sources

An evidence pointer's source is a path inside the graph's folder, a file elsewhere through a source root, a URL,
or a umx address. A path that climbs out of the folder (with `..` or `~`) is refused. Cite transcripts and
documents where they live:

- `root:repo/<path>` names a file in the git repository that holds the graph's folder. It needs no setup.
- Any other folder is a named source root, set once on each machine:

      umx root add calls /Users/me/calls
      umx node note <node> --append "Raised in the call" --evidence 'root:calls/2026-11-03-call.vtt#seq=120-140' --evidence-kind room

  A teammate who pulls the graph runs their own `umx root add calls <their folder>`; until then the pointer is
  refused with `root_unknown`, whose next is that command.

Copy a source into the graph folder (for example `sources/`) only when it should travel with the graph, and think
about whether it may be committed first.

## Retiring things

Nodes are retired, not deleted: there is no node delete. Move a node that no longer holds to a status such as
`ruled_out`, with a reason; it keeps its history, and its links still explain why.

Edges can be changed and removed:

    umx edge list --from <node>                          # edge ids
    umx edge edit <edge> --relation depends_on --reverse # retype and swap ends; the id stays
    umx edge delete <edge>

`umx batch` takes `edge edit` and `edge delete` lines for many at once. An edge with an end in another graph is
changed the same way, from the graph that stores it (`--graph` or its address). Its far end is never loaded or read,
and an edit never moves an end: to point an edge somewhere else, delete it and add a new one.

Vocabulary names can be removed once nothing uses them: `umx ontology remove type risk` (or `status`, `relation`,
`field`). A removal is refused with `ontology_in_use` while something still uses the name, and nothing changes:

- nodes or edges that hold it: change or delete them first;
- for a field, the type's default `node list` columns (`umx ontology set type <type> --columns`) when they name
  it, even if no node holds the field. Here the error's `next` is `umx ontology set type <type> --reset`, which
  changes those columns back to the usual ones rather than listing anything: reset them, or set them without the
  field, then remove it.

umx keeps its own names (`part_of`, `open`, the derived and provenance names). Declaring or removing a name a
second time the same way reports `unchanged` and writes nothing, so setup scripts can be rerun.
