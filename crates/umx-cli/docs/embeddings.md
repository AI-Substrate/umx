# Embeddings

When the daemon has an embedder, every node gets vectors: one per text field and one for the whole node. They
live in the graph folder, next to the graph files, so they commit and merge like them:

    embeddings/<model@dims>/<shard>.tsv

One line per vector: node id, field, the text's hash, the vector as base64 int8. Lines are sorted by node and
field, and a node's vectors are all in one shard (the last two characters of its id, in lower case). Every
vector of a model sits in its own folder, keyed by the model and its width (for example
`text-embedding-3-small@256`), never by a deployment or endpoint, so teammates on different Azure resources
share them.

What gets a vector: the title, the note, and each content field: a field the node's type declares as a string
and states content (`umx ontology add field <type> <name>:string --role content`), when not blank. A field with no
stated role is data (PRD 002 search-04), and data fields (labels, names, file names, ids, agent ids, seats,
hashes, locators, provenance) are never embedded (see `umx docs get ontology`). Numbers, dates, booleans and lists
are data and never embedded. Provenance fields and
nodes are never embedded, whatever graph.json says. The whole-node vector (`@node.v2`) embeds
`title: <title> | text: <the other content fields as name: value lines>`; a field vector embeds
`title: none | text: <value>`. Word search and `--where` find data fields of kind string as before.

Text form 2 (D56) left data fields out; form 1 had no roles. A graph made before keeps every vector whose text did
not change: a `@node.v1` line whose text is the same is still the node's vector, and takes the `@node.v2` name the
next time its shard is written, with no embed call. A node whose whole-node vector (of any version) was made from
text that included a field now data, as after a pull that marks a field data, or in a graph made while a string
field with no role was content, keeps that vector (it is still searched, and so are the field vectors of those
fields) and counts as pending: its load warns `text_form_outdated` with how many, and nothing remakes or clears it
until `umx embeddings reembed` (try `--dry-run` first for the cost). The graph records `text_form: 2` in graph.json
at reembed, or at its next graph.json write once no node is outdated; never at load. Changing a field's role
(`umx ontology set field`) remakes the nodes it changes through the background worker, with a `role_changed`
warning naming how many. A text planned before the change and not yet sent is dropped, so a field made data is not
sent after the change returns.

`umx embeddings status` also points out, for each open graph, a string field whose values say it is in the wrong
group: the hints `field_looks_like_labels` (labels declared content: how many embedder calls data would save) and
`field_looks_like_prose` (prose declared data: search by meaning never sees it), each naming the
`umx ontology set field <type> <field> --role data|content` that moves it (`umx docs get ontology`).

## Secured graphs

A graph whose `graph.json` says `"policy": "secured"` never sends text to an embedder that is not local: nothing of
it is queued, planned or embedded, query embeds included, and its load warns `embedder_not_local_for_secured`.
With a local embedder its vectors go to a cache on this machine, `$UMX_HOME/vectors/<graph id>/<folder key>/`,
never into the graph folder, so they are never committed. The cache is private (0700, yours); if it is not, no
secured vector is written anywhere.

## Trusted instances

Every embedder instance defined in the machine config is trusted (D62). A graph whose `embedder` setting is `trusted`
(`umx docs get graph`) may embed through any of them, local or hosted, with nothing more to confirm. So putting a
hosted instance in the machine config sends the text of graphs set to `trusted` (and `any`) to its endpoint. Only the
machine config can define an instance or change its endpoint: a repo's `.umx/config.toml`, an env var, a flag or a
graph file never can. `embedder` `local` (and `secured`) never uses a hosted instance.

When `umx config set`, `unset` or `import` adds, removes, repoints or partly unsets a hosted instance (its kind,
endpoint, deployment, model or dims) and that moves where loaded graphs that are not open send their text (by the
same instance choice and folder rules umx uses to embed, graphs that record no model included), the reply warns
`embedder_route_changed` with how many. It is a heads-up, not a gate: the move happens at the next daemon restart. Doctor and `umx daemon start`
name each hosted instance with its host. The secret hold below applies to every hosted instance.

