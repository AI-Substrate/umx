# Install umx and get started

For people and for agents. Every command here is exact: run it as written, setting only the values a section says
to set. Agents: install umx only when the person you work for asked for it; if you are not sure they did, ask them
first. Then run the install and the steps it prints. If `umx version` already works on this machine, ask the person
first: installing again restarts the running umx daemon, which other agents may be using.

## Install

One line installs the latest release (on Windows, use the PowerShell line in Windows (WSL 2), below):

    curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | sh

A pinned version (a leading `v` is optional; the flag wins over the variable):

    curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | sh -s -- --version 0.1.0
    curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | UMX_VERSION=0.1.0 sh

What the script does, in order:

1. Picks the release for this machine, or refuses (below). It needs `curl` and `sha256sum` or `shasum`.
2. Downloads `umx-<target>.tar.gz` and `SHA256SUMS`, over https only, into a private temporary folder.
3. Checks the archive's sha256 against its line in `SHA256SUMS` before opening it, and refuses an archive holding
   anything but `umx-<target>/umx`, `LICENSE` and `THIRD_PARTY_NOTICES`, or any link.
4. Checks the new `umx` runs here, then moves it to `~/.local/bin/umx`. Until this step nothing on the machine has
   changed, so a failure leaves any old umx as it was.
5. Gets the daemon running the new build: restarts a running daemon, starts a stopped daemon service installed for
   this umx, or else starts the daemon in the background (its log: `~/.umx/daemon.log`). Then it asks the daemon which
   build it runs, and says so (see Upgrade when that is another umx).
6. Says when `~/.local/bin` is not on your `PATH` and which line to add to your shell's startup file, or when another
   umx comes first on it.
7. Runs `umx doctor --human` and prints what to run next (After install).

It never uses sudo, never edits a shell startup file, and never installs the /umx skill or a daemon service: it prints
those commands for you or your agent to run.

