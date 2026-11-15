pub fn client_actor() {}
pub trait AgentIdentity {
    fn current(&self);
}
pub struct Stub;
impl AgentIdentity for Stub {
    fn current(&self) {}
}
