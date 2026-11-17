---
id: 205
title: Retry a batch safely with umx batch --idempotency-key
route: build
status: current
source: ws003 s09; graph-storage, graph-addressing and architect design verdicts and prime rulings (2026-11-17)
reqs: [req-88]
supersedes: none
---
## Do

When a batch's reply may be lost (a timeout, a daemon restart, an agent that stopped and resumes later), give the
batch a key you choose and retry by sending the same batch with the same key. A key is 1 to 128 of
`A-Z a-z 0-9 . _ : -`; anything else is `usage_invalid`, which names the rule, never your key. Name the window or
step in it, so one key means one batch: `launch:window-08`.

The first real run that succeeds records a receipt in the graph folder's `receipts.jsonl`, in the same write as the
batch's files, so a crash never leaves one without the other. Sent again with the same key and the same input,
umx returns that receipt and writes nothing: no id is drawn, nothing is queued for embedding, no node changes. The
same input means the same ops in the same order, `--reason`, `--refs-from` (by graph id) and `--raised-in`; line
numbers, `--dry-run`, `--echo-sentences` and `--related`/`--no-related` do not count, so adding a comment line to
the file keeps the batch the same. A replay runs no related check (usage 204): it has no rows.

Read the reply: every batch reply now carries `counts` (each op's `lines` and how many `existed`). A keyed one adds
`idempotency_key`, `replayed` and `recorded_at`. A replay has `replayed: true`, the `refs` and `counts` of the first
run, `applied: true` and no rows, with the hint `batch_replayed`. Save it and pass it to `--refs-from` as you would
the first reply. Dry-run with the key to ask whether a real run would replay (`replayed`); a dry run never records.

When copies of one graph are loaded, retry with the same `--graph` you named the first time: receipts belong to one
graph folder.

## Don't

Don't put client names, people, secrets or any other sensitive text in a key. umx keeps only the key's digest, but
that digest is committed with the graph and a short or predictable key can be guessed back from it, so treat the key
as public: a window or step name and a counter (`launch:window-08`), nothing more.

Don't reuse a key for another batch: the same key with another input is `idempotency_key_reused`, nothing changes,
and the message says when the key was used. Use a new key.

Don't expect a receipt forever. Receipts are kept 7 days, and at least the most recent 1,000 whatever their age, in
a file of at most 4 MiB (the oldest are dropped first past it). A key whose receipt was dropped runs as new, and the
hint `idempotency_key_expired` says when it was recorded; after 30 more days (or past 10,000 dropped keys) umx
forgets the key, and the batch runs as new with no hint.

Don't expect a failed batch to be remembered: a line error, a refused reply bound or a failed write records
nothing, so the retry runs for real. A `write_not_durable` warning means the write and its receipt stand.

Don't expect a replay to say what the batch would do now: it returns what the batch did then, and later edits do
not change it. If the graph no longer holds an id the receipt names (nodes.jsonl changed under it, by a hand edit or
a checkout), the replay is refused with `idempotency_receipt_stale`; run the batch with a new key.

Don't hand-edit `receipts.jsonl`. A file umx cannot use (written by a newer umx, not a receipts file, over 16 MiB,
another graph's, or changed on disk while the batch ran) refuses keyed batches with `receipts_unreadable`, which
says why, and changes nothing; batches without a key never read it. After a merge, keep both sides' lines (they may
come in any order) and one line per key. Commit it with the graph's other files.

Don't expect a key to reach another graph or copy: one graph folder's receipts answer only for batches that write
that folder. A rename or a move keeps them, since the file names no alias.

Don't expect an older umx to know the file. It ignores it, and cannot run keyed batches. If a crash cut short a
write whose set held `receipts.jsonl` beside other files, an older umx refuses every command on that graph ("the
marker is not one umx wrote") and changes nothing: open the graph with this umx or later, which finishes the write.

## Example

```sh
umx batch --file window-08.jsonl --graph launch --idempotency-key launch:window-08 > window-08.receipt.json
# the reply was lost, or the agent resumed: send it again
umx batch --file window-08.jsonl --graph launch --idempotency-key launch:window-08 > window-08.receipt.json
# replayed: true, the same refs; nothing written
umx batch --file links-08.jsonl --graph launch --refs-from window-08.receipt.json
```

## Why

A batch whose reply never arrived left an agent two bad choices: send it again and maybe add every node twice, or
read the graph back to work out what landed. A key makes a retry safe. The receipt lives beside the graph's files
and is written in their set, so a checkout or a crash moves both together, and it holds digests, ids, counts and
times only: never the key, a ref name, a title or an op, so a secured graph's folder gains no text.
