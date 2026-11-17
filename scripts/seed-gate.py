"""The checks of scripts/seed-gate (see that script for the contract). Run it only through the script.

Two modes share the checks: the seed mode reads every blob of one tree (D72i L4); the range mode reads only the blobs
that the new commits of a push add or change, and the text and identity of each of those commits.
"""
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys

REPO, SCANNER, *ARGS = sys.argv[1:]

NAMES_LIST = pathlib.Path.home() / ".config/unimatrix/customer-names"
HOSTS_LIST = pathlib.Path.home() / ".config/unimatrix/private-hosts"
MIN_NAMES = 16  # entries the real names list holds today; fewer means a truncated list (G1)
EXCLUDED_TREES = ("docs/plans", "docs/project/workshops", "docs/project/workstreams")
NOREPLY = "@users.noreply.github.com"
OLD_SLUG = b"AI-Substrate/" + b"Unimatrix"  # the old repository's name; spelled in halves so this file does not hold it
SECRET_BASELINE = "scripts/seed-gate-secret-baseline"
NAMES_EXCEPTIONS = "scripts/customer-names-exceptions"
# Binaries that may be in the seed, by exact path and sha256 (G5): the three stub model fixtures a test reads.
BINARY_ALLOW = {
    "crates/umx-adapter-llamacpp/tests/fixtures/models/umx--stub/fixture/stub.gguf":
        "9a8ae47fec88f720b91ce079afdd04bbfd699f3b33024088dd8948385faf52a6",
    "crates/umx-adapter-llamacpp/tests/fixtures/stub-bf16-widened.gguf":
        "9956c225740b6555687342bbe5e038abef110ac9be07f821c0ff71666737c8ad",
    "crates/umx-adapter-llamacpp/tests/fixtures/stub-bf16.gguf":
        "5e3bf044c8a0692d921cf17a50a483c4164c99ba6df9b028d23542e7178d6344",
}
ALLOWED_GIT_SOURCES = {  # reviewed git sources in Cargo.lock
    "git+https://github.com/utilityai/llama-cpp-rs?rev=0364f5da1c6a6ee37f507031f116ce4a09b28594"
    "#0364f5da1c6a6ee37f507031f116ce4a09b28594",
}
EMAIL_OK_DOMAINS = ("users.noreply.github.com", "example.com", "example.org", "example.net", "x.com")


def usage():
    sys.exit("usage: scripts/seed-gate <STAGE> [--receipt FILE] [--control --control-names FILE [--control-hosts FILE]]\n"
             "       scripts/seed-gate --range <base>..<head> [--repo DIR] [--receipt FILE] [--control ...]\n"
             "       scripts/seed-gate --range <head> [--remote NAME] [--repo DIR] [--receipt FILE] [--control ...]\n"
             "       scripts/seed-gate --pre-push --remote NAME --url URL [--repo DIR]   (git's pre-push lines on stdin)")


def parse():
    if not ARGS:
        usage()
    opts = {"stage": None, "receipt": None, "control": False, "names": None, "hosts": None, "list_secrets": False,
            "range": None, "repo": None, "pre_push": False, "remote": None, "url": None}
    rest = list(ARGS)
    if not rest[0].startswith("--"):
        opts["stage"] = rest.pop(0)
    while rest:
        flag = rest.pop(0)
        if flag == "--range" and rest:
            opts["range"] = rest.pop(0)
        elif flag == "--remote" and rest:
            opts["remote"] = rest.pop(0)
        elif flag == "--url" and rest:
            opts["url"] = rest.pop(0)
        elif flag == "--repo" and rest:
            opts["repo"] = rest.pop(0)
        elif flag == "--pre-push":
            opts["pre_push"] = True
        elif flag == "--control":
            opts["control"] = True
        elif flag == "--list-secret-hits":
            opts["list_secrets"] = True
        elif flag in ("--receipt", "--control-names", "--control-hosts") and rest:
            opts[{"--receipt": "receipt", "--control-names": "names", "--control-hosts": "hosts"}[flag]] = rest.pop(0)
        else:
            usage()
    if (opts["names"] or opts["hosts"]) and not opts["control"]:
        sys.exit("seed-gate: a names or hosts list is only honoured with --control; the real run reads the fixed paths")
    if opts["control"] and not opts["names"]:
        sys.exit("seed-gate: --control needs --control-names FILE (a synthetic list, never the real one)")
    modes = [bool(opts["stage"]), bool(opts["range"]), opts["pre_push"]]
    if sum(modes) != 1:
        usage()
    if opts["pre_push"] and (opts["control"] or opts["receipt"]):
        sys.exit("seed-gate: --pre-push is the real gate of a push: no control lists, no receipt")
    if opts["list_secrets"] and opts["pre_push"]:
        usage()
    return opts


def git(stage, *args, check=True, input=None):
    env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null")
    out = subprocess.run(["git", "-C", stage, *args], capture_output=True, env=env, input=input)
    if check and out.returncode != 0:
        sys.exit("seed-gate: git failed in STAGE, so the gate did not run (is STAGE a seed repository?)")
    return out


def load_list(path):
    entries = []
    for raw in pathlib.Path(path).read_text(errors="replace").splitlines():
        line = raw.strip()
        if line and not line.startswith("#"):
            entries.append(line)
    return entries


class Tree:
    """The blobs of the tree the gate checks: HEAD's tree when a commit exists, else the staged index."""

    def __init__(self, stage):
        self.stage = stage
        self.commits = int(git(stage, "rev-list", "--all", "--count", check=False).stdout.strip() or 0)
        if self.commits:
            self.tree = git(stage, "rev-parse", "HEAD^{tree}").stdout.decode().strip()
        else:
            self.tree = git(stage, "write-tree").stdout.decode().strip()
        listing = git(stage, "ls-tree", "-r", "-z", "--full-tree", self.tree).stdout
        self.entries = []  # (mode, sha, path bytes)
        for rec in listing.split(b"\0"):
            if not rec:
                continue
            meta, path = rec.split(b"\t", 1)
            mode, _kind, sha = meta.decode().split()
            self.entries.append((mode, sha, path))
        self._blobs = None
        self.log_args = ["--all"]
        self.kind = "seed"

    def files(self):
        """[(path, (blob sha, bytes))] for every file the checks read."""
        return list(self.blobs().items())

    def file(self, path):
        """(blob sha, bytes) of one path of the whole tree, or None."""
        return self.blobs().get(path)

    def blobs(self):
        """{path: bytes} for every regular file, read once through `git cat-file --batch`."""
        if self._blobs is None:
            want = [(m, s, p) for m, s, p in self.entries if m in ("100644", "100755")]
            request = b"".join(s.encode() + b"\n" for _, s, _ in want)
            raw = git(self.stage, "cat-file", "--batch", input=request).stdout
            at, found = 0, {}
            for _m, sha, path in want:
                end = raw.index(b"\n", at)
                _sha, _kind, size = raw[at:end].split()
                body = raw[end + 1:end + 1 + int(size)]
                found[path] = (sha, body)
                at = end + 1 + int(size) + 1
            self._blobs = found
        return self._blobs


