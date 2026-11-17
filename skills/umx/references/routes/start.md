# /umx start: get umx running and learn it

Job: umx is not running, or you are new to it.

1. Check the binary: `umx version` (**usage 101**: `umx --version` and `umx -V` are the same command). Not found: from a checkout of the umx repo, `just dev` builds it,
   installs it into `~/.cargo/bin` and restarts the daemon. Still failing: `/umx doctor`.
2. Set JSON output for agent work (`/umx output`, usage 001).
3. **usage 002**: start the daemon yourself; nothing auto-starts. Check with `umx daemon status`; if
   `running` is false, run `umx daemon start` in another window (or a background job you keep) and leave
   it running. `umx version`, `umx agents-start-here`, `umx docs list`, `umx docs get` and `umx daemon status` work
   without it. Never read or edit graph files directly to work around a stopped daemon. **usage 025**: if a
   command fails with `protocol_mismatch`, `writer_stopped` or `graph_format_newer`, don't retry; restart the
   daemon (`/umx doctor`). **usage 033**: if `umx daemon start` refuses with `socket_unavailable` because the
   run directory belongs to another user, remove it or set `UMX_HOME` to a folder you own; if it is open to
   others, `chmod 700` it. Never chmod or chown another user's folder.
   **usage 056**: if `umx daemon status` shows `service.installed: true`, the
   human runs umx as a service: `umx daemon install` starts it again, and never `umx daemon start` by hand.
   Install or uninstall the service only when the human asks (`umx daemon install`, `umx daemon uninstall`).
   **usage 109**: after any restart (`umx daemon restart`, `just dev`, the service) loaded graphs come back by
   themselves, and a named graph that unloaded when idle loads on demand (`graph_loaded_on_demand`). Don't reload
   by hand. A graph missing after a restart: `umx doctor` (`doctor_graph_not_reloaded` gives the load command);
   `umx graph unload <alias>` forgets it. A graph you unloaded stays unloaded until `umx graph load`.
4. **usage 031**: graph folders live under the daemon's roots, `$HOME` and `$TMPDIR` by default (on macOS
   `/tmp` is neither): `umx graph new ~/graphs/acme --alias acme`. `umx config show` prints the roots and
   where they came from. Don't ask to widen them (`umx config set roots`, `UMX_ROOTS`) unless the human
   asked for it. Graph commands failing with `config_invalid`: `/umx doctor`.
5. Read the start page once: `umx agents-start-here`. **usage 063**: unsure what a noun takes? Try
   `umx <noun> --help` (offline; the same text as `umx docs get <noun>`), or `umx --help` for every noun.
   For anything else, list the topics and read the one you need (§ C4):

       umx docs list
       umx docs get output

6. **usage 086**: only when the human wants local embedding (secured graphs): fetch the model once with
   `umx models fetch embeddinggemma-2` (`--dry-run` first; 558 MB; no daemon needed). Embedding never downloads.
   **usage 085**: with no GPU it runs on the CPU at about 3 texts a second from an F32 copy umx makes beside the file
   (1.1 GB, once). `doctor_local_embedder_cpu` is a note; `doctor_local_embedder_slow` means under 2 texts a second:
   raise `embedders.<name>.threads`.
   **usage 102**: on Linux with an NVIDIA GPU, a umx built with `--features cuda` embeds on CUDA (doctor shows
   `(cuda)`); with no device it says `cpu: this cuda build found no CUDA device` and runs on the CPU.

Then hand over: making or changing a graph is `/umx build`; turning a source into nodes is `/umx ingest`;
asking a graph a question is `/umx explore`.

**usage 207** (supersedes usage 142): after a binary install (no umx checkout), run `umx doctor` and run what it
prints: `umx daemon install` (`doctor_service_none`) and the exact pinned skill install line
(`npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*' -y`; `-y` so it runs without a prompt).
In a umx repo's main checkout, `just skill-link` instead (it also links the Claude, Codex and Pi skills folders).
`umx doctor`'s skill row says when the installed skill is not the one this umx was built with, and names the command
to update it. The graph view (`umx ui`) ships only with a source build: in a binary install it refuses with
`ui_dir_refused`; clone the repo, check out the commit `umx version` reports (not the default branch: the view
and daemon must speak the same protocol), build its `apps/ui` (`umx docs get ui`), then set `UMX_UI_DIR` or
`ui.dir`.
