#!/usr/bin/env bash
# LadybugDB 0.22.3 conformance check over the Cypher design fixture (workstream 005).
# Usage: tools/lbug-conformance/run.sh [fixture_dir] [out.json]
#
# Exit 0: every conformance query agrees, or "skipped: lbug unavailable" (no prebuilt library for this
# platform, or the download failed). Exit 1: a query differs, the library fails its checksum, or the lbug
# build did not link the verified library.
#
# The lbug crate's build script picks its library from caller env (LBUG_LIBRARY_DIR, LBUG_SHARED,
# LBUG_LINUX_VARIANT, LBUG_VERSION, LBUG_SOURCE_DIR, ...), a legacy cache inside its registry folder, or a
# download of the "latest" release with no checksum, and falls back to a source build. So this script:
#   1. fetches the v0.22.3 static archive itself and checks the sha256 pinned below on every run
#      (cross-checked against GitHub's asset digests, 2026-11-10), then unpacks it fresh;
#   2. clears every LBUG_* variable (and DOCS_RS) it inherited and sets only LBUG_LIBRARY_DIR and
#      LBUG_INCLUDE_DIR to that folder, the build script's first branch, so nothing is downloaded or built;
#   3. fails unless the build script reports that folder as its library and no other searched folder holds a
#      liblbug.
# The checker runs no LOAD, so no extension is fetched at run time.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
# Paths are taken from the repo root, so the record names the fixture without a home folder.
cd "$root"
fixture="${1:-crates/umx-cypher/fixtures/design}"
target="${CARGO_TARGET_DIR:-$here/target}"
out="${2:-$target/design-lbug.json}"
cache="${LBUG_PREBUILT_CACHE_DIR:-$HOME/.cache/umx-lbug-prebuilt}"
slot="${CARGO_SLOT:-$root/scripts/cargo-slot}"
version=0.22.3

skip() {
	echo "skipped: lbug unavailable ($1)"
	exit 0
}

case "$(uname -s)-$(uname -m)" in
	Darwin-arm64) archive=liblbug-static-osx-arm64.tar.gz
		sha=9a6e6903ed36ad58fbc31412cf4aae6d269b90219137a8e29b47472a89c350c8 ;;
	Darwin-x86_64) archive=liblbug-static-osx-x86_64.tar.gz
		sha=8220ab819917245ff45bbd61bb5603fdf9d8a99d2697fdc092585e9ce031d359 ;;
	Linux-x86_64) archive=liblbug-static-linux-x86_64-compat.tar.gz
		sha=269bdde6173a35ba4a4e885ac187c37965df2becdbc4a48311651d3654f0e07a ;;
	Linux-aarch64) archive=liblbug-static-linux-aarch64-compat.tar.gz
		sha=4bf57a7ccd46f96765aa10a9b797f2e01f4390fd40484d74110f943e766359d8 ;;
	*) skip "no prebuilt library for $(uname -s)-$(uname -m)" ;;
esac

sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }

kept="$cache/v$version/$archive"
mkdir -p "$(dirname "$kept")"
if [ ! -f "$kept" ]; then
	tmp="$(mktemp -d)"
	trap 'rm -rf "$tmp"' EXIT
	url="https://github.com/LadybugDB/ladybug/releases/download/v$version/$archive"
	curl -fsSL --retry 2 --max-time 600 -o "$tmp/$archive" "$url" || skip "download failed: $url"
	mv "$tmp/$archive" "$kept.part"
	mv "$kept.part" "$kept"
fi
got="$(sha256 "$kept")"
if [ "$got" != "$sha" ]; then
	rm -f "$kept"
	echo "lbug-conformance: $archive sha256 $got, expected $sha; removed it, refusing to build" >&2
	exit 1
fi
lib="$cache/v$version/${archive%.tar.gz}"
rm -rf "$lib"
mkdir -p "$lib"
tar xzf "$kept" -C "$lib"
if [ ! -f "$lib/liblbug.a" ] || [ ! -f "$lib/lbug.hpp" ]; then
	echo "lbug-conformance: $archive did not unpack liblbug.a and lbug.hpp into $lib" >&2
	exit 1
fi

while IFS= read -r name; do
	unset "$name"
done < <(compgen -e | grep -E '^(LBUG_|DOCS_RS$)' || true)
export LBUG_LIBRARY_DIR="$lib" LBUG_INCLUDE_DIR="$lib" CARGO_TARGET_DIR="$target"

"$slot" build -q --locked --manifest-path "$here/Cargo.toml"

# The build script's report for the build just made: lbug must come from the verified folder, and no other
# folder the link searches (openssl, system, out) may hold a liblbug that could be picked instead.
report="$(ls -t "$target"/debug/build/lbug-*/output 2>/dev/null | head -1 || true)"
refuse() {
	echo "lbug-conformance: $1 (build report: ${report:-none}); refusing to run" >&2
	exit 1
}
[ -n "$report" ] || refuse "no lbug build report"
grep -qxF "cargo:rustc-link-search=native=$lib" "$report" || refuse "the lbug build did not link $lib"
grep -qxF "cargo:rustc-env=LBUG_PRECOMPILED_SOURCE=external" "$report" || refuse "the lbug build did not use LBUG_LIBRARY_DIR"
grep -qxF "cargo:rustc-env=LBUG_PRECOMPILED_LIBRARY_DIR=$lib" "$report" || refuse "the lbug build names another library folder"
while IFS= read -r dir; do
	if [ "$dir" != "$lib" ] && ls "$dir"/liblbug.* >/dev/null 2>&1; then
		refuse "$dir also holds a liblbug"
	fi
done < <(sed -n 's/^cargo:rustc-link-search=native=//p' "$report")

mkdir -p "$(dirname "$out")"
"$target/debug/lbug-conformance" "$fixture" "$out"
