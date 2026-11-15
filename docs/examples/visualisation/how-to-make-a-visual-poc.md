# How to make a visualisation proof of concept

The steps used to make [time-spine-replay](time-spine-replay/README.md), written down so a person or an agent can do the same for another graph, another dimension or another kind of picture.

The aim of a proof of concept is to answer one question cheaply: does showing the graph this way tell you something? Most of the work is in the data, not the drawing.

## The short version

1. Start from a question and a real graph, not from a picture.
2. Profile the graph, read only, and count what is there.
3. Find the dimension, and measure how much of the graph it covers.
4. Prove the insight as a table before drawing anything.
5. Work out placements once, apart from any camera.
6. Export a snapshot. Real text never goes into git.
7. Build one self-contained page.
8. Look at your own output, fix it, look again.
9. Write down what it showed and what it did not.
10. Make an invented sample and file it under `docs/examples`.

## 1. Start from a question and a real graph

Write the question in one line. For the time spine it was: "can I see a meeting as it happened, and where the ideas, problems and decisions arrived?" Pick a loaded graph that should be able to answer it.

```
umx graph list
umx graph guide <alias>
```

## 2. Profile the graph

Count before you design. All of this is read only.

```
umx node list --graph <alias> --limit 10000        # types and statuses: count them
umx graph guide <alias>                             # relations with counts, fields per type
umx node get <id> --graph <alias>                   # one full node of each interesting type
```

Look for:

- which types are many and which are few;
- which relations point many nodes at a few nodes (those few are candidate buckets);
- which fields hold dates, times or step numbers, and on how many nodes;
- what the evidence looks like, since a position in a transcript is a fine-grained order.

Two things found this way shaped the time spine. Almost no node had a date, but half were linked to a segment by `raised_in`, and the segments had start times and step numbers. And `created` was no use as a time, because every node had been written in the same two hours.

## 3. Find the dimension and measure its cover

A dimension is a rule that puts each node in a bucket. To run along a spine, the buckets must have an order. A dimension can be a field on the node, a link to a bucket node, an order link such as `precedes`, or a position in the evidence.

For the dimension you choose, count three groups. Put the numbers on the screen later.

| Group | Meaning |
| --- | --- |
| placed by data | a field or a link on the node says where it goes |
| placed by rule | nothing says so, but its neighbours are placed, so it sits with them |
| off the axis | the dimension says nothing about it |

If few nodes are placed by data, the picture will be mostly guesswork. Say so, or pick another dimension.

## 4. Prove it as a table first

Run the projection as a grouped count. If the table tells a story, a picture is worth making. If it does not, a picture will not rescue it.

```
umx query "MATCH (x)-[:raised_in]->(s:segment)
           RETURN s.title AS band, s.starts_at AS at, x.type AS kind, count(*) AS total
           ORDER BY at, total DESC" --graph <alias> --limit 500
```

Two things to know. A column name may not be the same as a variable name: `RETURN n.type AS n` is refused with `query_alias_shadows_variable`, so name the node `x` and the columns something else. And a plain query hides empty buckets and nodes with no bucket, so count those apart.

## 5. Work out placements once, apart from the camera

Give every node the same few numbers, whatever view will draw it:

- **along**: how far along the spine, from 0 to 1;
- **around**: which group it belongs to, as an angle;
- **out**: how far from the spine, for example major nodes on the surface;
- **placed by**: data, rule, or off the axis.

Every view is then just a camera on those numbers. The flat strip, the funnel and the flight view in the exemplar share one set of placements, which is why moving between them is a single camera move and why hover, highlight and replay work the same in each.

## 6. Export a snapshot

Read the graph through the CLI and write one data file beside the page. Do not open graph folders. `time-spine-replay/export-graph.py` is the pattern: one `umx query` for the nodes with the few fields the view needs, one `umx edge list` for the links.

The export holds real graph text. Git-ignore it. Do not paste it, or screenshots of it, anywhere it could travel.

## 7. Build one self-contained page

One HTML file with its styles and script, and the data in a second file loaded with a script tag so it opens straight from disk. No libraries, no build step, no network. Draw on a 2D canvas. A few hundred nodes need nothing more, and doing the perspective sums yourself is about twenty lines.

Habits that paid off:

- **Text is text.** Canvas text, or `textContent`. Never set HTML from graph text.
- **Colours as a handful of variables**, so a second skin is a second set of values.
- **Three levels of brightness**: data brightest, structure in the middle, chrome dimmest.
- **A cap on labels.** About twelve on screen, placed so they do not overlap, and the rest on hover.
- **Honest marks.** Solid for placed by data, hollow for placed by rule, a different colour again for anything an agent proposes.
- **Real readouts only.** Every number on screen is counted from the data.
- **Motion is finite, glow means something.** Every animation ends (and stops under reduced motion); glow only for selection, an agent's highlight or a live change.
- **Switches in the address.** `#view=...`, `#theme=...`, `#still=1` and the like, so any state can be opened directly. This is what makes step 8 possible.

## 8. Look at your own output

You cannot judge a picture you have not seen. Take stills with a headless browser and read them.

```
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu --hide-scrollbars \
  --window-size=1600,1000 --virtual-time-budget=4000 \
  --screenshot=shot.png "file:///path/to/index.html#view=funnel&still=1"
```

Two switches make this work for anything that moves:

- `still=1` skips camera moves and jumps to the end state, so a still is the settled view.
- `sim=<seconds>` runs that much of an animation in one go before the first frame. A headless browser does not run animation frames reliably, so without this you cannot check a replay at all.

Add `--enable-logging=stderr` and search the output for `Uncaught` to catch script errors. Then go round: framing, overlaps, labels, the light skin, each state of each mode. Expect three or four rounds.

Be straight about what this proves. Stills show layout and correctness. They do not show pacing, smoothness or frame rate. Someone has to watch it move.

## 9. Write down what it showed

In the example's README, in plain words:

- what it shows and how to run it;
- what it does not show (the real renderer, real data paths, scale, the things you faked);
- what would have to change before it could be first class.

Keep the design note that led to it, with the rulings and open questions, wherever your project keeps such notes.

## 10. Make it an example

To file it under `docs/examples`:

1. Take out everything specific to the one graph: type names, band titles, counts written by hand. Read them from the data file or a small config block in it.
2. Write a generator for an invented sample, and commit the sample so the page runs with no graph.
3. Run the page on the sample and again on a real export, to be sure both paths work.
4. Git-ignore the real export and any stills.
5. Follow [the rules for examples](../README.md).
