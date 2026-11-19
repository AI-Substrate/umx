# Install, run, and where files live

## Install a release

One line installs the latest release on macOS (Apple silicon) and Linux (x86_64 or arm64, glibc 2.35 or newer):

    curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | sh
    curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | sh -s -- --version 0.1.0

On Windows, umx runs only inside WSL 2. One PowerShell line runs the line above in the default WSL 2 distro (it
refuses WSL 1 and changes nothing on Windows); then run `umx` in a WSL terminal (`wsl`):

    irm https://github.com/AI-Substrate/umx/releases/latest/download/install.ps1 | iex

It checks the download against the release's `SHA256SUMS`, puts `umx` in `~/.local/bin` (never with sudo), and
restarts a running daemon on the new build, or starts it. Then it runs `umx doctor` and prints what to run next:
`umx daemon install` (the service) and the pinned /umx skill line (The /umx skill, below). Run them, then
`umx agents-start-here`. To upgrade, run the line again. It refuses other systems with nothing changed: build from
source. Checking provenance (`gh attestation verify`), upgrading when the daemon runs another umx, and
uninstalling: https://github.com/AI-Substrate/umx/blob/main/docs/install.md.

## Install from source

On a machine no release fits, or to work on umx, build it from a clone of its repository. You need rustup (it
fetches the pinned toolchain), `just`, a C and C++ compiler and `cmake` (for the bundled local embedder), and 20 GB
of free disk: the build refuses below that (`UMX_CARGO_MIN_FREE_GB` sets the floor).

    git clone https://github.com/AI-Substrate/umx.git
    cd umx
    just dev

`just dev` builds umx, copies the one binary to `~/.cargo/bin/umx` (`$CARGO_HOME/bin` when that is set; rustup
puts it on `PATH`), and restarts the daemon on the new build: through the service when one is installed, else in
the background with its output in `$UMX_HOME/daemon.log`. It fails unless the new build answers, and ends by
printing `umx version`. Run it again after every pull. There is no separate daemon binary: the daemon is
`umx daemon start`.

## Run the daemon

Every command that touches a graph needs the daemon. Start it by hand in another window (it runs in the
foreground), or install it as a per-user service that starts at login and comes back if it crashes:

    umx daemon start          # by hand; leave it running
    umx daemon install        # or as a service (launchd on macOS, systemd on Linux)
    umx daemon restart        # after an upgrade or config_restart_needed
    umx daemon status         # works with the daemon down: data.running says whether it is up
    umx doctor                # every check, each with the command that fixes it

The daemon, `~/.umx` and its config may be shared with people and other agents: don't start, stop, restart or
install it without the person's yes. More: `umx docs get daemon` and `umx docs get doctor`.

## Where files live

`UMX_HOME` (default `~/.umx`) holds this machine's umx state, owner-only:

- `config.toml`: machine settings, written by `umx config set` (`umx docs get config`); absent until you set one.
- `secrets.env`: optional, keys for embedders (`umx docs get config`, Secrets).
- `registry/graphs.toml`: every graph loaded on this machine, and what reloads after a restart.
- `run/umx.sock`: the daemon's socket. The daemon opens no network port.
- `state/graph-policies.json`: this machine's record of each graph's policy (`umx docs get graph`).
- `vectors/`: vectors of every graph that is not fully open, kept out of the graph folder.
- `daemon.log`: the daemon's output when the service, `just dev` or install.sh started it.

Elsewhere: the binary in `~/.local/bin/umx` (a release) or `~/.cargo/bin/umx` (`just dev`; keep one, or the first
on `PATH` runs); local embedding models in `$XDG_CACHE_HOME/umx/models` when `XDG_CACHE_HOME` is an absolute path,
else `~/.cache/umx/models`; the service file in
`~/Library/LaunchAgents/dev.umx.daemon.plist` (macOS) or `umx.service` in `$XDG_CONFIG_HOME/systemd/user` when
`XDG_CONFIG_HOME` is an absolute path, else `~/.config/systemd/user` (Linux); the /umx skill in
`~/.agents/skills/umx`; a repository's own settings in `<repo>/.umx/config.toml`.

A graph is a folder under your roots (`umx docs get graph`, Where graphs may live), and it is the truth:

- `graph.json`: the graph's id, alias, title, settings and vocabulary.
- `nodes.jsonl` and `edges.jsonl`: one record per line, sorted by id.
- `embeddings/<model@dims>/`: an open graph's vectors, committed with it (`umx docs get embeddings`).
- `.gitignore`: umx adds the line `.umx-tmp-*`.
- `.umx-tmp-*`: a write in progress. Never commit or delete them while the daemon writes.

Commit the folder to git: `umx docs get git`.

## The /umx skill

The /umx skill sends an agent to the right docs for a job. Install it once per machine, for every harness.
`umx doctor`'s skill row says what to run. When the skill is missing (`doctor_skill_missing`) or is an
out-of-date copy (`doctor_skill_out_of_date`), it prints the install line pinned to the commit this umx was
built from:

    npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*' -y
    just skill-link           # instead, in a umx clone: links ~/.agents/skills/umx to it

The line carries `-y`, so it runs without a prompt. With a umx binary that was not built from a clone here, run
`umx doctor` right after installing it: it prints `umx daemon install` (the service row) and this skill line, pinned to
the binary's commit; run both. Such a umx has no graph view: `umx ui` comes only with a source build of the
commit `umx version` reports, not the default branch (`umx docs get ui`, In a binary install).

When the skill is a link (from `just skill-link`) into a clone whose skill differs from this umx's
(`doctor_skill_link_out_of_date`), don't run the npx line: in the clone umx is built from, run
`just skill-link`, then `just dev`.

`just skill-link` refuses a git worktree; run it in the main clone so the skill follows its pulls.
