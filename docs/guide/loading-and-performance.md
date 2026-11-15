# Loading, unloading and performance

What it costs to keep graphs loaded, what reads and writes cost as graphs grow, and how to keep big graphs
quick. The commands are in `umx docs get graph` and `umx docs get daemon`; this page is about the behaviour and
the numbers.

## Loaded and unloaded

The files are the truth. The daemon keeps a working copy of each loaded graph in memory and writes every change
to the files before the command returns. So unloading never loses anything, and a graph that is not loaded is
just a folder.

    umx graph load ~/graphs/launch          # read the folder into the daemon
    umx graph list                          # what is loaded, counts, last use, time to idle unload
    umx graph unload launch                 # drop it from memory; the files stay as they are

`umx graph new` loads the graph it creates. Loading a folder that is already loaded replaces the earlier load,
however its path is spelled. Loading reads the vector files once; vectors that are already current (a teammate
committed them) are not made again.

### Idle unload

A graph no command has used for 30 minutes unloads itself, so a long-running daemon does not keep every graph
you ever opened. It is still remembered: the next command that names it (`--graph <alias>` or an address) loads it
again first, with a `graph_loaded_on_demand` warning, and a restart reloads it. A command that names no graph never
loads one. `umx graph list` shows `last_used` and `unloads_in_s`, and does not itself count as using a
graph. Change the timeout for the machine, or for one daemon:

    umx config set daemon.idle_unload 2h     # 0 means never; applies at the next daemon start
    umx daemon restart
    umx daemon start --idle-unload 90m       # this daemon only

An idle daemon sleeps until the next graph is due, and uses no CPU.

### Restarts

The daemon remembers every graph you load, in this machine's graph registry (`~/.umx/registry/graphs.toml`), and
loads them again whenever it starts (an upgrade, `umx daemon restart`, `just dev`, the service bringing it back),
in the order you loaded them and under the same aliases, before it answers its first command. Background
embedding that was in progress runs again as each graph loads.

A graph that cannot come back (its folder is gone or outside the roots, a newer umx wrote it, or the folder now
holds another graph) stays unloaded and never stops the daemon starting. `umx doctor` reports it as
`doctor_graph_not_reloaded` with the `umx graph load` command to run, and `umx daemon status` lists it under
`registry.not_reloaded`. If you no longer want it, `umx graph unload <alias>` makes the daemon forget it.

`umx graph unload` is the one way to make a graph stay unloaded: it does not come back at a restart and naming it
does not load it, until you `umx graph load` it again.

A write is on disk before its command returns, so a restart never loses one. If the daemon dies in the middle of
a change to several files, the next command on that folder finishes it (see "Writes and crashes" in
`umx docs get graph`).

## Memory

Measured on rich synthetic graphs (several fields per node, about three edges per node):

| | 10,000 nodes | 100,000 nodes |
|---|---|---|
| Graph in memory, edges included | about 14 MB (1.4 KB a node) | about 142 MB (1.4 KB a node) |
| Daemon resident size with the graph loaded | about 31 MB | about 225 MB |
| `nodes.jsonl` + `edges.jsonl` on disk | about 10 MB + 5.5 MB | about 99 MB + 55 MB |

An idle daemon with nothing loaded is about 7 MB. Vectors add about 1.1 KB a node in memory (four vectors of 256
dimensions a node). A big query holds its working rows in memory while it runs; the
heaviest supported query on the 100,000-node graph peaked at about 340 MB.

Memory is the main reason to unload graphs you are not using, and the reason idle unload is on by default.

## Reads

| | 10,000 nodes | 100,000 nodes |
|---|---|---|
| `umx graph load` from the CLI | about 45 ms | about 0.44 s (about 0.63 s with vectors) |
| `umx node find` from the CLI | about 7 ms | about 17 ms |
| `umx node list --limit 100` from the CLI | | about 10 ms |
| Walk to depth 3, both directions (in memory) | about 1 µs | about 1.5 µs |
| Shortest path between two nodes (in memory) | about 0.06 ms | about 0.85 ms |
| Slowest supported Cypher query | | about 1 s |

