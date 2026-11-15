#[test]
fn contract() {
    let subject = ();
    umx_contracts::graph_files::gf_c1_write_then_read_is_identical(&subject);
    umx_contracts::graph_files::gf_c2_list_is_sorted(&subject);
}
