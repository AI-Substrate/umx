# Evidence

Evidence says where a node's claim came from. Add it with `umx node note <node> --append <text> --evidence
<pointer>`, or when you create the node with `umx node add ... --evidence <pointer>` (repeat `--evidence` for
several). Each pointer is a source, `#`, and a selector:

    calls/3-nov.vtt#seq=120-140                 # caption cues 120 to 140
    docs/brief.md#L10-L24                       # lines 10 to 24
    docs/brief.md#L7                            # one line
    https://example.com/memo#quote=we will slip&prefix=said&suffix=today   # quote it in a shell
    docs/brief.md#section=Scope/Risks           # the Risks heading under Scope
    docs/brief.pdf#page=3-4                     # pages 3 and 4
    calls/3-nov.ndjson#seq=120-140&quote=we%20ship   # numbers, plus a quote that survives drift
    umx:launch/01JC8XA4H7Q1V9K2M3N4P5R6S7#quote=approval

    umx node note 01JC8XA4H7 --append "Raised in the call" --evidence 'calls/3-nov.vtt#seq=120-140' --evidence-kind room

- **Sources.** A path relative to the graph's folder, an absolute path inside it, a path under a source root
  (`root:<name>/<path>`, below), an `http(s)` URL or a umx address. Paths may not use `..` or start with `~`;
  `file://` is refused (`evidence_outside_roots`). URLs are stored, never fetched. A umx address may end in a
  `?repo=<git url>` hint after its selectors (`umx:launch/<id>#L3-L4?repo=https://...`); it is kept on the
  stored source, never fetched. In any other source, `?repo=` is just text.
- **Selectors.** `seq=<from>-<to>`, `L<from>-L<to>` (or `L<n>`), `section=<heading>/<subheading>` (a heading
  path, top level first), `quote=<text>` with optional `&prefix=<text>` and `&suffix=<text>` to pin which
  occurrence, and `page=<from>-<to>`. Join several with `&`; each kind at most once, and `prefix=`/`suffix=` need
  `quote=`. Text may be typed raw (spaces, and an `&` that does not start another selector) or percent-encoded
  (`%20`, `%26`); it is stored decoded, so write a literal `%20` as `%2520`. A `/` inside one heading is `%2F`.
  umx's canonical form puts the keys in the order `seq`, `L`, `section`, `quote`, `prefix`, `suffix`, `page`
  and percent-encodes the text.
- **Fixing evidence.** Never hand-edit nodes.jsonl. `umx node evidence <node> list` shows each pointer with its
  position; `umx node evidence <node> remove '<pointer>' --reason "<why>"` moves it to the node's
  `evidence_removed` trail, and `list --removed` shows the trail. Each entry is `{removed, removed_at, reason, by,
  by_session}`: `removed` is the pointer exactly as stored, and the entry's `by_session` is the session that removed
  it. So `evidence_removed[i].by_session` is the remover and `evidence_removed[i].removed.by_session` the session that
  added the pointer (absent for a pointer stored before sessions were recorded).
  Matching is on source and selectors in canonical form, never a lookup of the pointer's target. For many at once,
  batch lines `{"op":"evidence remove","node":"<id>","pointer":"<pointer>"}` with one `umx batch --reason "<why>"`
  (see `umx docs get batch`; usage 144).
