# Troubleshooting

When umx does something you did not expect, start here. Every umx error and warning carries a `code` and a `next`:
a command to run. Run the `next` first; it is usually the fix. `umx docs get errors` lists every code with its
message and what to run.

## Start with the doctor

    umx doctor --human

The doctor works with the daemon up or down, starts nothing, and never touches graph files. It checks, in order:
the daemon (running, same protocol, same build, its writer, its config), the permissions of `~/.umx` and its socket,
the config, the embedder, the loaded graphs (folder still there, `umx check` clean), and the service. Each finding
names a command to run.

Its verdict is also its exit code: `ok` (0), `degraded` (5: umx works, but something needs attention) or `broken`
(6: something umx needs is unsafe or unusable).

    umx doctor --fix

makes only safe repairs, then checks again: it removes a socket nothing answers on, tightens files and folders under
`~/.umx` that other users can read, and restarts the service when it runs an old build. It never starts or stops the
daemon by hand, never edits config, and never touches graphs.

## The daemon

| You see | What it means | Do |
|---|---|---|
| `daemon_not_running` (exit 3) | No daemon answers | `umx daemon start` in another window, or `umx daemon install` to run it as a service |
| `daemon_already_running` | `umx daemon start` while one runs | Nothing: it is running. `umx daemon status` shows its pid and build |
| `socket_unavailable` (exit 4) | The daemon cannot open its socket | Read the cause. Run directory open to others: `chmod 700 ~/.umx/run`. Path too long: set `UMX_HOME` to a shorter folder |
| `protocol_mismatch` | The CLI and the daemon are different builds | `umx daemon restart` so it runs the same build (or `just dev`) |
| `daemon_busy` | 64 commands are already running | Wait a moment and run it again |
| `internal` with `reply_timed_out` | The daemon did not answer in time | A write may or may not have happened: check (`umx node get`, `umx graph list`) before running it again. If it repeats, `umx doctor` |
| `internal` with `writer_stopped` | A write failed inside the daemon (a bug), so memory may not match the files | `umx daemon restart` |
| `service_conflict` | A umx service is installed for another binary or `UMX_HOME` | `umx daemon uninstall`, then `umx daemon install` |

Where the daemon's output goes: the window you started it in, or `~/.umx/daemon.log` when it runs as a service or
was started by `just dev`.

## Graphs

| You see | What it means | Do |
|---|---|---|
| `graph_not_loaded` | Someone unloaded the graph, or this daemon never loaded it; its message says which | Run its `next` (`umx graph load <folder>`) |
| `graph_loaded_on_demand` | A warning: the graph you named had idled out or not come back, so it was loaded for this command | Nothing; the command ran |
| A graph missing after a restart | The daemon could not reload it | `umx doctor` names it (`doctor_graph_not_reloaded`) with the command to load it |
| `graph_id_changed` | A remembered folder now holds another graph | Load it under its own alias with the `next`; don't retry |
| `graph_ambiguous` | Several graphs are loaded and the command named none | Add `--graph <alias>`, or `export UMX_GRAPH=<alias>` |
| `path_outside_roots` | The folder is outside the folders the daemon may use | `umx config set roots <folders>`, then `umx daemon restart` |
| `graph_exists` | `umx graph new` on a folder that has a graph | `umx graph load <folder>` |
| `graph_not_found` | `umx graph load` on a folder with no graph | `umx graph new <folder> --alias <alias>` |
| `alias_in_use` | Another loaded graph has that alias | `umx graph load <folder> --as <other alias>` |
| `graph_format_newer` | A newer umx wrote this graph, for example after its first cross-graph link | Upgrade umx (`just dev`) |
| `graph_too_large` | A graph file is over its size cap; nothing was read or written | Find where the file came from (`git log -p -- <file>`): almost always a bad merge or a file copied in by mistake |
| `copy_ambiguous` | Several copies of one graph are loaded and none is primary | Name one with `--graph` |
| `node_not_found`, `id_ambiguous` | The id or prefix matches no node, or several | Use a longer prefix or the full id; `umx node find "<words>"` gets an id from a title |

### After a git merge or pull

    umx check --graph <alias>
    umx embeddings repair --graph <alias>      # when the graph has vectors

`umx check` lists what a merge left in the files, one row each: lines out of order, an id on two lines, an edge
whose node was deleted on the other branch, keys this umx does not know. Nothing is dropped. To settle a repeated
id or a dangling edge: unload the graph, keep the line you want in the file, load it again and check again.

`umx doctor` reports `doctor_graph_problems` for a loaded graph whose check is not clean. That includes a
cross-graph link whose target graph is not loaded (`graph_not_loaded` in `umx check`): load that graph, and the
finding goes.

### recovery_conflict

The daemon stopped part way through a change to several files, and the files it names changed since (a pull, a
checkout, a hand edit). Every command on that folder fails until a person chooses:

- keep the files as they are now: delete `.umx-tmp-set.json` and the other `.umx-tmp-` files in the folder; or
- finish the cut-off write: put the named files back as they were (for example `git checkout <file>`) and run the
  command again.

