---
id: 075
title: A list's total may be null; page on with truncated and --offset (walk stops early)
route: explore
status: current
source: F41 (architect note), architect ruling 2026-11-10 (007 list shape, PROTOCOL 13); PR for plan 039
reqs: [req-28]
supersedes: none
---
## Do
- Read `truncated` to know whether more rows exist; it is always exact. Page on with `--offset` set to your offset plus the rows you got.
- Use a small `--limit` with `umx walk` on a big graph: the walk stops once the page is full and one more row is known, so it stays cheap.
- Treat `total` as the exact count only when it is a number. `umx walk` returns `total: null` when `truncated` is true.

## Don't
- Don't treat a null `total` as 0, and don't walk a whole graph just to count it.
- Don't assume every list counts everything: any list may return null `total` once it pages lazily.

## Example
```bash
umx walk 01JC8XA4H7 --depth 5 --limit 20
umx walk 01JC8XA4H7 --depth 5 --limit 20 --offset 20
```

## Why
Counting every reachable node costs a full walk. Lazy paging keeps a small page cheap on a big graph (F41).
