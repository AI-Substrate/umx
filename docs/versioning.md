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
   in the tag) and makes the GitHub release of that name.

The configuration is `release-please-config.json` (the `simple` strategy with the files above as extra files) and
`.release-please-manifest.json` (the last released version). It pins the first release to exactly 0.1.0 with
`release-as`; s8 removes `release-as` after that release, or every later release PR would ask for 0.1.0 again.
`bootstrap-sha` stops release-please reading the history before Conventional Commits began.

## What `umx version` reports

`umx version` reports `data.cli.version`, the release version (Cargo's package version, set by the release PR), and
`data.cli.commit`, the full 40-character commit the binary was built from. A build of tag `v0.1.0` reports `0.1.0`
and that tag's commit. A build between releases reports the last release's version and its own commit. With the
daemon up, `data.daemon` reports the daemon's build the same way.
