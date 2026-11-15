# Graphfile

A `Graphfile` is a plain TOML file at the top of a git repository that lists the repository's graphs: each one's
folder (relative to the repository top), a suggested alias and its graph id, plus optional named sets of graphs to
load together. It is committed with the repository and you may edit it. It holds no graph data, only where the
graphs are. Every graph works without one.

The graph index (`umx graph list --all`) records what this machine has loaded; the Graphfile records what the
repository holds. Loading through the Graphfile also records each graph in the index.

## What it looks like

    # Graphfile: the umx graphs in this repo. umx keeps it sorted; see `umx docs graphfile`.
    graphfile = 1

    [[graph]]
    path = "docs/graphs/acme"
    alias = "acme"
    id = "01JDCYPHER00000000000000A1"

    [[graph]]
    path = "docs/graphs/acme_risks"
    alias = "acme_risks"
    id = "01JDCYPHER00000000000000B1"

    [sets]
    acme = ["acme", "acme_risks"]

umx writes it sorted by path, with the keys in this order, one blank line between tables and a final newline, so
writing an unchanged file gives the same bytes and `git diff` stays empty. A graph at the repository top has
`path = "."`. Paths use `/`, never `..`, `~`, a leading `/`, a backslash or a `:` in the first part. Keys umx does
not know are kept.

The id is the graph's identity, the path is where it is, and the alias is only the name it loads under.

## Making graphs: umx graph new

`umx graph new` adds the new graph to its repository's Graphfile, creating the file if there is none. The reply's
`data.graphfile` names the line it added (`path` and `alias`) and `undo`, the command that removes it again.

    umx graph new /Users/me/work/repo/docs/graphs/acme --alias acme
    umx graph scan /Users/me/work/repo --drop acme                # the undo, if you did not want it listed

- `--no-graphfile` leaves the Graphfile alone. Use it for a private graph, and for a graph in a folder git ignores:
  umx does not read `.gitignore`, so it cannot tell.
- A secured graph, or one only reads that name it may see, is not added (`graphfile_skipped_secured`): listing it
  would tell everyone who clones the repository that it exists. `--graphfile` adds it anyway.
- A graph outside any git repository has no Graphfile, so nothing is added. Only `--graphfile` says so, with the
  hint `graphfile_no_repo`.

## Finding graphs: umx graph scan

On a repository you have not worked in, run `umx graph scan`. With no folder named it walks the whole repository
from its top, wherever in it you are; a folder you name narrows the walk to that folder. It finds every graph
folder (one holding `graph.json`, or only the marker of a write cut short while the graph was being made) and lists
each against the Graphfile. It loads nothing and reads only each `graph.json`'s id, alias and policy.

    umx graph scan
    umx graph scan /Users/me/work/repo/docs --depth 12

Each row has `path`, `alias`, `id` and a `state`:

- `listed`: in the Graphfile, and found where it says.
- `new`: found, not in the Graphfile. `secured: true` means `--add` leaves it out.
- `missing`: in the Graphfile, and no graph at its path. A missing graph is reported, never removed.
- `moved`: no graph at its path, and its id was found at exactly one new path (`from` is the old one). `unique` is
  true when the scan saw everything; null when it stopped early, met a folder it could not list, or met a graph it
  could not read or with a write cut short (any row `unreadable` or `pending`), so another copy may exist and
  `--add` leaves the entry's path as it is.
- `id_mismatch`: the Graphfile's path holds another graph (`found_id`).
- `unreadable`: its path holds a graph folder whose `graph.json` umx could not read.
- `pending`: the graph folder holds a write that was cut short (a crash), even one cut short while the graph was
  being made, before it had a `graph.json` (then `alias` and `id` are null). Scan never repairs it: load the graph
  (`umx graph load <folder>`), which finishes or undoes the write, then scan again.
- `outside_repo`: its path leads out of the repository (a symlink); umx does not follow it.
- `duplicate_id`: a second folder with an id already seen (a copy).
- `nested_repo`: another git repository inside this one. Its own Graphfile owns its graphs; `next` is the scan to
  run there.

The walk never follows a symlink, skips `.git`, `target`, `node_modules`, `.umx-tmp` and every folder whose name
starts with `.`, never goes inside a graph folder, and stops at 8 folders deep (`--depth`, at most 32), 100,000
folders or 10 seconds. When it stops early, `data.truncated` is true: scan a smaller folder. `data.unreadable`
counts the graph folders whose `graph.json` it could not read and the folders it could not list. An entry the walk did
not see is looked at in its own folder, so a scan that stopped early never calls a live graph missing or moved, and
`--add` never points an entry at a copy while the original is still there. Scan only reads: it changes nothing in
any graph folder.

`--add` writes the `new` graphs and the `moved` paths, in one write. A new graph's alias is the one in its
`graph.json`, with `_2` (and so on) added when the Graphfile already has it, shortened when needed to stay within
64 characters; the row shows the alias written, the name to load it by. `--drop <alias>` removes that entry
(and that alias from any set); an alias the Graphfile does not list is refused (`graphfile_unknown`) and nothing
changes.

    umx graph scan --add
    umx graph scan --drop old_notes

A Graphfile umx would write differently (comments, another order) is left as it is (`graphfile_not_canonical`).
`--rewrite` writes it in umx's form, losing comments:

    umx graph scan --add --rewrite

## Listing and loading: umx graph list --graphfile, umx graph load --graphfile

    umx graph list --graphfile
    umx graph load --graphfile
    umx graph load --graphfile --set acme

`graph list --graphfile` lists every entry with its `state`: `loaded` (with `loaded_as`), `not_loaded`, `missing`
or `outside_repo`, and `data.sets`. `graph load --graphfile` loads every entry, or one set, through the normal load
path: your roots, each graph's own policy, and the id in the Graphfile must be the id in the folder
(`graphfile_id_mismatch` otherwise). A secured graph loads and stays readable only by name. The reply lists what
loaded (`new_to_machine` for a graph this machine had not loaded before) and what did not. One entry that does not
load is one warning, `graphfile_entry_skipped` with its code (an alias already in use names the `--as` load to run);
the others still load. At most 256 graphs load at once: load a set.

Both use the repository holding the current folder, or the folder you name.

## A Graphfile umx will not use

The whole file is not used (`graphfile_invalid`) when any path, alias or id appears twice, a set names an alias no
graph has, a path is not in the form above, it is not valid TOML, or it lists over 1,000 graphs or 100 sets. A file
over 1 MiB is not read at all (`config_file_refused`, naming why). A file from a newer umx is `graphfile_newer`. Then `graph list --graphfile` and `graph load --graphfile` use nothing
from it, `graph scan` still lists what is on disk, and `graph new` neither adds to it nor rewrites it
(`graphfile_unusable`). The error names the lines, never the text. Fix the file by hand, then run `umx graph scan`.

umx refuses a Graphfile that is a symlink, reads it only at the repository top, and never writes one it would
refuse to read (`graphfile_too_large`). When another program changes the file while umx is writing it, umx tries
once more, then leaves it (`graphfile_changed`) and names the command to run again.

## Sets are not scopes

A set is in the Graphfile, committed, and used to load graphs together. A scope (`umx docs get scope`) is this
machine's and used to read graphs together. `--scope x` never reads a set.
