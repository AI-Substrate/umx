#[cfg(test)]
mod tests {
    fn subject() {}

    umx_contracts::graph_files_contract!(
        subject;
        gf_c1_write_then_read_is_identical,
        // gf_c2_list_is_sorted,
    );
}
