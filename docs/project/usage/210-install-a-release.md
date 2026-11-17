---
id: 210
title: To install umx, run the release install line from docs/install.md, then run what it prints
route: start
status: current
source: D76, D76a Q10, Q13 and D76b (2026-11-17), workstream 012 s11
reqs: [req-70, req-85, req-86]
supersedes: none
---
## Do

When umx is not installed, there is no umx checkout to build, and the person you work for asked for umx, read
`https://github.com/AI-Substrate/umx/blob/main/docs/install.md` and follow it. Install the latest release with one line:

    curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | sh

It puts `umx` in `~/.local/bin` and gets the daemon running the new build, then ends with `Run these next`. Run each
line it prints, as printed: `umx daemon install` and the pinned /umx skill line (usage 207), then
`umx agents-start-here`. If it says `~/.local/bin` is not on `PATH`, tell the person the line it names for their
shell's startup file. To upgrade, run the install line again; when it prints a note that the daemon runs another umx,
run the commands under it. On Windows, run it inside WSL 2.

## Don't

Don't run the install line without asking when `umx version` already works: it restarts the running daemon, which
other agents may be using. Don't use sudo, and don't edit a shell startup file yourself. Don't retry on a machine it
refuses (macOS on Intel, Alpine, glibc older than 2.35, WSL 1): build from source (README, Install). Don't take the
`SHA256SUMS` check as proof of who built umx: for that, verify and install by hand (docs/install.md), which checks
the archive's attestation and installs that same file.

## Example

    curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | sh
    umx daemon install
    npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*' -y
    export UMX_OUTPUT=json
    umx agents-start-here

## Why

The installation flow is how most people and agents meet umx (D76), so the README's first lines send agents to one
page, docs/install.md (D76a Q13). install.sh installs and starts the daemon but only prints the service and skill
steps, for agents to run (D76a Q10). Windows is WSL 2 only (D76b). Releases ship the binary only, so the graph view
is a source build (usage 207).
