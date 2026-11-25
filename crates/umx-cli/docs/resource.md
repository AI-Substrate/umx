# Resources: nodes that are files, folders and URLs

A resource node is an ordinary node (type, title, status, note, fields, edges, address) that also has a
`locator`: where the thing lives. Add each file, folder or URL once, then link people, decisions and questions to
it with ordinary edges. umx never fetches a URL; it reads a local file only to hash it (below).

## umx node add --type <type> --locator <path|url|root:...>

    umx node add --type document --locator docs/brief.md                  # titled brief.md
    umx node add --type transcript --locator root:repo/transcripts/016.ndjson --title "Customer call, 4 Nov"
    umx node add --type folder --locator specs/                           # a trailing / makes a folder
    umx node add --type url --locator https://example.com/pricing
    umx node add --type legislation --locator root:acme/acts/2026.pdf     # a root this machine set

- A source takes one of four forms:
  - a path relative to the graph folder (no `..`, no `~`);
  - `root:repo/<path>`, a file elsewhere in the git repo that holds the graph folder;
  - `root:<name>/<path>`, a file in a source root this machine set with `umx root add` (see
    `umx docs get evidence`);
  - an http(s) URL, stored and shown but never fetched.
- An absolute path or `file:` URL is refused with `locator_absolute`: committed files never hold one. Its `next`
  shows the `root:` form to use. Text that is no form at all, or holds `..`, `~`, a URL user or password, or a
  control, bidi or zero-width character, is refused with `locator_invalid`.
- Without `--title`, the title is the source's last part (a file or folder name, else a URL's host).
- The node `type` is yours to choose (`document`, `transcript`, `legislation`, `url`, ...). The locator's `kind`
  (`file`, `folder` or `url`) and `format` (`markdown`, `text`, `transcript_ndjson`, `docx`, `pdf`, `html` or
  `other`, from the extension) are umx's.
- **One resource node per locator in a graph.** Adding the same source again (by kind and normal form, so
  `./docs//brief.md` is `docs/brief.md`, and a graph-folder path is the same file as its `root:repo/` form)
  adds nothing: the answer is the existing node with `resource_exists`. When another loaded graph holds the
  same thing, `resource_elsewhere` names it: link to that node rather than keep a second one.
- A source that looks like a secret warns `secret_suspected` as an evidence source does.

## umx resource show <node>

    umx resource show 01JD7K2M3N
    umx resource show umx:acme/01JD7K2M3N4P5Q6R7S8T9V0W1X

Prints the node with its `locator`: `kind`, `format`, `source`, and the `hash` (`sha256:<hex>`), `length` and
`seq_max` umx saw when it last read the source, with `hash_state` saying why there is or is not a hash. A node
without a locator is `not_a_resource`.

- A file locator is hashed when it is added (`node add --locator`, `node edit --locator`), as an evidence source is
  (`umx docs get evidence`): read as a stream, only the sha256, the length and, for a `.ndjson` or `.vtt`
  transcript, `seq_max` are kept. `hash_state` is `hashed`, `missing`, `unreadable`, `outside` or `too_large`; a URL is
  `remote` and never fetched; a folder has none. A file that cannot be hashed warns `source_not_hashed`; the node
  is still added.
- Adding the same source again returns the existing node (`resource_exists`) and changes nothing, its hash
  included.

## umx resource refresh <node>

    umx resource refresh 01JD7K2M3N

Hashes the locator's source again and keeps what it holds now: the way to accept a `source_changed` that
`umx check --sources` reported. It writes the node only when the hash, length, `seq_max` or `hash_state` changed
(`data.changed`), and never changes its `updated`. `data.hashed` names the source it read. A state without a hash
(`missing`, `outside`, ...) is recorded as accepted, so `check --sources` leaves it out until it changes. A locator
whose `root:` is not usable on this machine is not read and keeps its stored hash, with `source_not_refreshed`. A
node without a locator is `not_a_resource`. As with evidence, a shared graph's locator may name a file you did not
choose: look at `umx resource show <node>` before you refresh it.

## Re-pointing or clearing a locator

    umx node edit 01JD7K2M3N --locator docs/brief-v2.md    # the file moved
    umx node edit 01JD7K2M3N --unset locator               # a plain node again

- The node keeps its id, edges, evidence and notes. The new locator is hashed as on add.
- A locator another resource node in the graph already holds is refused with `locator_in_use`, naming that node.
- `locator_changed` says how many edges touch the node and how many evidence pointers in the graph cited the old
  source: check those still hold.
- A node with a kept custom field named `locator` (from before umx reserved the name; `locator_field_kept` at
  load) cannot take a locator (`locator_field_invalid`) until you rename that field by hand.
  `--unset locator` on it removes that field.
- Graph format: the first locator raises the graph to the locator format, which an older umx refuses with
  `graph_format_newer`. Removing every locator never lowers it.

## In node list, columns and Cypher

`locator_kind`, `locator_source`, `locator_format`, `locator_hash`, `locator_length` and `locator_seq_max` answer
in `--where`, `--columns` and Cypher. Nothing stores them flat, and `locator` itself is not a property: `n.locator`
reads null with `query_unknown_property`.

    umx node list --where locator_kind=folder
    umx node get 01JD7K2M3N --columns title,locator_source,locator_format
    umx query "MATCH (n) WHERE n.locator_kind = 'file' RETURN n.title, n.locator_source"
