use umx_compose::{AgentIdentity, Stub};

pub fn wrong_side() {
    umx_compose::client_actor();
    Stub.current();
}
