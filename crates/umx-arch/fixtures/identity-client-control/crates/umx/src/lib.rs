use umx_compose::{AgentIdentity, Stub};

pub fn client() {
    umx_compose::client_actor();
    Stub.current();
}
