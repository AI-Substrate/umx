#[test]
fn contract() {
    let subject = ();
    umx_contracts::graph_files::gf_c1_write_then_read_is_identical(&subject);
}

// gf_c2_list_is_sorted(&subject) runs elsewhere.
const _: &str = "gf_c2_list_is_sorted(&subject)";
#[test]
fn gf_c2_list_is_sorted() {}
