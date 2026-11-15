#[expect(clippy::disallowed_methods)]
pub fn home() -> Option<String> {
    std::env::var("HOME").ok()
}
