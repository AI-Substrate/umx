//! `umx check`'s .gitignore verdict against real git (reviewer FU67-1): for each generated `.gitignore`, `git
//! check-ignore` decides whether every temp and marker name fs.graph-files can make is hidden, under core.ignorecase
//! false and true, and umx check must agree (no `gitignore_line_missing` row exactly when git hides them all under
//! both). Skipped, with a note, where git is not installed.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use umx_core::codec::GraphFile;
use umx_core::ports::GraphDir;
use umx_fakes::{FakeClock, FakeGraphFiles, FakeIds, FakeLocalState};
use umx_services::{Ctx, Ports, Services};

/// The names checked with git: the marker, and `.umx-tmp-<n>-<file>` for counters of one to seven digits. The
/// generated patterns tell counters apart only by their first digits and length, which these cover.
fn names() -> Vec<String> {
    let mut out = vec![".umx-tmp-set.json".to_string()];
    for n in [0, 1, 2, 3, 5, 7, 9, 10, 12, 19, 23, 90, 99, 100, 1234567] {
        for file in [
            "graph.json",
            "nodes.jsonl",
            "edges.jsonl",
            ".gitignore",
            "set.json",
        ] {
            out.push(format!(".umx-tmp-{n}-{file}"));
        }
    }
    out
}

struct Git {
    repo: PathBuf,
}

impl Git {
    /// A fresh repository that sees no user or system git config and no global excludes file; None only when no
    /// git executable is found (review F2: any other failure fails the test).
    fn new() -> Option<Git> {
        match Command::new("git").arg("--version").output() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => panic!("git --version: {e}"),
            Ok(out) => assert!(out.status.success(), "git --version: {out:?}"),
        }
        let repo =
            std::env::temp_dir().join(format!("umx-gitignore-vs-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = Git { repo };
        let init = git
            .command()
            .args(["init", "-q", "--template="])
            .output()
            .unwrap();
        assert!(init.status.success(), "git init: {init:?}");
        Some(git)
    }

    fn command(&self) -> Command {
        let mut c = Command::new("git");
        c.current_dir(&self.repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("HOME", &self.repo)
            .env("XDG_CONFIG_HOME", self.repo.join("xdg"));
        c
    }

    /// Whether git ignores every one of `names` under this `.gitignore` and core.ignorecase `fold`.
    fn hides_all(&self, gitignore: &[u8], names: &[String], fold: bool) -> bool {
        std::fs::write(self.repo.join(".gitignore"), gitignore).unwrap();
        let mut child = self
            .command()
            .args([
                "-c",
                &format!("core.ignorecase={fold}"),
                "check-ignore",
                "--stdin",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = names.join("\n") + "\n";
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        // check-ignore exits 0 when some path is ignored and 1 when none is; anything else is an error.
        assert!(
            matches!(out.status.code(), Some(0 | 1)),
            "git check-ignore: {out:?}"
        );
        String::from_utf8(out.stdout).unwrap().lines().count() == names.len()
    }
}

impl Drop for Git {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.repo);
    }
}

/// A graph over the fakes, and a way to ask umx check about a `.gitignore`.
struct Umx {
    files: Arc<FakeGraphFiles>,
    svc: Services,
}

impl Umx {
    fn new() -> Umx {
        let files = Arc::new(FakeGraphFiles::new(GraphDir::parse("/work").unwrap()));
        let ports = Ports {
            sources: std::sync::Arc::new(umx_fakes::FakeSourceReader::open(
                umx_core::ports::GraphDir::parse("/").unwrap(),
            )),
            files: files.clone(),
            vector_cache: Arc::new(FakeGraphFiles::new(GraphDir::parse("/cache").unwrap())),
            vector_cache_root: GraphDir::parse("/cache").unwrap(),
            clock: Arc::new(FakeClock::default()),
            ids: Arc::new(FakeIds::new()),
            state: Arc::new(FakeLocalState::new()),
            scopes: Arc::new(FakeLocalState::new()),
            registry: Arc::new(FakeLocalState::new()),
            allows: Arc::new(FakeLocalState::new()),
            lint_terms: Arc::new(FakeLocalState::new().with_absent_ok(true)),
        };
        let mut svc = Services::new(ports);
        svc.graph_new(&Ctx::new("tester"), "/work/g", "demo", None, false, None)
            .unwrap();
        Umx { files, svc }
    }

    fn hides_all(&self, gitignore: &[u8]) -> bool {
        let g = GraphDir::parse("/work/g").unwrap();
        self.files
            .tamper_for_tests(&g, GraphFile::GitIgnore, gitignore);
        let rows = self
            .svc
            .check(&Ctx::new("tester"), None)
            .unwrap()
            .value
            .page
            .rows;
        !rows.iter().any(|r| r.kind == "gitignore_line_missing")
    }
}

/// xorshift64*: the cases are the same on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.below(from.len())]
    }
}