Agents should stop and ask; which side wins is a person's call.

### graph_files_changed and stale_files

The graph's `graph.json`, `nodes.jsonl` or `edges.jsonl` changed on disk after umx loaded it: a `git checkout` or
`git pull`, a merge, a hand edit, another tool, even a `touch`. umx will not write the old copy over the new files,
so every write to that graph fails with `graph_files_changed` and changes nothing, and reads still answer from the
old copy with the warning `stale_files`. Load the files as they are now; the `next` of both is this line, ready to
run, with the folder in single quotes when its path holds a space:

    umx graph unload <alias> && umx graph load <path> --as <alias>

## Config

| You see | Do |
|---|---|
| `config_restart_needed` | The setting applies when the daemon starts: `umx daemon restart` |
| `config_invalid` | The roots or the machine config cannot be used, so every graph folder is closed. `umx config check` says what is wrong; fix it, then `umx daemon restart` |
| `config_file_refused` | The config file is a symlink, not yours, writable by others, or over 64 KiB. `chmod 600 ~/.umx/config.toml`, or rewrite it with `umx config set` |
| `config_repo_ignored` | A repository's `.umx/config.toml` tried to set something a repository may not (only `embedder.active` to `none` or a local instance). It is ignored |

`umx config show` lists every setting, its value and which layer it came from.

## Search finds less than you expect

- **Typos and short words.** Words match exactly, by their other forms (`vendor` finds `vendors`) and, from 4
  letters, as prefixes, but not with typos, and a 3-letter word is never a prefix. Try the whole word, or
  `umx node find`, which matches titles approximately and tolerates typos.
- **Words only.** `similarity_unavailable` means meaning was not used: no embedder, the graph has no vectors yet,
  the local model is still loading (search again in a moment), or the query looked like a secret. The results are
  word and exact matches.
- **Pending vectors.** `similarity_incomplete` means some nodes have no vector yet; `umx graph list` shows
  `vectors_pending` falling as they land.
- **A secured graph is skipped.** With no graph named, secured graphs are left out (`secured_graphs_skipped` says
  how many). Name it with `--graph`.
- **Another model.** `embedder_mismatch` means the graph's vectors are in a different model from this machine's
  embedder; that graph is searched by words only. See [embeddings](embeddings.md).
- **`node find` found nothing.** `find_no_match` means no title matched: find reads titles only, as whole words
  or close spellings, and does not stem. Words in notes, or another form of a word, need `umx search`.
- **Ids, numbers and dates** are not searched as words. Filter them: `umx node list --where 'due_on<2026-12-01'`.

## Embeddings

| You see | Do |
|---|---|
| `embedder_not_configured` | No embedder is chosen: set one up ([embeddings](embeddings.md)), or ignore it, everything else works |
| `embedder_unavailable` | Read the message: usually `az login`, a rejected key, or the service rate limiting. Then `umx embeddings probe` |
| `embedder_invalid` | The instance's config is incomplete or wrong: `umx config check` |
| `embedder_mismatch` | The graph records another model. Keep working; configure that model or `umx embeddings reembed --dry-run` |
| `embedder_not_local_for_secured` | A secured graph met a hosted embedder, so none of its text is embedded. Add a local instance for it |
| `secret_suspected` | A field looks like a credential and is held from hosted embedders. Redact it, or `umx embeddings allow --node <node> --field <field>` if it is not a secret |
| `doctor_local_embedder_cpu` | The local model runs on the CPU (about 3 texts a second). Fine for search; slow for big imports |

## Writing

| You see | What it means |
|---|---|
| `near_duplicate` | A node with a close title exists. The node was still added; if it was a duplicate, use the existing one |
| `off_vocabulary_type`, `off_vocabulary_relation`, `off_vocabulary_status` | A name the graph has not declared. Accepted; the `next` declares it |
| `edge_exists` | That edge is already there. Nothing was added |
| `edge_would_duplicate` | The edit would make the edge the same as another one. Delete one of the two instead |
| `edge_repeated` | A delete of an edge whose id is on several lines (after a merge). `umx check`, keep one line, then delete |
| `reason_missing` | A status move with no `--reason`. It moved; add reasons so the history explains itself |
| `use_status_verb` | Status changes go through `umx node status`, so every move is recorded |
| `evidence_outside_roots` | An evidence path climbs out of the graph folder or a source root. Cite the file through a root: `root:repo/<path>`, or `umx root add <name> <folder>` then `root:<name>/<path>` |
| `root_unknown` | The pointer names a source root this machine has not set. Run its `next` (`umx root add <name> <folder>`) |
| `batch_too_large` | Over 10,000 lines or 8 MiB. Split the file |
| `write_not_durable` | The change was made, but syncing the folder after it failed. Check the disk |

## Still stuck

- `umx <noun> --help` prints that noun's docs topic; `umx docs list` lists them all.
- `umx daemon status` shows the build, the embedder and how the daemon is running.
- `umx version` shows the CLI and daemon builds; they should match.
