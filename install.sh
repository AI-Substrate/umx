#!/bin/sh
# Installs umx from a GitHub release on macOS (Apple silicon) and Linux (x86_64 or arm64, glibc 2.35 or newer;
# on Windows, inside WSL 2).
#
#   curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | sh
#   curl -fsSL https://github.com/AI-Substrate/umx/releases/latest/download/install.sh | sh -s -- --version 0.1.0
#
# The version is the latest release unless `--version <version>` or UMX_VERSION names one (the flag wins; a leading
# `v` is optional). It downloads the archive for this machine and SHA256SUMS with curl, https only, into a private
# temporary folder; checks the archive's sha256 against its SHA256SUMS line before opening it; takes only
# umx-<target>/umx, LICENSE and THIRD_PARTY_NOTICES out of it; and checks the new umx runs here. Only then does it
# rename the new `umx` into ~/.local/bin, so the old one stays if anything up to then fails. Then it restarts a running
# umx daemon (`umx daemon restart`), starts a stopped daemon service installed for this umx (`umx daemon install`,
# which reloads it), or starts the daemon in the background, and asks it which build it runs (`umx version`): when that
# is not the new umx, it says why and prints the commands that fix it. A restart or start that fails after the rename
# exits 1 saying umx is installed and what to run; nothing is rolled back. Then it runs `umx doctor --human` and
# prints what to run next. It never uses sudo, never edits a shell startup file, and never installs the /umx skill or a
# new daemon service itself: it prints those lines for you or your agent to run (the skill line only in the exact
# shape doctor prints it, pinned to AI-Substrate/umx at a full commit; otherwise it says to run umx doctor).
#
# Testing only: UMX_INSTALL_TEST_RELEASES=file:///<folder> reads the release from a local folder laid out like the
# release URLs (<folder>/latest/download/<asset>, <folder>/download/v<version>/<asset>); scripts/install-sh-test
# uses it. It takes a file:/// URL and nothing else, so every real download stays https.
set -eu

releases=https://github.com/AI-Substrate/umx/releases

say() { printf '%s\n' "$*"; }

die() {
    printf 'install.sh: %s (%s)\n' "$1" "$2" >&2
    exit 1
}

source_hint="build umx from source instead: see Install in the README at https://github.com/AI-Substrate/umx"

# Sets `target` to this machine's release target, or refuses one umx does not ship for.
pick_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
    Darwin)
        # A shell under Rosetta says x86_64 on Apple silicon.
        if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
            arch=arm64
        fi
        case "$arch" in
        arm64) target=aarch64-apple-darwin ;;
        *) die "no umx release for macOS on $arch" "releases are for Apple silicon only; $source_hint" ;;
        esac
        ;;
    Linux)
        # WSL 1's kernel release ends in "-Microsoft"; WSL 2's says "microsoft-standard-WSL2" and is plain Linux.
        case "$(uname -r)" in
        *Microsoft*) die "WSL 1 is not supported" "use WSL 2: wsl --set-version <distro> 2" ;;
        esac
        case "$arch" in
        x86_64) target=x86_64-unknown-linux-gnu ;;
        aarch64 | arm64) target=aarch64-unknown-linux-gnu ;;
        *) die "no umx release for Linux on $arch" "releases are for x86_64 and arm64; $source_hint" ;;
        esac
        if ldd --version 2>&1 | grep -qi musl; then
            die "no umx release for musl Linux (Alpine)" "releases need glibc 2.35 or newer; $source_hint"
        fi
        glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
        case "$glibc" in
        "glibc "[0-9]*.[0-9]*)
            glibc=${glibc#glibc }
            major=${glibc%%.*}
            minor=${glibc#*.}
            minor=${minor%%[!0-9]*}
            if [ "$major" -lt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -lt 35 ]; }; then
                die "glibc $glibc is older than the 2.35 umx releases need" "use Ubuntu 22.04, Debian 12 or newer; $source_hint"
            fi
            ;;
        esac
        ;;
    *) die "no umx release for $os" "releases are for macOS and Linux (on Windows, run this inside WSL 2); $source_hint" ;;
    esac
}

# Downloads $1 to $2. Real downloads are https only, redirects included; only the test folder reads file:///.
fetch() {
    if [ -n "$test_releases" ]; then
        curl --proto '=file' -fsSL -o "$2" "$1"
    else
        curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL -o "$2" "$1"
    fi
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sum=$(sha256sum "$1")
    else
        sum=$(shasum -a 256 "$1")
    fi
    printf '%s\n' "${sum%% *}"
}

running() { "$umx" daemon status --json </dev/null 2>/dev/null | grep -q '"running":true'; }

