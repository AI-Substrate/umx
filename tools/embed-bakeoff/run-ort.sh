#!/bin/sh
# The Rust (ort) runs for the parity gate: check, parity, quality on the 300-node fixture, then speed
# in the shipped config (1 session x 4 threads, batch 16). Each run takes the lock via locked.sh.
# Speed waits up to WAIT_MIN minutes (default 20) for a 1-minute load below MAX_LOAD (default 16).
# Each run writes <name>.json.part and renames it only on success. The first failure stops the
# script with its exit status and removes that stage's old <name>.json, so no stale result remains.
# Usage: tools/embed-bakeoff/run-ort.sh <model> ...
set -eu
cd "$(dirname "$0")"
OUT=../../docs/project/benchmarks/embed-bakeoff/results
run() {
	name="$1"; shift
	if ./locked.sh run "$@" > "$OUT/$name.json.part"; then
		if mv "$OUT/$name.json.part" "$OUT/$name.json"; then
			echo "done $name"
			return 0
		fi
		st=1
	else
		st=$?
	fi
	rm -f "$OUT/$name.json.part" "$OUT/$name.json"
	echo "FAILED $name (exit $st); removed results/$name.json" >&2
	exit "$st"
}
load1() { sysctl -n vm.loadavg | awk '{print $2}'; }
for m in "$@"; do
	run "ort-check-$m" check "$m"
	run "parity-$m" parity "$m"
	run "ort-quality-$m" quality "$m"
done
waited=0
while [ "$(load1 | cut -d. -f1)" -ge "${MAX_LOAD:-16}" ] && [ "$waited" -lt "${WAIT_MIN:-20}" ]; do sleep 60; waited=$((waited + 1)); done
echo "speed runs start at load $(load1) after waiting ${waited} min"
for m in "$@"; do run "ort-speed-$m" speed "$m"; done
