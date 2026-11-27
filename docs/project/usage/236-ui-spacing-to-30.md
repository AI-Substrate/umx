---
id: 236
title: Spacing in the graph view goes to 30 on a logarithmic slider; tell the person to press Fit after a big spread
route: explore
status: current
source: Project lead 2026-11-27 ("spacing is still too weak, make it 10x more powerful"), umx view spacing fix
reqs: []
supersedes: none
---
## Do
When a person says the nodes in `umx ui` are too close, point them at Spacing under Clusters. It now runs from 0.5 to
30 (it stopped at 3). The slider is logarithmic, so the left half covers the old range and the right half spreads much
further. Above 3 the layout grows in step with the spacing and spreads nodes more evenly, opening up dense clusters.

Tell them to press Fit after a big move: while the slider moves, the view keeps its frame, so a spread to 30 can fill
the screen with one corner of the graph.

## Don't
Don't expect a big spacing to make every gap at Fit 10 times wider: the whole graph still has to fit the window, so at
Fit the gain is a more even spread, not 10 times the room. Zoom in to see the full spread.

Don't tell someone their old spacing changed: a value saved from the old slider (0.5 to 3, in the browser or a
`spacing=` link) lays out exactly as before.

## Example
    umx ui --graph acme
    # then in the view: drag Spacing to the right, press Fit
    # or share a link with the look kept: http://127.0.0.1:<port>/#spacing=12

## Why
On real graphs of 200 to 300 nodes even the old maximum left nodes close at Fit. The layout grew only about 1.4 times
from the default to the old maximum, and that growth disappears at Fit because nodes draw at a fixed screen size. Now
the top of the slider makes the layout 10 times wider than the old maximum, and the extra gravity against link pull
spreads nodes evenly enough that the smallest gaps at Fit grow by about half.