# The string at key $1 in the JSON $2 (one without quotes or backslashes), and the same limited to a version's
# characters.
json_path() { printf '%s\n' "$2" | sed -n 's/.*"'"$1"'":"\([^"\\]*\)".*/\1/p'; }
json_word() { printf '%s\n' "$2" | sed -n 's/.*"'"$1"'":"\([0-9A-Za-z.+-]*\)".*/\1/p'; }

# Waits up to 10 seconds for the daemon to answer; $1 is the pid of a daemon this script started, if any.
wait_up() {
    deadline=$(($(date +%s) + 10))
    until running; do
        if [ -n "$1" ] && ! kill -0 "$1" 2>/dev/null; then
            failed="the umx daemon stopped as it started"
            return 1
        fi
        if [ "$(date +%s)" -ge "$deadline" ]; then
            failed="the umx daemon did not answer within 10 seconds"
            return 1
        fi
        sleep 0.1
    done
}

# The commands that make the daemon run the umx in ~/.local/bin, one per line: stop the daemon running now (when $1
# is `stop`), remove a service that runs another umx or UMX_HOME, then install the service for this umx, which starts
# it now and at every login.
fix_cmds() {
    # shellcheck disable=SC2088 # printed for the user to type, so the ~ stays
    me='~/.local/bin/umx'
    if [ -n "$1" ]; then say "$me daemon stop"; fi
    if [ -n "$svc_other" ]; then say "$me daemon uninstall"; fi
    say "$me daemon install"
}

# After the rename there is no going back: umx is installed and the daemon does not run it. One line, exit 1.
after_fail() {
    die "umx $newver is installed at ~/.local/bin/umx, but $1, so the umx daemon is not running the new build" \
        "run $(fix_cmds "$2" | awk '{ printf "%s%s", (NR > 1 ? ", then " : ""), $0 }'); the daemon's log is $log"
}

cleanup() {
    rm -rf "$tmp"
    if [ -n "$staged" ]; then rm -f "$staged"; fi
}

# Everything runs from here, so a download cut short runs nothing.
main() {
    tmp=""
    staged=""
    version=${UMX_VERSION-}
    while [ $# -gt 0 ]; do
        case "$1" in
        --version)
            [ $# -ge 2 ] || die "--version needs a version" "for example --version 0.1.0"
            version=$2
            shift 2
            ;;
        --version=*)
            version=${1#--version=}
            shift
            ;;
        -h | --help)
            say "usage: install.sh [--version <version>]   (or UMX_VERSION=<version>; the latest release by default)"
            exit 0
            ;;
        *) die "unknown argument $1" "the only option is --version <version>" ;;
        esac
    done
    version=${version#v}
    if [ -n "$version" ]; then
        case "$version" in
        *[!0-9A-Za-z.-]* | [!0-9]*) die "the --version or UMX_VERSION given is not a version" "use one like 0.1.0 or v0.1.0" ;;
        esac
        printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' ||
            die "the --version or UMX_VERSION given is not a version" "use one like 0.1.0 or v0.1.0"
    fi

    test_releases=${UMX_INSTALL_TEST_RELEASES-}
    base=$releases
    if [ -n "$test_releases" ]; then
        case "$test_releases" in
        file:///*) ;;
        *) die "UMX_INSTALL_TEST_RELEASES takes only a file:/// URL" "it is for scripts/install-sh-test; unset it" ;;
        esac
        base=$test_releases
        say "install.sh: reading the release from $base (UMX_INSTALL_TEST_RELEASES)" >&2
    fi

    pick_target
    command -v curl >/dev/null 2>&1 || die "curl is not installed" "install curl, then run this again"
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 ||
        die "neither sha256sum nor shasum is installed" "install coreutils, then run this again"
    [ -n "${HOME-}" ] || die "HOME is not set" "set HOME, then run this again"

    if [ -n "$version" ]; then
        url=$base/download/v$version
        what="umx $version"
    else
        url=$base/latest/download
        what="the latest umx"
    fi
    archive=umx-$target.tar.gz
    dir=umx-$target

    tmp=$(mktemp -d "${TMPDIR:-/tmp}/umx-install.XXXXXX")
    trap cleanup EXIT
    trap 'exit 1' HUP INT TERM

    say "Downloading $what for $target..."
    fetch "$url/SHA256SUMS" "$tmp/SHA256SUMS" ||
        die "could not download $url/SHA256SUMS" "check the version and your connection; nothing changed"
    fetch "$url/$archive" "$tmp/$archive" ||
        die "could not download $url/$archive" "check the version and your connection; nothing changed"

    want=$(awk -v f="$archive" 'NF == 2 && $2 == f { print $1; n++ } END { exit n == 1 ? 0 : 1 }' \
        "$tmp/SHA256SUMS") || die "SHA256SUMS has no single line for $archive" "the release is incomplete; nothing changed"
    case "$want" in
    *[!0-9a-f]* | "") die "SHA256SUMS has no valid sha256 for $archive" "the release is broken; nothing changed" ;;
    esac
    [ ${#want} -eq 64 ] || die "SHA256SUMS has no valid sha256 for $archive" "the release is broken; nothing changed"
    [ "$(sha256 "$tmp/$archive")" = "$want" ] ||
        die "$archive does not match its sha256 in SHA256SUMS" "the download is not the published file; nothing changed"
    say "Checked $archive against SHA256SUMS."

    # Before anything is unpacked: only the expected names, and only plain files and folders (no links, no devices).
    names=$(tar -tzf "$tmp/$archive") || die "cannot read $archive" "the release is broken; nothing changed"
    while IFS= read -r name; do
        case "$name" in
        "$dir" | "$dir/" | "$dir/umx" | "$dir/LICENSE" | "$dir/THIRD_PARTY_NOTICES") ;;
        *) die "$archive holds something other than $dir/umx, LICENSE and THIRD_PARTY_NOTICES" "the release is broken; nothing changed" ;;
        esac
    done <<EOF
