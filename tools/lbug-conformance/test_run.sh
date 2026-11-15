#!/usr/bin/env bash
# run.sh's guards, with a fake cargo (CARGO_SLOT) and no LadybugDB build:
#   1. inherited lbug build overrides never reach cargo: only LBUG_LIBRARY_DIR and LBUG_INCLUDE_DIR, naming the
#      sha256-checked folder, do;
#   2. a build report that links any other folder is refused (exit 1) and the checker never runs;
#   3. so is one where another searched folder (say /usr/local/lib) also holds a liblbug;
#   4. with the real checker built (run.sh first), a float 3.3e-3 off (a02's mean as 0.480001) differs;
#   5. and a fixture storing both halves of a symmetric pair (D49) is refused (exit 2), not compared.
#   6. a checked type(r) refusal probe changed to accepted label(r) fails (exit 1).
# Needs the verified archive in the cache (run.sh fetches it); without it, prints a skip and exits 0.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
cache="${LBUG_PREBUILT_CACHE_DIR:-$HOME/.cache/umx-lbug-prebuilt}"
if ! ls "$cache"/v0.22.3/liblbug-static-*.tar.gz >/dev/null 2>&1; then
	echo "skipped: lbug unavailable (no verified archive in $cache; run run.sh first)"
	exit 0
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The fake cargo records the LBUG_* env it was given, then writes the lbug build report named by
# FAKE_LINK_DIR (empty: the folder cargo was told to link) and a checker that only says it ran.
cat > "$tmp/cargo" <<'FAKE'
#!/usr/bin/env bash
env | grep -E '^(LBUG_|DOCS_RS=)' | sort > "$FAKE_ENV_OUT"
mkdir -p "$CARGO_TARGET_DIR/debug/build/lbug-fake"
link="${FAKE_LINK_DIR:-$LBUG_LIBRARY_DIR}"
printf 'cargo:rustc-link-search=native=%s\ncargo:rustc-env=LBUG_PRECOMPILED_SOURCE=external\ncargo:rustc-env=LBUG_PRECOMPILED_LIBRARY_DIR=%s\n' "$link" "$link" \
	> "$CARGO_TARGET_DIR/debug/build/lbug-fake/output"
[ -n "${FAKE_EXTRA_DIR:-}" ] && echo "cargo:rustc-link-search=native=$FAKE_EXTRA_DIR" >> "$CARGO_TARGET_DIR/debug/build/lbug-fake/output"
printf '#!/bin/sh\necho checker-ran\n' > "$CARGO_TARGET_DIR/debug/lbug-conformance"
chmod +x "$CARGO_TARGET_DIR/debug/lbug-conformance"
FAKE
chmod +x "$tmp/cargo"

hostile=(
	LBUG_LINUX_VARIANT=perf LBUG_SHARED=1 LBUG_BUILD_FROM_SOURCE=1 LBUG_RUST_BUILD_FROM_SOURCE=1
	LBUG_SOURCE_DIR=/nonexistent LBUG_LIBRARY_DIR=/nonexistent LBUG_INCLUDE_DIR=/nonexistent
	LBUG_VERSION=0.0.1 LBUG_PRECOMPILED_RUN_ID=1 LBUG_GITHUB_REPOSITORY=someone/else LBUG_LIB_KIND=shared
	LBUG_LOCALIZE_BUNDLED_SYMBOLS=1 LBUG_TARGET_DIR=/nonexistent LBUG_BUNDLED=1 LBUG_STATIC_DEFINE=1 DOCS_RS=1
)
lib="$cache/v0.22.3/$(basename "$(ls "$cache"/v0.22.3/liblbug-static-*.tar.gz | head -1)" .tar.gz)"
fail=0

out="$(env "${hostile[@]}" LBUG_PREBUILT_CACHE_DIR="$cache" CARGO_SLOT="$tmp/cargo" CARGO_TARGET_DIR="$tmp/t1" \
	FAKE_ENV_OUT="$tmp/env1" "$here/run.sh" crates/umx-cypher/fixtures/design "$tmp/out1.json")"
want="$(printf 'LBUG_INCLUDE_DIR=%s\nLBUG_LIBRARY_DIR=%s' "$lib" "$lib")"
if [ "$(cat "$tmp/env1")" = "$want" ] && [ "$out" = checker-ran ]; then
	echo "ok    inherited overrides cleared; cargo saw only the verified folder"
else
	echo "FAIL  cargo saw:"; cat "$tmp/env1"; echo "run.sh said: $out"; fail=1
