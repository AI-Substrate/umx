fn main() -> std::process::ExitCode {
    // ws009 M1b2 evidence: a stand-in browser opener and a short idle stop, from `<this binary>.knobs` when there is
    // one (`opener=<path>`, `idle_ms=<n>` lines); never in a release build.
    let knobs = std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::read_to_string(exe.with_extension("knobs")).ok())
        .unwrap_or_default();
    let knob = |name: &str| {
        knobs
            .lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
            .map(str::to_owned)
    };
    umx::ui_test_knobs(
        knob("opener").map(std::path::PathBuf::from),
        knob("idle_ms").and_then(|v| v.parse().ok()),
    );
    umx::run(std::env::args().skip(1), umx::generous_budgets())
}