$names
EOF
    types=$(tar -tvzf "$tmp/$archive" | cut -c1 | tr -d '\n')
    case "$types" in
    *[!d-]*) die "$archive holds a link or a special file" "the release is broken; nothing changed" ;;
    esac
    mkdir "$tmp/x"
    tar -xzf "$tmp/$archive" -C "$tmp/x" || die "cannot unpack $archive" "the release is broken; nothing changed"
    for f in umx LICENSE THIRD_PARTY_NOTICES; do
        if [ ! -f "$tmp/x/$dir/$f" ] || [ -L "$tmp/x/$dir/$f" ]; then
            die "$archive has no plain file $dir/$f" "the release is broken; nothing changed"
        fi
    done
    if [ "$(find "$tmp/x" | wc -l | tr -d ' ')" -ne 5 ] || [ -n "$(find "$tmp/x" -type f -links +1)" ]; then
        die "$archive holds more than $dir/umx, LICENSE and THIRD_PARTY_NOTICES" "the release is broken; nothing changed"
    fi

    bin=$HOME/.local/bin
    umx=$bin/umx
    if [ ! -d "$bin" ]; then mkdir -p "$bin" && chmod 0755 "$bin"; fi
    [ ! -d "$umx" ] || die "$umx is a folder" "move it away, then run this again; nothing changed"
    staged=$bin/.umx-install.$$
    cp "$tmp/x/$dir/umx" "$staged"
    chmod 0755 "$staged"
    built=$("$staged" version --json </dev/null 2>/dev/null) ||
        die "the downloaded umx does not run on this machine" "nothing changed; $source_hint"
    newver=$(json_word version "$(printf '%s\n' "$built" | sed -n 's/.*"cli":\({[^{}]*}\).*/\1/p')")
    newver=${newver:-${version:-unknown}}
    mv -f "$staged" "$umx"
    staged=""
    say "Installed $umx."

    home=${UMX_HOME:-$HOME/.umx}
    if [ ! -d "$home" ]; then (umask 077 && mkdir -p "$home"); fi
    log=$home/daemon.log
    # The service umx runs, as `umx daemon service` reads it back: its program and UMX_HOME are canonical paths.
    service=$("$umx" daemon service --json </dev/null 2>/dev/null || true)
    here=$(cd -P "$bin" && pwd -P)/umx
    here_home=$(cd -P "$home" && pwd -P)
    svc_program=$(json_path program "$service")
    svc_home=$(json_path home "$service")
    svc_other=""
    svc_here=""
    case "$service" in
    *'"installed":true'*)
        if [ "$svc_program" = "$here" ] && [ "$svc_home" = "$here_home" ]; then svc_here=1; else svc_other=1; fi
        ;;
    esac

    if running; then
        "$umx" daemon restart --human </dev/null || after_fail "umx daemon restart failed" stop
        say "Restarted the running umx daemon."
    elif [ -n "$svc_here" ]; then
        # The service for this umx and this UMX_HOME is installed but stopped. `umx daemon start` never goes through
        # the service manager, so start it the way umx does: installing the same service again rewrites and reloads it,
        # which starts it now (launchd: bootout and bootstrap with RunAtLoad; systemd: enable and restart).
        "$umx" daemon install --human </dev/null || after_fail "umx daemon install, to start the umx daemon service, failed" ""
        wait_up "" || after_fail "$failed" ""
        say "Started the umx daemon service."
    else
        nohup "$umx" daemon start </dev/null >>"$log" 2>&1 &
        wait_up $! || after_fail "$failed" ""
        say "Started the umx daemon in the background (its log: $log)."
    fi

    # Never say the new umx runs without asking the daemon: a restart in place runs the binary the daemon was started
    # from, and a service restart runs the program in the service, either of which may be another umx.
    builds=$("$umx" version --json </dev/null 2>/dev/null || true)
    cli=$(printf '%s\n' "$builds" | sed -n 's/.*"data":{"cli":\({[^{}]*}\),"daemon":.*/\1/p')
    daemon=$(printf '%s\n' "$builds" | sed -n 's/.*"data":{"cli":{[^{}]*},"daemon":\({[^{}]*}\)}.*/\1/p')
    if [ -n "$cli" ] && [ "$cli" = "$daemon" ]; then
        say "The umx daemon runs the umx just installed ($newver)."
        if [ -n "$svc_other" ]; then
            say "Note: the umx daemon service runs ${svc_program:-a program umx cannot read} with UMX_HOME ${svc_home:-unknown}, so the next login starts that one. To make the service run this umx:"
            fix_cmds stop | sed 's/^/  /'
        fi
    else
        if [ -n "$daemon" ]; then
            runs="umx $(json_word version "$daemon") (commit $(json_word commit "$daemon"), built $(json_word built "$daemon"))"
        else
            runs="a umx that did not report its build"
        fi
        if [ -n "$svc_other" ]; then
            why="the umx daemon service runs ${svc_program:-a program umx cannot read} with UMX_HOME ${svc_home:-unknown}"
        else
            why="it was started from another umx binary, and a restart runs the binary it was started from"
        fi
        say "Note: the umx daemon does not run the umx just installed ($newver): it runs $runs, because $why. To run the new one:"
        fix_cmds stop | sed 's/^/  /'
    fi

    case ":${PATH-}:" in
    *":$bin:"*)
        found=$(command -v umx 2>/dev/null || true)
        if [ -n "$found" ] && [ "$found" != "$umx" ]; then
            say "Note: umx on your PATH is $found, which comes before $bin; remove it or put $bin first."
        fi
        ;;
    *)
        say ""
        say "$bin is not on your PATH. Add this line to the end of your shell's startup file, then open a new terminal:"
        case "${SHELL-}" in
        */zsh) say "  ~/.zshrc:   export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
        */bash) say "  ~/.bashrc:  export PATH=\"\$HOME/.local/bin:\$PATH\"   (on macOS, ~/.bash_profile)" ;;
        */fish) say "  ~/.config/fish/config.fish:  fish_add_path \$HOME/.local/bin" ;;
        *) say "  ~/.profile:  export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
        esac
        ;;
    esac

    say ""
    say "umx doctor --human:"
    doctor=$(NO_COLOR=1 "$umx" doctor --human </dev/null 2>"$tmp/doctor.err") || true
    if [ -n "$doctor" ]; then say "$doctor"; fi
    cat "$tmp/doctor.err"

    # The skill line and `umx daemon install` exactly as doctor printed them; doctor leaves out what is already done.
    # The skill line is taken only whole, in the shape of doctor_skill_missing or doctor_skill_out_of_date, pinned to
    # AI-Substrate/umx at a full commit; anything else in its place gets the fixed line that says to run umx doctor.
    cmd="npx skills@[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]* add AI-Substrate\/umx#[0-9a-f]\{40\} --skill umx -g --agent '\*' -y"
    skill=$(sed -n \
        -e "s/^warning: No \/umx agent skill at .*\. Install it: \($cmd\) (or just skill-link in a umx checkout)\. \[doctor_skill_missing\]\$/\1/p" \
        -e "s/^warning: The \/umx skill at .* differs from the one this umx (.*) was built with\. Update it: \($cmd\) \[doctor_skill_out_of_date\]\$/\1/p" \
        "$tmp/doctor.err" | head -n 1)
    say ""
    say "Run these next (agents: run them; they are not run for you):"
    if [ -n "$skill" ]; then
        say "  $skill"
    elif grep -Eq '\[doctor_skill_(missing|out_of_date)\]$' "$tmp/doctor.err"; then
        say "  umx doctor   (for the /umx skill: run umx doctor and follow it)"
    fi
    if grep -qx '  next: umx daemon install' "$tmp/doctor.err"; then say "  umx daemon install"; fi
    say "  umx agents-start-here"
}

main "$@"
