# One gate, the same locally and in CI (blueprint 001, c-check).
# Every recipe that builds runs its cargo through scripts/cargo-slot (decision D41, 2026-11-10): two build slots
# shared by concurrent builds, and slot 2 is usually reserved (D59), so in practice one build runs at a time.
# Waiters queue first come, first served; UMX_CARGO_PRIORITY=gate goes ahead of normal runs and =urgent ahead of
# gate ones (F87). Niced, 4 jobs, with a shared sccache cache where installed, no incremental dirs and
# line-tables-only debug info. It refuses to build below 20 GB free (exit 75; the floor is 0 on CI). Run `just check`
# as is; no lock, no taskpolicy. Outside just, call scripts/cargo-slot <cargo args> for any cargo.
#
# Docs are part of done (ws002 s14): `test` fails while a flag the CLI parses (crates/umx-cli/src/parse.rs FLAGS) is
# missing from the topic its `umx <cmd> --help` prints, while agents-start-here does not link every topic or links a
# topic that does not exist, and while a docs/project/usage note breaks its README's format or cites a req the PRD
# lacks. A PR that changes how an agent uses umx also carries a usage note cited in its skill route; the PR
# template's `Docs:` line lists what was touched.

# Build, lint, test and prove the architecture rules.
check: slots home-paths customer-names private-refs install-sh release-assets fmt build clippy test arch walkthrough bakeoff

# scripts/cargo-slot's guarantees (two slots, TERM and INT stop the build before the slot frees, exit codes), with a
# fake cargo in a private slot folder.
slots:
    scripts/cargo-slot-test

# No tracked file holds an absolute home path (F184): the self-test proves the check fires, then the check runs on
# this checkout.
home-paths:
    scripts/home-paths-test
    scripts/home-paths

# No tracked file holds a customer name (D72), read from the names list outside the repo; with no list (CI) the check
# skips. The self-test proves it fires with a planted name of its own.
customer-names:
    scripts/customer-names-test
    scripts/customer-names

# No shipped file cites a private brief or harness file (D72g); the self-test proves the check fires.
private-refs:
    scripts/private-refs-test
    scripts/private-refs

# install.sh on a fake release with no network (fresh install, upgrade, pinned version, tampered or unsafe archives,
# unsupported machines, the PATH hint), under sh and under dash when it is installed; then shellcheck over both scripts,
# skipped with a note when shellcheck is not installed.
install-sh:
    scripts/install-sh-test
    if command -v shellcheck >/dev/null; then shellcheck install.sh scripts/install-sh-test; else echo "install-sh: shellcheck is not installed; skipped"; fi

# The release archives, SHA256SUMS and smoke run's argument checks (ws012 s7, s8), on a stand-in binary: no build.
release-assets:
    scripts/release-assets-test

# The seed export, the seed gate and its range and pre-push modes prove themselves on fixtures and planted controls
# (needs ddocs and a built umx-core example, not the names list). Not part of `check`: run it when any of them changes.
seed-tests:
    scripts/seed-export-test
    scripts/seed-gate-test
    scripts/seed-gate-range-test

# Installs the pre-push hook that runs `scripts/seed-gate --pre-push` on what each push adds.
pre-push-hook:
    scripts/install-pre-push-hook

fmt:
    cargo fmt --all --check

build:
    scripts/cargo-slot build --workspace --all-targets --locked

# Every feature but those r-gate-features excludes (they need a toolkit this machine may lack), so stub-only code is
# linted too (E-3). umx-arch prints the list from rules.toml and fails the gate on any feature in neither list.
clippy:
    scripts/cargo-slot clippy --workspace --all-targets --features "$(scripts/cargo-slot run -q -p umx-arch --locked -- gate-features)" --locked -- -D warnings

# Every crate's tests except umx-arch, which `arch` runs with its table printed; then the compose wiring
# test again with the fakes built, so the stub rows of the adapter matrix are checked (E-1, E-3).
test:
    scripts/cargo-slot test --workspace --exclude umx-arch --features umx/test-driver --locked
    scripts/cargo-slot test -p umx-compose --features stub --locked --test compose

