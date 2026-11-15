#![no_std]
extern crate alloc;

pub fn n() -> usize {
    umx_core::ids().len()
}
