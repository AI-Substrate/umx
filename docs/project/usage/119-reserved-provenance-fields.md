---
id: 119
title: Provenance field names are reserved
route: build
status: current
source: ws007 s01 plan 085 and s02 plan 097, the project manager compatibility ruling 2026-11-11
reqs: [req-94]
supersedes: none
---
## Do

Use a different custom field name for your own author labels.

On `provenance_key_invalid`, inspect the file, line and key named by the warning or `umx check`.
An invalid stored value is kept, not treated as valid attribution.

## Don't

Do not set or unset `prov_key`, `authored_by_session` or `edited_by_session` on nodes or edges.
These three names are now reserved; `--set` on them is refused with `reserved_name`.
The reservation applies even without automatic provenance capture.
Do not assume malformed stored values stop a load or disappear on the next write.

## Example

    umx node add --type problem --title "Approval delays" --set author_label=reviewer

`umx node add --type problem --title "Approval delays" --set authored_by_session=garbage`
is refused with `reserved_name`, without writing a node.

## Why

umx owns provenance keys. Treating a user value as a typed session id could otherwise produce an invalid record.
Malformed values in an existing file load with `provenance_key_invalid` and remain available for repair.
An untouched node keeps its line bytes. Edges are rewritten in canonical form, with the bad raw value kept.
An explicit edit replaces the last-editor key but never invents or clears the original author.