- **Redacting a pointer.** If a pointer itself is sensitive (a credential in a path, a private quote), remove it with
  `umx node evidence <node> remove '<pointer>' --redact --reason "<why>"` (usage 175). This differs from a
  plain remove, which keeps the pointer in the trail:
  - Every pointer in this graph with that source is removed: on every node, live or already in a trail, and on
    edges. The selector does not have to match, so a secret only in one quote removes every pointer to that
    source; re-add the ones you need with a selector that does not quote it. A source matches as the pointer the
    node stores: `umx:`, `http://` and `https://` in any case are one scheme, a `umx:` id may be typed as a
    prefix in any case, and a `?repo=` hint is ignored. Paths and the rest of a URL match only as written. A
    prefix that names two stored sources is refused as `evidence_ambiguous`.
  - Each one becomes a trail entry `{kind, redacted, removed_at, by, by_session, reason}`, with no source, selector
    or hash. `list --removed` shows it as `(redacted)`.
  - A status move that cites it (`node status --evidence`) keeps the move: its `evidence` becomes a redacted entry
    of the same shape, in place. The node may hold the pointer only there: copy it from `node get`'s
    `status_history`.
  - The reply gives only counts (`redacted.pointers`, `.nodes`, `.edges`). A refusal never lists the node's
    pointers.
  - It is refused with `evidence_redact_blocked`, changing nothing, while the source or a selector's text would
    still be anywhere in the graph's files after it: any value or field name of any node or edge (title, type,
    created_by, a locator, a status reason or place, another pointer or trail entry), the kind, author or reason a redacted
    entry keeps, a repeated line in nodes.jsonl or edges.jsonl, or graph.json as it is on disk. The session and agent
    records the redact's own write puts down count too, so a writer whose name or role holds the text is refused: redact
    as another writer. A node the redact does not change is read as its line is stored, so a field a hand edit wrote
    twice in one line counts even where umx reads the other copy; edges.jsonl is always written again from its edges.
    JSON escapes are read, and case does not matter (Unicode case folding: `ß` and `ss` are one). The candidates name
    each record and a place: a standard field by its name, a list entry by its position counted from 1 (`evidence:2`
    is the second pointer), a custom field by its number among the record's custom fields as its line holds them,
    counted from 1 (`field 3`). `check --lint` and a read's `secret_suspected` name the same entry alike.
    They never give the text or a custom field's name. Edit those places, then redact again; for a field written
    twice, edit that line in nodes.jsonl so it holds the field once.
  - The reason may not hold any part of the pointer, in any case, and may not look like a secret.
  - Any text that contains the source or a selector string blocks the redact, even if it is not a pointer. A short
    quote selector (`quote=ok`, or one that is also a word umx writes, such as `quote=document`) can therefore
    block. Spell the pointer with a longer quote, or edit the text.

  The warning `redacted_in_files_only` is the limit: umx rewrote this graph's files, nothing else. Git history,
  other graphs, backups and any copy made before still hold the pointer. Rewrite the history that holds it, or
  treat what it pointed at as exposed.
- **Kinds.** `--evidence-kind` is `room`, `chat`, `screen`, `document` (the default) or `analyst`. Room and
  screen evidence is stored as `lossy` (it came through captions or capture); the others as `exact`.
- Each piece is stored on the node as `{ source, selector, kind, fidelity, hash, hash_state }`, with `length` and
  `seq_max` when the source was read. Adding the same pointer twice keeps one, and keeps its stored hash.

## Source hashes

When a write adds a pointer (`node add --evidence`, `node note`, `node status --evidence`, `node evidence add`,
`edge add --evidence` and the same batch lines), umx reads its source if it is a local file and stores what it
found, so you can tell later whether the file changed (usage 220):

    {"source": "calls/3-nov.ndjson", "selector": {"seq": [120, 140]}, "kind": "room", "fidelity": "lossy",
     "hash": "sha256:9f2c...", "hash_state": "hashed", "length": 1843302, "seq_max": 3511}

- The file is found as the pointer names it: a path under the graph folder, or under its `root:` (the same
  confinement as above). It is read as a stream; only the sha256, the length in bytes and, for a transcript, the
  highest sequence number are kept, never the content. Each file is read once per write however many pointers cite
  it. A file over 256 MiB is not read (`too_large`). Nothing is read on a read.
- `seq_max`: for a `.ndjson` file, the largest whole-number `seq` among its lines (a line over 1 MiB is not read for
  it); for a `.vtt` file, the number of cues (lines holding `-->`). Other files have none.
- `hash_state` says why there is or is not a hash: `hashed`; `remote` (an `http(s)` URL or a `umx:` address: never
  fetched, nothing leaves the machine); `missing` (no file there); `unreadable` (a folder, or the system refused);
  `outside` (a symlink that leads out of the roots: never read); `too_large` (over 256 MiB: not read past it).
- A local file that cannot be hashed never refuses the write: the pointer is stored with that state and the write
  warns `source_not_hashed`, whose next is `umx check --graph <graph> --sources`.