ZERO = "0" * 40
# What the range checks rely on and read from the pushed head: a change to one needs security's verdict (tripwire).
GATE_FILES = {
    "scripts/seed-gate", "scripts/seed-gate.py", "scripts/seed-gate-secret-baseline", "scripts/customer-names-exceptions",
    "scripts/home-paths", "scripts/install-pre-push-hook", "crates/umx-core/examples/secret-scan-stream.rs",
    "apps/ui/scripts/check-lock-sources.mjs",
}


class RangeTree:
    """What a push adds: every new commit of a range, and the blobs those commits add or change (read from git objects).

    `revs` are rev-list arguments: [head, "^base"] for <base>..<head>, or [head, "--not", "--remotes"] for a branch the
    remote does not have yet (the commits on no remote ref, never the whole history). Deleted files are not read. A
    merge contributes only what differs from every parent (`diff-tree -c`): what it merges in is either old or another
    new commit of the range, which is read on its own.
    """

    kind = "range"

    def __init__(self, repo, revs, label):
        self.stage, self.label = repo, label
        self.log_args = list(revs)
        out = git(repo, "rev-list", *revs)
        self.commit_list = out.stdout.decode().split()
        self.commits = len(self.commit_list)
        self.head = git(repo, "rev-parse", "--verify", revs[0] + "^{commit}").stdout.decode().strip()
        self.tree = git(repo, "rev-parse", self.head + "^{tree}").stdout.decode().strip()
        self.entries = []  # (mode, sha, path) of each added or changed entry, possibly the same path more than once
        self.touched = set()  # every path a new commit adds, changes or deletes
        seen = set()
        for commit in self.commit_list:
            parents = git(repo, "rev-list", "--parents", "-n", "1", commit).stdout.decode().split()[1:]
            args = ["diff-tree", "-r", "-z", "--no-renames", "--no-abbrev", "--no-commit-id", "--root"]
            if len(parents) > 1:
                args.append("-c")
            raw = git(repo, *args, commit).stdout
            fields = raw.split(b"\0")
            i = 0
            while i < len(fields) - 1:
                head = fields[i].decode()
                path = fields[i + 1]
                i += 2
                if not head.startswith(":"):
                    continue
                n = len(head) - len(head.lstrip(":"))
                parts = head.lstrip(":").split()
                mode, sha = parts[n], parts[2 * n + 1]
                self.touched.add(path)
                if sha == ZERO or (path, sha) in seen:
                    continue
                seen.add((path, sha))
                self.entries.append((mode, sha, path))
        self._files = None
        self._whole = None

    def files(self):
        if self._files is None:
            want = [(s, p) for m, s, p in self.entries if m in ("100644", "100755")]
            raw = git(self.stage, "cat-file", "--batch", input=b"".join(s.encode() + b"\n" for s, _ in want)).stdout
            at, found = 0, []
            for sha, path in want:
                end = raw.index(b"\n", at)
                size = int(raw[at:end].split()[2])
                found.append((path, (sha, raw[end + 1:end + 1 + size])))
                at = end + 1 + size + 1
            self._files = found
        return self._files

    def file(self, path):
        """(blob sha, bytes) of a path in the head commit's whole tree (the baselines and exceptions), or None."""
        out = git(self.stage, "cat-file", "-p", self.head + ":" + path.decode("utf-8", "surrogateescape"), check=False)
        if out.returncode != 0:
            return None
        sha = git(self.stage, "rev-parse", self.head + ":" + path.decode("utf-8", "surrogateescape")).stdout.decode().strip()
        return (sha, out.stdout)


# An absolute home path whose account is not one of the synthetic examples (me, jo, Jo, j, u), as scripts/home-paths.
HOME_PATH = re.compile(rb"/(?:Users|home)/(?!(?:me|jo|Jo|j|u)(?![A-Za-z0-9._-]))[A-Za-z0-9._-]+")


def word_regex(terms, flags=re.I):
    alternation = b"|".join(re.escape(t) for t in terms)
    return re.compile(rb"(?<![A-Za-z0-9])(" + alternation + rb")(?![A-Za-z0-9])", flags)


def foreign_emails(body):
    """Matches of an email address that is not noreply, example, reserved or git's ssh user, with URL userinfo ignored."""
    plain = re.sub(rb"://[^\s/@]*@", b"://", body)
    found = []
    for m in re.finditer(rb"[A-Za-z0-9._%+-]+@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,})", plain):
        domain = m.group(1).decode().lower()
        if (domain in EMAIL_OK_DOMAINS or domain.endswith((".example.com", ".example.org"))
                or domain.rsplit(".", 1)[1] in ("example", "invalid", "test", "localhost")  # RFC 2606 names
                or domain.startswith("example.")):
            continue
        if domain == "github.com" and m.group(0).split(b"@")[0] == b"git":
            continue
        found.append((plain, m.start()))
    return found


def is_utf8(body):
    try:
        body.decode("utf-8")
        return True
    except UnicodeDecodeError:
        return False


def line_of(body, start):
    return body.count(b"\n", 0, start) + 1


# G9 reads workflows line by line, not with a YAML parser (stdlib only), so it fails closed: a runs-on, os, matrix or
# strategy it cannot read as GitHub-hosted labels is refused, and so is any YAML that could spell those keys or values in
# a way the line reading does not see (escaped or explicit keys, other line breaks, anchors in a matrix). The matching is
# by text: any line mentioning runs-on that is not the one-line runner key is refused, even in a step name or a run
# script, and in a file using matrix.os every os: key is read as a runner. A job may call a reusable workflow only from
# this repository (./.github/workflows/<file>.yml), since a job calling another repository's workflow runs where that
# workflow says; a uses: G9 cannot place inside a sequence item (a step) is read as a job's, and a file with a remote
# uses: may hold no alias that could carry one up to a job. A quoted scalar must close on the line it opens on, since
# its later lines are text G9 would read as structure (a `- ` line there makes a job's uses: look like a step's); only
# the text of a plain `key: |` or `key: >` block scalar is exempt.
# The labels G9 accepts, exactly. Adding one is a gate change and needs a security verdict (D76a: no Windows runner;
# no larger, -intel or core-count runners).
HOSTED_RUNNERS = frozenset({
    "ubuntu-latest", "ubuntu-24.04", "ubuntu-22.04", "ubuntu-24.04-arm", "ubuntu-22.04-arm",
    "macos-latest", "macos-15", "macos-14",
})
MATRIX_OS = re.compile(r"\$\{\{\s*matrix\.os\s*\}\}")
YAML_TAIL = r"(?:[ \t]+#.*)?[ \t]*$"  # an optional comment, which YAML starts only after whitespace


def yaml_key(name):
    """Any mention of `name` as a key: plain, quoted, spaced before the colon, in a block or in a flow mapping."""
    return re.compile(rf"(?i)(?<![\w.$-])['\"]?{name}['\"]?[ \t]*:(?:[ \t]|$)")


