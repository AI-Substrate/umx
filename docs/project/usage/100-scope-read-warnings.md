---
id: 100
title: Saved scopes report the primary folder, secured members and skipped copies
route: multi-graph
status: current
source: PR 143 review follow-ups, the project manager ruling 2026-11-11
reqs: [req-66]
supersedes: 098
---
## Do

Use `--graph a,b` for loaded aliases or `--scope both` for a saved scope on node list, edge list,
query and search. Use `umx scope save both --graph a,b` to save a selection; list/show/delete manage it,
and `--replace` replaces an existing name. Scope names are snake_case in their own namespace.

Scopes store graph ids, with the primary's alias and folder as hints, in `$UMX_HOME/scopes.toml`.
Saving a non-primary alias warns `copy_dropped`; naming the primary does not.
Saved ids always resolve to the loaded primary, even when it now lives in another folder.
Read `scope_member_moved` to see the scope, saved member, saved folder and folder actually read.
Use `umx scope show both` to inspect the saved hints before accepting unexpected content.

Every saved-scope read warns `scope_includes_secured` for each secured member, including a member
that became secured after saving. The saved scope still counts as explicitly naming that graph.
Other loaded copies are omitted with `copies_skipped`; secured copies are counted, never named.

## Don't

Do not treat a saved scope as a pinned folder or copy selection. Use explicit `--graph` aliases for copies.
Do not use `--set`, `--scope a,b`, `--all`, or `--scope` with `--graph`.
Writes, title find, walk, neighbours and path still take one graph.
Do not ignore `scope_graph_not_loaded`: no partial rows were returned. Its load next quotes supported
paths containing spaces. Unsafe shell punctuation remains withheld; inspect the folder under scope show's data.
A missing primary with several copies left is `copy_ambiguous`, including when saving a scope.
Do not overwrite a newer scope file: `scope_format_newer` means upgrade umx; no file was changed.

## Example

    umx node list --graph design,delivery
    umx scope save both --graph design,delivery
    umx node list --scope both
    umx edge list --scope both --type blocks
    umx query 'MATCH (n) RETURN n' --scope both
    umx search approval --scope both
    umx scope show both
    umx scope list
    umx scope delete both

## Why

This supersedes usage 098 and retains its selection, row-provenance and pagination rules.
Scoped node/edge rows carry graph/address even with custom columns; limit/offset apply globally.
One-graph defaults are unchanged. Unnamed query/search keep secured graphs out and one primary per id.
Read-time warnings make changes to a saved scope's effective folder, policy and copy selection visible
without changing its graph-id meaning. Scope format/version checks preserve newer files.
