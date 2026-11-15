# Config: umx's own settings

umx reads its own config, and another tool's only when you ask it to copy from it (`umx config import`). `umx config show` lists every setting, its value and
where the value came from; `umx config check` names anything wrong with a command that fixes it.

    umx config show
    umx config check
    umx config set embedders.nomic.kind local
    umx config set embedder.active nomic
    umx config set embedder.active none --repo      # this repository only
    umx config unset daemon.idle_unload
    umx config unset embedders.nomic                # removes the whole instance
    umx config import --from flowspace3 --dry-run   # what it would copy, writes nothing

## Layers

Lowest first; a later layer wins for each key:

1. default (built in; for `roots`, your home folder and the temp folder)
2. machine: `$UMX_HOME/config.toml` (default `~/.umx/config.toml`)
3. repo: `<repo>/.umx/config.toml`, for the git repository the command runs in (inside the graph roots)
4. env: `UMX_ROOTS` for `roots`
5. flag: `umx daemon start --idle-unload <duration>` for `daemon.idle_unload`

`umx config set` writes the machine file; add `--repo` to write the repo file. The repo layer may only
narrow: it can set `embedder.active` to `none` or to an instance whose kind is `local`, and nothing else. A
repo file you cloned cannot switch a graph to a hosted embedder, point it at another endpoint, or widen the
roots: anything else in it is ignored with `config_repo_ignored`.

## Settings

| key | what it is | applies |
| --- | --- | --- |
| `roots` | Folders graph files may live in, colon-separated on the command line. | daemon restart |
| `daemon.idle_unload` | A loaded graph unused this long unloads itself; `0` means never. Default `30m`. | daemon restart |
| `embedder.active` | The embedder instance graphs use, or `none` (the default). A repo may narrow it per graph (no restart). | daemon restart |
| `ui.dir` | The built graph view `umx ui` serves, an absolute path. Default: apps/ui beside the umx source. `UMX_UI_DIR` overrides it. Machine only. | now |
| `ui.node` | The node `umx ui` runs, an absolute path. Default: the first `node` on `PATH`. Machine only. | now |
| `embedders.<name>.kind` | `azure_openai` or `local`. | daemon restart |
| `embedders.<name>.endpoint` | The Azure OpenAI resource URL, `https://` only. | daemon restart |
| `embedders.<name>.deployment` | The Azure deployment name (not the model name). | daemon restart |
| `embedders.<name>.model` | The model, such as `text-embedding-3-small`. | daemon restart |
| `embedders.<name>.dims` | The width of the vectors the instance makes, 1 to 4096. | daemon restart |
| `embedders.<name>.api_version` | Default `2024-02-01`. | daemon restart |
| `embedders.<name>.auth` | `cli` (the default: signs in through `az login`) or `api_key`. | daemon restart |
| `embedders.<name>.api_key_env` | The NAME of the env var that holds the key, never the key. | daemon restart |
| `embedders.<name>.tokens_per_minute` | How fast background embedding may send to the instance, in estimated tokens a minute. Default 1500000 for `azure_openai`; a `local` instance is unpaced unless set. Machine only, like every instance key. See `umx docs get embeddings`. | daemon restart |
| `embedders.<name>.threads` | `local`: CPU threads per session. Default 4. | daemon restart |
| `embedders.<name>.sessions` | `local`: sessions in the pool, each loading the model once. Default 1. | daemon restart |
| `embedders.<name>.batch_size` | `local`: texts per model run. Default 16. | daemon restart |
| `embedders.<name>.execution` | `local`: `gpu` (the default where umx has a GPU backend: Metal on macOS, CUDA in a cuda build) or `cpu` (about 3 texts a second). | daemon restart |
| `embedders.<name>.search_floor` | A number from -1 to 1: the cosine a vector row needs to take part in `umx search` for this instance's model, replacing the calibrated floor. | daemon restart |
| `embedders.<name>.similar_floor` | A number from -1 to 1: the score a similar_to edge needs for this instance's model, replacing the calibrated floor; a graph's own `embedding.similarity.floor` still wins. | daemon restart |
| `source_roots.<name>` | A folder evidence may cite as `root:<name>/<path>`; machine only, inside `roots`. Prefer `umx root add` (see `umx docs get evidence`). | now |

Instance names are snake_case, at most 32 characters, and never `none`, which means no embedder (its keys are
`config_key_unknown`). Every `embedders.*` key is machine only. An instance needs `kind`, `model` and `dims`;
`azure_openai` also needs `endpoint` and `deployment`; `auth = "api_key"` also needs `api_key_env`.
Every instance here is trusted with the text of graphs whose `embedder` setting is `trusted` (D62; `umx docs get
embeddings`). A `trusted_endpoint` left from before D62 is ignored and warns `config_key_deprecated`.

The daemon builds every instance once when it starts, never during a command, so an `embedders.*` change warns
`config_restart_needed`. An instance that cannot be built does not stop the daemon: `umx daemon status` and
`umx embeddings probe` say why. `auth = "cli"` signs in through `az login` (then `azd`), found on `PATH` when the
daemon starts. See `umx docs get embeddings`.