fi

if env "${hostile[@]}" LBUG_PREBUILT_CACHE_DIR="$cache" CARGO_SLOT="$tmp/cargo" CARGO_TARGET_DIR="$tmp/t2" \
	FAKE_ENV_OUT="$tmp/env2" FAKE_LINK_DIR=/somewhere/else "$here/run.sh" crates/umx-cypher/fixtures/design \
	"$tmp/out2.json" > "$tmp/run2" 2>&1; then
	echo "FAIL  a build that linked another folder was accepted"; cat "$tmp/run2"; fail=1
elif grep -q checker-ran "$tmp/run2"; then
	echo "FAIL  the checker ran after a refused build"; fail=1
else
	echo "ok    a build that linked another folder is refused: $(tail -1 "$tmp/run2")"
fi
mkdir -p "$tmp/stray" && touch "$tmp/stray/liblbug.a"
if env "${hostile[@]}" LBUG_PREBUILT_CACHE_DIR="$cache" CARGO_SLOT="$tmp/cargo" CARGO_TARGET_DIR="$tmp/t3" \
	FAKE_ENV_OUT="$tmp/env3" FAKE_EXTRA_DIR="$tmp/stray" "$here/run.sh" crates/umx-cypher/fixtures/design \
	"$tmp/out3.json" > "$tmp/run3" 2>&1; then
	echo "FAIL  a stray liblbug in another searched folder was accepted"; cat "$tmp/run3"; fail=1
else
	echo "ok    a stray liblbug in another searched folder is refused: $(tail -1 "$tmp/run3")"
fi
checker="${CARGO_TARGET_DIR:-$here/target}/debug/lbug-conformance"
if [ -x "$checker" ]; then
	cp -R "$here/../../crates/umx-cypher/fixtures/design" "$tmp/plant"
	sed -i.bak 's/"0.48333333333333334"/"0.480001"/' "$tmp/plant/queries.json"
	if "$checker" "$tmp/plant" "$tmp/plant.json" > "$tmp/run4" 2>&1; then
		echo "FAIL  a02 with a mean 3.3e-3 off was accepted"; fail=1
	elif grep -q '^DIFFERS   a02-agg-float' "$tmp/run4" && [ "$(grep -c '^DIFFERS' "$tmp/run4")" = 1 ]; then
		echo "ok    a float 3.3e-3 off differs: $(tail -1 "$tmp/run4")"
	else
		echo "FAIL  unexpected checker output:"; cat "$tmp/run4"; fail=1
	fi
	cp -R "$here/../../crates/umx-cypher/fixtures/design" "$tmp/pair"
	grep '"type":"conflicts_with"' "$tmp/pair/edges.jsonl" | head -1 |
		sed -e 's/"id":"\([^"]*\)E14"/"id":"\1E99"/' \
			-e 's/"from":"\([^"]*\)","to":"\([^"]*\)"/"from":"\2","to":"\1"/' >> "$tmp/pair/edges.jsonl"
	rc=0
	"$checker" "$tmp/pair" "$tmp/pair.json" > "$tmp/run5" 2>&1 || rc=$?
	if [ "$rc" = 0 ]; then
		echo "FAIL  a reversed symmetric pair was compared"; fail=1
	elif [ "$rc" = 2 ] && grep -q 'umx keeps one edge of such a pair' "$tmp/run5"; then
		echo "ok    a reversed symmetric pair is refused: $(head -c 120 "$tmp/run5")"
	else
		echo "FAIL  unexpected checker output:"; cat "$tmp/run5"; fail=1
	fi
	cp -R "$here/../../crates/umx-cypher/fixtures/design" "$tmp/accepted-type"
	sed -i.bak '/"id": "z21-lbug-type-refusal"/s/type(r)/label(r)/' "$tmp/accepted-type/queries.json"
	rc=0
	"$checker" "$tmp/accepted-type" "$tmp/accepted-type.json" > "$tmp/run6" 2>&1 || rc=$?
	if [ "$rc" = 1 ] && grep -q '^DIFFERS   z21-lbug-type-refusal' "$tmp/run6" &&
		[ "$(grep -c '^DIFFERS' "$tmp/run6")" = 1 ]; then
		echo "ok    a checked refusal probe that now succeeds is a mismatch"
	else
		echo "FAIL  an accepted refusal probe was not the sole mismatch:"; cat "$tmp/run6"; fail=1
	fi
else
	echo "skip  real checker plants: no checker at $checker (run run.sh first)"
fi
exit "$fail"
