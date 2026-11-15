fn main() -> std::process::ExitCode {
    umx::run(
        std::env::args().skip(1),
        umx_compose::ClientBudgets::default(),
    )
}
