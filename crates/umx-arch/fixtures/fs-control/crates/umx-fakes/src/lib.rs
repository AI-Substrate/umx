pub struct FakeModelFetcher;

impl FakeModelFetcher {
    pub fn fetch(&self, path: &str) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::write(path, b"model");
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}
