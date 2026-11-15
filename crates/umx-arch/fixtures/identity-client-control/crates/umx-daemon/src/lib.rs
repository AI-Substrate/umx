#[cfg(test)]
fn test_only() {
    umx_compose::client_actor();
}

#[cfg(all(test, feature = "fixture"))]
mod helpers {
    use umx_compose::{AgentIdentity, Stub};
    pub fn test_only() {
        Stub.current();
    }
}

pub fn ordinary() {
    let _ = "client_actor() and AgentIdentity::current() in data are not calls";
}
