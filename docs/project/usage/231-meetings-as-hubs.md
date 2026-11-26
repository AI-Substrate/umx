---
id: 231
title: When nodes come from a meeting, find or make its one meeting node, keep it on the meeting spine, and link every point to it with raised_in
route: ingest
status: current
source: PRD 003 graph rules, rules-30 (project lead 2026-11-26: meetings go on the meeting spine, nodes from a meeting link to it, every meeting is a hub)
reqs: []
supersedes: none
---
## Do
Only when the nodes come from a meeting. Find the meeting first: `umx node list --type meeting --where
'starts_on=<date-time>'` or `umx node find "<title>" --type meeting`. Add it only if it is missing, with
`starts_on` (and `ends_on`) as full date-times with the offset. If `umx graph spine show <graph>` has no meeting
spine, declare one once with `umx graph spine set <graph> --file meetings-spine.json` (anchors `meeting`, ordered by
`starts_on`, placing by `raised_in`). If `umx hub list` has no meetings hub, set one once:
`umx hub set meetings --type-hub meeting --via raised_in`. Link every node you take from the meeting to it, or to
its segment, with `--raised-in <meeting>` on the add or the batch, so the link lands in the same write. For a graph
that wants the reminder, offer `umx rule add from_a_meeting --type question,decision,action,risk --edge raised_in
--to meeting,segment,event,transcript`: a point added with no meeting then warns `rule_unmet`, and
`umx check --rules` lists every point not yet tied to a meeting.

## Don't
Do not make a second node for a meeting you already have: ingesting the same meeting twice uses the node you
found. Do not declare the spine or the hub again once they exist. Do not do any of this for nodes that did not
come from a meeting; nothing fails when it is skipped. Do not give the meetings hub a pattern: umx cannot know
which meeting a node belongs to, so you link it.

## Example
```bash
umx node list --type meeting --where 'starts_on=2026-11-24T09:00:00+10:00' --graph demo
umx node add --type meeting --title "Weekly sync, 24 Nov" --set starts_on=2026-11-24T09:00:00+10:00 --set ends_on=2026-11-24T10:00:00+10:00 --graph demo
umx graph spine set demo --file meetings-spine.json
umx hub set meetings --type-hub meeting --via raised_in --graph demo
umx batch --file points.jsonl --raised-in 01JZ0000000000000000000002 --graph demo
```
where `meetings-spine.json` holds
`{"name": "meetings", "title": "Meetings", "main": true, "anchors": {"type": "meeting"}, "order": ["starts_on"], "place": ["raised_in"]}` (`"main": true` only if the graph has no main spine yet; a second one is refused with `spine_main_taken`, so use `false` then).

## Why
Meetings come up often, and each one is where its points gather. One node per meeting on one time spine keeps
them in date order, and `raised_in` on every point says when and where it was said.
