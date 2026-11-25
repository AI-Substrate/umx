# umx graph view (apps/ui)

A local, read-only view of the graphs your umx daemon has loaded (workstream 009, step S1). Next.js with sigma.js
and graphology. It reads only through the installed `umx` CLI and binds 127.0.0.1 only.

## Run it

Build once (and after each pull), then start it with umx:

    pnpm --dir apps/ui install --frozen-lockfile
    pnpm --dir apps/ui build
    umx ui                     # or: umx ui --graph acme, umx ui --port 4747, umx ui --no-open

`umx ui` checks the daemon is up, runs `node bin/serve.mjs --stdin` in a process group of its own, waits for its
`{"ready":true,"port":N}` line and opens your browser at `http://127.0.0.1:N/`. There is no token: any browser tab
on this machine can open that URL (or `http://localhost:N/`); another host name, or a page on another site, is
refused. Any process or user on this machine that can reach 127.0.0.1 can read the view: the boundary is your user
account. Ctrl-C stops it; when umx ends in any way the server sees its stdin close and stops its whole group. See
`umx docs get ui`.

- `UMX_HOME` picks the daemon, as for `umx` itself. umx passes its own path as `UMX_BIN`; run by hand, `UMX_BIN`
  (an absolute path) picks the `umx` binary, otherwise the first `umx` on `PATH`.
- The view reads an `umx` whose protocol is between `UMX_PROTOCOL_MIN` and `UMX_PROTOCOL` in `lib/umx/index.ts`. It refuses
  an older one, and reads a newer one with a `ui_protocol_newer` warning (logged once by the server), since a protocol bump
  usually only adds fields. Agent control talks to the daemon's socket directly, and the daemon refuses any other
  protocol there, so `UMX_PROTOCOL` must equal `umx_protocol::PROTOCOL`: umx-arch's `protocol_pin` test fails a wire
  bump until this pin moves with it.
- `pnpm start` (`node bin/serve.mjs`) still works for local work and prints its URL; `pnpm dev` runs
  `next dev` behind the same launcher. The run's settings live in a process global (`lib/guard.ts` `runSettings`),
  never in `process.env`, so nothing the server starts inherits them.

## What it shows

Choose graphs with the button on the left, or press `G`: a dialog lists every graph `umx graph list --all` knows (with counts), in two columns grouped by loaded, not loaded (opening one loads it) and cannot open (with why and the fix). Type to filter by name; check and uncheck freely, "Select all loaded" (or "Select loaded matches" while filtering) and "Select none", then "Open N graphs" (Cmd/Ctrl+Enter). Nothing is read until then, and Cancel or Esc leaves the canvas as it was. Opening none clears the canvas. Arrow keys move through the rows and Space checks one. The sidebar lists the open graphs in their colours, each with × to take it off. A reload keeps what you had: the open graphs are kept in the tab's session storage (so another site can never choose what the view reads), a secured one you opened in this tab included, and the look (colour by, group by, resolution, spacing, merge entities, the provenance and out-of-play switches, the chips turned off) is kept in the address bar's fragment, so a copied link carries it. "Include secured graphs" is never kept. Each loads in pages of 10,000 rows (up to 100,000 nodes and
200,000 edges a graph; past that a banner says it is truncated) and lays itself out with ForceAtlas2 in a web worker.
Scroll to zoom, drag to pan, the corner buttons zoom and fit. Click a node for its fields, note, evidence and
neighbours (`umx node get`, `umx neighbours`); click a neighbour to move to it. A neighbour umx could not resolve
shows its state and id. `Esc` clears focus and then the selection. Provenance nodes, and out-of-play nodes (status
retired, superseded or ruled_out), are hidden until you switch them on; the canvas says how many out-of-play nodes
are hidden, with "show". The theme button cycles light, dark and system.

Several graphs (plan 133): up to 12 graphs share one canvas. In the dialog a click on a graph checks it or unchecks
it; "only" selects it alone; "Select all loaded" selects every loaded graph that is not secured; a saved scope
(`umx scope list`) selects its members. A secured graph opens only when you click it, when a launch link from umx names it
(`#t=<code>&g=a,b,c`; a lone `#g=` opens nothing), or while "Include secured graphs" is on: that switch is off whenever the view opens, is never stored,
and no agent command sets it. "Select all loaded" and scopes count the secured graphs they left out and never name them. Of
two rows that share an alias only the loaded one opens. Nodes and edges are keyed `<alias>/<id>`, so equal ids in two
graphs never meet. An edge between two open graphs is drawn; one to a graph that is not open is left off and counted
in the top bar. Hulls group by graph when more than one graph is open, and Colour by has Graph; each graph is laid
out in a region of its own. The node panel names the node's graph; a neighbour in another open graph is a link to
it, one in a graph that is not open is labelled and not followed. umx's hints from the reads (a warning with a next
command, or an envelope's `hints`) show on the canvas and in Find as umx gave them. The picker's view is
`components/graph-picker.tsx` behind `GraphPickerProps`; its state is `components/use-graph-set.ts` and its rules
`lib/graph-set.ts`, so another picker replaces only the view.

