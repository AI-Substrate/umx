fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let reply = match args.as_slice() {
        [json, verb] if json == "--json" && verb == "whoami" => {
            r#"{"ok":true,"data":{"id":"pij-fast-fixture","harness":"copilot"}}"#
        }
        [json, verb, seat] if json == "--json" && verb == "state" && seat == "pij-fast-fixture" => {
            r#"{"ok":true,"data":{"sessionStatus":{"status":{"model":{"value":"fixture-model"}}}}}"#
        }
        _ => return std::process::ExitCode::from(64),
    };
    println!("{reply}");
    std::process::ExitCode::SUCCESS
}
