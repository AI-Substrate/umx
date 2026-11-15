#[test]
fn contract() {
    let subject = ();
    umx_contracts::embedder::em_c1_one_vector_per_input_in_order(&subject);
}
