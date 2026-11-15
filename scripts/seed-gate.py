"""D72i L4: the checks of scripts/seed-gate (see that script for the contract). Run it only through the script."""
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
    sys.exit("usage: scripts/seed-gate <STAGE> [--receipt FILE] [--control --control-names FILE [--control-hosts FILE]]")


def parse():
    if not ARGS:
        usage()
    opts = {"stage": ARGS[0], "receipt": None, "control": False, "names": None, "hosts": None, "list_secrets": False}
    rest = ARGS[1:]
    while rest:
        flag = rest.pop(0)
        if flag == "--control":
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


class Gate:
    def __init__(self, opts, tree):
        self.opts, self.tree = opts, tree
        self.results = []  # (name, status, count, hint)
        self.where = []  # operator-terminal locations, never in the receipt
        self.used_exceptions = []

    def add(self, name, ok, count, hint):
        self.results.append((name, "pass" if ok else "fail", count, "" if ok else hint))

    def locate(self, path, line=None):
        text = path.decode("utf-8", "replace")
        self.where.append(f"{text}:{line}" if line else text)

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
        for path, (sha, body) in self.tree.blobs().items():
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
        blob = self.tree.blobs().get(NAMES_EXCEPTIONS.encode())
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
        for path, (_sha, body) in self.tree.blobs().items():
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
        for path, (_sha, body) in self.tree.blobs().items():
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
        blobs = self.tree.blobs()
        stream = b"".join(p + b"\0" + str(len(b)).encode() + b"\0" + b for p, (_s, b) in blobs.items())
        out = subprocess.run([scanner], input=stream, capture_output=True)
        if out.returncode != 0:
            return None, 0
        rows, scanned = [], 0
        for row in out.stdout.decode("utf-8", "replace").splitlines():
            fields = row.split("\t")
            if fields[0] == "#scanned":
                scanned = int(fields[1])
            else:
                rows.append((fields[0], blobs[fields[0].encode("utf-8", "surrogateescape")][0], fields[1]))
        return rows, scanned

    def secrets(self):
        rows, scanned = self.secret_rows()
        if rows is None:
            self.add("secret-scan", False, 0, "the secret scan failed to run; rebuild it with scripts/cargo-slot build -p umx-core --example secret-scan-stream")
            return
        baseline = set()
        blob = self.tree.blobs().get(SECRET_BASELINE.encode())
        if blob:
            for line in blob[1].decode().splitlines():
                if line.strip() and not line.startswith("#"):
                    baseline.add(tuple(line.split("\t")))
        hits = 0
        for row in rows:
            if row not in baseline:
                hits += 1
                self.locate(row[0].encode())
        for stale in sorted(baseline - set(rows)):  # a row for a file that changed or is no longer flagged
            hits += 1
            self.locate(stale[0].encode())
        self.add("secret-scan", hits == 0 and scanned == len(self.tree.blobs()), hits,
                 "umx's secret scan flags a file that is not in scripts/seed-gate-secret-baseline: remove the secret, or if "
                 "it is test or code text that only looks like one, a person reviews it and adds the line that "
                 "(a baseline row whose file changed or is no longer flagged also fails: drop or refresh it) "
                 "`scripts/seed-gate STAGE --list-secret-hits` prints for it (path, blob, rule)")

    # G5 and check 4: raster and opaque files denied by magic bytes; other binaries only by path and sha256
    def binaries(self):
        raster = (b"\x89PNG\r\n\x1a\n", b"\xff\xd8\xff", b"GIF87a", b"GIF89a", b"II*\x00", b"MM\x00*", b"\x00\x00\x01\x00")
        opaque = (b"PK\x03\x04", b"PK\x05\x06", b"\x1f\x8b", b"\xfd7zXZ\x00", b"7z\xbc\xaf\x27\x1c", b"BZh", b"\x28\xb5\x2f\xfd",
                  b"%PDF", b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1", b"SQLite format 3\x00", b"\x00asm")
        rasters = opaques = unlisted = 0
        for path, (sha, body) in self.tree.blobs().items():
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
        big = [p for p, (_s, b) in self.tree.blobs().items() if len(b) > cap]
        for p in big:
            self.locate(p)
        self.add("size-cap", not big, len(big),
                 f"a file is over {cap} bytes: remove or shrink it, or raise SEED_GATE_MAX_BYTES after a person agrees")

    # G9: workflows run only on GitHub-hosted labels
    def workflows(self):
        label = re.compile(r"^(ubuntu|macos|windows)-(latest|\d+(\.\d+)?)(-arm|-intel|-xlarge)?$")
        bad = 0
        for path, (_sha, body) in self.tree.blobs().items():
            if not re.match(rb"\.github/workflows/[^/]+\.ya?ml$", path):
                continue
            text = body.decode("utf-8", "replace")
            matrix = [v.strip(" '\"") for m in re.finditer(r"^\s*os:\s*\[(.*?)\]", text, re.M) for v in m.group(1).split(",")]
            for n, line in enumerate(text.splitlines(), 1):
                if "self-hosted" in line.lower():
                    bad += 1
                    self.locate(path, n)
                m = re.match(r"^\s*runs-on:\s*(.+?)\s*(#.*)?$", line)
                if not m:
                    continue
                value = m.group(1).strip("'\"")
                if value.startswith("${{") and "matrix.os" in value:
                    ok = bool(matrix) and all(label.match(v) for v in matrix)
                else:
                    ok = bool(label.match(value))
                if not ok:
                    bad += 1
                    self.locate(path, n)
        self.add("workflow-runners", bad == 0, bad,
                 "every runs-on must be a GitHub-hosted label (ubuntu-latest, macos-latest, windows-latest or a numbered image); "
                 "remove self-hosted and custom labels")

    # G6: the lock files
    def locks(self):
        bad = 0
        blobs = self.tree.blobs()
        cargo = blobs.get(b"Cargo.lock")
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
        pnpm = blobs.get(b"apps/ui/pnpm-lock.yaml")
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
        for path, (_sha, body) in self.tree.blobs().items():
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
            for email in git(stage, "log", "--all", f"--format={fmt}").stdout.decode().split():
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
        text = git(self.tree.stage, "log", "--all", "--format=%an%n%cn%n%B").stdout
        patterns, _hosts = self.infra_patterns()
        try:
            entries = load_list(self.names_source())
        except OSError:
            entries = []
        if entries:
            patterns.append(word_regex([e.encode() for e in entries]))
        patterns.append(re.compile(rb"/(?:Users|home)/(?!(?:me|jo|Jo|j|u)(?![A-Za-z0-9._-]))[A-Za-z0-9._-]+"))
        hits = sum(len(p.findall(text)) for p in patterns) + len(foreign_emails(text))
        self.add("commit-text", hits == 0, hits,
                 "the commit's author or committer name, or its message, holds a customer name, private host, old repository "
                 "slug, home path or email: commit again with a plain message and an identity from "
                 "`git -c user.name=<name> -c user.email=<id>+<name>@users.noreply.github.com`")

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


def main():
    opts = parse()
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
        rows, _ = gate.secret_rows()
        if rows is None:
            sys.exit("seed-gate: the secret scan failed to run")
        print("# path<TAB>blob<TAB>rule: files umx's secret scan flags; a person reviews each before it is listed")
        for row in sorted(rows):
            print("\t".join(row))
        sys.exit(0)
    for check in (gate.shape, gate.listing, gate.names, gate.private_infra, gate.personal, gate.secrets, gate.binaries,
                  gate.sizes, gate.workflows, gate.locks, gate.prds, gate.commit_identity, gate.commit_text, gate.scripts):
        check()
    failed = [r for r in gate.results if r[1] == "fail"]
    pending = [r for r in gate.results if r[1] == "pending"]
    for name, status, count, hint in gate.results:
        print(f"seed-gate: {status.upper():7} {name} count={count}")
    for location in gate.where:
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