The two floors are kept per model (`model@dims`): when two instances of one model set one, the first by name is
used. Results then say `floor_source: config` and `floor_calibrated: false`. Leave them unset to use the calibrated
floors (`umx docs get search`). Tell the owner of any graph on that model before changing one.

Vectors are keyed by `model@dims`, never by instance name or deployment: two instances with the same model and
dims share vectors, and renaming an instance re-embeds nothing.

A setting that applies at daemon restart warns `config_restart_needed`; run `umx daemon restart`.

## Copying an embedder from flowspace3

`umx config import --from flowspace3` copies the embedder flowspace3 uses into the machine layer as one instance,
so you do not retype it. umx copies config and never shares it: later changes on either side do not follow.

    umx config import --from flowspace3 --dry-run
    umx config import --from flowspace3
    umx config import --from flowspace3 --as azure --model text-embedding-3-large --dims 256 --force

- It reads only flowspace3's `config.toml` (in `FS3_CONFIG_DIR` when the daemon started, else
  `~/.config/flowspace3`), with the same checks as umx's own config file. It never opens flowspace3's
  `secrets.env` and copies no key, token or key name.
- It takes the provider flowspace3's `[embedder] active` names, which must be `azure_openai`, and sets exactly
  `kind`, `endpoint`, `deployment`, `model`, `dims`, `api_version` (when flowspace3 sets one) and `auth = "cli"`.
  If flowspace3 signs in with a key, umx still signs in with `az login`, and `auth_note` says so.
- The instance is named after the provider in snake_case (`azure-embed` becomes `azure_embed`); `--as <name>`
  names it yourself. `model` comes from flowspace3's model key, else from a deployment name that starts with a
  known model (`text-embedding-3-small-01` gives `text-embedding-3-small`); otherwise it is refused
  (`config_import_refused`) and `--model <model>` gives it. `dims` comes from `--dims <n>`, else flowspace3's
  `dimensions`, else the model's full width.
- The reply's `key` is the vector space the instance makes (`model@dims`). It warns `import_key_unmatched` when no
  loaded graph records that key (the instance would then embed only graphs with no recorded model) and
  `similarity_floor_uncalibrated` when that key has no calibrated similarity floor (only embeddinggemma-2@256+p1,
  text-embedding-3-large@256 and text-embedding-3-small@256 are calibrated; see `umx docs get search`).
- Every command it suggests repeats what you asked: the instance (as `--as <name>`), `--model`, `--dims` and
  `--dry-run`, so following it changes only that instance, and a dry run's fix is still a dry run.
- `--dry-run` prints the keys and values it would set (`set`) and writes nothing. An instance the machine config
  already defines is left as it is (`config_import_exists`) unless you pass `--force`, which replaces all its keys.
- It does not change `embedder.active`. The instance it writes is in the machine config, so it is trusted: a graph
  whose embedder setting is `trusted` may send its text to it once the daemon uses it (`umx docs get embeddings`);
  a graph that must stay local never does. Probe the new instance with `umx embeddings probe
  --embedder <name>` (no restart needed), then set `embedder.active` and run `umx daemon restart` to use it. Tell
  each graph's owner before you switch the embedder its graphs use.

## Secrets

Secrets never go in a config file. `umx config set` refuses a value that looks like a key or token. An
instance with `auth = "api_key"` names an env var in `api_key_env`; umx reads its value from the environment
first, then from `$UMX_HOME/secrets.env`:

    AZURE_OPENAI_API_KEY=the-key-itself

That file holds plain `NAME=value` lines (`#` comments and blank lines allowed). There is no `export`, no
quoting, no `$VAR` expansion and no multi-line value; another line is ignored with `secrets_line_invalid`,
which names its line number, never its content. umx ignores the file unless it is a plain file you own that
nobody else can read or write (`chmod 600`). `umx config show` prints a secret only as `set (env)`,
`set (secrets.env)` or `unset`.

## The file

umx writes `config.toml` as flat TOML, one `key = value` per line in a fixed order, owner-only (0600), so the
same settings always give the same bytes. You may edit it by hand, and `[embedders.nomic]` tables read the same
as dotted keys; the next `set` rewrites it flat and drops comments. umx refuses a config file that is a
symlink, not owned by you, writable by group or others, or over 64 KiB (`config_file_refused`; a repo's file
arrives by git pull, so umx never reads a big one whole), and will not rewrite one that does not parse
(`config_file_invalid`, naming the line and column). The secrets file and the graph-policies state file have
the same 64 KiB limit.

## When the config is invalid

An invalid `roots` value, or a machine `config.toml` that is refused or does not parse, closes every graph
folder rather than falling back to the wider default. The daemon still starts, with `config_invalid` in its
ready output and `"config": "invalid"` in `umx daemon status`; graph commands fail with `config_invalid`
until the config is fixed and the daemon restarts. `umx config check` shows what is wrong. umx never
suggests a roots value: which folders it may use is the human's choice.

    umx config check
    umx config set roots <folders>      # folders the human chose, colon-separated
    umx daemon restart

A rewrite never drops or repairs a setting it was not asked to change: `umx config set` on another key keeps an
invalid `roots` exactly as written, so the folders stay closed until roots itself is fixed.
