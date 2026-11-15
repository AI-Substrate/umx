# Building a graph well

How to add and change nodes and edges so the graph stays true and easy to read. The flags are in
`umx docs get node`, `umx docs get edge`, `umx docs get batch` and `umx docs get evidence`; this topic is the
practice. Turning a source into nodes, step by step, is `umx docs get ingest`.

## Before you write

- Know the graph: `umx graph guide <alias>` (its purpose, conventions and words) and `umx docs get organise`.
- Be its only writer, or send your changes to the seat that is.
- Look before you add. A node that exists gets more evidence or fields, not a twin:

      umx node find "approval delays"
      umx search approval delay --graph launch

  `node add` still adds a near copy, with `near_duplicate` naming the node that exists. Check it and use that one
  unless the two are really different.

## What earns a node

A node is something someone could agree with, argue against, own or be blocked by: a problem, an option, a
decision, an open question, an action, a risk, a person, a meeting. A passing remark, a restatement or a topic no
one took a position on is not a node; put it in a note.

Titles say what was said, no more. "Finance targets 111M" is not "111M converts"; "Trying to unify the data" is
not "Unifies the data". Keep estimates, targets and commitments apart. Your own inference goes in the note,
starting "Analyst:", with evidence kind `analyst`, or in its own `question` or `tension` node.

## Adding

    umx node add --type question --title "Which vendor pack is current" --evidence 'root:calls/2026-11-03.vtt#seq=120-140' --evidence-kind room
    umx edge add <option id> answers <question id>

Give a node its evidence in the same `node add`. Read the sentence `edge add` prints back: it is the direction you
stored. Names are snake_case.

Dates go in fields declared as kind `date`, and a date-time always carries its offset (`+11:00`) or `Z`. Date
the meeting or event and link the points to it with `raised_in`, rather than dating every node. A span is
`starts_on` and `ends_on`; when something was said is a link, not a date. `umx docs get time`.

An edge can carry fields too (`umx edge add <a> supports <b> --set confidence=0.7`). Declare them on the relation,
as you declare node fields on a type: `umx ontology add field --relation supports confidence:float`. They are not in
`umx edge list` rows unless you ask: `--columns all`, or name them.

More than a handful of changes: one `umx batch`, dry run first:

    umx batch --file ops.jsonl --dry-run
    umx batch --file ops.jsonl

- A `ref` names a node the batch adds, so later lines can point at it (`parent_ref`, `from_ref`, `to_ref`,
  `node_ref`). Any lower-case name works: `q1`, `estate`, `answer_1`.
- Refs live only inside one batch. When a later batch must point at a node an earlier one added, read its id from
  the earlier batch's `data.refs` (ref to id) and save that map before you run anything else. If a run stops part
  way, look before you add again rather than trusting the map.
- Porting from another umx graph? Give each add its `id` to keep the source's ids (`umx docs get batch`, Keeping
  source identities). From anything else, let umx mint ids and keep the old one in a field such as `source_id`.
- A batch is all or nothing: fix the lines the errors name and run it again.

## Changing

    umx node note <node> --append "Finance confirmed it on 4 Nov" --evidence 'root:calls/2026-11-03.vtt#seq=300-310' --evidence-kind room
    umx node edit <node> --set job_title="Head of Finance"
    umx node status <node> agreed --reason "Both sides said yes at seq 410"

- **Enrich, don't skip.** When a source mentions a node you already have, append what it adds and its evidence,
  and fill fields that are empty. A dedupe that skips existing nodes throws that knowledge away.
- **Move statuses with a reason.** Status changes go through `umx node status`, so the move is recorded with who,
  when and why. `agreed` only where both sides said yes; `leaning` where one side moved and the other echoed it;
  `proposed` for one voice; `contested` where it was raised and then hedged.
- **Close questions.** When a question is answered, link the answer (`answers`) and move the question off `open`.
  A partial answer: say what is missing in the question's note and keep it open.
- **A promise is not an answer.** "We'll come back to you on that" is an action that the question `depends_on`, not
  an `answers` edge. Otherwise "which questions are unanswered" returns nothing. To fix one already stored as
  `action answers question`: `umx edge edit <edge> --relation depends_on --reverse`.
- **Who raised it is not who owns it.** `raised_by` says who said it; `owns` says who must act. Give actions an
  owner where the source names one.
- **Fix edges with edge edit.** A wrong relation or direction: `umx edge edit <edge> --relation <r>` or
  `--reverse`; the edge keeps its id. An edge that should not exist: `umx edge delete <edge>`. Get the edge id from
  `umx edge list --from <node>`. Many at once: `edge edit` and `edge delete` lines in a batch. An edge with an end
  in another graph is edited or deleted the same way, from the graph that stores it. Never edit `edges.jsonl` by
  hand.
- **Retire nodes, don't delete them.** There is no node delete. A node that no longer holds moves to `ruled_out`
  (or a status such as `superseded`) with a reason; its links still explain the history.

## Evidence

Every claim carries evidence: a source, a selector and a kind. The source is a path in the graph folder, a file
elsewhere through a source root (`root:<name>/<path>`, or `root:repo/<path>` in the graph's own git repository),
a URL or a umx address. Cite transcripts and documents where they live, through a root, rather than copying them
in (`umx docs get evidence`, Source roots).

| Kind | Use it for | Stored as |
|---|---|---|
| `room` | speech, through captions or a recording | lossy |
| `chat` | text people typed in a meeting chat | exact |
| `screen` | what was shown on screen, through a capture | lossy |
| `document` | a file or page (the default) | exact |
| `analyst` | your own inference | exact |

- **One kind per pointer.** A range that mixes captions and chat lines is two pointers: the caption lines as `room`,
  the chat lines as `chat`. Never label a mixed range with one kind.
- Captions are not screen: spoken words shown as captions are `room`. `screen` is a slide, a demo or a shared
  document as captured.
- Quote chat and documents (`#quote=...`); cite captions by sequence numbers (`#seq=120-140`), never by quote,
  since captions are lossy.
- A pointer that quotes another node quotes that node's own words.

## Placeholders

A source that has no node yet (a deck, a report someone mentioned) can be a `document` node now, linked with
`sourced_from`. Resource nodes for files are planned and will take over such placeholders in place, keeping their
ids and edges.

## After a session

    umx check --graph <alias>
    umx node list --graph <alias> --status open --type question

Run the check, read back what you changed, and report the ids you added or changed. Send each friction with umx to
whoever maintains it.