From the CLI, most of a read's time is starting the process and the round trip to the daemon, not the graph.
Reads stay quick to 100,000 nodes and beyond.

Some reads can be made big by what you ask: a deep walk from a hub, an unbound query. umx keeps them in check:

- `umx walk` stops as soon as its page is full and it knows another row exists, so a small `--limit` is cheap on
  any graph. `total` is then null and `truncated` true; page on with `--offset`.
- `umx query` has a work budget, a cap of 1,000,000 rows held at once, and a 10 s backstop. A query over them
  is refused (`query_too_large`, `query_timed_out`) with no rows. Bind the start node by id, lower the bound, add
  `LIMIT`, or use `umx walk` or `umx path`. At most two queries run at once.

## Writes

Every write rewrites, whole, each file it changes: to a temp file, synced, renamed over the old one. That is
what makes a write safe through a crash and keeps the files clean for git, and it means a write costs more as
the file it rewrites grows.

| From the CLI, median (95th percentile) | 10,000 nodes | 100,000 nodes |
|---|---|---|
| `umx node add` (rewrites `nodes.jsonl`) | 24 ms (36 ms) | 52 ms (91 ms) |
| `umx node add --parent` (also rewrites `edges.jsonl`) | 63 ms (96 ms) | 174 ms (302 ms) |

So a single node write stays well under 100 ms to 100,000 nodes. Writes that add edges cost more, about 150 to
300 ms with 300,000 edges, because `edges.jsonl` is still rewritten whole.

The daemon applies writes one at a time, so a slow write makes the next one wait.

### Batch

Many changes belong in one `umx batch`: one call, one write of each file, all or nothing.

- The write itself is cheap: 1,000 node adds into the 100,000-node graph are written in about 0.1 s in process,
  where 1,000 single adds would rewrite `nodes.jsonl` 1,000 times.
- Checking each new title against the graph for near-duplicates costs about 0.01 ms an add, and is included in
  that 0.1 s.
- A batch holds the daemon's writer while it runs, so other commands wait. Split very large imports when others
  are reading.
- `--dry-run` checks every line exactly as the real run would and writes nothing.

Limits: 10,000 lines and 8 MiB per batch.

### Embedding after writes

A write returns as soon as the node is on disk. Its vectors are made afterwards in the background and written to
the vector files as a separate change, and a node's `similar_to` list updates when its vector lands (about 4 ms
a node at 100,000 nodes). Rebuilding every `similar_to` list of a 100,000-node graph from scratch takes about
25 s. A field whose text did not change is never embedded again.

## The daemon under load

- The daemon serves at most 64 commands at once. Past that, a command fails with `daemon_busy` and nothing runs:
  wait a moment and run it again.
- The CLI waits up to 600 s for a write and 120 s for a read. Past that it fails with `reply_timed_out`, and a
  write may or may not have happened: check before running it again.

## Size limits

`nodes.jsonl` and `edges.jsonl` may each be up to 4 GiB, `graph.json` 1 MiB, and one vector shard 64 MiB. A file
over its cap is refused with `graph_too_large` and nothing is read past the cap. A file that big is almost always
a bad merge or something copied in by mistake.

## Keeping big graphs quick

- Batch your writes.
- Unload graphs you are not using, or let idle unload do it.
- Use `--limit` on walks and lists, and bind query start nodes by id.
- Split a graph along a real seam (audience, owner, lifecycle) before it grows without bound; see
  [organising graphs](organising-graphs.md).

## Where the numbers come from

`just bench` on an Apple M4 Max (16 cores, 128 GiB), release build, under `nice -n 19`, with synthetic graphs (no
client data): `docs/project/benchmarks/2026-11-10-124330-8a40a3aba401-10000n-rich.md` and
`docs/project/benchmarks/2026-11-10-130137-9b2fa7c86da9-100000n-rich.md`. The batch figures are the 100,000-node
run's `batch.adds_ms` and `near_duplicate_insert_ms`. They are reports, not promises: your machine and graphs will differ, so measure with
`just bench --nodes <n>` when it matters.
