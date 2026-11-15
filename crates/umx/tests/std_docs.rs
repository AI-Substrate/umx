//! ws008 T1 (D71 R5): the organise topic teaches std v1 from the one table in umx-core, so a std name or status the
//! table gains and the docs miss fails here.

use umx_core::ontology::{NOT_SAME_AS, SAME_AS};
use umx_core::standard::{self, Group, Standing};

const ORGANISE: &str = include_str!("../../umx-cli/docs/organise.md");

fn named(name: &str) -> bool {
    ORGANISE.contains(&format!("`{name}`"))
}

#[test]
fn the_organise_topic_names_every_std_user_type_and_relation() {
    for t in standard::types().filter(|t| !t.flags.reserved && t.group != Group::Resources) {
        assert!(named(t.name), "type {} is missing from organise.md", t.name);
    }
    // same_as and not_same_as are written by `umx entity link` and `deny`, and taught in the entity topic.
    for r in standard::relations()
        .filter(|r| !r.flags.reserved && ![SAME_AS, NOT_SAME_AS].contains(&r.name))
    {
        assert!(
            named(r.name),
            "relation {} is missing from organise.md",
            r.name
        );
    }
}

#[test]
fn the_organise_topic_lists_every_lifecycle_status_with_its_standing() {
    for s in standard::STATUSES {
        let standing = match s.standing {
            Standing::Unsettled => "unsettled",
            Standing::Settled => "settled",
            Standing::OutOfPlay => "out of play",
        };
        let row = format!("| `{}` | {standing} |", s.name);
        assert!(ORGANISE.contains(&row), "organise.md has no row {row}");
        if s.reason_expected {
            let line = ORGANISE.lines().find(|l| l.starts_with(&row)).unwrap();
            assert!(line.contains("say why"), "{line}");
        }
    }
}
