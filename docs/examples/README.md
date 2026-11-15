# Examples

Proofs of concept we want to keep and come back to, before anything in them becomes a first-class part of the project.

An example shows that an idea is possible and what it looks like. It is not a feature, it is not supported, and nothing in the product may import it.

## What is here

| Folder | What it shows |
| --- | --- |
| [visualisation/time-spine-replay](visualisation/time-spine-replay/README.md) | A graph projected along time: a spine with the other nodes hanging off it in soft 3D, three cameras, and a self-playing replay of a meeting as it happened |

How these were made, so the next one can be made the same way: [visualisation/how-to-make-a-visual-poc.md](visualisation/how-to-make-a-visual-poc.md).

## Rules for this folder

1. **No real graph text in git.** An example ships with invented sample data. Real exports are written to a git-ignored file beside the example and stay on your machine. Screenshots of real graphs stay out of git too.
2. **Self-contained.** One page, plain files, no libraries, no build step, no network requests, no remote fonts or images. Open the file and it runs.
3. **Read only.** An example reads a graph through the `umx` CLI. It never opens a graph folder and never writes to a graph.
4. **Text is text.** Graph text is drawn as canvas text or set as plain text, never as HTML.
5. **Not product code.** Nothing under `apps/` or `crates/` depends on anything here, and the project's checks do not run these files.
6. **Every example has a README** that says what it shows, how to run it, how to point it at your own graph, what it does not show, and what would have to change before it could be first class.
