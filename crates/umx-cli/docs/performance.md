# Performance: what things cost

What an agent should know so its work stays quick on big graphs. Numbers are medians from `just bench` on an Apple
M4 Max with rich synthetic graphs (about three edges a node); your machine will differ.

## Reads are cheap

| | 10,000 nodes | 100,000 nodes |
|---|---|---|
| `umx graph load` | about 45 ms | about 0.44 s (0.63 s with vectors) |
| `umx node find` | about 7 ms | about 17 ms |
| `umx node list --limit 100` | | about 10 ms |
| walk to depth 3, path, neighbours (in memory) | under 0.1 ms | under 1 ms |

Most of a CLI read is starting the process and the round trip. Read as often as you need to; look before every add.

- Use `--limit` on walks from hubs: a walk stops once its page is full, so a small limit is cheap on any graph
  (`total` is then null; page with `--offset`).
- Bind a query's start node by id and keep variable-length steps short: a step's upper bound is at most 10
  (`*1..10`), and at most 2 on a step naming `similar_to`. A query returns 1,000 rows by default (`--limit` up to
  100,000; page with `--offset`). A query over its work budget, 1,000,000 rows held, or 10 s is refused
  (`query_too_large`, `query_timed_out`). At most two queries run at once (`query_busy`: try again).

## Writes cost more as a graph grows

Each write rewrites, whole, every file it changes.

| From the CLI, median | 10,000 nodes | 100,000 nodes |
|---|---|---|
| `umx node add` (nodes.jsonl) | 24 ms | 52 ms |
| `umx node add --parent` (nodes.jsonl and edges.jsonl) | 63 ms | 174 ms |

- Use `umx batch` for more than a handful of changes: each file is written once for the whole batch.
- A batch is quick: at 100,000 nodes, 1,000 adds are checked (near-duplicates included, about 0.01 ms an add) and
  written in about 0.1 s in process. It holds the daemon's writer while it runs, so other commands wait; split very
  large imports when others are reading.
- Batch limits: 10,000 lines and 8 MiB.
- A write returns before its vectors are made. Don't wait for embedding; `vectors_pending` in `umx graph list`
  falls as they land.

## Memory

A loaded graph takes about 1.4 KB of memory a node, edges included, plus about 1.1 KB a node for vectors. A
100,000-node graph keeps the daemon at about 225 MB; the local embedding model adds about 1 GB. Graphs unused for
30 minutes unload themselves and load again, with `graph_loaded_on_demand`, when a command names them.

## The daemon's limits

- 64 commands at once; past that `daemon_busy`: wait a moment and run it again.
- The CLI waits up to 600 s for a write and 120 s for a read. `reply_timed_out` on a write means it may or may not
  have happened: check with `umx node get` or `umx graph list` before running it again.
