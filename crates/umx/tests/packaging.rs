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
    ] {
        assert!(!workflow.contains("test-driver"));
        assert!(!workflow.contains("--all-features"));
    }
}
