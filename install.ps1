# Installs umx on Windows inside WSL 2, as Linux: umx has no Windows build (D76b).
#
#   irm https://github.com/AI-Substrate/umx/releases/latest/download/install.ps1 | iex
#   $env:UMX_VERSION = '0.1.0'; irm https://github.com/AI-Substrate/umx/releases/latest/download/install.ps1 | iex
#   .\install.ps1 -Version 0.1.0
#
# The version is the latest release unless -Version or UMX_VERSION names one (-Version wins; a leading `v` is
# optional), checked as install.sh checks it before anything runs. It finds the default WSL distro with
# `wsl.exe -l -v` and refuses, with nothing changed, when there is no WSL, no distro, or the default distro is WSL 1.
# Then, in that distro, it has WSL's own curl, https only, find the release (the version named, or the one
# /releases/latest leads to, as its tag), download that release's install.sh and SHA256SUMS into a temporary folder,
# and run install.sh with `--version <version>` only when it matches its line in SHA256SUMS: install.sh downloads the
# same release's archive, checks it against SHA256SUMS and installs umx into the distro's ~/.local/bin
# (docs/install.md). Windows itself downloads and runs nothing; the version reaches WSL as an argument, never as shell
# text. install.sh's output is shown as it runs, and its exit code is
# install.ps1's. It needs no administrator rights, changes no execution policy and installs nothing on Windows.
#
# Run as a file, it exits with that code. Run with `irm ... | iex`, it leaves the PowerShell window open: it sets
# $LASTEXITCODE and, when the install failed, ends with an error.
#
# Everything runs in a child scope, so `irm ... | iex` sets nothing in the caller's session but $LASTEXITCODE.
& {
    param([string]$Version)
    # Set only when this block was read from install.ps1 run as a file; empty under iex or [scriptblock]::Create, even
    # inside a script, whose own exit must not be taken.
    $umxFile = $MyInvocation.MyCommand.ScriptBlock.File

    $umxInstall = & {
        param([string]$Version)
        Set-StrictMode -Version Latest
        $ErrorActionPreference = 'Stop'

        $releases = 'https://github.com/AI-Substrate/umx/releases'
        $setup = 'install WSL 2 and Ubuntu with `wsl --install` in PowerShell run as administrator, restart Windows if it asks, open Ubuntu once to make your Linux user, then run this again; see https://learn.microsoft.com/windows/wsl/install'

        function Fail([string]$what, [string]$hint) {
            [Console]::Error.WriteLine("install.ps1: $what ($hint)")
            1
        }

        # wsl.exe's output: UTF-8 when it honours WSL_UTF8, else UTF-16LE, which reads as UTF-8 with a NUL after each
        # character. Either way, only printable ASCII is kept: distro names and the -l -v columns are ASCII.
        function Invoke-Wsl([string]$wsl, [string[]]$wslArgs) {
            $ErrorActionPreference = 'Continue'
            $utf8 = $env:WSL_UTF8
            $encoding = $null
            try {
                $env:WSL_UTF8 = '1'
                try { $encoding = [Console]::OutputEncoding; [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch { $encoding = $null }
                $lines = @(& $wsl @wslArgs 2>&1 | ForEach-Object { ("$_" -replace '[^\x20-\x7E]', '') })
                @{ Code = $LASTEXITCODE; Lines = $lines }
            } finally {
                $env:WSL_UTF8 = $utf8
                if ($null -ne $encoding) { try { [Console]::OutputEncoding = $encoding } catch { Write-Verbose "install.ps1: the console encoding was not restored: $_" } }
            }
        }

        $want = if ($Version) { $Version } else { $env:UMX_VERSION }
        if ($null -eq $want) { $want = '' }
        if ($want.StartsWith('v', [StringComparison]::Ordinal)) { $want = $want.Substring(1) }
        if ($want -ne '' -and $want -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?\z') {
            return Fail 'the -Version or UMX_VERSION given is not a version' 'use one like 0.1.0 or v0.1.0; nothing changed'
        }

        $wsl = Get-Command wsl.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($null -eq $wsl) {
            return Fail 'WSL is not installed on this Windows: umx runs on Windows only inside WSL 2' "$setup; nothing changed"
        }
        $wsl = $wsl.Source

        $list = Invoke-Wsl $wsl @('-l', '-v')
        $default = $null
        if ($list.Code -eq 0) {
            foreach ($line in $list.Lines) {
                # `* <name> <state> <version>`; the state is in Windows' language and may be more than one word.
                if ($line -match '^\s*\*\s*(\S+)\s.*\s([0-9]+)\s*$') {
                    $default = @{ Name = $Matches[1]; Wsl = $Matches[2] }
                    break
                }
            }
        }
        if ($null -eq $default) {
            return Fail 'WSL has no default Linux distro, or `wsl.exe -l -v` did not list one' "$setup; nothing changed"
        }
        $distro = $default.Name
        if ($default.Wsl -ne '2') {
            return Fail "the default WSL distro $distro runs WSL $($default.Wsl), and umx needs WSL 2" "convert it with ``wsl --set-version $distro 2`` in PowerShell, or make a WSL 2 distro the default with ``wsl --set-default <distro>``, then run this again; nothing changed"
        }

        # Runs in the distro, as sh: no double quotes or backslashes, so Windows PowerShell 5.1 and PowerShell 7 pass it to
        # wsl.exe the same. $1 is the releases URL; $2, when given, is the version. With no version it first resolves
        # /releases/latest to that release's tag, so install.sh, SHA256SUMS and the archive install.sh fetches all come
        # from one release, even if another is published meanwhile. It downloads that release's install.sh and SHA256SUMS
        # into a private temporary folder and runs install.sh only when it matches its line in SHA256SUMS.
        $script = 'set -euf; r=$1; v=${2-}; d=; ' +
            'no() { rm -rf $d; echo install.ps1: $@ >&2; exit 1; }; ' +
            'command -v curl >/dev/null 2>&1 || no curl is not installed in this WSL distro, so install.sh cannot be downloaded: install curl, for example sudo apt install curl, then run install.ps1 again; ' +
            'command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 || no neither sha256sum nor shasum is installed in this WSL distro, so install.sh cannot be checked: install coreutils, then run install.ps1 again; ' +
            'g() { curl --proto =https --proto-redir =https --tlsv1.2 -fsSL $@; }; ' +
            'if [ ${#v} -eq 0 ]; then ' +
            'u=$(g -I -o /dev/null -w %{url_effective} $r/latest) || no could not find the latest umx release at $r/latest inside WSL: check the connection, nothing changed; ' +
            'v=${u#$r/tag/v}; case $u in $r/tag/v[0-9]*) ;; *) v=/ ;; esac; ' +
            'case $v in *[!0-9A-Za-z.-]*) no $r/latest led to $u, not to a umx release: nothing changed ;; esac; ' +
            'fi; ' +
            'd=$(mktemp -d /tmp/umx-install-sh.XXXXXX); u=$r/download/v$v; ' +
            'for a in install.sh SHA256SUMS; do g -o $d/$a $u/$a || no could not download $u/$a inside WSL: check the version and the connection, nothing changed; done; ' +
            'w=$(awk -v f=install.sh ''NF == 2 && $2 == f { print $1; n++ } END { exit n == 1 ? 0 : 1 }'' $d/SHA256SUMS) || w=; ' +
            'case $w in *[!0-9a-f]*) w= ;; esac; ' +
            '[ ${#w} -eq 64 ] || no SHA256SUMS of umx $v has no single valid line for install.sh, so install.sh was not run: nothing changed; ' +
            'if command -v sha256sum >/dev/null 2>&1; then s=$(sha256sum $d/install.sh); else s=$(shasum -a 256 $d/install.sh); fi; ' +
            '[ x${s%% *} = x$w ] || no install.sh of umx $v does not match its sha256 in SHA256SUMS, so it was not run: the download is not the published file, nothing changed; ' +
            'echo install.ps1: checked install.sh of umx $v against its SHA256SUMS.; ' +
            'rc=0; sh $d/install.sh --version $v || rc=$?; rm -rf $d; exit $rc'
        $shArgs = @($releases)
        if ($want -ne '') { $shArgs += $want }

        $release = if ($want -ne '') { "umx $want" } else { 'the latest umx' }
        Write-Host "install.ps1: installing $release inside WSL 2, in the default distro $distro, with that release's install.sh..."
        $encoding = $null
        try {
            try { $encoding = [Console]::OutputEncoding; [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch { $encoding = $null }
            & $wsl -d $distro -e sh -c $script sh @shArgs | Out-Host
            $code = $LASTEXITCODE
        } finally {
            if ($null -ne $encoding) { try { [Console]::OutputEncoding = $encoding } catch { Write-Verbose "install.ps1: the console encoding was not restored: $_" } }
        }
        Write-Host ''
        if ($code -eq 0) {
            Write-Host "install.ps1: umx is installed inside WSL ($distro), not on Windows. Open WSL with ``wsl`` and run umx there, including each line under ""Run these next"" above."
        } else {
            [Console]::Error.WriteLine("install.ps1: install.sh failed inside WSL ($distro) with exit code $code (its message is above)")
        }
        $code
    } $Version

    if ($umxFile) { exit $umxInstall }
    $global:LASTEXITCODE = $umxInstall
    if ($umxInstall -ne 0) { throw "install.ps1: umx was not installed (exit code $umxInstall; the reason is above)" }
} @args
