# umx doctor

`umx doctor` checks the whole umx setup and says what to run to fix each thing it finds. It works with the
daemon up or down, never starts a process, and never reads or changes graph files itself.

    umx doctor          # check everything; the verdict is ok, degraded or broken
    umx doctor --fix    # also make the safe repairs, then check again

It checks, in this order:

- the daemon: is one answering, does it speak this CLI's protocol, is it the same build as this CLI, is its
  writer working, and did it start with a usable config (a daemon started on a bad config keeps every graph
  folder closed until it restarts, even after the file is fixed: `doctor_daemon_config_closed`);
- the run state: `$UMX_HOME` and its `run` folder must be mode 0700, the socket and `secrets.env` 0600, and
  `config.toml` not writable by group or others, all owned by you and none of them a symlink;
- the config: a machine config umx cannot use closes every graph folder (`doctor_config_closed`, broken); any
  other problem `umx config check` finds is `doctor_config_problems`;
- the embedder `embedder.active` names: none is fine (similarity search is off; everything else works); a
  chosen one must build from its `embedders.<name>.*` keys and be ready, which for Azure means `az login`
  (or `azd`) gives a token. No text is sent and the endpoint is not called to check it, so an api key is only
  checked to be present (the row says "api key present, not verified"), and its value never appears in the
  report. A ready embedder's row has `credential`: `az` or `azd` with the program it ran, or `api_key` with the
  env var name; never a token or key. A local embedder's row names what runs its model: `metal`, `cuda` (a umx
  built with `--features cuda`) or `cpu`; a cuda build that finds no CUDA device says
  `cpu: this cuda build found no CUDA device` (see `umx docs get models`);
- the graph registry the daemon read when it started (`~/.umx/registry/graphs.toml`): one it could not use, or
  rows it skipped, is `doctor_registry_unreadable`; each remembered graph that did not come back is
  `doctor_graph_not_reloaded`, with the reason's code and the `umx graph load` command that loads it;
- the graphs the daemon has loaded: each folder must still exist inside the roots, and `umx check` must find
  nothing in its files. Doctor runs one check naming every loaded graph, so a link into another loaded graph is read
  whatever its visibility and is never a problem; for each graph it reports only a count, and its next is the
  explicit `umx check --graph <graph>`. A graph `umx check` cannot read is `doctor_graph_unchecked`, never clean, and a daemon
  that does not list its graphs is `doctor_graphs_unlisted`. A loaded graph made without std (`graph new --no-std`,
  or made before std) gets the hint `doctor_graph_not_std`, whose next is
  `umx ontology adopt-std --graph <graph> --dry-run`; it changes neither the verdict nor the exit code, and is
  asked only about the graphs `umx graph list` returned. The check reads the files each graph cites again, so a
  changed or missing cited file (`source_changed`, `source_missing`) counts in `doctor_graph_problems` like any
  other finding. Doctor reads at most 256 MiB of cited files per run; a graph whose cited files were not all read
  gets the hint `doctor_graph_sources_unchecked` (not checked, never clean), whose next is
  `umx check --graph <graph>`. With the daemon down, graphs are not checked (the
  daemon reloads the remembered ones when it starts);
- the service (`umx daemon install`): not installed is information; installed but not loaded, loaded with no
  daemon running, a daemon started by hand running instead, or the umx binary it runs gone are each a finding;
- the /umx agent skill in `~/.agents/skills/umx`: it must be the skill this umx was built with, compared by a
  fingerprint of its files (names starting with `.` skipped), offline. Below `~/.agents/skills/umx` itself, doctor
  follows no link and reads only plain folders and regular files; a link, a FIFO or other special file, or more
  than 512 entries, 8 MiB or 8 folder levels counts as a different skill. Doctor looks at the skill's root, lists it
  and reads it through the file adapter: the root is looked at without following it, a folder or file is opened
  without following a link and checked on the open handle, and a FIFO is refused without waiting, so a part swapped
  after doctor saw it is never followed or blocked on. A copy (from `npx skills add`) that differs
  is `doctor_skill_out_of_date` and a link (from `just skill-link`) into a checkout whose skill differs is
  `doctor_skill_link_out_of_date`, both degraded, since a stale skill teaches agents the wrong commands. No skill
  there is `doctor_skill_missing`, information. To install or update it, run the exact line the warning prints:

      npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*' -y   # teammates, binary installs
      just skill-link                                                                       # in a umx checkout, then just dev

  The installer is pinned to skills 1.7.6, and the repo to the full commit this umx was built from, so the install
  is exactly the skill doctor compares with. A build whose skills/umx was not as committed, or that does not know
  its commit, prints the line without `#<commit>`. A build from a local commit that was never pushed prints a commit
  GitHub does not have, and that install fails: use the line without `#<commit>` then. The line carries `-y`, so it
  runs without a prompt: an agent runs it (D76a Q10), and the pins already fix what it installs.

After a binary install (umx copied onto `PATH`, no checkout), `umx doctor` is where the next steps are: the service
row's next is `umx daemon install`, and the skill row prints the pinned install line. Run both. The graph view is
not in a binary install (`umx docs get ui`, In a binary install).

Each finding is a warning with a `code` (all start `doctor_`), a `message` and a `next` to run. `data.rows`
has one row per check with its outcome: `ok`, `info`, `degraded` or `broken`. `data.verdict` is the worst of
them, and the exit code says the same: 0 ok (info included), 5 degraded (umx works, but something needs
attention), 6 broken (something umx needs is unsafe or unusable). Exit 2 still means the command line was
wrong.

## --fix

`--fix` makes only these repairs, each printed on stderr before it runs and listed in `data.fixed`:

- remove a socket that nothing answers on (a daemon was killed);
- tighten a folder or file from the run state list that is open to other users, by clearing its group and
  other bits. It never widens a mode and never touches a path you do not own or a symlink;
- restart the service's daemon through launchd or systemd when it runs another build than this umx. It never
  starts a daemon by hand.

Then every check runs again, so the report shows what is true after the repairs. `--fix` never starts or
stops the daemon, never edits config, never widens the roots and never touches graph files: for those, run the
`next` of each finding.