OS_KEY, MATRIX_KEY, STRATEGY_KEY = (yaml_key(k) for k in ("os", "matrix", "strategy"))
ESCAPED_KEY = re.compile(r'"(?:[^"\\]|\\.)*\\(?:[^"\\]|\\.)*"[ \t]*:(?:[ \t]|$)')
EXPLICIT_KEY = re.compile(r"^[ \t]*(?:-[ \t]+)*\?(?:[ \t]|$)")
USES_KEY = yaml_key("uses")
LOCAL_WORKFLOW = re.compile(r"\./\.github/workflows/[\w.-]+\.ya?ml")
ALIAS = re.compile(r"(?:^[ \t]*|[:?,\[{-][ \t]*)\*")  # a `*` where YAML could read an alias (or merge one in)
# a quote where YAML could start a quoted scalar: at the line start or after an indicator, past any tag or anchor
QUOTE_OPEN = re.compile(r"(?:^|(?<=[:?,\[{-]))[ \t]*(?:[!&][^ \t]*[ \t]+)*(['\"])")
QUOTED = {'"': re.compile(r'"(?:[^"\\]|\\.)*"'), "'": re.compile(r"'(?:[^']|'')*'(?!')")}


def unquoted(value):
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
        return value[1:-1]
    return value


def hosted(value):
    return unquoted(value) in HOSTED_RUNNERS


def open_quote(line):
    """True when a quoted scalar starts on this line and does not close on it."""
    pos = 0
    while m := QUOTE_OPEN.search(line, pos):
        closed = QUOTED[m.group(1)].match(line, m.start(1))
        if not closed:
            return True
        pos = closed.end()
    return False


def indent_of(line):
    return len(line) - len(line.lstrip(" \t"))


def runner_faults(body):
    """The 1-based lines of one workflow file that G9 refuses (line 1 when the file itself cannot be read)."""
    try:
        text = body.decode("utf-8")
    except UnicodeDecodeError:
        return [1]
    text = text.removeprefix("\ufeff")
    # YAML also breaks lines at a lone CR, NEL, LS and PS; splitting only at LF would let a value hide past them
    if re.search("\x00|\r(?!\n)|[\x85\u2028\u2029]", text):
        return [1]
    lines = [line.removesuffix("\r") for line in text.split("\n")]
    code = [bool(line.strip()) and not line.lstrip().startswith("#") for line in lines]
    # the content of a block scalar (`run: |`) is text, so its quotes start nothing; only the plain `key: |` form, with
    # no indentation digit, is read, and its content ends at the first line indented less than the first content line
    text_lines, content, header = set(), None, 0
    for i, line in enumerate(lines):
        spaces = len(line) - len(line.lstrip(" "))
        if content is not None and (not line.strip() or content == -1 or spaces >= content):
            if line.strip() and content == -1:
                content = spaces if spaces > header else None
            if content is not None:
                text_lines.add(i)
                continue
        content = None
        m = code[i] and re.match(rf"^( *(?:-[ \t]+)*)[\w.-]+:[ \t]+[|>][+-]?{YAML_TAIL}", line)
        if m and not open_quote(line):
            header, content = len(m.group(1)), -1

    def continues(i, col):
        """True when the next non-blank line is indented past `col`: a plain scalar going on, or a block value."""
        for j in range(i + 1, len(lines)):
            if lines[j].strip():
                return indent_of(lines[j]) > col
        return False

    def block(i, col):
        """The lines of the block value under the key at `col` on line i (a sequence may sit at the key's own column)."""
        out = []
        for j in range(i + 1, len(lines)):
            if not code[j]:
                continue
            ind = indent_of(lines[j])
            if ind > col or (ind == col and lines[j].lstrip().startswith("-")):
                out.append(j)
            else:
                break
        return out

    def item_key(i, col):
        """True when the key at `col` on line i sits under a block sequence item, so it is not a job's own key (a job's
        parent line is its id, never a `- ` item)."""
        for j in range(i - 1, -1, -1):
            if not code[j] or indent_of(lines[j]) >= col:
                continue  # nested content, a sibling key, or a sequence at a sibling key's column
            return bool(re.match(r" *-(?:[ \t]|$)", lines[j]))
        return False

    bad, os_bad, matrix_bad, uses_matrix, os_values = set(), set(), set(), [], []
    remote_uses, aliases = False, set()
    for i, line in enumerate(lines):
        if "self-hosted" in line.lower():
            bad.add(i)
        if not code[i]:
            continue
        if ESCAPED_KEY.search(line) or EXPLICIT_KEY.match(line) or (i not in text_lines and open_quote(line)):
            bad.add(i)
        if "runs-on" in line.lower():  # any spelling of the key (quoted, spaced, in a flow mapping) must be the one-line form
            m = re.match(rf"^( *)runs-on:[ \t]+(.+?){YAML_TAIL}", line)
            value = m and m.group(2)
            if not m or continues(i, len(m.group(1))):
                bad.add(i)
            elif MATRIX_OS.fullmatch(unquoted(value)):
                uses_matrix.append(i)
            elif not hosted(value):
                bad.add(i)
        if OS_KEY.search(line):
            m = re.match(rf"^( *(?:-[ \t]+)?)os:(?:[ \t]+(.+?))?{YAML_TAIL}", line)
            col = m and len(m.group(1))
            if not m:
                os_bad.add(i)
            elif m.group(2) is None:  # a block list: `os:` then `- label` lines
                items = block(i, col)
                for j in items:
                    item = re.match(rf"^( *)-[ \t]+(.+?){YAML_TAIL}", lines[j])
                    if item and indent_of(lines[j]) == indent_of(lines[items[0]]) and hosted(item.group(2)):
                        os_values.append(item.group(2))
                    else:
                        os_bad.add(j)
                if not items:
                    os_bad.add(i)
            elif continues(i, col):
                os_bad.add(i)
            elif m.group(2).startswith("["):
                flow = re.fullmatch(r"\[([^\[\]{}]*)\]", m.group(2))
                values = [v.strip() for v in flow.group(1).split(",")] if flow else []
                if values and all(hosted(v) for v in values):
                    os_values.extend(values)
                else:
                    os_bad.add(i)
            elif hosted(m.group(2)):
                os_values.append(m.group(2))
            else:
                os_bad.add(i)
        if USES_KEY.search(line):  # one line, `uses: x` or `- uses: x`; a remote one only inside a step
            m = re.match(rf"^( *(?:-[ \t]+)?)uses:[ \t]+(.+?){YAML_TAIL}", line)
            if not m:
                bad.add(i)
            elif LOCAL_WORKFLOW.fullmatch(unquoted(m.group(2))) and not continues(i, len(m.group(1))):
                pass
            elif "-" in m.group(1) or item_key(i, len(m.group(1))):
                remote_uses = True
            else:
                bad.add(i)
        if ALIAS.search(line):
            aliases.add(i)
        if STRATEGY_KEY.search(line) and not re.match(rf"^ *strategy:{YAML_TAIL}", line):
            matrix_bad.add(i)
        if MATRIX_KEY.search(line):
            m = re.match(rf"^( *)matrix:{YAML_TAIL}", line)
            if not m:
                matrix_bad.add(i)
            else:
                # what a matrix holds must be literal: no expression, anchor, alias or merge can add an os
                matrix_bad.update(j for j in block(i, len(m.group(1))) if re.search(r"\$\{\{|[*&]|<<", lines[j]))
    if uses_matrix:
        bad |= os_bad | matrix_bad
        if not os_values:
            bad.update(uses_matrix)
    if remote_uses:
        bad |= aliases
    return sorted(n + 1 for n in bad)


