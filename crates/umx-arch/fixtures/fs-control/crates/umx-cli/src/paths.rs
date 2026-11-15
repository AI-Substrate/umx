// Through the crate root's std alias from another file, never reaching fs (F119 control).
use crate::base::{collections::BTreeSet as Set};

pub fn count() -> usize {
    let _ = super::base::io::stderr();
    let _ = crate::base::io::stdout();
    Set::<u8>::new().len()
}
