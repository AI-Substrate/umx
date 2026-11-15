#[test]
fn contract() {
    let subject = ();
    umx_contracts::service_manager::sm_c1_fresh_is_not_installed(&subject);
}