class Gate:
    def __init__(self, opts, tree):
        self.opts, self.tree = opts, tree
        self.results = []  # (name, status, count, hint)
        self.where = []  # operator-terminal locations, never in the receipt
        self.used_exceptions = []

    def add(self, name, ok, count, hint):
        self.results.append((name, "pass" if ok else "fail", count, "" if ok else hint))

    def locate(self, path, line=None):
        self.where.append((path, line))

    def locations(self):
        """`<path>:<line>` for the terminal, with any name, private host or home path inside a path shown as <term>."""
        patterns, _hosts = self.infra_patterns()
        try:
            entries = load_list(self.names_source())
        except OSError:
            entries = []
        if entries:
            patterns.append(word_regex([e.encode() for e in entries]))
        patterns.append(HOME_PATH)
        shown = []
        for path, line in self.where:
            for p in patterns:
                path = p.sub(b"<term>", path)
            text = path.decode("utf-8", "replace")
            shown.append(f"{text}:{line}" if line else text)
        return shown

    # G8 and G3: the shape of the repository and the tree
    def shape(self):
        bad = 0
        for mode, _sha, path in self.tree.entries:
            parts = path.split(b"/")
            if mode in ("120000", "160000") or b".git" in parts or path == b".gitmodules" or path.endswith(b"/.gitmodules"):
                bad += 1
                self.locate(path)
        if self.tree.commits:
            stage = self.tree.stage
            roots = git(stage, "rev-list", "--all", "--max-parents=0", "--count").stdout.strip()
            refs = git(stage, "for-each-ref", "--format=%(refname)").stdout.decode().split()
            dirty = git(stage, "status", "--porcelain").stdout.strip()
            bad += (self.tree.commits != 1) + (int(roots) != 1) + (refs != ["refs/heads/main"]) + (1 if dirty else 0)
        self.add("shape", bad == 0, bad,
                 "no .git, .gitmodules, links or submodules; with commits exactly 1 commit, 0 parents, only refs/heads/main "
                 "(no tags, no notes) and a clean tree: rebuild with scripts/seed-export and commit once with "
                 "GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1, which stops a global hook adding notes or commits (L7)")

    # check c and G2: the excluded trees are absent by listing
    def listing(self):
        bad = 0
        for _m, _s, path in self.tree.entries:
            text = path.decode("utf-8", "surrogateescape")
            parts = text.split("/")
            if (any(text == t or text.startswith(t + "/") for t in EXCLUDED_TREES) or "briefs" in parts
                    or ".harness" in parts or "__pycache__" in parts or text.endswith(".pyc")):
                bad += 1
                self.locate(path)
        self.add("excluded-trees", bad == 0, bad,
                 "docs/plans, docs/project/workshops, docs/project/workstreams, the briefs and .harness folders, and bytecode must not be "
                 "in the seed: re-run scripts/seed-export, which drops them")

    # G1, G2, G3, G4: customer names over every blob and every path, no skips
    def names(self):
        control = self.opts["control"]
        source = self.opts["names"] if control else NAMES_LIST
        try:
            entries = load_list(source)
        except OSError:
            entries = None
        minimum = 1 if control else MIN_NAMES
        self.names_entries = 0 if entries is None else len(entries)
        if not entries or len(entries) < minimum:
            self.add("customer-names", False, self.names_entries,
                     f"the names list is missing, unreadable or short ({self.names_entries} entries, need {minimum}); "
                     "restore ~/.config/unimatrix/customer-names (0600), the gate fails closed without it")
            return
        rx = word_regex([e.encode() for e in entries])
        excused = self.names_exceptions()
        hits = 0
        for path, (sha, body) in self.tree.files():
            if rx.search(path):
                hits += 1
                self.locate(rx.sub(b"<name>", path))  # the path shows where, never the matched term
            if excused.get(path.decode("utf-8", "surrogateescape")) == sha:
                self.used_exceptions.append({"path": path.decode("utf-8", "replace"), "blob": sha})
                continue
            for m in rx.finditer(body):
                hits += 1
                self.locate(path, line_of(body, m.start()))
        self.add("customer-names", hits == 0, hits,
                 "a customer name is in the seed: replace it with a synthetic alias (acme, globex, initech, demo) in the "
                 "source repository, then export again")

    def names_source(self):
        return self.opts["names"] if self.opts["control"] else NAMES_LIST

    def infra_patterns(self):
        patterns = [re.compile(re.escape(OLD_SLUG), re.I),
                    re.compile(rb"(^|[^A-Za-z0-9_.-])(briefs|\.harness)/"),
                    re.compile(rb"(?<![0-9.])(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3}|"
                               rb"172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}|169\.254\.\d{1,3}\.\d{1,3})(?![0-9])")]
        hosts_source = self.opts["hosts"] if self.opts["control"] else HOSTS_LIST
        hosts = None
        try:
            hosts = [h.encode() for h in load_list(hosts_source)]
        except OSError:
            pass
        if hosts:
            patterns.append(re.compile(b"|".join(re.escape(h) for h in hosts), re.I))
        return patterns, hosts

    def names_exceptions(self):
        blob = self.tree.file(NAMES_EXCEPTIONS.encode())
        found = {}
        if blob:
            for line in blob[1].decode("utf-8", "replace").splitlines():
                if line.strip() and not line.startswith("#") and ":" in line:
                    path, sha = line.rsplit(":", 1)
                    found[path] = sha.strip()
        return found

    # G6 and G7: private infrastructure, private references, personal data
    def private_infra(self):
        patterns, hosts = self.infra_patterns()
        hits = 0
        for path, (_sha, body) in self.tree.files():
            if any(p.search(path) for p in patterns):
                hits += 1
                shown = path
                for p in patterns:
                    shown = p.sub(b"<term>", shown)
                self.locate(shown)
            for p in patterns:
                for m in p.finditer(body):
                    hits += 1
                    self.locate(path, line_of(body, m.start()))
        ok = hits == 0 and bool(hosts)
        self.add("private-infrastructure", ok, hits,
                 "write the private hosts to ~/.config/unimatrix/private-hosts, one per line (the gate fails closed without "
                 "them), and remove any old repository slug, internal host, private IP or briefs-folder or .harness-folder path from the seed"
                 if not hosts else
                 "remove the old repository slug, internal host, private IP or briefs-folder or .harness-folder path from the source repository")

    def personal(self):
        keyname = re.compile(rb"(^|/)(\.env(\..*)?|id_(rsa|dsa|ecdsa|ed25519)(\.pub)?|[^/]*\.pem|[^/]*\.p12|[^/]*\.pfx)$")
        authline = re.compile(rb"_authToken|(^|[^A-Za-z])_auth\s*=|//[^\s]+:_password|npm_[A-Za-z0-9]{20,}|"
                              rb"^\s*(token|password|username)\s*=|^\s*\[registries[.\]]", re.I | re.M)
        hits = 0
        for path, (_sha, body) in self.tree.files():
            if keyname.search(path):
                hits += 1
                self.locate(path)
            base = path.rsplit(b"/", 1)[-1]
            if (base == b".npmrc" or b".cargo/config" in path) and authline.search(body):
                hits += 1
                self.locate(path)
            for plain, start in foreign_emails(body):
                hits += 1
                self.locate(path, line_of(plain, start))
        self.add("personal-data", hits == 0, hits,
                 "remove the email address, key file or registry credential from the source repository; only noreply and "
                 "example addresses may appear")

    # check 2: umx's own secret scan, over the blobs; hits are baselined by path, blob and rule
    def secret_rows(self):
        """[(path, blob sha, rule)] flagged by umx's scan, or None when the scan could not run."""
        scanner = SCANNER
        if self.opts["control"] and os.environ.get("SEED_GATE_SECRET_SCAN"):
            scanner = os.environ["SEED_GATE_SECRET_SCAN"]
        files = self.tree.files()
        # Each record is named by its index, so two versions of one path (two commits of a range) stay apart.
        stream = b"".join(str(i).encode() + b"\0" + str(len(b)).encode() + b"\0" + b for i, (_p, (_s, b)) in enumerate(files))
        out = subprocess.run([scanner], input=stream, capture_output=True)
        if out.returncode != 0:
            return None, 0
        rows, scanned = [], 0
        for row in out.stdout.decode("utf-8", "replace").splitlines():
            fields = row.split("\t")
            if fields[0] == "#scanned":
                scanned = int(fields[1])
            else:
                path, (sha, _body) = files[int(fields[0])]
                rows.append((path.decode("utf-8", "surrogateescape"), sha, fields[1]))
        return rows, scanned

    def secrets(self):
        rows, scanned = self.secret_rows()
        if rows is None:
            self.add("secret-scan", False, 0, "the secret scan failed to run; rebuild it with scripts/cargo-slot build -p umx-core --example secret-scan-stream")
            return
        baseline = set()
        blob = self.tree.file(SECRET_BASELINE.encode())
        if blob:
            for line in blob[1].decode().splitlines():
                if line.strip() and not line.startswith("#"):
                    baseline.add(tuple(line.split("\t")))
        hits = 0
        for row in rows:
            if row not in baseline:
                hits += 1
                self.locate(row[0].encode())
        stale_rows = baseline - set(rows)
        if self.tree.kind == "range":  # a range judges only the rows of the files it changes
            changed = {p.decode("utf-8", "surrogateescape") for p, _ in self.tree.files()}
            stale_rows = {r for r in stale_rows if r and r[0] in changed}
        for stale in sorted(stale_rows):  # a row for a file that changed or is no longer flagged
            hits += 1
            self.locate(stale[0].encode())
        self.add("secret-scan", hits == 0 and scanned == len(self.tree.files()), hits,
                 "umx's secret scan flags a file that is not in scripts/seed-gate-secret-baseline: remove the secret, or if "
                 "it is test or code text that only looks like one, a person reviews it and adds the line that "
                 "`scripts/seed-gate --range origin/main..HEAD --list-secret-hits` (or `scripts/seed-gate STAGE "
                 "--list-secret-hits`) prints for it (path, blob, rule). A row whose file changed or is no longer flagged "
                 "fails too: refresh or drop it")

    # G5 and check 4: raster and opaque files denied by magic bytes; other binaries only by path and sha256
    def binaries(self):
        raster = (b"\x89PNG\r\n\x1a\n", b"\xff\xd8\xff", b"GIF87a", b"GIF89a", b"II*\x00", b"MM\x00*", b"\x00\x00\x01\x00")
        opaque = (b"PK\x03\x04", b"PK\x05\x06", b"\x1f\x8b", b"\xfd7zXZ\x00", b"7z\xbc\xaf\x27\x1c", b"BZh", b"\x28\xb5\x2f\xfd",
                  b"%PDF", b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1", b"SQLite format 3\x00", b"\x00asm")
        rasters = opaques = unlisted = 0
        for path, (sha, body) in self.tree.files():
            head = body[:16]
            is_raster = (head.startswith(raster) or (head[:4] == b"RIFF" and body[8:12] == b"WEBP")
                         or (head[:2] == b"BM" and len(body) > 18 and int.from_bytes(body[14:18], "little") in (12, 40, 52, 56, 64, 108, 124)
                             and int.from_bytes(body[2:6], "little") == len(body)))
            if head.startswith(b"\x00\x00\x01\x00") and int.from_bytes(body[4:6], "little") == 0:
                is_raster = False  # an ICO header with no images is not an icon
            if is_raster:
                rasters += 1
                self.locate(path)
                continue
            if head.startswith(opaque) or body[257:262] == b"ustar":
                opaques += 1
                self.locate(path)
                continue
            if b"\x00" in body[:8192] or not is_utf8(body):  # a compressed stream has no NUL but is not text either
                digest = hashlib.sha256(body).hexdigest()
                if BINARY_ALLOW.get(path.decode("utf-8", "replace")) != digest:
                    unlisted += 1
                    self.locate(path)
        self.add("rasters", rasters == 0, rasters,
                 "an image is in the seed: a person must view the exact bytes before it can ship, so remove it from the seed "
                 "or get that review, then export again")
        self.add("opaque-and-binary", opaques + unlisted == 0, opaques + unlisted,
                 "an archive, document, database, wasm, non-UTF-8 or unlisted binary file is in the seed: the scans cannot read inside it; "
                 "remove it, or add its exact path and sha256 to BINARY_ALLOW after a person has reviewed it")

    def sizes(self):
        cap = int(os.environ.get("SEED_GATE_MAX_BYTES", "1048576"))
        big = [p for p, (_s, b) in self.tree.files() if len(b) > cap]
        for p in big:
            self.locate(p)
        self.add("size-cap", not big, len(big),
                 f"a file is over {cap} bytes: remove or shrink it, or raise SEED_GATE_MAX_BYTES after a person agrees")

    # G9: workflows run only on GitHub-hosted labels
    def workflows(self):
        bad = 0
        for path, (_sha, body) in self.tree.files():
            if not re.match(rb"\.github/workflows/[^/]+\.ya?ml$", path):
                continue
            for n in runner_faults(body):
                bad += 1
                self.locate(path, n)
        self.add("workflow-runners", bad == 0, bad,
                 "every runs-on must be one of " + ", ".join(sorted(HOSTED_RUNNERS)) + " (a new label is a gate change "
                 "and needs a security verdict); remove self-hosted, custom, larger and Windows labels. Write it on one "
                 "line, `runs-on: ubuntu-latest`, or as `runs-on: ${{ matrix.os }}` with every os a listed label: "
                 "`os: [ubuntu-latest, macos-latest]`, `- os: ubuntu-latest` in an include, or `os:` over `- ubuntu-latest` "
                 "lines. Refused: runs-on as a block or flow mapping or list (group:, labels:, - x), quoted, escaped or "
                 "explicit (?) keys, a flow-style job, a value that runs onto the next line, and expressions, anchors or "
                 "aliases inside a matrix. A job may call only a workflow in this repository, `uses: "
                 "./.github/workflows/<file>.yml`; a remote uses: must be a step's (`- uses: owner/action@sha`), and that "
                 "file may hold no YAML alias (`*name`). A quoted scalar ('x' or \"x\") must close on the line it opens "
                 "on; for text over several lines use a block scalar, `key: |`. G9 matches text, not YAML: a line "
                 "mentioning runs-on that is not the one-line runner key is refused, and in a file using matrix.os every "
                 "os: key is read as a runner, so rename the step text or the key")

    # G6: the lock files
    def locks(self):
        bad = 0
        cargo = self.tree.file(b"Cargo.lock")
        if cargo:
            for n, line in enumerate(cargo[1].decode().splitlines(), 1):
                m = re.match(r'^source = "(.*)"$', line)
                if m and m.group(1) != "registry+https://github.com/rust-lang/crates.io-index" and m.group(1) not in ALLOWED_GIT_SOURCES:
                    bad += 1
                    self.locate(b"Cargo.lock", n)
                m = re.match(r'^checksum = "(.*)"$', line)
                if m and not re.fullmatch(r"[0-9a-f]{64}", m.group(1)):
                    bad += 1
                    self.locate(b"Cargo.lock", n)
        else:
            bad += 1
        pnpm = self.tree.file(b"apps/ui/pnpm-lock.yaml")
        if pnpm:
            import tempfile
            with tempfile.NamedTemporaryFile(suffix=".yaml") as tmp:
                tmp.write(pnpm[1])
                tmp.flush()
                run = subprocess.run(["node", os.path.join(REPO, "apps/ui/scripts/check-lock-sources.mjs"), tmp.name], capture_output=True)
                if run.returncode != 0:
                    bad += max(1, len(run.stderr.splitlines()))
        else:
            bad += 1
        self.add("lock-sources", bad == 0, bad,
                 "a lock file names a source that is not crates.io, the reviewed git source, or registry.npmjs.org with sha512 "
                 "integrity: re-lock without the local proxy (the proxy rewrites locks to internal URLs)")

    # G6: delivered_by gone from the PRDs, views regenerated
    def prds(self):
        bad = 0
        for path, (_sha, body) in self.tree.files():
            if path.startswith(b"docs/project/prds/") and path.endswith(b".dd.json") and b'"delivered_by"' in body:
                bad += 1
                self.locate(path)
        stage = self.tree.stage
        doctor = subprocess.run(["ddocs", "doctor", "--path", "docs/project"], cwd=stage, capture_output=True)
        if doctor.returncode != 0:
            bad += 1
        self.add("prd-and-views", bad == 0, bad,
                 "a PRD still names delivered_by, or ddocs doctor is not clean: run scripts/seed-export, which clears them and rebuilds the views")

    # check 6: author AND committer, separately
    def commit_identity(self):
        if not self.tree.commits:
            self.results.append(("commit-identity", "pending", 0, "no commit yet; L7 commits it and the gate runs again on the commit"))
            return
        stage = self.tree.stage
        bad = 0
        for fmt in ("%ae", "%ce"):
            for email in git(stage, "log", *self.tree.log_args, f"--format={fmt}").stdout.decode().split():
                if not email.lower().endswith(NOREPLY):
                    bad += 1
        self.add("commit-identity", bad == 0, bad,
                 "author and committer must each be a ...@users.noreply.github.com address: commit with "
                 "`git -c user.name=... -c user.email=<id>+<name>@users.noreply.github.com commit`")

    # N1: the commit's own text is published with the seed: names, infrastructure, home paths and emails in it
    def commit_text(self):
        if not self.tree.commits:
            self.results.append(("commit-text", "pending", 0, "no commit yet; the gate runs again on the commit"))
            return
        text = git(self.tree.stage, "log", *self.tree.log_args, "--format=%an%n%cn%n%B").stdout
        patterns, _hosts = self.infra_patterns()
        try:
            entries = load_list(self.names_source())
        except OSError:
            entries = []
        if entries:
            patterns.append(word_regex([e.encode() for e in entries]))
        patterns.append(HOME_PATH)
        hits = sum(len(p.findall(text)) for p in patterns) + len(foreign_emails(text))
        self.add("commit-text", hits == 0, hits,
                 "the commit's author or committer name, or its message, holds a customer name, private host, old repository "
                 "slug, home path or email: commit again with a plain message and an identity from "
                 "`git -c user.name=<name> -c user.email=<id>+<name>@users.noreply.github.com`")

    # range mode: the same rule as scripts/home-paths (its allowed accounts and per-file exceptions), over changed blobs
    def home_paths(self):
        rx = re.compile(rb"(/fake|<work>)?/(Users|home)/([A-Za-z0-9._-]+)")
        allowed = {b"me", b"jo", b"Jo", b"j", b"u"}
        exceptions = set()
        script = pathlib.Path(REPO, "scripts/home-paths").read_text()
        block = re.search(r"^EXCEPTIONS=\((.*?)^\)", script, re.M | re.S)
        for line in (block.group(1).splitlines() if block else []):
            line = line.strip().replace("'", "")
            if line and not line.startswith("#") and ":" in line:
                exceptions.add(tuple(part.encode() for part in line.split(":", 1)))
        hits = 0
        for path, (_sha, body) in self.tree.files():
            if HOME_PATH.search(path):
                hits += 1
                self.locate(HOME_PATH.sub(b"<home>", path))
            for m in rx.finditer(body):
                if m.group(3) in allowed or (path, m.group(0)) in exceptions:
                    continue
                hits += 1
                self.locate(path, line_of(body, m.start()))
        self.add("home-paths", hits == 0, hits,
                 "an absolute home path is in a new or changed file: write <repo> or <home> instead, then "
                 "`git commit --amend` (or rebase the commit that added it) and push again")

    # range mode: a link, submodule or nested .git added by a new commit
    def range_shape(self):
        bad = 0
        for mode, _sha, path in self.tree.entries:
            parts = path.split(b"/")
            if mode in ("120000", "160000") or b".git" in parts or parts[-1] == b".gitmodules":
                bad += 1
                self.locate(path)
        self.add("shape", bad == 0, bad,
                 "a new commit adds a symlink, submodule, .gitmodules or a .git path: remove it with `git rm --cached <path>`, "
                 "amend the commit and push again")

    # range mode: a tripwire, not a workflow gate (Actions stays off): any workflow change needs security's verdict
    def workflow_tripwire(self):
        changed = sorted(p for p in self.tree.touched if p.startswith(b".github/workflows/"))
        for path in changed:
            self.locate(path)
        self.add("workflow-change", not changed, len(changed),
                 "a new commit adds, changes or deletes a file under .github/workflows: security gives a verdict on the exact "
                 "head first (`git rev-parse HEAD` names it); push it only after that verdict")

    # range mode: the gate cannot approve its own change. Its code, its baseline and exceptions, and the hook installer
    # are read from the pushed head, so a change to any of them needs security's verdict on that exact head.
    def gate_tripwire(self):
        changed = sorted(p for p in self.tree.touched if p.decode("utf-8", "surrogateescape") in GATE_FILES)
        for path in changed:
            self.locate(path)
        self.add("gate-change", not changed, len(changed),
                 "a new commit changes the gate, its baseline or exceptions, or the hook installer: security gives a "
                 "verdict on the exact head first (`git rev-parse HEAD` names it); push it only after that verdict")

    # range mode: the locks are judged when a new commit changes one (a local proxy rewrites them to internal URLs)
    def range_locks(self):
        if self.tree.touched & {b"Cargo.lock", b"apps/ui/pnpm-lock.yaml"}:
            self.locks()

    # git's own scripts (check 1): counts only, their output is never shown
    def scripts(self):
        stage = self.tree.stage
        for name in ("home-paths", "private-refs", "customer-names"):
            env = dict(os.environ)
            env.pop("UMX_CUSTOMER_NAMES", None)
            if name == "customer-names":
                env["HOME"] = os.environ["HOME"]
                if self.opts["control"]:
                    env["UMX_CUSTOMER_NAMES"] = self.opts["names"]
            run = subprocess.run([os.path.join(REPO, "scripts", name), stage], capture_output=True, env=env)
            hits = len(re.findall(rb"^[^\s:][^\n]*:\d+", run.stderr, re.M))
            self.add("script-" + name, run.returncode == 0, hits if run.returncode == 1 else (0 if run.returncode == 0 else 1),
                     f"scripts/{name} failed on the seed (exit {run.returncode}); run it by hand on STAGE for the locations")


LS_REMOTE_TIMEOUT = 60  # seconds; a remote that does not answer in time fails the check closed


def shown_url(url):
    """The URL for the terminal, with any user or token before an @ left out."""
    return re.sub(r"//[^/@]*@", "//", url)


def remote_exclusion(repo, remote, url=None):
    """rev-list arguments that leave out what the remote being pushed to really has, or None when it cannot be asked.

    The remote is asked (`git ls-remote <url>`), not this clone's remote-tracking refs: a clone whose remote was
    re-pointed (`git remote set-url`) keeps tracking refs to commits the new remote never saw. Only tips that also
    exist here are left out (with their history); a tip this clone lacks is skipped. A remote that reports nothing
    (empty) leaves nothing out. A remote this clone does not know by name (a push to a bare URL) leaves nothing out
    either, so the whole history of the head is checked. `url` is git's second hook argument; without it the
    remote's push URL is read from the config.
    """
    if remote not in git(repo, "remote").stdout.decode().split():
        return [], "(not a named remote here, so the whole history)"
    if not url:
        url = git(repo, "remote", "get-url", "--push", remote).stdout.decode().strip()
    env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_TERMINAL_PROMPT="0")
    try:
        out = subprocess.run(["git", "-C", repo, "ls-remote", "--", url], capture_output=True, env=env,
                             timeout=LS_REMOTE_TIMEOUT)
    except subprocess.TimeoutExpired:
        return None
    if out.returncode != 0:
        return None
    tips = sorted({line.split()[0] for line in out.stdout.decode("utf-8", "replace").splitlines() if line.strip()})
    if not tips:
        return [], f"(the remote at {shown_url(url)} is empty, so the whole history)"
    check = git(repo, "cat-file", "--batch-check", input="".join(f"{s}^{{commit}}\n" for s in tips).encode()).stdout
    here = [s for s, line in zip(tips, check.decode().splitlines()) if not line.endswith("missing")]
    if not here:
        return [], f"(none of the {len(tips)} tip(s) at {shown_url(url)} are here, so the whole history)"
    return ["--not", *here], f"--not the {len(here)} tip(s) {shown_url(url)} reports"


