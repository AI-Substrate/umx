#[test]
fn contract() {
    let subject = ();
    umx_contracts::clock::clk_c1_reads_the_set_instant(&subject);
}