/// Pieces of patterns that touch the names: POSIX classes (good, unknown and unclosed), ranges, escapes, case.
const PIECES: &[&str] = &[
    ".umx-tmp-",
    ".UMX-TMP-",
    ".umx-",
    "*",
    "**",
    "?",
    "[[:digit:]]",
    "[[:alpha:]]",
    "[[:alnum:]]",
    "[[:lower:]]",
    "[[:upper:]]",
    "[[:punct:]]",
    "[[:xdigit:]]",
    "[[:space:]]",
    "[[:graph:]]",
    "[^[:digit:]]",
    "[![:lower:]]",
    "[[:digit:][:punct:]]",
    "[[:digit]",
    "[[:foo:]]",
    "[[:x]-]",
    "[0-9]",
    "[1-9]",
    "[a-z]",
    "[A-Z]",
    "[!0-9]",
    "[]-]",
    "[-.]",
    "[a-]",
    "[\\]]",
    "[\\-]",
    "[",
    "]",
    "\\.",
    "\\U",
    "\\u",
    "-",
    "1",
    "0",
    "set.json",
    "nodes.jsonl",
    "NODES.jsonl",
    ".jsonl",
    "graph.json",
    "edges",
    ".gitignore",
];

const PREFIXES: &[&str] = &["", "", "", "/", "**/", "\\#", "#"];
const ENDS: &[&str] = &["", "", "", " ", "  ", "\t", "\\ ", "\r", " \r", "/", "\\"];

fn line(rng: &mut Rng) -> String {
    let mut out = String::new();
    if rng.below(3) > 0 {
        out.push('!');
    }
    out.push_str(rng.pick(PREFIXES));
    for _ in 0..1 + rng.below(5) {
        out.push_str(rng.pick(PIECES));
    }
    out.push_str(rng.pick(ENDS));
    out
}

fn cases() -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = [
        // The review matrix, then the classes and case rules it left open.
        ".umx-tmp-*\n",
        "/.umx-tmp-*   \n",
        "  .umx-tmp-*\n",
        "!.umx-tmp-*\n.umx-tmp-*\n",
        ".umx-tmp-*\n!.umx-tmp-*\n",
        ".umx-tmp-*\n!.umx-tmp-1-nodes.jsonl\n",
        ".umx-tmp-*\n!.umx-tmp-*-edges.jsonl\n",
        ".umx-tmp-*\n!keep[0-9].txt\n",
        ".umx-tmp-*\n!.umx-tmp-*/\n",
        ".umx-tmp-*\n!.umx-tmp-set.json\n.umx-tmp-set.json\n",
        ".umx-tmp-*\n!.umx-tmp-[[:digit:]]-nodes.jsonl\n",
        ".umx-tmp-*\n!.umx-tmp-[[:digit:]]*\n",
        ".umx-tmp-*\n!.umx-tmp-[^[:alpha:]]*\n",
        ".umx-tmp-*\n!.umx-tmp-[[:alpha:]]*\n",
        ".umx-tmp-*\n!.umx-tmp-[[:foo:]]*\n",
        ".umx-tmp-*\n!.umx-tmp-[0-9*\n",
        ".umx-tmp-*\n!.UMX-TMP-*\n",
        ".umx-tmp-*\n!.umx-tmp-*-[[:upper:]]*\n",
        ".umx-tmp-*\n!.umx-tmp-*-[A-Z]*\n",
        ".umx-tmp-*\n!.umx-tmp-*-\\N*\n",
        ".UMX-TMP-*\n",
        ".umx-tmp-*\t\n",
        ".umx-tmp-*\\ \n",
        ".umx-tmp-*\r\n",
        "\u{feff}.umx-tmp-*\n",
        "#.umx-tmp-*\n",
        "\\#.umx-tmp-*\n.umx-tmp-*\n!\\#*\n",
    ]
    .iter()
    .map(|c| c.as_bytes().to_vec())
    .collect();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for i in 0..600 {
        let mut text = String::new();
        if i % 3 != 0 {
            text.push_str(".umx-tmp-*\n");
        }
        for _ in 0..1 + rng.below(3) {
            text.push_str(&line(&mut rng));
            text.push_str(if rng.below(4) == 0 { "\r\n" } else { "\n" });
        }
        out.push(text.into_bytes());
    }
    out
}

#[test]
fn umx_check_agrees_with_git_check_ignore() {
    let Some(git) = Git::new() else {
        eprintln!("skipped: git is not installed");
        return;
    };
    let (names, umx) = (names(), Umx::new());
    let mut disagree = Vec::new();
    let mut hidden = 0;
    for case in cases() {
        let by_git = git.hides_all(&case, &names, false) && git.hides_all(&case, &names, true);
        hidden += usize::from(by_git);
        if umx.hides_all(&case) != by_git {
            disagree.push((String::from_utf8_lossy(&case).into_owned(), by_git));
        }
    }
    assert!(
        disagree.is_empty(),
        "{} cases where umx check and git disagree (case, git hides all):\n{disagree:#?}",
        disagree.len()
    );
    // Both answers occur, so neither side can pass by always giving one.
    assert!(
        hidden > 50 && hidden < 550,
        "{hidden} of the cases hide every name"
    );
}