- `umx check --sources` reads each file again and reports `source_changed` and `source_missing` (see
  `umx docs get graph`). To accept what a file holds now, run `umx node evidence <node> refresh '<pointer>'`,
  `refresh --at <i>` or `refresh --all` (every pointer; a refresh with none of these is refused). It hashes them
  again and writes the node only if a hash key changed, without changing its `updated`. It also hashes a pointer
  stored before umx hashed sources. A state without a hash (`missing`, `outside`, ...) is recorded as accepted
  (`hash_accepted`), so `check --sources` leaves it out until it changes. A pointer whose `root:` is not usable on
  this machine is not read and keeps its stored hash, with `source_not_refreshed`. The reply lists each pointer with
  `changed` and `read`, and `hashed` lists every local source it read.
- **Pointers you did not write.** A refresh reads the file each pointer names and writes its hash and length into
  the graph. A graph someone else shared may hold pointers to files you would not want described in a commit
  (`root:repo/.env`, say). Read `umx node evidence <node> list` first, refresh the pointers you mean by text or
  `--at`, and check `hashed` after an `--all`.

A pointer that does not parse is refused with `evidence_pointer_invalid`, which says why. Nothing is written
when any pointer is refused.

A syntactically valid `umx:` node or edge source is checked against the loaded graphs. Missing targets warn
rather than refuse, so the order you add things in does not matter:

- its graph is loaded and the id names no node there, or a prefix matches several: the pointer is kept, with
  warning `dangling_evidence` (an ambiguous prefix lists its candidates). Find the node you meant with
  `umx node find <words> --graph <alias>` and add evidence that names it.
- its graph is not loaded, or is secured (even when loaded): the pointer is kept as written, with warning
  `evidence_unchecked`. A write never learns anything about a secured graph's nodes. Load an open graph if you
  want it checked, or run `umx resolve` on the address.
- a prefix that finds one node is stored as the full id (the alias as you wrote it), so it cannot become
  ambiguous later. `umx resolve <address>` shows what an address names.
- Graph fragments such as `umx:<alias>#L1` are `evidence_pointer_invalid` (`umx docs get evidence`).
  The explicit `umx:<alias>/<graph id>#L1` form is also refused once visible metadata identifies the virtual
  graph node. A hidden or unloaded explicit-id source still gives `evidence_unchecked`; a citation never
  loads or writes its target.

## Source roots

Files outside the graph's folder are cited through a named source root, so the pointer stays the same on every
machine:

    umx root add acme /Users/me/work/acme
    umx node note 01JC8XA4H7 --append "Agreed in the call" --evidence 'root:acme/transcripts/016.ndjson#seq=1-5'
    umx node add --type decision --title "Ship in May" --evidence 'root:repo/docs/plan.md#L12'
    umx root list
    umx root remove acme

- `root:repo/<path>` is built in: the git folder holding the graph's folder. It needs no setup. If that git
  folder is not inside the roots (`UMX_ROOTS`), root:repo is refused with `root_unavailable`.
- Any other name is set on this machine with `umx root add <name> <folder>` (stored as `source_roots.<name>`
  in the machine config; `umx config set source_roots.<name> <folder>` is the same). A repo's config cannot add
  one. Names are snake_case, at most 32 characters; `repo` is taken.
- The folder must exist and be inside the roots once symlinks are resolved, or `root add` refuses with
  `path_outside_roots`. Every evidence write checks it again, so a root later swapped for a symlink that leads
  out is refused then. A path under a root may not use `..`, `~` or `/` at the start, and a symlink in it that
  leads out of the root is refused (`evidence_outside_roots`).
- A pointer naming a root this machine does not set is refused with `root_unknown`, whose next is the
  `umx root add` to run. A teammate who pulls a graph citing `root:acme/...` adds their own `acme`.
- The pointer is stored as written (`root:acme/transcripts/016.ndjson`), never expanded to a path. umx reads the
  file only to hash it (above).
- `umx root remove acme` is allowed while evidence cites `acme`. Those pointers keep their spelling, but new
  citations of `acme` get `root_unknown` on this machine until it is added again.
- `root list` and `root remove` report a machine config umx cannot trust or parse as that file's problem
  (`config_file_refused`, `config_file_invalid`), never as an empty list or `root_unknown`, and leave it as it
  is. A `source_roots.*` value umx cannot use is listed as a warning, not a row.