def ls_remote_hint(remote, url):
    target = shown_url(url) if url else remote
    return (f"`git ls-remote {target}` failed or timed out, so nothing was checked (the gate asks the remote what "
            f"it has). Run `git ls-remote {target}` to see why, or check an explicit range: "
            "`scripts/seed-gate --range <base>..<head>`")


def range_revs(repo, spec, remote=None):
    """rev-list arguments for `<base>..<head>`, or for `<head>` alone: its commits that `remote` (default origin) lacks."""
    if ".." in spec:
        if remote:
            sys.exit("seed-gate: --remote goes with --range <head>, not with <base>..<head>")
        base, head = spec.split("..", 1)
        if not base or not head or "." in base[-1:] + head[:1]:
            sys.exit("seed-gate: --range takes <base>..<head> (two dots) or a single <head>")
        return [head, "^" + base], spec
    remote = remote or "origin"
    if remote not in git(repo, "remote").stdout.decode().split():
        sys.exit(f"seed-gate: no remote named '{remote}' here; name the remote you push to with --remote "
                 "(`git remote -v` lists them), or give the range as <base>..<head>")
    found = remote_exclusion(repo, remote)
    if found is None:
        print("seed-gate: FAIL    remote-tips count=1")
        sys.exit("seed-gate: hint (remote-tips): " + ls_remote_hint(remote, None))
    exclusion, text = found
    return [spec, *exclusion], f"{spec} {text}"


