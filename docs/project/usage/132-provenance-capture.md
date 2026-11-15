---
id: 132
title: Writes record provenance claims without changing author labels
route: build
status: current
source: ws007 s02 plan 097, architect and security rulings 2026-11-11
reqs: [req-93, req-94]
supersedes: none
---
## Do

Use the normal node, edge and batch commands. umx records the writing session with the content change.
Creation records the author and last editor; an edit preserves the original author and replaces the last editor.
Editing an older node does not invent its original author. New evidence pointers from node add, node note and
batch record `by_session`; old pointers are not back-filled. Edge evidence remains reserved for its own step.

Treat provenance as **recorded by the writer, not verified**. Check `data.actor_tier` when capture falls back:
`pij`, `harness`, `cli` or `unknown`. A failed identity tool does not prevent an otherwise valid write.
Keep an explicit `--by` or `UMX_AUTHOR` label when that is the author text you intend; capture does not replace it.
That label still controls `created_by`. For `edge edit`, `updated_by` is instead the captured writer's display
name (pij seat, else login, else unknown), never a `--by` label or the daemon's environment.
The edge keeps its `authored_by_session` and replaces `edited_by_session` with the editing session's same-graph
id. Follow `session_of` from that session to the agent, `agent/pij/<seat>` for pij.
Captured role, model and effort are session facts: a missing capture does not erase a last known value, and a
new known value replaces it. Say "model of this session", not "model of this write".

## Don't

Never use provenance to grant permission or establish who is trusted. A graph writer can forge these claims.
Do not set or unset `prov_key`, `authored_by_session` or `edited_by_session` yourself (`reserved_name`, usage 119).
Do not add `authored_by` or `edited_by` edges: these are derived from the session keys, not stored by hand.
Do not put a session identifier in a path, URL or command.

Provenance nodes are hidden from default content reads, counts and traversal.
Their text is never embedded, word-indexed or used for similarity, even when vector rows were imported.
No public provenance opt-in or `who` command is included in this step.

## Example

    umx node add --type problem --title "Approval delays"
    umx node note <node> --append "New evidence" --evidence "docs/approval.md#L1"
    umx batch --file ops.jsonl --dry-run
    umx batch --file ops.jsonl

An unchanged edit or a dry run creates no attribution records.
On `provenance_key_invalid`, inspect the reported file, line and key with `umx check`; the malformed value is kept.
On `session_agent_conflict`, the session keeps its first agent rather than accepting a different claim.
On `duplicate_prov_key`, both records are kept and the lower node id is used.

A user node with a reserved type warns `reserved_type` and remains content without a valid matching key.
User edges on `session_of`, `runs_on`, `uses_model` or `registered_in` are refused with `reserved_relation`.
An attempt to store `authored_by` or `edited_by` is refused with `derived_relation`.

## Why

The graph can retain how its content was written without treating an agent's own assertion as authentication.
Harness session identifiers are validated and hashed before the client sends the request; raw identifiers are
never stored or returned. Only known model, harness and platform facts are linked. Otherwise umx records the
safe fallback tier instead of guessing.
