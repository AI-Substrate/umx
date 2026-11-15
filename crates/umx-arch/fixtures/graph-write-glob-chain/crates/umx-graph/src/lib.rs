#![no_std]

// Review R4 chain: the root glob brings in the setup module, a private alias renames it, and a public glob of
// the alias re-exports its contents, so no `pub use` names setup or GraphFilesSetup (F144).
use umx_core::*;
use self::setup as seed;
pub use self::seed::*;
