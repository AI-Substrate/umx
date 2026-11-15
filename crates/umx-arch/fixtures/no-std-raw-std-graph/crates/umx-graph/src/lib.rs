#![no_std]

extern crate r#std;

pub fn review_negative_control(path: &str) {
    let _ = std::fs::read(path);
}