def check_range(opts, repo, revs, label):
    """Runs the range checks; prints the summary; returns (passed, receipt)."""
    for rev in revs:
        if rev.startswith("--"):
            continue  # --not
        if git(repo, "rev-parse", "--verify", "--quiet", rev.lstrip("^") + "^{commit}", check=False).returncode != 0:
            sys.exit(f"seed-gate: {rev.lstrip('^')} is not a commit here; fetch it first (`git fetch`), then run again")
    tree = RangeTree(repo, revs, label)
    gate = Gate(opts, tree)
    prefix = "seed-gate:"
    receipt = {"mode": "CONTROL" if opts["control"] else "RANGE", "range": label, "head": tree.head,
               "base": revs[1].lstrip("^") if len(revs) > 1 and revs[1].startswith("^") else
                       (f"not reachable from {len(revs) - 2} remote tip(s)" if len(revs) > 2 else "none (whole history)"),
               "commits": tree.commits, "changed_blobs": 0, "range_pass": False}
    if tree.commits == 0:
        print(f"{prefix} range {label}: no new commits, nothing to push")
        receipt["range_pass"] = not opts["control"]
        receipt["checks"] = {}
        return True, receipt
    for check in (gate.range_shape, gate.names, gate.private_infra, gate.home_paths, gate.personal, gate.secrets,
                  gate.binaries, gate.sizes, gate.workflow_tripwire, gate.gate_tripwire, gate.range_locks,
                  gate.commit_identity, gate.commit_text):
        check()
    failed = [r for r in gate.results if r[1] == "fail"]
    print(f"{prefix} range {label}: {tree.commits} new commit(s), {len(tree.files())} changed blob(s)")
    for name, status, count, _hint in gate.results:
        print(f"{prefix} {status.upper():7} {name} count={count}")
    for location in gate.locations():
        print(f"  at {location}", file=sys.stderr)
    for name, status, _count, hint in gate.results:
        if status == "fail":
            print(f"{prefix} hint ({name}): {hint}")
    print(f"{prefix} {'CONTROL RUN' if opts['control'] else 'RANGE'} " + ("FAIL" if failed else "PASS"))
    receipt.update({
        "changed_blobs": len(tree.files()),
        "range_pass": not failed and not opts["control"],
        "names_list_entries": getattr(gate, "names_entries", 0),
        "checks": {n: {"status": s, "count": c} for n, s, c, _ in gate.results},
        "names_exceptions_used": gate.used_exceptions,
    })
    return not failed, receipt


