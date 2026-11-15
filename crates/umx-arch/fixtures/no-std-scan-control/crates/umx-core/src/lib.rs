#![no_std]
extern crate alloc;

// extern crate std is not allowed here; this comment must not trip the scan.
pub const NOTE: &str = "extern crate std";
pub const RAW: &str = r#"extern crate r#std;"#;
