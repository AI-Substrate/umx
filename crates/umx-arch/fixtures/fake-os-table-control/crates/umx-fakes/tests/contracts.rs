#[test]
fn contract() {
    let subject = ();
    umx_contracts::service_manager::sm_c1_fresh_is_not_installed(&subject);
    umx_contracts::service_manager::sm_c2_install_is_read_back_and_loaded(&subject);
}