def list_secret_hits(gate):
    """Prints the baseline rows for what the scan flags (the whole tree, or a range's changed files) and exits."""
    rows, _ = gate.secret_rows()
    if rows is None:
        sys.exit("seed-gate: the secret scan failed to run")
    print("# path<TAB>blob<TAB>rule: files umx's secret scan flags; a person reviews each before it is listed")
    for row in sorted(rows):
        print("\t".join(row))
    sys.exit(0)


def pre_push(opts, repo):
    """git's pre-push hook: one range per pushed ref, from the `<local ref> <local sha> <remote ref> <remote sha>` lines.

    `--remote` is git's first hook argument: the name (or URL) of the remote being pushed to. A new branch is checked
    for its commits that remote lacks; a line that is not four fields fails closed.
    """
    if not opts["remote"] or not opts["url"]:
        sys.exit("seed-gate: --pre-push needs --remote <name or URL> and --url <URL> (the hook passes git's $1 and "
                 "$2); run scripts/install-pre-push-hook again to update the hook")
    ok = True
    for line in sys.stdin.read().splitlines():
        if not line.strip():
            continue
        fields = line.split()
        if len(fields) != 4 or not all(re.fullmatch(r"[0-9a-f]{40}([0-9a-f]{24})?", f) for f in (fields[1], fields[3])):
            print("seed-gate: FAIL    pre-push-input count=1")
            print("seed-gate: hint (pre-push-input): a pre-push line is not `<local ref> <local sha> <remote ref> "
                  "<remote sha>`, so nothing was checked; push with plain `git push <remote> <branch>`")
            ok = False
            continue
        _local_ref, local, remote_ref, remote = fields
        if set(local) == {"0"}:
            print(f"seed-gate: {remote_ref} is deleted, nothing to check")
            continue
        known = set(remote) != {"0"} and git(repo, "cat-file", "-e", remote + "^{commit}", check=False).returncode == 0
        if known:
            revs, label = [local, "^" + remote], f"{remote}..{local}"
        else:
            found = remote_exclusion(repo, opts["remote"], opts["url"])
            if found is None:
                print("seed-gate: FAIL    remote-tips count=1")
                print("seed-gate: hint (remote-tips): " + ls_remote_hint(opts["remote"], opts["url"]))
                ok = False
                continue
            exclusion, text = found
            revs, label = [local, *exclusion], f"{local} {text}"
        passed, _receipt = check_range(opts, repo, revs, label)
        ok = ok and passed
    if not ok:
        print("seed-gate: push refused. Fix what the hints name, then push again; `scripts/seed-gate --range "
              "origin/main..HEAD` repeats the check without pushing")
    return ok


