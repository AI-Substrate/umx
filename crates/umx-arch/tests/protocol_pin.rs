//! R3 (architect, ws009 A5 and S6b): the graph view's socket client (apps/ui/lib/umx) speaks exactly
//! umx_protocol::PROTOCOL, since the daemon refuses any other (protocol_mismatch). So a PR that bumps the wire fails
//! here until it updates the view's pin too. Both pins are read as source text, as the other umx-arch checks read the
//! workspace, so umx-arch needs no dependency on umx-protocol.

use std::fs;
use std::path::Path;

use umx_arch::CRATE_DIR;

const RUST_FILE: &str = "crates/umx-protocol/src/lib.rs";
const RUST_PIN: &str = "pub const PROTOCOL: u32 =";
const TS_FILE: &str = "apps/ui/lib/umx/index.ts";
const TS_PIN: &str = "export const UMX_PROTOCOL =";

/// The one number `prefix` pins in `source` (a line `<prefix> <n>;`).
fn pinned(source: &str, prefix: &str) -> Result<u32, String> {
    let mut found = None;
    for line in source.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix(prefix) else {
            continue;
        };
        let value = rest.trim().trim_end_matches(';').trim();
        let n = value
            .parse::<u32>()
            .map_err(|_| format!("`{line}` does not pin a number"))?;
        if found.replace(n).is_some() {
            return Err(format!("`{prefix}` is pinned twice"));
        }
    }
    found.ok_or_else(|| format!("no line starts with `{prefix}`"))
}

fn same_protocol(rust: &str, ts: &str) -> Result<u32, String> {
    let wire = pinned(rust, RUST_PIN).map_err(|e| format!("{RUST_FILE}: {e}"))?;
    let view = pinned(ts, TS_PIN).map_err(|e| format!("{TS_FILE}: {e}"))?;
    if wire == view {
        Ok(wire)
    } else {
        Err(format!(
            "{TS_FILE} pins UMX_PROTOCOL = {view}, but umx_protocol::PROTOCOL is {wire}: the view's socket client \
             would be refused (protocol_mismatch). Set UMX_PROTOCOL to {wire} once the view reads the new wire \
             (`just ui-check`)."
        ))
    }
}

#[test]
fn the_view_speaks_the_wire_protocol() {
    let root = Path::new(CRATE_DIR).join("../..");
    let read = |p: &str| fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let n = same_protocol(&read(RUST_FILE), &read(TS_FILE)).unwrap_or_else(|e| panic!("{e}"));
    println!("R3: umx_protocol::PROTOCOL and apps/ui UMX_PROTOCOL are both {n}");
}

#[test]
fn a_mismatched_missing_or_doubled_pin_fails() {
    let rust = "/// doc\npub const PROTOCOL: u32 = 37;\n";
    assert_eq!(
        same_protocol(rust, "export const UMX_PROTOCOL = 37;\n"),
        Ok(37)
    );
    let off = same_protocol(rust, "export const UMX_PROTOCOL = 36;\n").unwrap_err();
    assert!(
        off.contains("pins UMX_PROTOCOL = 36, but umx_protocol::PROTOCOL is 37"),
        "{off}"
    );
    assert!(same_protocol(rust, "const UMX_PROTOCOL = 37;\n").is_err());
    assert!(same_protocol(rust, "export const UMX_PROTOCOL = 3_7;\n").is_err());
    assert!(
        same_protocol(
            rust,
            "export const UMX_PROTOCOL = 37;\nexport const UMX_PROTOCOL = 37;\n"
        )
        .is_err()
    );
    assert!(
        same_protocol(
            "pub const PROTOCOL: u32 = PROTO;\n",
            "export const UMX_PROTOCOL = 37;\n"
        )
        .is_err()
    );
}