A trusted graph's vectors go to this machine's cache, like a secured graph's, so a copy shared through git carries
none. Each machine embeds it again through its own instance. A `trusted_endpoint` key left from before D62 is
ignored with `config_key_deprecated`; remove it with `umx config unset`.

## Vectors left from when a graph was open

A graph that was open and is now named, trusted or local (secured included) may still hold, in its own folder,
the `embeddings/` and `similar/` files written while it was open. `graph load` and `graph set` warn
`secured_shards_remain` with the count, and list the paths in `candidates`. umx does not check git: if the folder is
in a repository, git tracks those files (`git ls-files` says). umx no longer reads or writes them. The graph's vectors
now live in this machine's cache, and the policy change queues every node to be embedded there again, under the
graph's own embedder rule, so nothing needs copying and umx never copies them. `umx embeddings status` shows the
cache refilling.

Removing them is a person's job:

- If git tracks the folder: `git rm` the listed paths (not `git rm --cached`, which leaves them on disk), commit
  and push. If it does not: just delete them. The warning stops once the folder holds none: `umx graph unload` and
  `umx graph load` the graph to see.
- If they were committed, the old files stay in git history and in every clone. If the graph's text must not be
  recoverable from its vectors, that takes a history rewrite (`git filter-repo` on those paths), a force push and
  every clone made again: the repository owner's decision, never routine. Vectors are not text, but they allow similarity and
  membership inference, and the `similar_to` lists show relationships directly.

An agent that sees this warning reports it and the listed paths to the person and stops. It may unload and load the
graph, and run `umx embeddings status`, to show the warning's state and the cache refilling. Only a person deletes
the files, commits or rewrites history: an agent never does, never force-pushes, and never sets the graph back to
open to clear the warning.

`secured_shards_unchecked` means umx could not list the folder (a folder it cannot read), so whether any such files
remain is unknown: fix the folder's permissions and load the graph again.

## Secrets in text

Every text umx would embed (the title, the note, each string field) is scanned on `node add`, `node edit`,
`node note` and `umx batch` for likely credentials: cloud and API keys (AWS, GitHub, Slack, OpenAI-style, Google,
Stripe), a bare 40-character AWS secret key near the word "aws" or "secret", private key blocks, JWTs, bearer tokens, passwords in URLs or connection strings, a secret-named key
assigned a value (`api_key: ...`, `AZURE_OPENAI_API_KEY=...`), and long random-looking tokens. Ids and digests
(hex of 40 digits or fewer, or exactly 64) and placeholders (`<password>`, `${TOKEN}`, `***`, `changeme`) are not
flagged. Neither are file names and paths made of words, dates and versions joined by `-` or `_`
(`RECAP-GLOBEX-roadmaps-2026-03-14.md`): a random-looking part still counts, even next to an extension or with a
leading or trailing `-` or `_`, and a named key (`sk-...`, `ghp_...`, `AKIA...`) inside a path is still caught.
Nor are long camelCase or PascalCase names with no separator, such as a column name
(`totalOutstandingInvoiceAmountExcludingGST2024`): 3 or more words, each a capital and lowercase letters, an
acronym of 2 to 5 capitals (at most 2 of them) or up to 4 digits, averaging 4 or more characters. Random-case runs
(`HIstory`), an acronym of 6, a run of 5 digits or an acronym straight before a capitalised word (`UTCFor`) still
count; allow such a text if it is safe to send.

