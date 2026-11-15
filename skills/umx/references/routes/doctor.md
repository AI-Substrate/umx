# /umx doctor: a umx command failed, or a graph looks wrong

Job: get from an error to a working command, or settle what a merge left in a graph.

1. Read the error's `code` and `next` (§ C1). Run the `next`, filling any placeholder. Most errors end here.
2. The code is not clear: `umx docs get errors` lists every code with when it happens and what to run.
   Not sure what is wrong at all: **usage 055**: run `umx doctor`. It works with the daemon down and checks
   the daemon, the run folder's modes, the embedder and every loaded graph. Exit 0 is ok, 5 degraded, 6
   broken; run each finding's `next`. `umx doctor --fix` removes a stale socket and tightens modes, nothing
   else.
3. Common states:
   - `daemon_not_running` (exit 3): `umx daemon start` in another window, then retry.
   - **usage 025**: `protocol_mismatch`, `internal` with `writer_stopped` (`umx daemon status` shows
     `writer: failed`) and `graph_format_newer` do not clear by themselves; never retry them in a loop. Stop
     and start the daemon with the CLI you are using (`umx daemon stop`, then `umx daemon start`); for
     `graph_format_newer`, check `umx version` and install a newer umx first.
   - **usage 080**: `config_restart_needed`, `writer_stopped` and a doctor finding whose `next` is
     `umx daemon restart`: run it. It waits for queued writes, restarts through the service when the daemon
     runs as one, else in place with the same flags, and never starts a daemon that is not running.
     `daemon_restart_refused` means an older daemon: `umx daemon stop`, then `umx daemon start`.
   - **usage 142**: `doctor_skill_out_of_date` or `doctor_skill_link_out_of_date`: this skill is not the one
     your umx was built with, so some routes may name the wrong verbs. Tell the human the pinned update command
     the message names, and trust `umx <noun> --help` over this skill until it is updated.
   - **usage 093**: `internal` with `request_timed_out`: the daemon waited (60 s for the first byte, 10 s
     for each later pause, 120 s in all) and the request did not complete, usually an overloaded machine. Wait a
     moment and run it again; if it repeats, `umx doctor`. `request_unreadable` is different: the request
     arrived but could not be read.
   - **usage 087**: `internal` with `reply_timed_out`: the CLI stopped waiting for the daemon's reply (600 s for a
     write, 120 s for a read, 60 s for `umx daemon status`, or a reply that stalled). A write may or may not have
     run: check with a read (`umx node get`, `umx graph list`) before running it again; `umx doctor` if it repeats.
   - **usage 109**: a graph missing after a restart: `doctor_graph_not_reloaded` names the reason and the
     `umx graph load <folder> --as <alias>` to run once it is fixed; `umx graph unload <alias>` forgets it.
     `graph_id_changed`: the folder holds another graph now, so load it under its own alias. `registry_unreadable`:
     nothing reloads until the human fixes or removes `~/.umx/registry/graphs.toml`; never edit it yourself.
   - `daemon_version_mismatch` (a warning): the CLI and daemon are different builds. Run its `next`, then
     start the daemon from the same build (`just dev` in the umx repo).
   - **usage 090**: `daemon_busy` (exit 1): the daemon is serving as many requests as it takes at once and
     the command did not run. Wait a moment and run it again; `umx daemon status`, `stop` and `restart` still
     answer. If it keeps coming back, `umx doctor`, then `umx daemon restart`. Exception: `daemon_busy` from
     `umx daemon restart` itself may come after the restart was taken, so the restart may have happened. Run
     `umx daemon status` and compare `started_ms` or `uptime_s` before restarting again; never `umx daemon start`.
   - `path_outside_roots`: the folder is outside the daemon's roots; `umx docs get graph` § UMX_ROOTS.
   - **usage 031**: `config_invalid` on every graph command: an invalid `roots` or an unreadable
     `config.toml` closed every graph folder. Run `umx config check` and fix what it names; for `roots`,
     ask the human which folders (umx never suggests them), then `umx daemon restart`.
   - **usage 076**: `config_file_refused` "over 64 KiB": a `config.toml` (often a repo's, from git pull), the
     secrets file or the state file is too big to be umx's. Find what wrote it; shorten or remove it.
   - **usage 097**: is the service installed? Ask `umx daemon service`: it reads launchd or systemd only, so a
     slow or refusing daemon cannot hide the service (`umx daemon status` drops `service` when its call fails).
   - **usage 088**: `config_file_refused` (or a refused secrets or state file) naming chmod 600 or chown: a
     permission problem with umx's own file, even one you cannot open. Apply that repair (not wider `roots`);
     after a machine-config repair, `umx daemon restart`. Other permission errors stay io errors.
   - A graph should be dropped from memory: `umx graph unload <alias>`. Its files stay; load it again later.
     **usage 127**: to drop it from `umx graph list --all` too, `umx graph forget <alias>` once it is unloaded.
4. **usage 024**: after a git merge, or whenever `umx graph load` warns, run `umx check --graph <alias>`.
   Each row is a `duplicate_id` (the last line is the one in use), a `dangling_edge` (kept in the file, not in
   the loaded graph; `kept_lines` counts it) or an `unknown_key` (kept as it is). Nothing is dropped. To
   settle one: `umx graph unload <alias>`, fix the file by hand, `umx graph load` it, and `umx check` again
   to confirm. Never edit the files while the graph is loaded, and never delete keys you don't recognise.
   **usage 146**: `duplicate_locator` means two resource nodes hold one file or URL (a merge added it twice): keep
   the lower id (in use), move what you need onto it, then remove the other. `locator_not_normalised` is a
   hand-edited source; it still matches.
   **usage 151**: rows `graph_not_loaded` with `link_targets_unnamed` mean links into graphs the check did not
   name (secured, named-visibility or unloaded graphs look the same). Check the linked graphs together:
   `umx check --graph acme,acme_d1,acme_d2` (or `--scope <s>`); links between them then resolve.
5. **usage 052**: `recovery_conflict` means a crash cut off a write to several files and the named files changed
   since (a pull, a checkout). Every command on that folder fails until a person chooses: keep the files as
   they are (delete `.umx-tmp-set.json` and the other `.umx-tmp-` files), or put the named files back and run
   the command again to finish the write. Stop and ask; don't choose yourself. Leave `.umx-tmp-` files alone
   otherwise; the `.umx-tmp-*` line in the folder's `.gitignore` keeps them out of git.
   **usage 166**: `graph_files_changed` (a write) or `stale_files` (a read's warning) means a loaded graph's
   files changed behind it (a checkout, a pull, a hand edit, a touch). Nothing was written. Don't retry: reload
   with the `next` (`umx graph unload <a> && umx graph load <path> --as <a>`), then check your write is still
   needed against the files as they are now.
6. **usage 030**: umx's own settings (roots, idle unload, embedder instances). `umx config show` lists every
   setting and where it came from; `umx config check` names what is wrong, each with a `next`. Change one with
   `umx config set <key> <value>` (add `--repo` for this repository; a repo may only choose `none` or a local
   embedder) and remove one with `umx config unset <key>`. Never put a key or token in config: name the env
   var in `api_key_env`. Never copy another tool's config file; set the values. `umx docs get config`.
7. **usage 058**: `umx check` row or warning `gitignore_line_missing`: the folder's `.gitignore` no longer hides
   `.umx-tmp-` files. Add `.umx-tmp-*` as the last line (nothing before it) and run `umx check` again.
   **usage 072**: the check reads lines as git does, `[[:class:]]` brackets included, under both
   `core.ignorecase` values, so `!.UMX-TMP-*` counts as un-hiding.
8. **usage 070**: `graph_too_large`: a graph file is over umx's cap (graph.json 1 MiB, .gitignore 64 KiB, a
   shard 64 MiB, nodes.jsonl and edges.jsonl 4 GiB). Nothing was read or written. Don't trim the file or retry;
   tell the human which file and size.
9. **usage 071**: `unsorted_file` (a load warning and a `umx check` row): nodes.jsonl or edges.jsonl has a line
   out of id order or a blank line. It loads as usual; the next write of that file puts it in order. Don't sort
   it by hand.
8. **usage 054**: after a merge or pull, also run `umx embeddings repair --graph <alias>`: it keeps the first of
   two equal vector lines, drops leftovers and queues nodes whose vectors are missing, stale or conflicting
   (`embedder_not_configured` means there is no embedder yet; nothing to do). `vector_orphan` is a line for a
   node the graph no longer has and `vector_line_kept` one umx cannot read (often a merge marker); both are
   kept, nothing is lost.
9. **usage 059**: `policy_downgrade_unconfirmed` means graph.json now says a lower embedding policy than this
   machine confirmed (an edit, a pull, a deleted key or section). The graph stays secured. Tell the human and ask
   them to run `umx graph confirm-policy <alias>` in their own terminal, where it asks them to type the alias. Never
   run it yourself, and never wrap it to reach a terminal. `local_state_unreadable`: every graph counts as secured
   until the human fixes the file it names; umx never rewrites it. The same holds for either D53 setting
   (usage 128): local to trusted, trusted to any, named to open. `config_key_deprecated` on
   `embedders.<name>.trusted_endpoint` means a key left from before D62: run its fix, `umx config unset <key>`.
10. **usage 068**: `embedder_mismatch` means the graph's vectors are another model than the configured embedder's.
   Run its `next`, `umx embeddings reembed --graph <alias> --dry-run`, which costs nothing, and show the human the
   cost. Re-embed (the same command without `--dry-run`) only when they say so; or they configure the graph's model.
11. **usage 061** and **usage 120**: the embedder. Instances are set in the machine config (`umx docs get config`)
   and built when the daemon starts, so restart it after changing one or `embedder.active`. Azure signs in with
   `az login` by default; an api key is named by its env var in `api_key_env`, never written down. To reuse the
   embedder flowspace3 has, run `umx config import --from flowspace3 --dry-run`, then without `--dry-run`; it
   copies no secret. `umx embeddings probe` checks the sign-in and embeds one fixed sentence umx owns (never graph
   text), also for an instance added since the daemon started (`embedder_not_in_use`: it works, restart to use it);
   `--graph <alias>` checks the instance that graph would use. On `embedder_unavailable`, do what its message says
   (often `az login`), then run its `next`. `umx daemon status` shows the active instance and the credential that
   worked, never a token. `umx embeddings status` shows each graph's vectors pending, held and done and its
   embedder. Tell a graph's owner before switching `embedder.active` or reembedding their graph.
12. **usage 084**: each graph embeds with the instance that makes its recorded model (and that its policy allows);
   `embedder.active` is only the default. A graph whose model no instance makes warns `embedder_mismatch` and waits.
   A broken repo `.umx/config.toml` means no embedder for that repo's graphs until `umx config check` is clean.
13. **usage 145**: `secured_shards_remain` (a graph tightened from open still has its old `embeddings/` and `similar/`
   files in its folder): report the warning and its listed paths to the person and stop. Never delete or commit
   them, rewrite history, or set the graph back to open. umx re-embeds into the cache by itself.
14. **usage 188**: before sharing a graph, run `umx check --graph <alias> --lint`: read-only hints for orphans,
   proposed nodes holding an `entity_key`, words from this machine's lint terms file, "lacks no" titles and
   `raised_in` links whose evidence sits elsewhere. `data.lint.rows` names each node and place; follow each
   hint's next, and page a kind with `--kind <kind> --offset <n>`. Never put credentials in the terms file.
15. Still stuck: report the command, the full JSON reply and `umx version` to whoever owns umx.
