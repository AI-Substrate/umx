use std::process::Command;

use umx_protocol::Envelope;

#[test]
fn install_and_shipping_recipes_do_not_enable_the_fixture_driver() {
    let justfile = include_str!("../../../justfile");
    for recipe in ["dev", "bench *flags"] {
        let body = justfile
            .split_once(&format!("\n{recipe}:\n"))
            .unwrap_or_else(|| panic!("missing shipping recipe {recipe}"))
            .1
            .split("\n\n")
            .next()
            .unwrap();
        assert!(!body.contains("test-driver"), "{recipe}: {body}");
        assert!(!body.contains("--all-features"), "{recipe}: {body}");
    }
    let install = include_str!("../../../scripts/dev-restart.sh");
    assert!(!install.contains("umx-test-driver"));
    assert!(!install.contains("umx-fake-pij"));
    for workflow in [
        include_str!("../../../.github/workflows/check.yml"),
        include_str!("../../../.github/workflows/conformance.yml"),
        include_str!("../../../.github/workflows/release.yml"),
    ] {
        assert!(!workflow.contains("test-driver"));
        assert!(!workflow.contains("--all-features"));
    }
}

/// ws012 s7: `umx version` names the full 40-character commit this binary was built from and the workspace version,
/// so a release binary names its tag's commit exactly (docs/versioning.md). The release workflow's smoke run checks
/// the same two fields against the commit it checked out.
#[test]
fn version_reports_the_full_commit_it_was_built_from() {
    let home = std::env::temp_dir().join(format!("umx-t-{}-version-commit", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_umx"))
        .arg("version")
        .env("UMX_HOME", &home)
        .env("HOME", &home)
        .env_remove("UMX_OUTPUT")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&home);
    assert!(out.status.success(), "{out:?}");
    let env = Envelope::from_json(&out.stdout).unwrap();
    let cli = &env.data["cli"];
    let commit = cli["commit"].as_str().unwrap();
    assert!(
        commit.len() == 40
            && commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{cli}"
    );
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(head.status.success(), "git rev-parse HEAD failed");
    assert_eq!(commit, String::from_utf8_lossy(&head.stdout).trim());
    assert_eq!(cli["version"], env!("CARGO_PKG_VERSION"));
}
