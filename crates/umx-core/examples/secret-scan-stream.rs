//! The seed gate's secret scan (D72i L4): runs `umx_core::secrets::scan`, the scan umx runs on ingest, over files.
//!
//! stdin holds records `<path>\0<byte length>\0<bytes>`, as `scripts/seed-gate` writes them from the blobs of the
//! tree it checks. stdout holds one `<path>\t<rule name>` per flagged file and nothing else: never the text, an
//! offset or a prefix. A file that is not UTF-8 is scanned lossily (`from_utf8_lossy`), so a key inside a binary is
//! still seen. Exit 0 when every record was scanned (flagged or not); 2 on a malformed stream.
use std::io::{Read, Write};
use umx_core::secrets::scan;

fn main() {
    let mut input = Vec::new();
    if std::io::stdin().lock().read_to_end(&mut input).is_err() {
        std::process::exit(2);
    }
    let mut out = std::io::stdout().lock();
    let (mut at, mut scanned) = (0usize, 0usize);
    while at < input.len() {
        let Some(path_end) = input[at..].iter().position(|b| *b == 0).map(|n| at + n) else {
            std::process::exit(2)
        };
        let Some(len_end) = input[path_end + 1..]
            .iter()
            .position(|b| *b == 0)
            .map(|n| path_end + 1 + n)
        else {
            std::process::exit(2)
        };
        let len: usize = match std::str::from_utf8(&input[path_end + 1..len_end])
            .ok()
            .and_then(|s| s.parse().ok())
        {
            Some(n) => n,
            None => std::process::exit(2),
        };
        let body_end = len_end + 1 + len;
        if body_end > input.len() {
            std::process::exit(2);
        }
        let path = String::from_utf8_lossy(&input[at..path_end]);
        let text = String::from_utf8_lossy(&input[len_end + 1..body_end]);
        if let Some(rule) = scan(&text) {
            let _ = writeln!(out, "{path}\t{}", rule.name());
        }
        scanned += 1;
        at = body_end;
    }
    let _ = writeln!(out, "#scanned\t{scanned}");
}
