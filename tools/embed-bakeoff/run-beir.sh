#!/bin/sh
# Every BEIR encode and score, one model-task at a time (each takes the lock via beir.sh).
# Stops at the first failed stage with its exit status, and then removes that model's
# results/beir-<model>.json so a stale score never stands in for a failed rerun.
# Usage: BAKEOFF_PY=<venv python> tools/embed-bakeoff/run-beir.sh [model ...]
set -eu
cd "$(dirname "$0")"
RESULTS=../../docs/project/benchmarks/embed-bakeoff/results
models="${*:-qwen3-embedding-0.6b harrier-oss-v1-0.6b voyage-4-nano embeddinggemma-2}"

# stage <model> <beir.sh args...>: print the last output line on success; on failure stop.
stage() {
	m="$1"; shift
	if out=$(./beir.sh "$@"); then
		printf '%s\n' "$out" | tail -1
	else
		st=$?
		rm -f "$RESULTS/beir-$m.json"
		echo "FAILED beir.sh $* (exit $st); removed results/beir-$m.json" >&2
		exit "$st"
	fi
}

each() {
	m="$1"
	for t in fixture scifact nfcorpus arguana; do stage "$m" encode "$m" "$t"; done
	stage "$m" score "$m"
}
for m in $models; do each "$m"; done
case " $models " in *" qwen3-embedding-0.6b "*) each qwen3-embedding-0.6b-noprompt ;; esac