def main():
    opts = parse()
    if opts["range"] or opts["pre_push"]:
        repo = os.path.realpath(opts["repo"] or REPO)
        if not subprocess.run(["git", "-C", repo, "rev-parse", "--git-dir"], capture_output=True).returncode == 0:
            sys.exit("seed-gate: --repo is not a git repository")
        if opts["pre_push"]:
            sys.exit(0 if pre_push(opts, repo) else 1)
        revs, label = range_revs(repo, opts["range"], opts["remote"])
        if opts["list_secrets"]:
            list_secret_hits(Gate(opts, RangeTree(repo, revs, label)))
        passed, receipt = check_range(opts, repo, revs, label)
        if opts["receipt"]:
            pathlib.Path(opts["receipt"]).write_text(json.dumps(receipt, indent=2) + "\n")
        sys.exit(0 if passed else 1)
    stage = os.path.realpath(opts["stage"])
    repo_top = os.path.realpath(subprocess.run(["git", "-C", REPO, "rev-parse", "--show-toplevel"], capture_output=True, text=True).stdout.strip())
    stage_top = subprocess.run(["git", "-C", stage, "rev-parse", "--show-toplevel"], capture_output=True, text=True).stdout.strip()
    if not stage_top:
        sys.exit("seed-gate: STAGE is not a git repository; run scripts/seed-export first")
    if os.path.realpath(stage_top) == repo_top or stage == repo_top:
        sys.exit("seed-gate: STAGE is the repository this runs from; the gate checks an exported seed, never the private checkout")
    tree = Tree(stage)
    gate = Gate(opts, tree)
    if opts["list_secrets"]:
        list_secret_hits(gate)
    for check in (gate.shape, gate.listing, gate.names, gate.private_infra, gate.personal, gate.secrets, gate.binaries,
                  gate.sizes, gate.workflows, gate.locks, gate.prds, gate.commit_identity, gate.commit_text, gate.scripts):
        check()
    failed = [r for r in gate.results if r[1] == "fail"]
    pending = [r for r in gate.results if r[1] == "pending"]
    for name, status, count, hint in gate.results:
        print(f"seed-gate: {status.upper():7} {name} count={count}")
    for location in gate.locations():
        print(f"  at {location}", file=sys.stderr)
    for name, status, _count, hint in gate.results:
        if status in ("fail", "pending"):
            print(f"seed-gate: hint ({name}): {hint}")
    seed_pass = not failed and not pending and not opts["control"]
    print(f"seed-gate: tree {tree.tree}")
    print(f"seed-gate: {'CONTROL RUN' if opts['control'] else 'SEED'} " + ("PASS" if not failed and not pending else
          ("FAIL" if failed else "PENDING (no commit yet)")))
    if opts["receipt"]:
        receipt = {
            "mode": "CONTROL" if opts["control"] else "SEED",
            "seed_pass": seed_pass,
            "tree": tree.tree,
            "commits": tree.commits,
            "names_list_entries": getattr(gate, "names_entries", 0),
            "checks": {n: {"status": s, "count": c} for n, s, c, _ in gate.results},
            "names_exceptions_used": gate.used_exceptions,
        }
        pathlib.Path(opts["receipt"]).write_text(json.dumps(receipt, indent=2) + "\n")
    sys.exit(1 if failed else (3 if pending else 0))


main()
