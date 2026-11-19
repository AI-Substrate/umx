---
id: 217
title: Turn a custom field's values into std tags with ontology adopt-std --field-to-tag
route: build
status: current
source: ws008 t2b-p3 (req-117)
reqs: [req-117]
supersedes: none
---
## Do
When a custom field holds a short label (`origin`, `stage`), move it into std tags with `umx ontology adopt-std --field-to-tag origin --dry-run` first, then without `--dry-run`. Each value becomes `origin:<value>` in stored form, with `tag_invalid`'s snake_case fix applied (`Double Diamond` becomes `origin:double_diamond`), merged into the node's tags. It works while a graph adopts std (with `--map` in the same write) and on a graph that already uses std. Read the reply's `field_to_tag`: `nodes` moved, `tags` gained, `kept` (nodes whose values stayed), `collapsed` (values that became one tag) and `declaration_removed`. Then filter with `umx node list --tag 'origin:*'`. On a large graph always run `--dry-run` first: the move is one write over every node that holds the field, so the dry run's counts and listed values tell you what that write will do before memory and disk take it.

## Don't
Don't expect every value to move: one with no snake_case form (`123`), one whose snake_case form is longer than a tag (never cut), one that is not text (shown only as `<object>`, `<list>` or `<not text>`), one that holds anything secret-shaped, and every value of a node that would pass 64 tags stay in the field. An empty list just leaves the node. `adopt_tag_value_invalid` counts them per reason and lists node ids and values (a secret as `<masked>`); its next lists the field. Fix them, then run the command again: the declaration goes once no node keeps a value. Don't name a std field, `tags`, a field only edges have, or one a spine reads (`adopt_field_to_tag_refused`), and don't give `--map` on a graph that already uses std (`usage_invalid`).

## Example
```bash
umx ontology adopt-std --field-to-tag origin --dry-run --graph launch
umx ontology adopt-std --field-to-tag origin --graph launch
umx node list --tag 'origin:*' --graph launch
```

## Why
A label kept as a free-text field cannot be filtered from an index, and its spellings drift. Moving it into one stored tag form keeps ids and links, lets every read filter by it, and leaves anything that would be lost or exposed where it was, listed.