Find: Title runs `umx node find` (typos allowed), Search runs `umx search` (exact, words and meaning), in each open
graph, and lists what umx returns in its order (graphs taking turns, rank by rank, each hit labelled with its graph), with its warnings and hints as given (for example the hint `similarity_unavailable`
when meaning was not used). Click a result to select it and move to it; a hidden one says why (provenance, retired,
a filtered type or status) and Show brings it back. "Similar to this node" in the node panel reads the node's
similar_to list from the daemon (`umx neighbours <id> --relation similar_to`); vectors never reach the browser.
`/` or Cmd+K focuses the search box, `Esc` clears it.

Filters: chips for relations, types and statuses, each with a count. Turning one off hides exactly what it counts;
the clusters, hulls, focus, legend and top-bar counts all follow, and "Reset filters" turns every chip back on.

Clusters: the worker runs Louvain (seeded) on the nodes on screen, with part_of edges weighted up, and lays the graph
out from those clusters; the same graph opens the same way every time (the layout stops on an iteration count). Each
group of 4 or more gets a tinted hull, and a label when zoomed out. The Clusters section switches grouping between
Louvain (with a resolution slider) and Parent (the part_of trees). Colour by type, cluster or status from the top
bar; the legend follows it and counts only what is on screen. Focus hops in the node panel (1 to 3) dims everything
further away; the nodes it keeps are the ones `umx walk <id> --depth n --direction both` returns over the nodes on
screen. If the focused node is hidden (a toggle, or picking a hidden neighbour), focus pauses: nothing is dimmed
until the node is shown again.

Spine: a graph with declared spines (`umx graph spine show <alias>`) opens laid out along its main spine (else its
first), and the Spine section lists each one with None. Where each node sits is umx's own placement, read with
`umx node list --columns id,spine:<name>`: the spine is a line through the anchors, earliest band on the left (by
date on a calendar spine, by step or chain on any other), each anchor on the line and every node placed in a band
beside it, above or below the line as the force layout had it; nodes placed only by rule sit in the band the rule
gives; nodes the spine does not place wait on a shelf below. A band is a grid of rows and sub-columns, one row apart;
the number of rows is picked so the whole spine fills the canvas at Fit, so a big band stands tall and each band is
as wide as its sub-columns. Anchors are labelled first, each cut to the room before the next (crowded, every few
bands); other nodes are labelled once rows are a label apart on screen, never over another label. None puts every
node back where the force layout put it. Each open graph keeps its own spine, laid out in its own region. Cluster hulls are hidden while a spine is shown.
The choice is kept across a reload in the tab's session storage beside the open graphs, only for graphs still open,
never in the fragment. A umx older than the `spine:<name>` column says so instead of laying the graph out, as does a
spine the graph no longer declares.

