# Versioning and releases

umx uses [semantic versioning](https://semver.org): `MAJOR.MINOR.PATCH`. One version covers the whole repository:
the `umx` binary, its daemon, its built-in docs and the /umx skill. Every crate in the workspace carries it.

## While umx is 0.x

umx starts at 0.1.0. While the major is 0:

- a **breaking** change bumps the minor: 0.1.4 to 0.2.0;
- **anything else** bumps the patch: 0.1.4 to 0.1.5.

1.0.0 comes only when Jordan says so. From then on, breaking bumps the major, a new feature the minor, a fix the
patch.

## What is breaking

A change is breaking when something that worked with the previous release stops working, or works differently,
without the caller changing anything. Surface by surface:

| Surface | Breaking (minor while 0.x) | Not breaking (patch while 0.x) |
|---|---|---|
| CLI commands, flags and exit codes | A command or flag removed or renamed; a flag's meaning or default changed; an exit code's meaning changed; a command that used to succeed now refuses the same input | A new command or flag; a new value a flag accepts; a clearer message |
| JSON replies | A field removed or renamed, or its meaning or type changed, in the envelope (`umx`, `command`, `status`, `data`, `warnings`, `hints`, `errors`, `untrusted`) or in any command's `data`; a code removed or renamed; a warning becoming an error | A new field; a new warning, hint or error code; new candidates; a reworded `message` or `next` |
| Daemon wire `PROTOCOL` (crates/umx-protocol) | Any `PROTOCOL` bump: the daemon refuses a client on another `PROTOCOL` (`protocol_mismatch`), so a CLI or UI from the previous release stops working against the new daemon until it is restarted on one build | A change that leaves `PROTOCOL` as it is |
| Graph files on disk (`graph.json`, `nodes.jsonl`, `edges.jsonl`, `similar/`) | A change the previous release cannot read: a new graph format (`FORMAT_VERSION` in umx-core) that older umx refuses, even if only graphs using the new feature get it; a field's meaning changed | A change older umx still reads as before |
| Config and environment variables | A setting or variable removed or renamed; its meaning or default changed; a file moved | A new setting or variable |
| The /umx skill (`skills/umx/`) | A route removed or renamed, or a route now telling agents to do something the previous release cannot | New routes; clearer wording |

Built-in docs (`umx docs`, `umx agents-start-here`) follow whatever they describe: a docs change on its own is
never breaking.

`PROTOCOL` and the graph format have their own numbers. They are not the release version: `PROTOCOL` moves on
every wire change and the graph format only when a graph needs something new. A release whose commits bumped
either is a breaking release.

## Commit subjects

Every commit subject is a [Conventional Commit](https://www.conventionalcommits.org). The prime merges by fast
forward, so subjects land on main as written, and the release tool reads them.

| Subject | Bump while 0.x | Changelog section |
|---|---|---|
| `feat!:`, `fix!:` (any type with `!`), or a `BREAKING CHANGE:` footer | minor | the type's section, marked breaking |
| `feat:` | patch | Features |
| `fix:` | patch | Bug Fixes |
| `perf:` | patch | Performance |
| `docs:` | patch | Documentation |
| `revert:` | patch | Reverts |
| `chore:`, `refactor:`, `test:`, `build:`, `ci:`, `style:` | none on its own | hidden |

A release opens only when at least one commit since the last release lands in a visible section, so a run of
`chore:` commits alone makes no release. Write `feat!:` (or the footer) whenever the table above calls the change
breaking.

## How a release is cut

1. [release-please](https://github.com/googleapis/release-please) runs on main (its workflow is ws012 s8). It reads
   the commit subjects since the last release and keeps one release PR open, titled `chore: release <version>`.
2. That PR sets `version` in `[workspace.package]` in `Cargo.toml`, every workspace crate's entry in `Cargo.lock`,
   `umx-version` in `skills/umx/SKILL.md` and `.release-please-manifest.json`, and adds the release's section to
   `CHANGELOG.md`. Every crate inherits the workspace version, so `cargo build --locked` passes on it.
3. The prime merges it. release-please then tags the merge commit `v<version>` (for example `v0.1.0`, no component
   in the tag) and makes the GitHub release of that name as a draft; the release workflow attaches the binaries and
   publishes it (Releases, below).

The configuration is `release-please-config.json` (the `simple` strategy with the files above as extra files) and
`.release-please-manifest.json` (the last released version). The first release was pinned to exactly 0.1.0 with
`release-as`, removed once 0.1.0 shipped, so each release now takes its version from the commits since the last one.
`bootstrap-sha` stops release-please reading the history before Conventional Commits began.

## Releases

`.github/workflows/release.yml` runs on every push to main. It always builds `umx` for the three release targets,
each on a GitHub-hosted runner of its own architecture, never emulated or cross built, Linux on Ubuntu 22.04. Each
build runs the binary once (version and commit, daemon start and stop, a graph write and read, a docs topic) and
packages it. A manual run (`gh workflow run release.yml`) builds the same way and never publishes.

When the push is a merged release PR, the same run publishes:

1. release-please tags the commit and makes the release as a draft, so nobody sees it before its files are there.
2. The three builds use that tagged commit.
3. `publish` checks each archive against the sha256 its build recorded and writes `SHA256SUMS`. It attests every
   archive and `SHA256SUMS` (GitHub build provenance), uploads them to the draft, downloads them back and checks
   them, and only then publishes the release as latest.

If any build fails, nothing is published: the release stays a draft, the previous release stays latest, and
re-running the failed jobs finishes it.

Each release holds these files. The names carry no version, so `releases/latest/download/<file>` always works.

| File | What it is |
|---|---|
| `umx-aarch64-apple-darwin.tar.gz` | macOS on Apple silicon |
| `umx-x86_64-unknown-linux-gnu.tar.gz` | Linux x86-64 (glibc 2.35 or later) |
| `umx-aarch64-unknown-linux-gnu.tar.gz` | Linux arm64 (glibc 2.35 or later) |
| `install.sh`, `install.ps1` | the install scripts: `install.ps1` runs `install.sh` inside WSL 2 on Windows |
| `SHA256SUMS` | the sha256 of every other file, in `sha256sum` format |

Each archive holds one folder, `umx-<target>/`, with `umx`, `LICENSE` and `THIRD_PARTY_NOTICES`. The binaries are not
signed.

To check a download (on Linux, `sha256sum -c` in place of `shasum -a 256 -c`):

    base=https://github.com/AI-Substrate/umx/releases/latest/download
    curl -fsSLO "$base/umx-aarch64-apple-darwin.tar.gz" -O "$base/SHA256SUMS"
    grep ' umx-aarch64-apple-darwin.tar.gz$' SHA256SUMS | shasum -a 256 -c
    gh attestation verify umx-aarch64-apple-darwin.tar.gz --repo AI-Substrate/umx --signer-workflow AI-Substrate/umx/.github/workflows/release.yml

The checks prove different things:

- **The sha256** proves the file arrived intact. It does not prove where the file came from, because `SHA256SUMS` comes
  from the same release.
- **The attestation** proves this repository's release workflow built the file, and names the commit it built.

## What `umx version` reports

`umx version` reports `data.cli.version`, the release version (Cargo's package version, set by the release PR), and
`data.cli.commit`, the full 40-character commit the binary was built from. A build of tag `v0.1.0` reports `0.1.0`
and that tag's commit. A build between releases reports the last release's version and its own commit. With the
daemon up, `data.daemon` reports the daemon's build the same way.