Supported: macOS on Apple silicon, and Linux on x86_64 or arm64 with glibc 2.35 or newer (Ubuntu 22.04, Debian 12 or
newer), including Linux inside WSL 2 on Windows (below). Refused, with nothing changed: macOS on Intel, musl Linux
(Alpine), older glibc, WSL 1, other systems and other processors. On those, build from source:
[Install in the README](../README.md#install).

## Windows (WSL 2)

umx runs on Windows only inside WSL 2, as Linux: there is no Windows build. In PowerShell (Windows PowerShell 5.1 or
PowerShell 7, not as administrator), one line installs the latest release into your default WSL distro:

    irm https://github.com/AI-Substrate/umx/releases/latest/download/install.ps1 | iex

A pinned version (a leading `v` is optional; `$env:UMX_VERSION = '0.1.0'` before the line above also works, and stays
set in that window):

    & ([scriptblock]::Create((irm https://github.com/AI-Substrate/umx/releases/latest/download/install.ps1))) -Version 0.1.0

If umx may already be installed, check first, and ask the person before installing again (Install, above):
`wsl ~/.local/bin/umx version --human`.

What `install.ps1` does, in order:

1. Checks a named version as install.sh does, before anything runs.
2. Finds the default WSL distro with `wsl -l -v`, and refuses, with nothing changed, when WSL is not installed, there
   is no distro, or the default distro is WSL 1.
3. In that distro, has WSL's own `curl` (https only) find the release, the version you named or the tag
   `/releases/latest` leads to, and download that release's `install.sh` and `SHA256SUMS` into a temporary folder. It
   runs `install.sh --version <that version>` only when `install.sh` matches its line in `SHA256SUMS`, and otherwise
   stops with nothing changed. That is the Install line in a WSL terminal, with `install.sh` checked first, so
   everything under Install happens inside WSL, from one release even if another is published meanwhile. Windows
   itself downloads and runs nothing else, and the version reaches WSL as an argument, never as shell text.
4. Shows install.sh's output as it runs, and ends with install.sh's exit code. Run with `irm ... | iex`, it leaves the
   PowerShell window open, sets `$LASTEXITCODE` and nothing else in that session, and ends with an error when the
   install failed.

It needs no administrator rights, changes no execution policy, and installs nothing on Windows.

Set up WSL 2 first if `wsl -l -v` lists no distro marked `*` with VERSION 2: in PowerShell run as administrator,
`wsl --install` installs WSL 2 and Ubuntu (restart Windows if it asks, then open Ubuntu once to make your Linux user).
Convert a WSL 1 distro with `wsl --set-version <distro> 2`, or make a WSL 2 distro the default with
`wsl --set-default <distro>`. The distro needs glibc 2.35 or newer: Ubuntu 22.04 or newer.

Then use umx inside WSL: open a WSL terminal (`wsl`, or Ubuntu from the Start menu) and run `umx` there, starting
with each line install.sh printed under `Run these next` (After install). umx is not on Windows' `PATH`. If a new
WSL terminal says `umx` is not found, add the `PATH` line install.sh printed. The daemon service
(`umx daemon install`) needs systemd in WSL: if it fails, set `systemd=true` under `[boot]` in `/etc/wsl.conf`, run
`wsl --shutdown` in PowerShell, and open the terminal again. WSL can stop a distro, and a daemon started in it,
after its last terminal closes: when `umx version` says the daemon is down, run what `umx doctor` names.

To upgrade, run the PowerShell line again; Upgrade and Uninstall below apply inside WSL. To check provenance, do
Verify and install by hand inside WSL, with target `x86_64-unknown-linux-gnu` (or `aarch64-unknown-linux-gnu` on an
Arm PC). In a WSL terminal you can also run the curl Install line directly; `install.ps1` only runs it for you.

`irm ... | iex` runs `install.ps1` as it downloads it, before anything is checked: it rests on https and on the
release's files being immutable once published. To check `install.ps1` first, download it and the release's
`SHA256SUMS` into a new folder, then check `SHA256SUMS`' build attestation with the GitHub CLI (`gh`, signed in), as
Verify and install by hand does for the archive. In PowerShell:

    $version = '0.1.0'
    $base = "https://github.com/AI-Substrate/umx/releases/download/v$version"
    Set-Location (New-Item -ItemType Directory (Join-Path $env:TEMP "umx-$(Get-Random)"))
    Invoke-WebRequest -UseBasicParsing "$base/install.ps1" -OutFile install.ps1
    Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS" -OutFile SHA256SUMS
    gh attestation verify SHA256SUMS --repo AI-Substrate/umx
    (Get-FileHash install.ps1).Hash; Select-String '  install\.ps1$' SHA256SUMS

Go on only if `gh` says the attestation verified and the two hashes printed last are the same (letter case aside);
`gh release verify-asset v$version install.ps1 --repo AI-Substrate/umx` is another check. Then run the file you
checked, which needs no change to the execution policy:

    & ([scriptblock]::Create((Get-Content -Raw .\install.ps1))) -Version $version

That proves the `install.ps1` you ran is the one this repository's release workflow published. What it runs inside
WSL is checked against that release's `SHA256SUMS`, downloaded again inside WSL: `install.sh` by `install.ps1`, then
the archive by `install.sh` (What the one line checks, below). That proves integrity, not who built it; to know who
built the umx you run, Verify and install by hand inside WSL.

## What the one line checks

The script downloads the archive for this machine and the release's `SHA256SUMS` when it runs, and checks the
archive against its line in that `SHA256SUMS` before opening it. That proves integrity: the file is the one published
on the release. It does not prove who built it: the script checks no attestation. Checking `SHA256SUMS` or
`install.sh` yourself first does not change that, because the script downloads `SHA256SUMS` again and checks the
archive against that copy.

## Verify and install by hand

To know who built the umx you run, check the archive's build attestation and install that same file. You need the
GitHub CLI (`gh`, signed in). Set the version and this machine's target: `aarch64-apple-darwin` (macOS on Apple
silicon), `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu` (Linux, glibc 2.35 or newer):

    version=0.1.0
    target=aarch64-apple-darwin

Then paste this as one command. It works in a new temporary folder, and each step runs only when the one before it
worked, so a failed download or check stops it before anything is installed:

    archive=umx-$target.tar.gz &&
    base=https://github.com/AI-Substrate/umx/releases/download/v$version &&
    dir=$(mktemp -d) &&
    cd "$dir" &&
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSLO "$base/$archive" &&
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSLO "$base/SHA256SUMS" &&
    gh attestation verify "$archive" --repo AI-Substrate/umx &&
    awk -v f="$archive" '$2 == f' SHA256SUMS | shasum -a 256 -c &&
    tar -xzf "$archive" "umx-$target/umx" &&
    mkdir -p ~/.local/bin &&
    install -m 0755 "umx-$target/umx" ~/.local/bin/umx &&
    ~/.local/bin/umx version --human

On Linux, use `sha256sum -c` in place of `shasum -a 256 -c`. `gh attestation verify` proves this repository's release
workflow built the archive; the `SHA256SUMS` line proves it is the one published for that version. The last line
prints the version just installed.

Then do what the one line does after it installs. If `~/.local/bin` is not on your `PATH`, add
`export PATH="$HOME/.local/bin:$PATH"` to the end of your shell's startup file (`~/.zshrc`, `~/.bashrc`; on fish,
`fish_add_path $HOME/.local/bin` in `~/.config/fish/config.fish`) and open a new terminal; until then, use the full
path as below. Get the daemon running the new build. `umx daemon start` runs in the foreground until stopped, so:

    ~/.local/bin/umx daemon status --human     # running: true or false

- `running: true` (an upgrade): `~/.local/bin/umx daemon restart`.
- `running: false`: `~/.local/bin/umx daemon install` installs the per-user service, which starts now and at every
  login. Or, with no service, start it in the background the way the script does (its log: `~/.umx/daemon.log`; with
  `UMX_HOME` set, use that folder in place of `~/.umx`):

      (umask 077 && mkdir -p ~/.umx) && nohup ~/.local/bin/umx daemon start </dev/null >>~/.umx/daemon.log 2>&1 &

Then check it and finish setup:

    ~/.local/bin/umx version --human     # cli and daemon: the same version and commit
    ~/.local/bin/umx doctor --human      # run each command it names, as printed (After install, below)

When the daemon reports another build, or `umx daemon install` refuses with `service_conflict`, the daemon or its
service runs another umx binary: see Upgrade.

## After install

The script ends with `Run these next`. Run each line it prints. Later, `umx doctor` prints the same:

- `umx daemon install`: installs the daemon as a per-user service (launchd on macOS, systemd on Linux) that starts at
  login and comes back if it crashes. Printed only when no service is installed.
- `npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*' -y`: installs the /umx skill for every
  agent harness, pinned to the commit this umx was built from (it needs Node.js with `npx`). Run the line as printed,
  never one you typed: an unpinned line installs a skill that may not match this umx.
- `umx agents-start-here`: what umx is and how agents use it. Agents: `export UMX_OUTPUT=json` first, read it, and
  follow it.

Check it worked:

    umx version --human          # cli and daemon: the same version and commit
    umx doctor --human           # fix anything it reports, with the command it names

## Upgrade

Run the install line again. A running daemon is restarted on the new build; your loaded graphs come back by
themselves. Then run what it prints: when the new umx carries a changed /umx skill, `umx doctor` reports yours out of
date and prints the new skill line.

When the daemon runs another umx binary (one built with `just dev` into `~/.cargo/bin`, say), or the daemon service
was installed for another umx binary or `UMX_HOME`, a restart keeps running that one. The script then prints a note
naming the build the daemon runs, and the commands that switch it to the new one, from:

    ~/.local/bin/umx daemon stop
    ~/.local/bin/umx daemon uninstall      # only when the service runs another umx or UMX_HOME
    ~/.local/bin/umx daemon install

It also says when another umx comes before `~/.local/bin` on your `PATH`: remove that one, or put `~/.local/bin`
first.

## Uninstall

    umx daemon uninstall                       # stops the service and removes it; UMX_HOME and the log stay
    umx daemon stop                            # a daemon started by hand; daemon_not_running means it is down
    npx skills@1.7.6 remove umx -g             # the /umx skill, from every agent harness; asks first
    rm ~/.local/bin/umx

Your graphs are kept. A graph is a folder you made (with `umx graph new`) and it stays where it is until you delete
it. umx's own state stays too: `~/.umx` (or `UMX_HOME`: config, the list of loaded graphs, vectors of graphs that are
not fully open, the daemon log) and downloaded embedding models in `~/.cache/umx/models` (or `$XDG_CACHE_HOME/umx/models`). Delete those folders only if
you want that state gone; nothing else removes them. `umx docs get install` (Where files live) lists every file.

## The graph view

`umx ui` needs a source build: a release has no graph view, and `umx ui` refuses with `ui_dir_refused`. Build the
view from the commit `umx version` reports, not the default branch (the view and the daemon must speak the same
protocol). It needs Node.js 20.9 or later and pnpm:

    git clone https://github.com/AI-Substrate/umx.git ~/src/umx
    git -C ~/src/umx checkout <commit>              # the commit umx version reports
    pnpm --dir ~/src/umx/apps/ui install --frozen-lockfile && pnpm --dir ~/src/umx/apps/ui build
    umx config set ui.dir ~/src/umx/apps/ui
    umx ui

After an upgrade, check out the new commit and build again. More: `umx docs get ui` (In a binary install).

## Build from source

For a machine no release fits, or to work on umx: [Install in the README](../README.md#install). It needs Rust,
just, a C and C++ compiler, cmake, Node.js and 20 GB of free disk, and `just dev` installs umx into `~/.cargo/bin`.
Keep one install: if `~/.local/bin/umx` is also there, the one first on your `PATH` is the one that runs.
