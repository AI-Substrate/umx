#![no_std]
extern crate alloc;
mod other;
mod settings;
// A local module named like a name another file binds from umx-core (merge with): not a re-export.
pub use settings::GraphSet;
// crate:: and super:: name the root module here, where `settings` is a mod, not the umx_core import in other.rs.
pub use crate::settings::GraphSet as RootSet;