# The req-39 agent walkthrough on the stub build, with the network denied by the OS (req-51).
walkthrough:
    scripts/cargo-slot test -p umx --features stub --locked --test walkthrough

# The rule checks over this workspace and the self-test over the planted fixtures.
arch:
    scripts/cargo-slot test -p umx-arch --locked -- --nocapture

# The embed-bakeoff runners stop at the first failed stage and leave no stale result (fake commands, no models).
bakeoff:
    python3 tools/embed-bakeoff/test_runners.py

# The local embedder on the real embeddinggemma-2 GGUF (not in `check`: it needs the 558 MB file, which CI does not
# have): parity with the s03 reference and batch invariance on the GPU and the CPU, and the contract. Prints the commit
# and the load; fails if the file is absent, never skips.
test-llamacpp-real:
    #!/usr/bin/env bash
    set -euo pipefail
    gguf="$HOME/.cache/umx/models/ggml-org--embeddinggemma-2-GGUF/6ea3e3a309d12a4779cfda98b8c19800d2d5da71/embeddinggemma-2-BF16.gguf"
    [ -f "$gguf" ] || { echo "test-llamacpp-real: $gguf is missing; run tools/embed-bakeoff/fetch.sh gguf" >&2; exit 1; }
    echo "commit $(git rev-parse --short HEAD)$(git diff --quiet HEAD || echo -dirty), load $(uptime | sed 's/.*load averages*: //')"
    scripts/cargo-slot test -p umx-adapter-llamacpp --locked --test gemma -- --nocapture --test-threads=1 2>&1 \
        | grep -E "min cosine|batch invariance|^test |test result|skipped"

# The excluded cuda feature (ws004 s19), on Linux with the CUDA toolkit; not in `check`, which runs on Macs. Lints
# with it, then runs the local embedder's tests (the real-model ones need the GGUF and run on the GPU when there is
# one). Set CMAKE_CUDA_ARCHITECTURES (89 for an RTX 4090) to compile for one card only; it is optional.
check-cuda:
    scripts/cargo-slot clippy -p umx-adapter-llamacpp -p umx --all-targets --features umx-adapter-llamacpp/cuda,umx/cuda --locked -- -D warnings
    scripts/cargo-slot test -p umx-adapter-llamacpp --features cuda --locked -- --nocapture --test-threads=1

# Print blueprint 001's d-4 layer table from crates/umx-arch/rules.toml.
render:
    scripts/cargo-slot run -q -p umx-arch --locked -- render

# Rebuild, restart the daemon and reinstall umx in one command (req-42). The daemon runs in the
# background, or as the service if `umx daemon install` set one up (req-86), and logs to $UMX_HOME/daemon.log;
# fails unless the new build answers.
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    build="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    UMX_BUILD_ID="$build" scripts/cargo-slot build -q -p umx
    scripts/dev-restart.sh target/debug/umx "${CARGO_HOME:-$HOME/.cargo}/bin" "$build"

# Benchmarks (req-43, D8: they report, never gate; not part of `check`). Builds release in a cargo slot, then
# measures under `nice -n 19` only (never taskpolicy -b: it slows file I/O about 50x and distorts the numbers).
# Writes docs/project/benchmarks/<date>-<HHMMSS>-<commit>-<nodes>n.json and .md. Extra flags pass through, for example
# `just bench --nodes 100000`.
bench *flags:
    scripts/cargo-slot build -q --release --locked -p umx -p umx-bench
    nice -n 19 target/release/umx-bench --umx target/release/umx --out docs/project/benchmarks \
        --commit "$(git rev-parse --short=12 HEAD)$(git diff --quiet HEAD || echo -dirty)" \
        --conditions "nice -n 19, release" {{flags}}

