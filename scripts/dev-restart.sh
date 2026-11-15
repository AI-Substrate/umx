#!/usr/bin/env bash
# Installs a freshly built umx and restarts the daemon on it (req-42). When the daemon runs as a service
# (`umx daemon install`, req-86), it reinstalls the service so the new build runs; otherwise it starts the
# daemon in the background. Exits non-zero, with the daemon's own error, unless the new build answers.
# Usage: dev-restart.sh <built umx> <install dir> <build id>
set -euo pipefail
src="$1" bin="$2" build="$3"
home="${UMX_HOME:-$HOME/.umx}"
log="$home/daemon.log"
umx="$bin/umx"

running() { "$umx" daemon status --json 2>/dev/null | grep -q '"running":true'; }

# `umx daemon service` asks launchd or systemd only, so a slow daemon cannot hide an installed service (F58). An
# installed umx older than that command falls back to `umx daemon status`.
installed() {
    { "$umx" daemon service --json 2>/dev/null || "$umx" daemon status --json 2>/dev/null; } |
        grep -q '"installed":true'
}

service=false
if [ -x "$umx" ] && installed; then
    service=true
elif [ -x "$umx" ]; then
    "$umx" daemon stop >/dev/null 2>&1 || true
    for _ in $(seq 50); do running || break; sleep 0.1; done
fi
mkdir -p "$bin" && cp "$src" "$umx.tmp" && mv -f "$umx.tmp" "$umx"
mkdir -p -m 700 "$home"
: >>"$log"
from=$(wc -c <"$log")
pid=""
if $service; then
    # Rewrites the service file and reloads it, which stops the old build and starts this one.
    if ! out=$("$umx" daemon install --json 2>&1); then
        echo "just dev: umx daemon install failed: $out" >&2
        exit 1
    fi
else
    nohup "$umx" daemon start >>"$log" 2>&1 &
    pid=$!
fi

fail() {
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
    echo "just dev: $1. Daemon output ($log):" >&2
    tail -c +"$((from + 1))" "$log" >&2
    exit 1
}
for _ in $(seq 100); do
    if "$umx" daemon status --json 2>/dev/null | grep -q "\"running\":true.*\"built\":\"$build\""; then
        exec "$umx" version --human
    fi
    [ -z "$pid" ] || kill -0 "$pid" 2>/dev/null || fail "the daemon exited at start"
    sleep 0.1
done
fail "the daemon did not answer as build $build within 10 s"
