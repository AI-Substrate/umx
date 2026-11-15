//! `cargo run -p umx-arch -- render`: prints blueprint 001's d-4 layer table from rules.toml.
//! `cargo run -p umx-arch -- gate-features`: prints the gate's clippy feature list (r-gate-features) for the justfile.

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let what = args.first().map(String::as_str);
    if !matches!(what, Some("render" | "gate-features")) {
        eprintln!("usage: umx-arch render | gate-features");
        return ExitCode::from(2);
    }
    match umx_arch::load_rules(&Path::new(umx_arch::CRATE_DIR).join("rules.toml")) {
        Ok(rules) if what == Some("render") => {
            println!("{}", rules.render());
            ExitCode::SUCCESS
        }
        Ok(rules) => match rules.gate_features() {
            Some(list) => {
                println!("{list}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("umx-arch: rules.toml has no r-gate-features rule");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("umx-arch: {e}");
            ExitCode::FAILURE
        }
    }
}