Live updates: a node or edge added, edited or removed through the CLI (or any umx client) shows in an open view
within about a second, without a reload: "Live" in the top bar, with a brief count of what changed ("+3 nodes, 1
edge removed"). New nodes appear next to their neighbours and settle with a short nudge; nothing else moves, and
the camera, selection, filters and agent state stay. A removed node leaves the selection, focus and agent highlight,
and the panel says it was removed. A big change (more than 500 ids, such as a large batch, or a reload of the graph)
reads the graph again and applies the difference the same way. Clusters recompute when the worker is next idle.

Agent control: while the view is open, an agent can point at things in it with `umx ui focus|highlight|filter|fit|
clear` (`umx docs get ui`). The agent's highlight is a second, cooler halo with a small "A" badge, never your
selection's ring; its focus dims everything further than its hops. Each command lands in the activity strip at the
bottom ("<who, or an agent> focused <title>: <note>", the note as the agent's text), with a history drawer whose
entries show it again. Follow agent (top bar, on by default) lets an agent move the camera, but never within 5
seconds of you touching the canvas: then the strip asks "Agent wants to show X" with Go. A filter turns chips off
or on and they pulse once; Undo puts them back. `clear` removes only what agents set. Agents never show what the
view hides: a provenance or retired node stays hidden unless you switch it on, and a node the view does not show is
named only as "not shown". A newly opened view applies the agent's current highlight and filter, but no camera move
older than 30 seconds.

Selection back to agents: a click selects a node, shift-click adds or removes one, and a shift-drag lasso adds the
shown nodes inside it (never a hidden one); each has the accent ring, and the panel shows the last one clicked. The
tab publishes the selection to each open graph (that graph's nodes, at most 1,000) about 250 ms after it stops changing, and an
agent reads it with `umx ui selection` (ids, addresses, titles and types). The top bar says "Agent can see your
selection (n)" whenever it is shared, with Share selection beside it: on when the tab opens; off clears what agents
see at once and publishes nothing until it is on again. A graph taken off the canvas has its selection cleared
first. The daemon keeps one selection per graph for every tab, so pausing or leaving a graph clears it whichever tab
published it; a failed publish or clear stays on the top bar ("they may still see your previous selection") and is
retried until that graph's publish succeeds.

## Rules the code keeps

- `lib/umx` is the only module that may run a process or open a socket, and nothing reads files
  (`eslint.config.mjs`, proved by `test/lint-plant.test.ts`); `lib/umx/socket.ts` alone may `lstat` the daemon's run
  folder, and refuses it unless it is a plain 0700 folder of yours holding your socket. It runs eleven read verbs (`scope list` the newest) with fixed arguments, no
  shell and an env of only `UMX_HOME` and `UMX_OUTPUT=json`; ids and aliases are checked before umx runs. Search
  text goes as one argument right after the verb's words: at most 200 characters, no control characters, and never
  starting with `--` (umx has no `--` terminator and would read it as a flag), and never `-h` or `-V` alone (umx's
  help and version flags). A reply must be for the verb that ran; a reply carrying errors fails whatever its status.
- Agent events: the server holds one `ui wait` long poll on the daemon socket (`lib/umx/socket.ts`, `lib/events.ts`)
  for the graphs the tabs have open, each first checked as the picker would open it, and every tab long-polls
  `GET /api/events` once for all the graphs it shows (`graph=` repeated, at most 12) behind the same guard as every
  API route (no SSE, no CORS). Events carry ids only; titles come
  from the graph the browser already read, and notes and titles render as text.
- Live updates: the same `ui wait` follows each open graph's revision (the change feed, `lib/live.ts`), and each
  tab's `GET /api/events` carries the revision it applied in each graph (`live=<alias>:<epoch>:<rev>`) and gets what
  changed since in each, merged, or an overflow. A tab
  applies a change only from the revision it is at, so none is applied twice or skipped. It reads the touched rows
  with `POST /api/graphs/<alias>/rows` (`lib/rows.ts`, JSON only, at most 1,000 ids of each kind), which runs the
  two id-list reads: `umx query` with fixed text and the ids in `--param-json ids=[...]`, at most 500 a read; edge
  reads are halved on `query_too_large`, since each edge id costs a scan of the graph's edges. With other graphs open
  the request names them (`with`, each checked as the picker would open it), and the edge read runs with
  `--graph <alias>,<others>` so umx follows edges between them; only one with both ends open is returned.
- The selection: a tab POSTs `{nodes}` to `/api/graphs/<alias>/selection` (JSON only, at most 1,000 node ids, the
  graph first checked as the picker would open it), and the server sends `ui select` on the daemon socket. Both
  POST routes (and the rows route) read bodies through `lib/http.ts` readBody, which stops at 64 KiB even when no length is declared (413). Publishes
  from a tab go one at a time, in order (`lib/selection.ts` Publisher).
- `proxy.ts` runs `lib/guard.ts` on every request before routing: Host must be `127.0.0.1:<port>` or
  `localhost:<port>` (DNS rebinding); an Origin must be the view's own; API calls must not be cross-site
  (Sec-Fetch-Site), writes must carry the Origin, and every API call must carry the `x-umx-view: 1` header the
  view's own fetches send (a page on another site cannot add it without a CORS preflight, which is never granted).
  It sets the CSP (`lib/csp.ts`) and no CORS headers. In `--dev`, WebSocket upgrades (HMR) skip the proxy, so `bin/upgrade-guard.mjs` checks
  their Host and Origin first. Graph content is shown as text; only whole http(s) values become links.

## Gate

`just ui-check` from the repo root: frozen install, lint, typecheck, unit tests, build.
