//! Stamps the commit into the binary for `umx version`, and the /umx skill's fingerprint for `umx doctor` (F77).
//! Reruns only when HEAD or its branch moves, or a file under skills/umx changes.

use std::path::Path;
use std::process::Command;

/// The caps umx-daemon's doctor applies; a skill over them is fingerprinted as unknown.
const SKILL_ENTRIES: usize = 512;
const SKILL_BYTES: u64 = 8 << 20;
const SKILL_DEPTH: usize = 8;

/// The skill's fingerprint: FNV-1a 64 over each file under `root` (sorted by `/`-joined relative path; names
/// starting with `.` skipped) as path, a zero byte, the length as 8 little-endian bytes, then the bytes. Must stay
/// equal to umx-daemon's doctor `skill_fingerprint`, file selection and caps included: below the root only plain
/// folders and regular files, checked without following links. crates/umx/tests/cli/daemon.rs proves the two agree.
fn skill_fingerprint(root: &Path) -> Option<String> {
    use std::io::Read;
    fn walk(
        dir: &Path,
        rel: &str,
        depth: usize,
        seen: &mut usize,
        out: &mut Vec<(String, std::path::PathBuf)>,
    ) -> Option<()> {
        if depth > SKILL_DEPTH {
            return None;
        }
        for entry in std::fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if name.starts_with('.') {
                continue;
            }
            *seen += 1;
            if *seen > SKILL_ENTRIES {
                return None;
            }
            let rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let kind = entry.file_type().ok()?;
            if kind.is_dir() {
                walk(&entry.path(), &rel, depth + 1, seen, out)?;
            } else if kind.is_file() {
                out.push((rel, entry.path()));
            } else {
                return None;
            }
        }
        Some(())
    }
    let root = std::fs::canonicalize(root).ok()?;
    let mut files = Vec::new();
    walk(&root, "", 0, &mut 0, &mut files)?;
    files.sort();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    let mut left = SKILL_BYTES;
    for (rel, path) in files {
        let file = std::fs::File::open(path).ok()?;
        let mut data = Vec::new();
        file.take(left + 1).read_to_end(&mut data).ok()?;
        if data.len() as u64 > left {
            return None;
        }
        left -= data.len() as u64;
        eat(rel.as_bytes());
        eat(&[0]);
        eat(&(data.len() as u64).to_le_bytes());
        eat(&data);
    }
    Some(format!("{h:016x}"))
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    // The full 40-character commit: a release binary names its tag's commit exactly (docs/versioning.md).
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=UMX_COMMIT={commit}");
    // The full commit, for doctor's pinned skill install line (F94): only when skills/umx is exactly as committed,
    // so the pinned commit holds the skill this build fingerprints. A dirty skill or no git gives "unknown".
    let skill_clean =
        git(&["status", "--porcelain", "--", "../../skills/umx"]).is_some_and(|s| s.is_empty());
    let full = git(&["rev-parse", "HEAD"])
        .filter(|_| skill_clean)
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=UMX_COMMIT_FULL={full}");
    println!("cargo:rerun-if-changed=build.rs");
    // Cargo runs a build script in its package's folder (crates/umx), so this is the repo's skills/umx with no env
    // read (r-env-in-compose) and no absolute path baked in for sccache to reuse across worktrees.
    let skill = Path::new("../../skills/umx");
    let fingerprint = skill_fingerprint(skill).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=UMX_SKILL={fingerprint}");
    if fingerprint == "unknown" {
        println!(
            "cargo:warning=skills/umx could not be fingerprinted (a link, special file or cap below it); umx doctor will not check the installed skill"
        );
    }
    println!("cargo:rerun-if-changed={}", skill.display());
    let mut watch = vec!["HEAD".to_owned()];
    watch.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watch {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
