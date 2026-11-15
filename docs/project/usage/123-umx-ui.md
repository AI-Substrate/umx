---
id: 123
title: umx ui starts the graph view and opens the browser; the link's token never goes to JSON
route: explore
status: current
source: workstream 009 step S2, plan 095 (architect rulings and security yes, 2026-11-11)
reqs: [req-104, req-105]
supersedes: none
---
## Do

Run `umx ui` when a person wants to look at a graph. It checks the daemon is up, starts the view
(apps/ui) on 127.0.0.1 with a fresh token, opens their default browser at it, and stays in the foreground
until Ctrl-C. Run it in a shell of its own (a tmux window, or an async shell), because it does not return
while the view is up.

- `umx ui --graph <alias>` opens that graph first. A secured graph opens only when named this way.
- `umx ui --port <n>` binds a fixed port instead of a free one.
- `umx ui --no-open --human` prints the link instead of opening the browser, for a person to click.
- With JSON output, read `data.url` (no token), `data.port`, `data.graph` and `data.opened` from the one
  envelope printed when the view is ready.

## Don't

- Don't run `pnpm start` or `node bin/serve.mjs` yourself to show a graph: `umx ui` is the one way.
- Don't ask for the link in JSON: `umx ui --no-open` with JSON output is refused (`usage_invalid`), so the
  token never lands in an agent transcript. Never copy a printed link into a message, file or log.
- Don't set `ui.dir` or `ui.node` in a repo's `.umx/config.toml`: they are machine only and a repo value is
  ignored (`config_repo_ignored`).
- Don't treat `ui_not_built` as a bug: build the view once with the pnpm command its message gives
  (`pnpm --dir <dir> install --frozen-lockfile && pnpm --dir <dir> build`), then rerun.

## Example

    umx ui --graph acme
    umx ui --no-open --human
    umx docs get ui

## Why

The operator and agents should never type pnpm commands to see a graph (req-104). The token goes to the server's
stdin and the browser only, never argv, env, JSON or a log, because other local users can read argv and
agent transcripts are kept (security, ws009 S2). The server's stdin stays open while umx runs, so Ctrl-C or
any end of umx stops the whole view, with no node left behind.
