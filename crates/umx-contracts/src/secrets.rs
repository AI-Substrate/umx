//! Secrets contract. A factory returns a fresh port whose environment holds only `FROM_ENV=env-value` and which has
//! no secrets file, plus a guard; `write(&port, text)` gives it a secrets file (owner-only) holding `text`.

use umx_core::ports::{SecretSource, Secrets, SecretsIssue};

pub const CLAUSES: &[&str] = &[
    "se_c1_env_beats_the_file",
    "se_c2_missing_is_none",
    "se_c3_bad_lines_are_named_by_number_only",
    "se_c4_secrets_never_format",
    "se_c5_line_ends_are_not_part_of_the_value",
    "se_c6_an_oversized_file_is_ignored_and_reported",
];

fn found<P: Secrets>(port: &P, name: &str) -> Option<(SecretSource, String)> {
    port.lookup(name).map(|(s, v)| (s, v.expose().to_owned()))
}

pub fn se_c1_env_beats_the_file<P: Secrets, G>(
    factory: impl Fn() -> (P, G),
    write: impl Fn(&P, &str),
) {
    let (port, _guard) = factory();
    write(&port, "FROM_ENV=file-value\nONLY_FILE=file-only\n");
    assert_eq!(
        found(&port, "FROM_ENV"),
        Some((SecretSource::Env, "env-value".to_owned()))
    );
    assert_eq!(
        found(&port, "ONLY_FILE"),
        Some((SecretSource::SecretsFile, "file-only".to_owned()))
    );
}

pub fn se_c2_missing_is_none<P: Secrets, G>(
    factory: impl Fn() -> (P, G),
    write: impl Fn(&P, &str),
) {
    let (port, _guard) = factory();
    assert!(port.lookup("ONLY_FILE").is_none());
    assert!(port.issues().is_empty(), "no file is not a problem");
    write(&port, "OTHER=x\nEMPTY=\n");
    assert!(port.lookup("ONLY_FILE").is_none());
    assert!(
        port.lookup("EMPTY").is_none(),
        "an empty value is no secret"
    );
}

pub fn se_c3_bad_lines_are_named_by_number_only<P: Secrets, G>(
    factory: impl Fn() -> (P, G),
    write: impl Fn(&P, &str),
) {
    let (port, _guard) = factory();
    write(&port, "GOOD=1\nsentinel-zq7 not a line\n# comment\n");
    let issues = port.issues();
    let lines: Vec<u64> = issues
        .iter()
        .map(|i| match i {
            SecretsIssue::BadLine { line, .. } => *line,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(lines, [2]);
    assert!(!format!("{issues:?}").contains("sentinel-zq7"));
}

pub fn se_c4_secrets_never_format<P: Secrets, G>(
    factory: impl Fn() -> (P, G),
    write: impl Fn(&P, &str),
) {
    let (port, _guard) = factory();
    write(&port, "KEY=sentinel-zq7\n");
    let got = port.lookup("KEY").expect("found");
    assert_eq!(got.1.expose(), "sentinel-zq7");
    let shown = format!("{got:?} {:?}", port.issues());
    assert!(!shown.contains("sentinel-zq7"), "{shown}");
    assert!(shown.contains("<redacted>"));
}

pub fn se_c5_line_ends_are_not_part_of_the_value<P: Secrets, G>(
    factory: impl Fn() -> (P, G),
    write: impl Fn(&P, &str),
) {
    let (port, _guard) = factory();
    // `lines()` drops a CRLF pair, but not trailing spaces or a final `\r` with no newline.
    write(&port, "CRLF_KEY=abc\r\nSPACES=def  \nLAST=ghi\r");
    for (name, want) in [("CRLF_KEY", "abc"), ("SPACES", "def"), ("LAST", "ghi")] {
        assert_eq!(
            found(&port, name).map(|(_, v)| v).as_deref(),
            Some(want),
            "{name}"
        );
    }
    assert!(port.issues().is_empty());
}

/// F45: a secrets file over SMALL_FILE_CAP is not read: none of its names resolve, and `issues` says why.
pub fn se_c6_an_oversized_file_is_ignored_and_reported<P: Secrets, G>(
    factory: impl Fn() -> (P, G),
    write: impl Fn(&P, &str),
) {
    let (port, _guard) = factory();
    let mut text = String::from("BIG=value\n");
    text.push_str(&"#".repeat(umx_core::ports::config_files::SMALL_FILE_CAP as usize));
    write(&port, &text);
    assert!(port.lookup("BIG").is_none());
    let reasons: Vec<_> = port
        .issues()
        .into_iter()
        .filter_map(|i| match i {
            SecretsIssue::Refused { reason, .. } => Some(reason),
            SecretsIssue::BadLine { .. } => None,
        })
        .collect();
    assert_eq!(reasons, [umx_core::ports::config_files::REFUSED_TOO_LARGE]);
}

/// One `#[test]` per Secrets clause plus a test that the list is exactly `CLAUSES`. Usage:
/// `secrets_contract!(factory, write; se_c1_<what>(), ...)`.
#[macro_export]
macro_rules! secrets_contract {
    ($factory:expr, $write:expr; $($clause:ident()),+ $(,)?) => {
        mod secrets_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::secrets::$clause($factory, $write)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::secrets::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
