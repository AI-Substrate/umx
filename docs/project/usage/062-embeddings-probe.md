---
id: 062
title: Check the embedder with umx embeddings probe; it sends no graph text
route: doctor
status: superseded
source: workstream 004 s05; architect ruling 2026-11-10 (verb `umx embeddings probe`, ready() sends no text)
reqs: [req-73, req-46, req-53]
supersedes: none
superseded_by: 120
---
## Do
After setting up an embedder (note 061), run `umx embeddings probe`. It first checks the embedder can run without
sending any text (for Azure: gets an `az login` token, or checks the key was found), then embeds one fixed sentence
that umx owns, never anything from a graph. It prints the instance, the model key (`model@dims`), `dims`, `norm`
(about 1), `latency_ms`, `locality` and the credential that worked. Use `--embedder <name>` to try another instance
the daemon built, and `--graph <alias>` to check the instance that graph would use (its folder's repo config and
its effective policy, the stricter of graph.json and this machine's record, apply).

On an error, read its message: `embedder_unavailable` names the fix (for example `run az login`) and its `next`
re-runs the same probe; `embedder_invalid` means the instance's config is wrong (`umx config check`);
`embedder_not_configured` means no instance is chosen.

## Don't
Don't use the probe to test what a graph's text embeds to: it never sends graph text. Don't expect it to pick a
hosted instance for a secured graph; it refuses (`embedder_unavailable`) and asks no one for a token. Don't retry
it in a loop on `embedder_unavailable`; fix what the message names first.

## Example
```bash
umx embeddings probe
umx embeddings probe --embedder azure_small
umx embeddings probe --graph launch
```

## Why
req-73: proving the sign-in works (az login by default, an api key when configured) before any graph is embedded,
without risking client text. The probe runs on its own thread, never on the daemon's writer, so a slow provider
never holds up writes.
