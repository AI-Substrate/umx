---
id: 186
title: Relate an instance to what it is one of with instance_of; it never joins an entity
route: multi-graph
status: current
source: ws008 T6, plan 190 (answer 6 in an internal entity review; D69)
reqs: [req-138]
supersedes: none
---
## Do
When one thing is an instance of another, add an `instance_of` edge from the instance to the thing: a deployment to its product, a copy to its template, an occurrence to the recurring meeting. Write it like any edge, with no declaration first: it is a std relation every graph knows. It may cross graphs: name the graph that stores it with `--graph`.

Keep `same_as` (`umx entity link`) for two nodes that are one thing, and `part_of` for a thing inside another.

## Don't
Don't use `instance_of` to say two nodes are the same: `umx entity show` never joins across it, so the two stay two entities. Don't give an instance the key of the thing it is an instance of: a shared key still joins them. Don't mark `instance_of` symmetric or remove it from a graph's vocabulary: both are refused.

## Example
```bash
umx edge add umx:deploy/01JC8XA4H7 instance_of umx:catalog/01JC8XB000 --graph deploy
umx entity show umx:deploy/01JC8XA4H7     # one member: the deployment alone
```

## Why
A deployment and its product are related, not the same: merging them would mix their owners, dates and evidence. One named relation for the link keeps the relation visible in walks and the view, while entities stay what each key and `same_as` says.