A hit never refuses the write. It warns `secret_suspected`, naming the node, the field and the rule (never the
text), and that field is held from every embedder that is not local: neither its own vector nor the whole-node
vector (which contains it) is sent, so the node counts in `vectors_held` and drops out of whole-node similarity
(`umx search`'s vector leg and `similar_to`) until the field is redacted or allowed. A local embedder embeds it as usual. The hold is
worked out from the text each time, right before anything is sent, so it holds across restarts, pulls and hand
edits. A search whose query looks like a secret is not sent to a hosted embedder either (`similarity_unavailable`).

To release it, redact the text (`umx node edit <node> --note "..."`), or, when it is not a secret or may go to
your provider, allow that exact text:

    umx embeddings allow --node <node> --field note

The allow covers the field's current text only: any change holds it again. It is kept on this machine only, in
`$UMX_HOME/allows/secret-allows.json` (owner-only, the text's hash, never the text), for at most 256 texts (and 60 KiB);
another machine holds it again. If that file cannot be read every flagged text stays held, and a new allow is
refused (`secret_allow_refused`) rather than overwriting it.

## Embedding happens after the edit

A write (`node add`, `node edit`, `node note`) returns as soon as the node is in `nodes.jsonl`. Its vectors are
made afterwards, in the background, and written to the shard as a separate change. A field whose text did not
change is not embedded again: a vector is current when its line's hash matches the text, so re-saving a node,
or changing a field that is not embedded, embeds nothing.

Loading a graph reads its shards once. A graph whose vectors are all current (a teammate's commit) embeds
nothing. A vector whose text has changed since (stale) stays in the file, still searchable, until its fresh one
is written. Background embedding waits when the provider says it is busy (429); it never drops a node for that.

Background embedding runs several provider calls at once, up to each instance's concurrency ceiling less one
slot kept for searches: 31 for `azure_openai`, and for `local` its `sessions` less one, at least one (one session:
one call at a time, as before). A search's query embed takes that kept slot, so it never waits behind background
calls. Each instance is paced by `embedders.<name>.tokens_per_minute` (default 1500000 for `azure_openai`, which
shares its deployment's quota with other tools; `local` is unpaced unless set). A 429 from an instance pauses only
that instance for the provider's `Retry-After` and halves its rate once, however many calls come back 429 at
once (never below an eighth of the setting); each quiet minute wins back an eighth, never above the setting.
Two instances on one Azure resource are paced apart, so together they can pass its cap: give them settings that
add up to less. Vectors are still written between client writes, one shard file at a time; while many calls run,
the writer's file writes, not the provider, set how fast a backlog drains.

## umx embeddings repair

Tidies the selected graph's shards and queues every node that needs embedding.

    umx embeddings repair --graph demo

- Two lines for one node and field with the same text (both branches of a merge embedded it) are equally
  valid: repair keeps the first in sort order and drops the other, without embedding.
- Two lines with different texts (both branches edited the field), a missing vector or a stale one: the node
  is queued and embedded again.
- Lines for fields a node no longer has are dropped.

It reports `queued`, `tidied`, `shards_written`, `orphans` and `kept_lines`. With no embedder configured it
fails with `embedder_not_configured`.

## Pending vectors

`umx graph list` and `graph load` show `vectors_pending`: nodes whose vectors in the graph's own model (the key in
graph.json, else the configured embedder's) are missing or stale. With an embedder for that model it falls to 0 as
the background work lands. A graph recorded with another model (a teammate's, pulled) is never embedded with this
machine's model: its new and edited nodes stay pending, and `graph load` warns `embedder_mismatch` naming both
models. `null` means the graph has no model yet and no embedder is configured.

`vectors_held` counts the nodes whose missing vectors wait only for the secret hold (see Secrets in text): they are not in
`vectors_pending`, so pending still reaches 0. They stay held until the text is redacted or allowed.

## umx embeddings status

One row per loaded graph: what it has embedded and with what. It sends nothing and reads no credential.

    umx embeddings status

Each row has `graph`, `key` (the model graph.json records), `policy`, `visibility` and `embedder_setting` (the
graph's two settings as umx acts on them), `nodes`, `vectors_pending`, `vectors_held`,
`vectors_done` (nodes with every vector current), `embedder` and `locality` (the instance that embeds it now, or
`null` when none may: no embedder, its folder's config, a secured graph with only hosted instances, or no instance
makes its recorded model), `state` and `check`, the
`umx embeddings probe --graph <alias>` that tests whether that embedder answers. `umx graph list` shows the same
counts beside each graph's other details.

`state` is `no_embedder` (no instance serves it), `unknown` (its shards could not be read when it loaded, so the
counts are `null`: `graph load` warned `vectors_unreadable`; run `umx embeddings repair --graph <alias>`),
`embedding` (some vectors are pending), `held` (none pending, but the secret hold keeps some from a hosted
instance) or `current` (every vector is current).

`vectors_digest` (64 hex characters) changes whenever the graph's stored vectors change: a node embedded, a node
removed with its vector, a reembed into another model, or an edited node's new vector once it lands (not at the edit:
until then the stored vector is the old one). It stays the same through writes that change no vector and across a
daemon restart (it comes from the vector lines on disk). A client that caches what it read from a graph's vectors,
such as `umx similarity pairs` rows, reads it again when the digest changes, even when `nodes` and `vectors_done`
did not. It is made in the model the graph's vectors are in (graph.json's `key`, else its embedder's, as
`similarity pairs` picks), so it moves when that changes too. It is `null` for a graph with no model yet, and for a
graph that is not open (status takes no `--graph`, so a secured graph is never named here with it).
`umx similarity pairs` gives the same digest for each graph it compared, in its `compared` list, taken with the rows
it read, so a cache can key the pairs by those digests and need not trust a status read made before or after.

It is a cache key, never proof that two sets of vectors match: it is made from the model key and each vector's node id
and text hash (the same lines as `similar/<model@dims>/_meta.tsv`'s hash), not from the vector values. So a provider
swapping the model behind the same key, or the same text giving slightly different numbers, leaves it unchanged.

Before you switch `embedder.active`, compare: the probe's `key` must equal the graph's `key` here, or the graph will
not use that instance (it waits, pending, until it is reembedded).

## umx embeddings reembed

Makes the selected graph's vectors with the configured embedder's model. It runs only when you run it, and shows
the cost first:

    umx embeddings reembed --graph demo --dry-run
    umx embeddings reembed --graph demo
    umx embeddings reembed --graph demo --model text-embedding-3-small@256

- `--dry-run` reports `nodes`, `texts` and `estimated_tokens` still to embed in the configured model, `from`
  (the model graph.json records) and `to` (the embedder's), and `locality` (`local` or `hosted`). Of the texts,
  `whole_node_texts_changed` are whole-node texts that replace a vector made from other text (as for a node that
  held a field now data); `field_vectors_dropped` counts the field vectors the reembed removes because their node
  no longer embeds the field (a field now data), which costs nothing. It calls no embedder and changes nothing.
- Without it, graph.json records the embedder's model, and every node that needs it is queued for the background
  worker, which paces itself as for any edit. `queued` says how many. `vectors_pending` falls to 0 as they land.
- Vectors of the old model stay in their own `embeddings/<model@dims>/` folder. They are never mixed in, and
  similarity uses only the graph's model. Switching back costs nothing to embed. The folder stays until removed
  by hand.
- The cost is worked out without holding up writes. Only recording the key and queueing use the writer.
- `--model <model@dims>` must be the configured embedder's model (`reembed_model_not_configured` otherwise; nothing
  changes).
- A secured graph with an embedder that is not local is refused (`secured_embed_refused`), dry run included; nothing
  is sent. With no embedder it fails with `embedder_not_configured`.

## Warnings

- `vector_orphan`: a line names a node that is not in the graph (a merge where one branch deleted the node).
  The line is kept and written back; it names only the shard, the line number and the node id.
- `vector_line_kept`: a line umx cannot read (a hand edit, a merge marker). It is kept and written back.

- `vector_wrong_width`: a vector whose width is not its model's dims. It is never used, and its node is embedded
  again.
- `vectors_unreadable`: the graph loaded but its shards could not be read (for example a folder where a shard
  file should be). Nothing was queued from them.

A text longer than the embedder takes is cut before it is sent; one the embedder still refuses is left out, and
the rest of its batch is embedded. The daemon's log (stderr) names the graph, node and field of each, and of any
node whose embedding failed, never the text.

Temp files during a shard write start with `.umx-tmp-`, which the graph's `.gitignore` already ignores.

## Choosing the embedder

Embedder instances are named in the machine config (`umx docs get config`). The daemon keeps every instance it
built, and each graph embeds with the one whose model and dims match the key the graph records, if its policy allows
it (several match: `embedder.active` first, then by name). `embedder.active` is the default: it serves graphs that
record no model and is `umx embeddings reembed`'s target; `none` (the default) means no default. The daemon builds every instance once when it starts, never during a command, and
wires the active one, so after changing `embedder.active` or any `embedders.*` key run `umx daemon restart`. An instance that cannot be built does not stop the daemon; `umx daemon status` and the probe
say why.

A graph's embedder also answers to the config of the graph's own folder: a repository's `.umx/config.toml` may narrow
it to `none` or a local instance, never widen it. A repository config umx refuses or cannot parse, or one whose
`embedder.active` umx ignores (an instance this machine does not define, or a hosted one), counts as `none` until it
is fixed. A secured graph only ever uses an instance whose adapter keeps
text on this machine; a hosted one is refused, whatever config, env or flag says.

## Signing in

`azure_openai` instances sign in with `az login` by default (`auth = "cli"`): umx runs `az` (then `azd`), found on
`PATH` when the daemon starts, to get a token on first use, and keeps it until shortly before it expires. With
`auth = "api_key"`, umx reads the key from the env var named in `api_key_env`, or from `$UMX_HOME/secrets.env`,
once when the daemon starts. No token or key is ever printed. `umx daemon status` shows under `embedder` the
active instance, the credential that last worked (`az` or `azd` with its path, or `api_key` with the env var
name) and where `az` and `azd` were found. Each instance there has a `background` row: its `slots`, calls
`in_flight` now, `calls` so far, `tokens_counted` (what the provider said it counted), `rate_limited` (429s),
`tokens_per_minute` (the setting) and `rate` (now, lower after a 429), and `last_halved_s_ago`.

## umx embeddings probe

Checks that the embedder can run without sending any text (for Azure: it gets a token, or checks the key was
found), then embeds one fixed sentence that umx owns, never graph text.

    umx embeddings probe
    umx embeddings probe --embedder azure_small
    umx embeddings probe --graph demo

`--embedder <name>` tries another instance. An instance added to the config, or changed, after the daemon started is
built from the config as it is now for this probe only and then dropped: the probe says whether it works and warns
`embedder_not_in_use`, because the daemon keeps using what it started with until `umx daemon restart`. The same
folder rule and the graph's embedder setting apply to it, as to any instance. `--graph <alias>` without `--embedder` always probes the instance
the running daemon uses for that graph, as it was built at start, because that is what embeds its text; after
changing that instance's config, run `umx embeddings probe --embedder <name>` to test the new settings. `--graph <alias>` checks the instance that graph would
use (its folder's repo config and its effective policy, the stricter of graph.json and this machine's record, apply) and warns `embedder_mismatch` when the graph records
another model key. It prints `embedder`, `key`, `model`, `dims`, `norm` (about 1), `latency_ms`, `locality`
(`local` or `hosted`) and `credential` (`null` for local embedders). It runs on its own, never on the daemon's
writer, so writes keep going while a slow provider answers.

- `embedder_not_configured`: no instance is chosen. Set one with `umx config set embedder.active <name>`.
- `embedder_unavailable`: no credentials (no `az login`, a rejected key), the provider cannot be reached or is rate
  limiting, or a secured graph met a hosted instance. The message says what to do; `next` re-runs the probe.
- `embedder_invalid`: the instance's config is incomplete or wrong, or it is a kind this umx cannot run yet.
  Run `umx config check`.

## similar_to edges

Every graph with vectors has `similar_to` edges of its own: from each node to its top matches by whole-node vector,
with a `score` (a cosine, 4 decimal places). They are kept current as vectors arrive, change or go, and saved beside
the embeddings, so a clone loads them without computing anything:

    similar/<model@dims>/<shard>.tsv     # node id, other id, score; sorted by (node, other)
    similar/<model@dims>/_meta.tsv       # what the lists were computed from

- `k` (the most edges per node, 10) and the score floor (an edge is kept at or above it) come from `graph.json`'s
  `embedding.similarity`, for example `{"k": 10, "floor": 0.6}`. With no floor there, the machine config's
  `embedders.<name>.similar_floor` is used, else the model's calibrated similar_to floor, else none (the model is not
  calibrated: only k limits, and loads warn `floor_uncalibrated`). See "Floors" in `umx docs get search`. A value out
  of range warns `similarity_setting_invalid`, uses the default, and stays in the file as written. Changing the floor
  makes the stored lists stale, so they are rebuilt.
- `a similar_to b` means b is in a's top k. It is not symmetric. A node never lists itself. A node with no
  whole-node vector yet has no list and is in none.
- Scores are the same on every machine (exact int8 dot products, rounded half to even), and a tie at the k-th
  place goes to the lower id.
- Stored edges only ever join nodes of one graph. Similarity across graphs is computed when a query needs it and
  never written.
- A secured graph keeps its `similar/` files in the same local cache as its vectors, never in the graph folder.
- Read them by naming the relation: `umx neighbours <node> --relation similar_to` (`out`: the node's list, best
  first; `in`: who lists it). Rows have a `score`, `derived: true` and no edge id; walks and paths along
  similar_to go at most 2 steps. An unnamed step never follows them. `similar_to` is reserved: `edge add` and
  `ontology add relation` refuse it.

`_meta.tsv` records the model, k, floor, whole-node text version, a hash of the vectors' texts and how many edges
the lists hold. When a load finds it does not match (a merge joined two branches' lists, the settings changed, a
shard is missing or a line is damaged), the lists
are rebuilt in the background, and until then the load and similar_to results warn `similar_to_incomplete`.
Never edit these files by hand: rebuild them.

While vectors arrive in the background, the lists in memory move with each one, so similar_to answers are current,
but the files lag: they are written once the daemon's whole backlog is quiet (all graphs), and on `daemon stop`,
`daemon restart` and `graph unload`. While a long backlog keeps going they are written a file at a time from about a
minute on, with `_meta.tsv` removed until the last one lands. Committing `similar/` mid-drain is safe: lagging files
are never served, a load rebuilds them. `umx graph unload <alias>` writes everything the graph owes, for a complete
commit mid-drain (`similarity rebuild` may refuse with `vectors_kept_changing` then). After a crash mid-drain the next
load warns `similar_to_incomplete` and lists are empty until the rebuild lands; empty then does not mean nothing is
similar.
Writes never wait on the backlog: vectors are written between client writes, one shard file at a time.

## umx similarity rebuild

Recomputes every similar_to list of the selected graph from its committed vectors and writes `similar/`. The files
then equal what the lists kept current would have written, byte for byte, so this is also the repair for a bad
merge. Writes are never held up while it computes.

    umx similarity rebuild --graph demo

It reports `model`, `k`, `floor`, `floor_calibrated`, `floor_source` (`calibrated`, `config`, `graph` or `none`),
`floor_run` (the calibration run, for a calibrated floor), `edges` and `files_written`
(0 when the files already matched).

## umx similarity pairs

Each node's top k among the nodes of the other graphs it reads: links by meaning between graphs, which
similar_to never stores (it stays inside one graph). Computed on every call from the graphs' whole-node vectors and
never written, so it sends nothing and changes nothing. Writes never wait on it.

    umx similarity pairs --graph alpha,beta
    umx similarity pairs --graph alpha,beta,gamma --k 5 --min 0.6000 --limit 200
    umx similarity pairs --all
    umx similarity pairs --scope work --same-graph

- Graphs: `--graph a,b` or `--scope <name>` names them, whatever their policy; with neither, the loaded open graphs
  (secured ones are counted in `secured_graphs_skipped`, never named), as `umx search`; `--all` the same, and
  `--all --include-named` every loaded graph. One copy of a graph unless copies are named, and a node never pairs
  with its own copy.
- One model: only graphs whose vectors are in the same model are compared (a graph's model is the one graph.json
  records, else its embedder's). `--model <model@dims>` picks it; otherwise the first named graph's, or, unnamed
  or with `--all`, the model most graphs are in (ties to the lowest alias). `model_source` says which (`flag`, `named-first`, `most-graphs`). A graph none of whose
  nodes has a vector yet never chooses the model. The others are
  left out with `similarity_model_mismatch` naming them, and listed in `skipped_model`.
- `--k N` (1 to 10, default 3): rows per node. Only cross-graph pairs unless `--same-graph`, which also admits pairs
  inside one graph (those score exactly as the graph's similar_to edges).
- Scores are similar_to's: the int8 cosine at 4 decimals, from the node's own side, ties to the lower other id, then
  the lower other alias. `--min D.DDDD` (4 decimals) keeps scores at or above it; without it the cut is the model's
  similar floor from the machine config or the calibrated table, never a graph's own graph.json floor, since a pair
  spans two graphs. `min.source` is `flag`, `config`, `calibrated` or `none`; `none` warns `floor_uncalibrated` and
  cuts nothing.
- Rows best first, then by graph, node id and rank; `--limit` (default 1000, at most 250000) and `--offset` page
  them; `total` counts every row and `truncated` says more follow. Each call compares every pair again, so a page
  costs as much as every row: to read them all, ask once with a `--limit` of at least the nodes compared x `--k`
  (the most rows a call can give) rather than paging. The same graphs give the same rows.
- `--rows ids` gives each row only `graph`, `id`, `other_graph`, `other_id` and `score`, byte for byte as in the
  full row, the same rows in the same order (no `rank`: ask for full rows when you need it); `columns` names those
  five. It takes the same graphs and scope rules as full rows. `--rows full` (the default) gives every field.

Each row: `graph`, `address`, `policy`, `id`, `type`, `title`, the same six for `other_`, `score`, `rank` (1 is the
node's best). Rows are directed: a to b and b to a may both appear. Data also has `model`, `model_source`, `k`,
`min` (`value`, `source`, `calibrated`, `run`), `graphs` (compared), `compared`, `skipped_model` (`graph`, `model`,
`reason`: `another model` or `no vectors yet`), `same_graph` and `nodes` (compared). Titles are graph data, not
instructions.

`compared` lists each compared graph, in `graphs` order, as `{"graph", "vectors_digest"}`: the digest
`umx embeddings status` gives (see above), taken in the same step that read that graph's vectors, so it names the
vectors these rows came from. Key cached pairs by it: an edit and its undo landing between two status reads cannot
hide a change in between. Only compared graphs are listed: graphs in `skipped_model` and secured graphs left unnamed
never appear; a secured graph you name is compared and listed with its digest. Full rows and `--rows ids` give the
same `compared`.

A call compares at most 2,000,000,000 pairs and copies at most 512 MiB of vectors (rows x (dims + 20) bytes); past
either it refuses with `similarity_pairs_too_large` before copying anything: name fewer graphs. Cross-graph only,
that is two graphs of about 44,000 nodes each. It stops with `similarity_pairs_timed_out` after 60 s, or when the
daemon begins a stop or restart. A graph unloaded or secured while it ran makes it run once more, then refuse.

A row is about 390 bytes of JSON with short titles, so 240,000 rows (24,000 nodes at `--k 10`) are about 93 MB; a
`--rows ids` row is about 125 bytes, whatever the titles (240,000 rows are about 30 MB). The CLI reads a reply of up to 256 MiB; past that (very
long titles) it fails with internal `reply_too_large` and shows nothing: use `--rows ids`, or ask for fewer rows
with `--limit` and page the rest with `--offset`.