# The bench ruler: the product's own rows on a realistic 100k fixture, its keyed variant and the rich fixture.
# Reports, never gates. Writes target/bench-ruler unless you pass your own --out.
bench-ruler *flags:
    scripts/cargo-slot build -q --release --locked -p umx-bench
    mkdir -p target/bench-ruler
    nice -n 19 target/release/umx-bench --ruler --nodes 100000 --out target/bench-ruler \
        --commit "$(git rev-parse --short=12 HEAD)$(git diff --quiet HEAD || echo -dirty)" \
        --conditions "nice -n 19, release" {{flags}}

# Teammates without a checkout install the skill with the pinned line `umx doctor` prints:
#     npx skills@1.7.6 add AI-Substrate/umx#<commit> --skill umx -g --agent '*'
# Here, it is a symlink, so the skill follows this checkout. Refuses a linked worktree, so a branch under
# development never replaces every agent's skill, unless you pass --from-worktree. Then links each harness's skills
# folder to it, relative, the layout pij uses (F77): ~/.claude/skills/umx and ~/.codex/skills/umx ->
# ../../.agents/skills/umx, ~/.pi/agent/skills/umx -> ../../../.agents/skills/umx. Rerunning changes nothing; a
# harness entry that is not a symlink is left alone with a warning.
# Link the /umx agent skill into ~/.agents/skills/umx and every harness's skills folder (req-70, F77).
skill-link *flags:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(git rev-parse --git-dir)" != "$(git rev-parse --git-common-dir)" ] && [ "{{flags}}" != "--from-worktree" ]; then
        echo "skill-link: this is a linked worktree; run it in the main checkout, or pass --from-worktree" >&2
        exit 1
    fi
    source="$(cd skills/umx && pwd -P)"
    target="$HOME/.agents/skills/umx"
    mkdir -p "$(dirname "$target")"
    if [ -e "$target" ] && [ ! -L "$target" ]; then
        echo "skill-link: $target exists and is not a symlink; move it away first" >&2
        exit 1
    fi
    ln -sfn "$source" "$target"
    echo "linked $target -> $source"
    for pair in ".claude/skills ../../.agents/skills/umx" ".codex/skills ../../.agents/skills/umx" \
        ".pi/agent/skills ../../../.agents/skills/umx"; do
        set -- $pair
        link="$HOME/$1/umx"
        if [ -L "$link" ] && [ "$(readlink "$link")" = "$2" ]; then
            echo "ok $link -> $2"
        elif [ -e "$link" ] && [ ! -L "$link" ]; then
            echo "skill-link: warning: $link exists and is not a symlink; left as it is" >&2
        else
            mkdir -p "$HOME/$1"
            ln -sfn "$2" "$link"
            echo "linked $link -> $2"
        fi
    done

# The graph view's gate (workstream 009), separate from `check` so Rust PRs pay nothing. First, before anything
# installs: the lock source check (npmjs hosts and sha512 only), the quarantine check (pnpm 10.16+ reading
# minimumReleaseAge of 7 days) and the lock age check (every lock entry new or changed since the merge base with
# origin/main published at least that long ago; the whole lock when there is no merge base, since pnpm skips the
# setting on a frozen install). Then a frozen pnpm install with install scripts off (apps/ui/.npmrc), lint (with the
# import-boundary plant), typecheck, unit tests and the standalone build. Everything niced, one step at a time (D59).
# Run it when apps/** changes.
ui-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cd apps/ui
    export NEXT_TELEMETRY_DISABLED=1
    node scripts/check-lock-sources.mjs
    node scripts/check-quarantine.mjs "$(pnpm config get minimumReleaseAge)"
    base_lock=$(mktemp)
    trap 'rm -f "$base_lock"' EXIT
    if base=$(git merge-base HEAD origin/main 2>/dev/null) &&
        git show "$base:apps/ui/pnpm-lock.yaml" >"$base_lock" 2>/dev/null; then
        node scripts/check-lock-ages.mjs --base "$base_lock"
    else
        node scripts/check-lock-ages.mjs
    fi
    nice -n 19 pnpm install --frozen-lockfile
    nice -n 19 pnpm lint
    nice -n 19 pnpm typecheck
    nice -n 19 pnpm test
    nice -n 19 pnpm build
