# Time spine replay

A graph projected along one dimension, time. The steps of a meeting form a spine. Everything raised in the meeting hangs off the spine in soft 3D, grouped into four quadrants by kind of thing. A replay plays the meeting back: nodes pop in when they were raised and the camera lingers on the major ones.

This is an exemplar, not a feature. It came out of a design workshop on showing a graph through a dimension, and is kept so the idea can be picked up again later. See [the rules for examples](../../README.md).

## Run it

Open `index.html` in a browser. It loads `sample-data.js`, an invented workshop, so it works with no graph and no daemon.

To start the replay at once, add `#play=1` to the address:

```
open "index.html#play=1"
```

## Keys

| Key | What it does |
| --- | --- |
| R | start or stop the replay |
| Space | pause the replay |
| = and - | replay faster, slower |
| 1, 2, 3 | strip, funnel and flight cameras |
| Left, Right, scroll wheel | travel along the spine (or scrub the replay) |
| Drag | look around a little, in the funnel view |
| T | dark and light skins |
| L | ribbons between bands on and off |
| A | agent demo: placeholder `umx ui` commands drive the view |
| Esc | let go of a clicked node |

Hover a node to see its links. Click to hold it. Drag the bar at the bottom to move along the spine.

## What you are looking at

- **The spine** is the dimension. Its **bands** are the meeting's segments, in order. Band width is part clock time and part how much was raised.
- **The funnel** widens with the running total of nodes, so its shape shows when the graph grew.
- **Quadrants** around the spine group nodes by kind of thing.
- **Solid dots** are placed by data: a link says which segment they were raised in. **Hollow dots** are placed by rule: they have no such link, so they sit at the middle of their placed neighbours. Nodes with neither are counted on a shelf, "off this axis".
- **Squares on the spine** are the band nodes themselves.
- **Labels on leaders** mark a few major nodes per band, capped so the screen stays calm.
- **Ribbons** add up the links between two bands and show the count.
- **Three cameras, one set of placements**: strip (flat, from above), funnel (side on) and flight (along the spine).
- **Replay**: a title card for each band, an event log, each node flashing its links to what is already there as it arrives, and a lock-on with a typed callout for the major nodes.
- **Every number and every line of text on screen is real**, counted or read from the data file.

## Point it at your own graph

```
python3 export-graph.py --graph <alias>
open "index.html#data=data.js&play=1"
```

`export-graph.py` reads a loaded graph with `umx query` and `umx edge list` and writes `data.js` beside the page. `data.js` holds real graph text and is git-ignored. Keep it, and any screenshots of it, out of commits. Only the default `data.js` in this folder is git-ignored: if you pass `--out`, keep it inside this folder named `data.js`, or write somewhere that is never committed.

The graph needs three things:

| Need | Default | Flag |
| --- | --- | --- |
| band nodes: one type whose nodes are the steps along the spine | `segment` | `--band-type` |
| an order for them: a step number, or a start and end written as HH:MM | `step`, `starts_at`, `ends_at` | `--step-field`, `--start-field`, `--end-field` |
| placing links: relations from other nodes to a band node | `raised_in,part_of` | `--place-relations` |

The quadrants are worked out by dealing the graph's types into four even groups. That is rarely the best split. For a better one, write a small JSON file and pass it with `--families`:

```json
[
  {"name": "Options and mechanisms", "types": ["option", "mechanism"]},
  {"name": "Problems and questions", "types": ["problem", "question"]},
  {"name": "Decisions and lessons", "types": ["decision", "lesson"]},
  {"name": "People and sources", "types": ["person", "team", "document"]}
]
```

The dimension does not have to be clock time. Any type with a step number works as the bands, for example the phases of a method. Without HH:MM times the bands are numbered and all the same weight in time.

## Files

| File | What it is |
| --- | --- |
| `index.html` | the whole exemplar: page, styles and script |
| `sample-data.js` | invented sample graph, safe to commit |
| `make-sample.py` | writes `sample-data.js`; same seed, same file |
| `export-graph.py` | writes `data.js` from a loaded graph, read only |
| `data.js` | your export; git-ignored |

## Address switches

Add these after `#`, joined with `&`. They exist so a headless browser can take a still of any state.

| Switch | Effect |
| --- | --- |
| `data=<name>.js` | load this data file from the same folder |
| `play=1` | start the replay |
| `view=strip`, `funnel` or `flight` | start on this camera |
| `theme=light` | light skin |
| `u=0.4` | start 40% of the way along the spine |
| `links=0` | ribbons off |
| `still=1` | no camera moves; jump straight to the end state |
| `replay=0.45` | a paused replay at 45% |
| `lock=1` | with `replay`, show the lock-on for the last major node passed |
| `demo=3` | the agent demo, stopped at step 3 |
| `sim=47` | run 47 seconds of replay in one go, then carry on from there |

## What it does not show

- The product's renderer. This draws on a plain 2D canvas with its own perspective sums. It says nothing about speed in sigma or three.js, and no frame time was measured.
- Real agent control. The agent demo is scripted in the page. The `umx ui` verbs it prints are placeholders.
- Any dimension other than one ordered set of bands. No pivot on a second dimension, no parallel tracks drawn as a braid, no bands merging as you zoom out.
- Dates. Start and end must be HH:MM; a graph that spans days needs a step number.
- Large graphs. It was tried on graphs of about 200 and 400 nodes.

## Before any of this is first class

- The placement (which band, placed by data or by rule, off the axis) belongs in a umx service, so the CLI and the UI give the same answer. Here it is worked out in the page.
- The view would be drawn by the UI's own renderer and read its data through the UI's allow-listed `umx` path, not through an export file.
- Which types are major, how quadrants are chosen, and the skin are display choices. They stay in the UI, never in a graph's vocabulary.
- An agent's proposed placement must never look like data. Keep solid, hollow and the agent's own colour distinct.
- The replay's scan lines, dust and title cards are decoration for effect. In a working view, glow and motion should mean something.
