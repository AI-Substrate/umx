#!/bin/sh
# Run one bake-off cargo command in a shared unimatrix build slot (tools/cargo-slot, rule D41:
# two slots fleet-wide, niced, 4 jobs). Model runs go through `cargo run`, so they take a slot too.
# Usage: tools/embed-bakeoff/locked.sh build | run <args...>
set -eu
cd "$(dirname "$0")"
SLOT="${CARGO_SLOT:-$HOME/substrate/unimatrix/tools/cargo-slot}"
unset HF_HOME HF_ENDPOINT HF_TOKEN
case "$1" in
	build) "$SLOT" build --release -q ;;
	run) shift; "$SLOT" run --release -q -- "$@" ;;
	*) echo "usage: locked.sh build|run <args>" >&2; exit 2 ;;
esac
