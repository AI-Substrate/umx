//! Node rows and the file image (s05; workshop 008 sections 2.2 and 5). The image holds nodes.jsonl's bytes as the
//! next write lays them out. Each row's hot columns, custom field names and scalar field columns are derived from its
//! line by one function (`derive`), used by the load, by every write and by the test that re-derives every row.
//! A write puts the record's line, in the form umx writes, into the image, then derives that row from it.

use crate::cols::NameCol;
use crate::evidence_kinds::EvidenceSrc;
use crate::json::{self, Refused};
use crate::line::{self, FieldVal, HotNode};
use crate::tags::TagSrc;
use crate::words::{self, Texts};
use crate::{Graph, ImageFull, NodeIx};
use alloc::borrow::Cow;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::ops::Range;
use umx_core::codec::{self, DecodeError, KeptLine};
use umx_core::ontology::TextFields;
use umx_core::time::{DateSpan, date_span};
use umx_core::{Id, Node, Value};

/// Image offsets, title offsets and handles are `u32`: the image stays below 4 GiB.
pub(crate) const LIMIT: usize = u32::MAX as usize;

/// The most `titles` bytes and `nf` entries: their offsets are `u32`. A batch's moved rows lay the columns out again
/// before passing it (`Graph::settle_before_cap`).
const COLUMN_LIMIT: usize = u32::MAX as usize;

/// A field list buffer reused across lines.
pub(crate) type FieldBuf<'a> = Vec<(Cow<'a, str>, FieldVal)>;

/// Where a row's title and field names are while a batch defers the column shifts (s26): ranges in `titles` and
/// `nf` past their row-ordered part, or the row's own slot for the one that did not move.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Moved {
    title: [u32; 2],
    nf: [u32; 2],
}

/// Row `h`'s slot in a row-ordered column whose ends are `ends`.
#[inline]
fn slot(ends: &[u32], h: usize) -> Range<usize> {
    let start = if h == 0 { 0 } else { ends[h - 1] as usize };
    start..ends[h] as usize
}

#[inline]
fn span(r: [u32; 2]) -> Range<usize> {
    r[0] as usize..r[1] as usize
}

/// A line's hot values, or the codec's record when the scanner refused the line.
pub(crate) enum Derived<'a> {
    Hot(HotNode<'a>),
    Record(Node),
}

impl Derived<'_> {
    pub fn id(&self) -> Id {
        match self {
            Derived::Hot(h) => h.id,
            Derived::Record(n) => n.id,
        }
    }

    pub fn invalid_keys(&self) -> impl Iterator<Item = &'static str> {
        let keys = umx_core::ontology::PROVENANCE_KEYS;
        let bits = match self {
            Self::Hot(hot) => hot.invalid_provenance,
            Self::Record(node) => umx_core::provenance::invalid_provenance_fields(&node.fields)
                .fold(0, |bits, key| {
                    bits | (1 << keys.iter().position(|k| *k == key).expect("reserved key"))
                }),
        };
        keys.iter()
            .copied()
            .enumerate()
            .filter_map(move |(i, key)| (bits & (1 << i) != 0).then_some(key))
    }
}

/// The one function from a line to its row's values. `no` is the line number for the codec's error.
pub(crate) fn derive<'a>(
    line: &'a [u8],
    no: usize,
    fields: &mut FieldBuf<'a>,
) -> Result<Derived<'a>, DecodeError> {
    match line::node_hot(line, fields) {
        Ok(hot) => Ok(Derived::Hot(hot)),
        Err(Refused) => {
            let n = line::decode_node(line, no)?;
            // Read from the line when asked, through the codec.
            fields.clear();
            fields.extend(
                n.fields
                    .keys()
                    .map(|k| (Cow::Owned(k.clone()), FieldVal::Other)),
            );
            Ok(Derived::Record(n))
        }
    }
}

fn hot_of<'b>(d: &'b Derived<'_>) -> HotNode<'b> {
    match d {
        Derived::Hot(h) => HotNode {
            id: h.id,
            kind: Cow::Borrowed(&h.kind),
            title: Cow::Borrowed(&h.title),
            note: Cow::Borrowed(&h.note),
            status: Cow::Borrowed(&h.status),
            created: Cow::Borrowed(&h.created),
            updated: Cow::Borrowed(&h.updated),
            by: Cow::Borrowed(&h.by),
            authored: h.authored,
            edited: h.edited,
            prov_key: h.prov_key.as_deref().map(Cow::Borrowed),
            invalid_provenance: h.invalid_provenance,
            locator: h.locator.clone(),
            entity_key: h.entity_key.as_deref().map(Cow::Borrowed),
            tags: h.tags.as_deref().map(Cow::Borrowed),
            evidence: h.evidence,
        },
        Derived::Record(n) => HotNode {
            id: n.id,
            kind: Cow::Borrowed(&n.kind),
            title: Cow::Borrowed(&n.title),
            note: Cow::Borrowed(&n.note),
            status: Cow::Borrowed(&n.status),
            created: Cow::Borrowed(&n.created),
            updated: Cow::Borrowed(&n.updated),
            by: Cow::Borrowed(&n.created_by),
            authored: n.authored_by_session,
            edited: n.edited_by_session,
            prov_key: n
                .fields
                .get("prov_key")
                .and_then(Value::as_str)
                .map(Cow::Borrowed),
            invalid_provenance: 0,
            locator: n.locator.clone(),
            entity_key: n
                .fields
                .get(umx_core::entity::ENTITY_KEY)
                .map(Cow::Borrowed),
            tags: n
                .fields
                .get(umx_core::tags::TAGS)
                .filter(|v| v.is_array())
                .map(Cow::Borrowed),
            evidence: EvidenceSrc::Record(&n.evidence),
        },
    }
}

/// A row's texts as the word index takes them (req-68): title, note, then each custom field that `text_fields` makes
/// text for the row's type and whose value is a string, read from the line.
fn texts_of<'a>(
    d: &'a Derived<'a>,
    line: &'a [u8],
    fields: &'a FieldBuf<'a>,
    text_fields: &TextFields,
) -> Texts<'a> {
    let mut out: Texts<'a> = Vec::new();
    let provenance = match d {
        Derived::Hot(h) => umx_core::provenance::is_provenance_key(&h.kind, h.prov_key.as_deref()),
        Derived::Record(n) => umx_core::provenance::is_provenance(n),
    };
    if provenance {
        return out;
    }
    match d {
        Derived::Hot(h) => {
            out.push((Cow::Borrowed(words::TITLE), Cow::Borrowed(&*h.title)));
            out.push((Cow::Borrowed(words::NOTE), Cow::Borrowed(&*h.note)));
            for (name, v) in fields {
                if let FieldVal::Str { rel, len, esc } = *v
                    && text_fields.is_text(&h.kind, name)
                {
                    let raw = core::str::from_utf8(&line[rel as usize..(rel + len) as usize])
                        .expect("checked as UTF-8 by the scanner");
                    out.push((Cow::Borrowed(&**name), json::text(raw, esc)));
                }
            }
        }
        Derived::Record(n) => {
            out.push((Cow::Borrowed(words::TITLE), Cow::Borrowed(&*n.title)));
            out.push((Cow::Borrowed(words::NOTE), Cow::Borrowed(&*n.note)));
            for (name, v) in &n.fields {
                if let Value::String(text) = v
                    && text_fields.is_text(&n.kind, name)
                {
                    out.push((Cow::Borrowed(name.as_str()), Cow::Borrowed(text.as_str())));
                }
            }
        }
    }
    out
}

/// Where a row's custom field values are read to find their dates (the typed time column, D71 step 10): its line,
/// where a string sits at `rel`, or the codec's record for a line the scanner left to it.
#[derive(Clone, Copy)]
enum DateSrc<'a> {
    Line(&'a [u8]),
    Record(&'a Node),
}

impl DateSrc<'_> {
    fn of<'a>(d: &'a Derived<'a>, line: &'a [u8]) -> DateSrc<'a> {
        match d {
            Derived::Hot(_) => DateSrc::Line(line),
            Derived::Record(n) => DateSrc::Record(n),
        }
    }

    /// The date custom field `name`'s value `v` reads as: a string's decoded text through `date_span`, the one date
    /// parser. A number, bool, list, object or null is no date.
    fn date(self, name: &str, v: FieldVal) -> Option<DateSpan> {
        match (self, v) {
            (DateSrc::Line(line), FieldVal::Str { rel, len, esc }) => {
                let raw = core::str::from_utf8(&line[rel as usize..(rel + len) as usize])
                    .expect("checked as UTF-8 by the scanner");
                date_span(&json::text(raw, esc))
            }
            (DateSrc::Record(n), FieldVal::Other) => {
                n.fields.get(name)?.as_str().and_then(date_span)
            }
            _ => None,
        }
    }
}

/// How the lines of an image are walked into rows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Walk {
    /// A file as read: stop (`Ok(false)`) at the first blank line or id not above the last; report decode errors.
    File(Option<Id>),
    /// The image the graph laid out: skip kept lines; every line decodes.
    Image,
}

/// Tests only: the address of the graph whose whole-record reads (`node_record`) are counted in [`RECORDS_READ`], so a
/// test can show a read answered without decoding a line (F186). Other tests' graphs, on other threads, never count.
#[cfg(test)]
pub(crate) static WATCHED: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) static RECORDS_READ: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

impl Graph {
    #[inline]
    pub(crate) fn line_range(&self, h: usize) -> Range<usize> {
        let start = self.line_at[h] as usize;
        start..json::newline(&self.image, start)
    }

    #[inline]
    pub(crate) fn line(&self, h: usize) -> &[u8] {
        &self.image[self.line_range(h)]
    }

    #[inline]
    fn moved_row(&self, h: usize) -> Option<&Moved> {
        if self.moved.is_empty() {
            None
        } else {
            self.moved.get(&(h as u32))
        }
    }

    /// Node `h`'s title in `titles`.
    #[inline]
    pub(crate) fn title_range(&self, h: usize) -> Range<usize> {
        match self.moved_row(h) {
            Some(m) => span(m.title),
            None => slot(&self.title_end, h),
        }
    }

    fn nf_range(&self, h: usize) -> Range<usize> {
        match self.moved_row(h) {
            Some(m) => span(m.nf),
            None => slot(&self.nf_end, h),
        }
    }

    /// Node `h`'s custom field names (numbers), in name order.
    pub(crate) fn field_names(&self, h: usize) -> impl Iterator<Item = u32> + '_ {
        self.nf_range(h).map(|i| self.nf.get(i))
    }

    /// Appends a row for the line at `start` (its id is above every other).
    fn push_row(
        &mut self,
        start: usize,
        n: &HotNode<'_>,
        fields: &FieldBuf<'_>,
        texts: &Texts<'_>,
        dates: DateSrc<'_>,
        building: bool,
    ) {
        let h = self.ids.len() as u32;
        self.words.index(h, texts);
        debug_assert!(self.ids.last().is_none_or(|l| *l < n.id));
        let k = self.names.intern(&n.kind);
        self.names.node_use(k, true);
        let (s, b) = (self.names.intern(&n.status), self.names.intern(&n.by));
        self.ids.push(n.id);
        self.provenance.authored.push(n.authored);
        self.provenance.edited.push(n.edited);
        if self
            .provenance
            .set(h, &n.kind, n.prov_key.as_deref(), building)
        {
            self.vectors.membership_changed();
        }
        self.locators.push(h, n.locator.as_ref(), building);
        self.entities
            .push(h, &n.kind, n.entity_key.as_deref(), building);
        self.ev_kinds.push(h, n.evidence);
        self.tags.push(h, TagSrc(n.tags.as_deref()));
        self.n_kind.push(k);
        self.n_status.push(s);
        self.n_by.push(b);
        self.n_created.push(n.id, &n.created);
        self.n_updated.push(n.id, &n.updated);
        self.settle_before_cap(n.title.len(), fields.len());
        let (t0, f0) = (self.titles.len(), self.nf.len());
        self.titles.push_str(&n.title);
        self.line_at.push(start as u32);
        for (name, v) in fields {
            let f = self.names.intern(name);
            self.nf.push(f);
            self.f_cols
                .entry(f)
                .or_default()
                .insert(h, *v, dates.date(name, *v));
        }
        let (t1, f1) = (self.offset(self.titles.len()), self.offset(self.nf.len()));
        if self.moved.is_empty() {
            self.title_end.push(t1);
            self.nf_end.push(f1);
        } else {
            // Past the row-ordered part, as a moved row: its own slots are empty until the batch settles.
            self.title_end
                .push(self.title_end.last().copied().unwrap_or(0));
            self.nf_end.push(self.nf_end.last().copied().unwrap_or(0));
            let m = Moved {
                title: [t0 as u32, t1],
                nf: [f0 as u32, f1],
            };
            self.moved.insert(h, m);
        }
    }

    /// Tests only lower it: the most `titles` bytes and `nf` entries a batch's moved rows may take the columns to.
    fn column_limit(&self) -> usize {
        #[cfg(test)]
        if let Some(limit) = self.column_limit {
            return limit;
        }
        COLUMN_LIMIT
    }

    /// A `titles` or `nf` length as a `u32` offset, checked against the column limit (never wrapped).
    fn offset(&self, len: usize) -> u32 {
        assert!(
            len <= self.column_limit(),
            "a column offset past the column limit"
        );
        len as u32
    }

    /// Lays the columns out again when a title of `title` bytes and `names` field names put past their end would
    /// take them over the column limit (true when it did): a rollback that never compacted moves every restored row
    /// again while the image shrinks, so the moved tails could outgrow it (review and graph-storage). Every
    /// range taken before it is stale.
    fn settle_before_cap(&mut self, title: usize, names: usize) -> bool {
        if self.moved.is_empty() {
            return false;
        }
        let limit = self.column_limit();
        if self.titles.len() + title <= limit && self.nf.len() + names <= limit {
            return false;
        }
        #[cfg(test)]
        {
            self.early_settles[usize::from(self.image_dead == 0)] += 1;
        }
        self.lay_out_columns();
        true
    }

    /// Takes row `h` out of the field columns and the type use counts.
    fn unuse(&mut self, h: usize) {
        self.names.node_use(self.n_kind.get(h), false);
        for i in self.nf_range(h) {
            let f = self.nf.get(i);
            self.f_cols
                .get_mut(&f)
                .expect("a field name in use has a column")
                .remove(h as u32);
        }
    }

    /// Runs `f` on the word index with row `h`'s texts, read from its line as it is now.
    fn with_row_texts(&mut self, h: usize, f: impl FnOnce(&mut words::WordIndex, u32, &Texts<'_>)) {
        let image = core::mem::take(&mut self.image);
        {
            let start = self.line_at[h] as usize;
            let line = &image[start..json::newline(&image, start)];
            let mut fields = Vec::new();
            let d = derive(line, 0, &mut fields).expect("every line of the image decoded at load");
            let texts = texts_of(&d, line, &fields, &self.text_fields);
            f(&mut self.words, h as u32, &texts);
        }
        self.image = image;
    }

    /// Row `h`'s words go into the word index (a rebuild walks every row in order).
    pub(crate) fn index_row(&mut self, h: usize) {
        self.with_row_texts(h, |w, h, texts| w.index(h, texts));
    }

    /// Row `h`'s words leave the word index, read from its line as it is now (before the image changes).
    pub(crate) fn unindex_row(&mut self, h: usize) {
        self.with_row_texts(h, |w, h, texts| w.unindex(h, texts));
    }

    /// Notes row `h`'s words as its line has them now, before an edit changes the line; `set_row` then moves only
    /// the postings that change (`WordIndex::reindex`).
    pub(crate) fn stash_words(&mut self, h: usize) {
        self.with_row_texts(h, |w, h, texts| w.stash(h, texts));
    }

    /// Sets row `h` (same id) from its new line's values; `stash_words` noted its old words before the image changed,
    /// so only the postings that change move. A field the row keeps is set in its column in place. A title or name
    /// list of the same length is overwritten in place. Otherwise, while a batch has dead image bytes or moved rows,
    /// the new one goes past the column's row-ordered part (`moved`, settled once by `settle_columns`); outside a
    /// batch, or just after `settle_before_cap` laid the columns out, the column shifts after row `h`.
    fn set_row(
        &mut self,
        h: usize,
        n: &HotNode<'_>,
        fields: &FieldBuf<'_>,
        texts: &Texts<'_>,
        dates: DateSrc<'_>,
    ) {
        // First, before any range is taken: an early settle moves every row's title and names.
        let settled = self.settle_before_cap(n.title.len(), fields.len());
        self.provenance.authored.set(h, n.authored);
        self.provenance.edited.set(h, n.edited);
        if self
            .provenance
            .set(h as u32, &n.kind, n.prov_key.as_deref(), false)
        {
            self.vectors.membership_changed();
        }
        self.locators.set(h as u32, n.locator.as_ref());
        self.entities
            .set(h as u32, &n.kind, n.entity_key.as_deref());
        self.ev_kinds.set(h as u32, n.evidence);
        self.tags.set(h as u32, TagSrc(n.tags.as_deref()));
        self.words.reindex(h as u32, texts);
        self.names.node_use(self.n_kind.get(h), false);
        let k = self.names.intern(&n.kind);
        self.names.node_use(k, true);
        let (s, b) = (self.names.intern(&n.status), self.names.intern(&n.by));
        self.n_kind.set(h, k);
        self.n_status.set(h, s);
        self.n_by.set(h, b);
        self.n_created.set(h, n.id, &n.created);
        self.n_updated.set(h, n.id, &n.updated);
        let names: Vec<u32> = fields.iter().map(|(f, _)| self.names.intern(f)).collect();
        for i in self.nf_range(h) {
            let f = self.nf.get(i);
            if !names.contains(&f) {
                self.f_cols
                    .get_mut(&f)
                    .expect("a field name in use has a column")
                    .remove(h as u32);
            }
        }
        // Shifting is right whenever no row is moved, dead image bytes or not; after an early settle it keeps the
        // columns to the live titles and names.
        let defer = !settled && (self.image_dead != 0 || !self.moved.is_empty());
        let (t, at) = (self.title_range(h), self.nf_range(h));
        let mut m = Moved {
            title: [t.start as u32, t.end as u32],
            nf: [at.start as u32, at.end as u32],
        };
        let mut moved = self.moved_row(h).is_some();
        if t.len() == n.title.len() {
            self.titles.replace_range(t, &n.title);
        } else if defer {
            let start = self.titles.len() as u32;
            self.titles.push_str(&n.title);
            m.title = [start, self.offset(self.titles.len())];
            moved = true;
        } else {
            let delta = n.title.len() as i64 - t.len() as i64;
            self.titles.replace_range(t, &n.title);
            for e in &mut self.title_end[h..] {
                *e = (i64::from(*e) + delta) as u32;
            }
        }
        if at.len() == names.len() {
            self.nf.splice(at, &names);
        } else if defer {
            let start = self.nf.len() as u32;
            names.iter().for_each(|&f| self.nf.push(f));
            m.nf = [start, self.offset(self.nf.len())];
            moved = true;
        } else {
            let delta = names.len() as i64 - at.len() as i64;
            self.nf.splice(at, &names);
            for e in &mut self.nf_end[h..] {
                *e = (i64::from(*e) + delta) as u32;
            }
        }
        if moved {
            self.moved.insert(h as u32, m);
        }
        for (&f, (name, v)) in names.iter().zip(fields) {
            self.f_cols
                .entry(f)
                .or_default()
                .set(h as u32, *v, dates.date(name, *v));
        }
    }

    /// Removes the last row.
    fn pop_row(&mut self) {
        let h = self.ids.len() - 1;
        self.unuse(h);
        let id = self.ids.pop().expect("a row");
        self.provenance.pop(h as u32);
        self.locators.pop(h as u32);
        self.entities.pop(h as u32);
        self.ev_kinds.pop(h as u32);
        self.tags.pop(h as u32);
        self.n_kind.remove(h);
        self.n_status.remove(h);
        self.n_by.remove(h);
        self.n_created.remove(h, id);
        self.n_updated.remove(h, id);
        self.title_end.pop();
        self.nf_end.pop();
        if !self.moved.is_empty() {
            self.moved.remove(&(h as u32));
        }
        // Once no row is moved, everything past the row-ordered part is dead.
        if self.moved.is_empty() {
            self.titles
                .truncate(self.title_end.last().copied().unwrap_or(0) as usize);
            self.nf
                .truncate(self.nf_end.last().copied().unwrap_or(0) as usize);
        }
        self.words.pop();
        self.line_at.pop();
    }

    /// Empties every node column (edges, kept lines and the image stay) before the rows are derived again.
    fn clear_rows(&mut self) {
        self.vectors.membership_changed();
        self.names.clear_node_uses();
        self.ids.clear();
        self.provenance.clear_nodes();
        self.locators.clear();
        self.entities.clear();
        self.ev_kinds.clear();
        self.tags.clear();
        self.n_kind = Default::default();
        self.n_status = Default::default();
        self.n_by = Default::default();
        self.n_created = Default::default();
        self.n_updated = Default::default();
        self.titles.clear();
        self.title_end.clear();
        self.words = words::WordIndex::new();
        self.line_at.clear();
        self.nf = Default::default();
        self.nf_end.clear();
        self.moved.clear();
        self.f_cols.clear();
    }

    /// Derives a row from every line of the image in order (the load path). With `Walk::File` it stops at the
    /// first line that a sorted file without repeats or blank lines would not have (`Ok(false)`).
    pub(crate) fn derive_rows(
        &mut self,
        walk: Walk,
        mut issues: Option<&mut Vec<crate::InvalidProvenance>>,
    ) -> Result<bool, DecodeError> {
        let file = matches!(walk, Walk::File(_));
        assert_eq!(self.image_dead, 0, "derive_rows needs a compact image");
        self.provenance.keys.begin_build();
        self.locators.begin_build();
        self.entities.begin_build();
        self.words.begin_build();
        let image = core::mem::take(&mut self.image);
        let mut fields = Vec::new();
        let (mut at, mut kept, mut no) = (0, 0, 0);
        let mut done = Ok(true);
        while at < image.len() {
            let end = json::newline(&image, at);
            no += 1;
            if self
                .kept_nodes
                .get(kept)
                .is_some_and(|k| k.1 as usize == at)
            {
                kept += 1;
                at = end + 1;
                continue;
            }
            let line = &image[at..end];
            if file && json::blank(line) {
                done = Ok(false);
                break;
            }
            let d = match derive(line, no, &mut fields) {
                Ok(d) => d,
                Err(e) if file => {
                    done = Err(e);
                    break;
                }
                Err(_) => unreachable!("every line of the image decoded at load"),
            };
            if matches!(walk, Walk::File(Some(id)) if id == d.id()) {
                done = Ok(false);
                break;
            }
            if self.ids.last().is_some_and(|l| *l >= d.id()) {
                assert!(file, "the image is in id order");
                done = Ok(false);
                break;
            }
            if let Some(out) = &mut issues {
                out.extend(d.invalid_keys().map(|key| crate::InvalidProvenance {
                    file: umx_core::codec::GraphFile::Nodes,
                    record: d.id(),
                    line: no,
                    key,
                }));
            }
            let texts = texts_of(&d, line, &fields, &self.text_fields);
            self.push_row(
                at,
                &hot_of(&d),
                &fields,
                &texts,
                DateSrc::of(&d, line),
                true,
            );
            at = end + 1;
        }
        self.image = image;
        self.provenance.keys.finish_build();
        self.locators.finish_build();
        self.entities.finish_build();
        self.words.finish_build();
        done
    }

    /// Derives row `h` again from its line in the image; `stash_words(h)` ran before the line changed.
    pub(crate) fn derive_row(&mut self, h: usize) {
        let image = core::mem::take(&mut self.image);
        let range = {
            let start = self.line_at[h] as usize;
            start..json::newline(&image, start)
        };
        let mut fields = Vec::new();
        let line = &image[range];
        let d = derive(line, 0, &mut fields).expect("a line umx wrote decodes");
        let texts = texts_of(&d, line, &fields, &self.text_fields);
        self.set_row(h, &hot_of(&d), &fields, &texts, DateSrc::of(&d, line));
        self.image = image;
    }

    /// The most bytes the image may hold.
    pub(crate) fn limit(&self) -> usize {
        #[cfg(test)]
        if let Some(limit) = self.limit {
            return limit;
        }
        LIMIT
    }

    /// Refuses a splice that would take the image past its limit (security: a refusal, never a panic in the
    /// daemon's writer). Every write that grows the image calls it before it changes anything.
    fn fits(&self, at: &Range<usize>, bytes: &[u8]) -> Result<(), ImageFull> {
        let size = self.image.len() - at.len() + bytes.len();
        let limit = self.limit();
        if size > limit {
            return Err(ImageFull {
                size: size as u64,
                limit: limit as u64,
            });
        }
        Ok(())
    }

    /// Replaces `at` in the image with `bytes`, moving every line that starts at or after `at.end`. The caller has
    /// checked that it `fits`.
    fn splice_image(&mut self, at: Range<usize>, bytes: &[u8]) {
        debug_assert!(self.fits(&at, bytes).is_ok(), "an unchecked splice");
        let delta = bytes.len() as i64 - at.len() as i64;
        self.image.splice(at.clone(), bytes.iter().copied());
        let moved = |p: &u32| *p as usize >= at.end;
        let shift = |p: &mut u32| *p = (i64::from(*p) + delta) as u32;
        if self.image_dead == 0 {
            let from = self.line_at.partition_point(|p| !moved(p));
            self.line_at[from..].iter_mut().for_each(shift);
        } else {
            self.line_at.iter_mut().filter(|p| moved(p)).for_each(shift);
        }
        let from = self.kept_nodes.partition_point(|k| !moved(&k.1));
        self.kept_nodes[from..]
            .iter_mut()
            .for_each(|k| shift(&mut k.1));
    }

    /// Where a line goes that must come after the rows from `h` on and the kept lines with an id up to `id`.
    fn place(&self, h: usize, id: Id) -> usize {
        assert_eq!(self.image_dead, 0, "place needs a compact image");
        let row = self
            .line_at
            .get(h)
            .map_or(self.image.len(), |&p| p as usize);
        let k = self.kept_nodes.partition_point(|k| k.0 <= id);
        let kept = self
            .kept_nodes
            .get(k)
            .map_or(usize::MAX, |&(_, p)| p as usize);
        row.min(kept)
    }

    /// Puts a line that is not in use into the image, after the kept lines of its id and before its line in use.
    pub(crate) fn keep_node_line(&mut self, k: KeptLine) -> Result<(), ImageFull> {
        assert_eq!(self.image_dead, 0, "kept lines need a compact image");
        let at = self.place(self.ids.partition_point(|x| *x < k.id), k.id);
        let mut bytes = k.line;
        bytes.push(b'\n');
        self.fits(&(at..at), &bytes)?;
        self.splice_image(at..at, &bytes);
        let i = self.kept_nodes.partition_point(|x| x.0 <= k.id);
        self.kept_nodes.insert(i, (k.id, at as u32));
        Ok(())
    }

    /// The line for `node`: the form umx writes, or the exact bytes a replace took out when `node` is that record
    /// again (an undo).
    pub(crate) fn line_for(&self, node: &Node) -> Vec<u8> {
        if let Some((id, bytes)) = &self.undo
            && *id == node.id
            && line::decode_node(bytes, 1).is_ok_and(|n| n == *node)
        {
            return bytes.clone();
        }
        let mut line = codec::encode_nodes([node]);
        line.pop();
        line
    }

    /// Replaces row `h`'s line (one write: the image tail, the titles and the field names shift together).
    pub(crate) fn replace_node(&mut self, h: usize, node: Node) -> Result<Node, ImageFull> {
        let old = self.node_record(h);
        if old == node {
            return Ok(old);
        }
        let line = self.line_for(&node);
        let range = self.line_range(h);
        self.fits(&range, &line)?;
        // Only once the write will happen: a refused write changes nothing, the word index included.
        self.stash_words(h);
        let before = self.image[range.clone()].to_vec();
        self.splice_image(range, &line);
        self.undo = Some((node.id, before));
        self.derive_row(h);
        Ok(old)
    }

    /// Puts back the exact line a batch's replace took out (a batch rollback). It had fit, so it fits again.
    pub(crate) fn restore_line(&mut self, h: usize, bytes: &[u8]) {
        self.stash_words(h);
        let range = self.line_range(h);
        self.splice_image(range, bytes);
        self.derive_row(h);
    }

    /// Adds a node whose id no row has, at handle `h`.
    pub(crate) fn add_node(&mut self, h: usize, node: &Node) -> Result<(), ImageFull> {
        let nodes = self.ids.len();
        let mut line = self.line_for(node);
        line.push(b'\n');
        let at = if self.image_dead != 0 && h == nodes {
            self.image.len()
        } else {
            assert_eq!(self.image_dead, 0, "compact before an out-of-order insert");
            self.place(h, node.id)
        };
        self.fits(&(at..at), &line)?;
        self.splice_image(at..at, &line);
        if h == self.ids.len() {
            let mut fields = Vec::new();
            let bare = &line[..line.len() - 1];
            let d = derive(bare, 0, &mut fields).expect("a line umx wrote decodes");
            let texts = texts_of(&d, bare, &fields, &self.text_fields);
            self.push_row(
                at,
                &hot_of(&d),
                &fields,
                &texts,
                DateSrc::of(&d, bare),
                false,
            );
            // A new node has no edges, so this is what a rebuild would give.
            self.out.push_node();
            self.inc.push_node();
        } else {
            // Handles from h on move up one.
            for e in self.e_from.iter_mut().chain(self.e_to.iter_mut()) {
                if *e & crate::EXTERNAL == 0 && *e as usize >= h {
                    *e += 1;
                }
            }
            self.vectors.inserted(h, nodes);
            self.similar.inserted(h, nodes);
            self.rederive();
        }
        Ok(())
    }

    /// Removes row `h`'s line; its edges are gone already.
    pub(crate) fn drop_node(&mut self, h: usize) -> Node {
        let old = self.node_record(h);
        if h + 1 == self.ids.len() {
            self.unindex_row(h);
        }
        let range = self.line_range(h);
        self.splice_image(range.start..range.end + 1, &[]);
        if h + 1 == self.ids.len() {
            self.vectors.drop_last(h as u32);
            self.similar.removed(h, self.ids.len());
            self.pop_row();
            self.out.pop_node();
            self.inc.pop_node();
        } else {
            for e in self.e_from.iter_mut().chain(self.e_to.iter_mut()) {
                if *e & crate::EXTERNAL == 0 && *e as usize > h {
                    *e -= 1;
                }
            }
            self.vectors.removed(h, self.ids.len());
            self.similar.removed(h, self.ids.len());
            self.rederive();
        }
        old
    }

    /// Every row again from the image, then adjacency (the rare path: handles moved).
    fn rederive(&mut self) {
        assert_eq!(self.image_dead, 0, "rederive needs a compact image");
        #[cfg(test)]
        {
            self.node_rebuilds += 1;
        }
        self.clear_rows();
        self.derive_rows(Walk::Image, None)
            .expect("every line of the image decoded at load");
        self.rebuild_adjacency();
    }

    pub(crate) fn bulk_nodes(
        &mut self,
        added: &[(Id, Vec<u8>)],
        removed: &BTreeSet<Id>,
    ) -> Result<(), ImageFull> {
        assert_eq!(self.image_dead, 0, "bulk insert needs a compact image");
        debug_assert!(
            self.kept_nodes.iter().all(|(id, _)| !removed.contains(id)),
            "bulk removal would erase a kept node line"
        );
        let growth: usize = added.iter().map(|(_, line)| line.len()).sum();
        let limit = self.limit();
        if self.image.len().saturating_add(growth) > limit {
            return Err(ImageFull {
                size: self.image.len().saturating_add(growth) as u64,
                limit: limit as u64,
            });
        }
        let mut lines: Vec<(usize, Id, bool)> = self
            .line_at
            .iter()
            .zip(&self.ids)
            .map(|(&at, &id)| (at as usize, id, false))
            .chain(
                self.kept_nodes
                    .iter()
                    .map(|&(id, at)| (at as usize, id, true)),
            )
            .collect();
        lines.sort_unstable_by_key(|l| l.0);
        let mut image = Vec::with_capacity(self.image.len() + growth);
        let mut kept = Vec::with_capacity(self.kept_nodes.len());
        let mut added = added.iter().peekable();
        for (at, id, is_kept) in lines {
            while let Some((_, line)) = added.next_if(|(new, _)| *new < id) {
                image.extend_from_slice(line);
            }
            if !removed.contains(&id) {
                if is_kept {
                    kept.push((id, image.len() as u32));
                }
                let end = json::newline(&self.image, at);
                image.extend_from_slice(&self.image[at..=end]);
            }
        }
        for (_, line) in added {
            image.extend_from_slice(line);
        }
        let old_ids = self.ids.clone();
        self.image = image;
        self.kept_nodes = kept;
        #[cfg(test)]
        {
            self.node_rebuilds += 1;
        }
        self.clear_rows();
        self.derive_rows(Walk::Image, None)
            .expect("bulk lines were encoded or already loaded");
        let map: Vec<u32> = old_ids
            .iter()
            .map(|id| self.handle(*id).unwrap_or(u32::MAX))
            .collect();
        for end in self.e_from.iter_mut().chain(&mut self.e_to) {
            if *end & crate::EXTERNAL == 0 {
                *end = map[*end as usize];
                assert_ne!(*end, u32::MAX, "removed a node before its edges");
            }
        }
        self.vectors.remap(&map);
        self.similar.remap(&map);
        self.rebuild_adjacency();
        Ok(())
    }

    /// Points row `h` at `line`, appended to the image; its old line and newline become dead bytes until the batch
    /// compacts (a batch's fill or edit). The caller has checked the live and physical sizes against the limit.
    pub(crate) fn append_line(&mut self, h: usize, line: &[u8]) {
        let old_len = self.line(h).len() + 1;
        self.stash_words(h);
        let at = self.image.len();
        self.image.extend_from_slice(line);
        self.image.push(b'\n');
        self.image_dead += old_len;
        self.line_at[h] = at as u32;
        self.derive_row(h);
    }

    /// Whole-line moves preserve the scalar fields' offsets relative to their line. The batch's moved titles and field
    /// names are laid out in row order again in the same step.
    pub(crate) fn compact_image(&mut self) {
        if self.image_dead == 0 {
            self.settle_columns();
            return;
        }
        let mut image = Vec::with_capacity(self.image.len() - self.image_dead);
        let mut line_at = Vec::with_capacity(self.ids.len());
        let mut kept = Vec::with_capacity(self.kept_nodes.len());
        let mut old_kept = self.kept_nodes.iter().peekable();
        for (h, id) in self.ids.iter().enumerate() {
            while let Some(&(id, at)) = old_kept.next_if(|(k, _)| k <= id) {
                kept.push((id, image.len() as u32));
                let at = at as usize;
                let end = json::newline(&self.image, at);
                image.extend_from_slice(&self.image[at..=end]);
            }
            line_at.push(image.len() as u32);
            image.extend_from_slice(self.line(h));
            image.push(b'\n');
        }
        for &(id, at) in old_kept {
            kept.push((id, image.len() as u32));
            let at = at as usize;
            let end = json::newline(&self.image, at);
            image.extend_from_slice(&self.image[at..=end]);
        }
        self.image = image;
        self.line_at = line_at;
        self.kept_nodes = kept;
        self.image_dead = 0;
        self.settle_columns();
    }

    /// Lays titles and field names out in row order again once a batch's deferred moves are over (s26): at its
    /// compaction, and at the end of a rollback that never compacted.
    pub(crate) fn settle_columns(&mut self) {
        if self.moved.is_empty() {
            return;
        }
        assert_eq!(self.image_dead, 0, "settle the columns with the image");
        self.lay_out_columns();
    }

    /// Lays titles and field names out in row order, in one pass with exact allocations, and empties `moved`. Every
    /// row reads the same title and names after as before. It never reads the image, so `settle_before_cap` may run
    /// it with dead image bytes; a row `push_row` is adding (an id without its title end yet) is not laid out.
    fn lay_out_columns(&mut self) {
        let rows = self.title_end.len();
        #[cfg(debug_assertions)]
        let before: Vec<(alloc::string::String, Vec<u32>)> = (0..rows)
            .map(|h| (self.title(h).into(), self.field_names(h).collect()))
            .collect();
        let moved = core::mem::take(&mut self.moved);
        let (mut t_len, mut f_len) = (
            self.title_end.last().copied().unwrap_or(0) as usize,
            self.nf_end.last().copied().unwrap_or(0) as usize,
        );
        for (&h, m) in &moved {
            t_len = t_len + span(m.title).len() - slot(&self.title_end, h as usize).len();
            f_len = f_len + span(m.nf).len() - slot(&self.nf_end, h as usize).len();
        }
        let mut titles = alloc::string::String::with_capacity(t_len);
        let mut nf = match &self.nf {
            NameCol::Narrow(_) => NameCol::Narrow(Vec::with_capacity(f_len)),
            NameCol::Wide(_) => NameCol::Wide(Vec::with_capacity(f_len)),
        };
        let mut moves = moved.iter().peekable();
        let (mut t0, mut f0) = (0, 0);
        for h in 0..rows {
            let (t1, f1) = (self.title_end[h] as usize, self.nf_end[h] as usize);
            let (t, f) = match moves.next_if(|(k, _)| **k as usize == h) {
                Some((_, m)) => (span(m.title), span(m.nf)),
                None => (t0..t1, f0..f1),
            };
            titles.push_str(&self.titles[t]);
            match (&mut nf, &self.nf) {
                (NameCol::Narrow(to), NameCol::Narrow(from)) => to.extend_from_slice(&from[f]),
                (NameCol::Wide(to), NameCol::Wide(from)) => to.extend_from_slice(&from[f]),
                _ => unreachable!("built as wide as the column"),
            }
            self.title_end[h] = titles.len() as u32;
            self.nf_end[h] = nf.len() as u32;
            (t0, f0) = (t1, f1);
        }
        debug_assert!(moves.next().is_none(), "a moved row past the last row");
        debug_assert_eq!((titles.len(), nf.len()), (t_len, f_len));
        self.titles = titles;
        self.nf = nf;
        #[cfg(debug_assertions)]
        for (h, (title, names)) in before.iter().enumerate() {
            debug_assert!(
                self.title(h) == title && self.field_names(h).eq(names.iter().copied()),
                "row {h} reads the same title and names once settled"
            );
        }
    }

    /// The whole record of row `h`: hot values from the columns, the rest decoded from its line.
    pub(crate) fn node_record(&self, h: usize) -> Node {
        #[cfg(test)]
        if WATCHED.load(core::sync::atomic::Ordering::SeqCst) == core::ptr::from_ref(self) as usize
        {
            RECORDS_READ.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }
        let line = self.line(h);
        let Ok(cold) = line::node_cold(line) else {
            return line::decode_node(line, 0).expect("every line of the image decoded at load");
        };
        let id = self.ids[h];
        Node {
            id,
            kind: self.names.text(self.n_kind.get(h)).into(),
            title: self.title(h).into(),
            status: self.names.text(self.n_status.get(h)).into(),
            note: cold.note,
            evidence: cold.evidence,
            status_history: cold.history,
            created: self.n_created.get(h, id),
            updated: self.n_updated.get(h, id),
            created_by: self.names.text(self.n_by.get(h)).into(),
            locator: cold.locator,
            authored_by_session: self.provenance.authored.get(h),
            edited_by_session: self.provenance.edited.get(h),
            fields: cold.fields,
        }
    }

    /// Custom field `name` of row `h`, from its column (a list, object or null from the line).
    pub(crate) fn custom_value(&self, h: usize, name: &str) -> Option<Value> {
        let f = self.names.get(name)?;
        let v = self.f_cols.get(&f)?.get(h as u32)?;
        Some(self.field_value(h, name, v))
    }

    pub(crate) fn field_value(&self, h: usize, name: &str, v: FieldVal) -> Value {
        match v {
            FieldVal::Num(n) => n.value(),
            FieldVal::Bool(b) => Value::Bool(b),
            FieldVal::Str { rel, len, esc } => {
                let at = self.line_at[h] as usize + rel as usize;
                let raw = core::str::from_utf8(&self.image[at..at + len as usize])
                    .expect("checked as UTF-8 at load");
                Value::String(json::text(raw, esc).into_owned())
            }
            FieldVal::Other => match line::node_field(self.line(h), name) {
                Ok(Some(v)) => v,
                _ => self
                    .node_record(h)
                    .fields
                    .remove(name)
                    .expect("the field's column names this node"),
            },
        }
    }

    /// The value of custom field `name` on the lowest-id node that holds it: the field's first use, which fixes an
    /// undeclared field's kind (same answer as scanning every node in id order for `custom_field(name)`).
    pub fn first_use(&self, name: &str) -> Option<Value> {
        let f = self.names.get(name)?;
        let (h, v) = self.f_cols.get(&f)?.first()?;
        Some(self.field_value(h as usize, name, v))
    }

    /// Checks that every row's columns are what its line derives to (s05 design note 1). For tests: panics on the
    /// first row that differs.
    #[doc(hidden)]
    pub fn check_rows(&self) {
        assert_eq!(self.image_dead, 0, "check_rows needs a compact image");
        assert!(self.moved.is_empty(), "check_rows needs settled columns");
        let mut again = Graph {
            image: self.image.clone(),
            kept_nodes: self.kept_nodes.clone(),
            names: self.names.clone(),
            text_fields: self.text_fields.clone(),
            ..Graph::default()
        };
        again.clear_rows();
        again.set_std_tags(self.std_tags());
        again
            .derive_rows(Walk::Image, None)
            .expect("every line of the image decodes");
        assert_eq!(again.ids, self.ids, "ids");
        assert_eq!(again.line_at, self.line_at, "line starts");
        assert_eq!(again.titles, self.titles, "titles");
        assert_eq!(again.title_end, self.title_end, "title ends");
        assert_eq!(
            again.words.image(&again.ids),
            self.words.image(&self.ids),
            "word index"
        );
        for h in 0..self.ids.len() {
            let row = |g: &Graph| {
                (
                    g.n_kind.get(h),
                    g.n_status.get(h),
                    g.n_by.get(h),
                    g.n_created.get(h, g.ids[h]),
                    g.n_updated.get(h, g.ids[h]),
                    g.field_names(h).collect::<Vec<_>>(),
                    g.locator_kind(NodeIx(h as u32)),
                )
            };
            assert_eq!(row(&again), row(self), "row {h}");
            for f in self.field_names(h) {
                let name = self.names.text(f);
                assert_eq!(
                    again.custom_value(h, name),
                    self.custom_value(h, name),
                    "row {h} field {name}"
                );
            }
        }
        for (f, col) in &self.f_cols {
            let mut held = Vec::new();
            col.each(|h, _| held.push(h));
            held.sort_unstable();
            let mut want = Vec::new();
            if let Some(c) = again.f_cols.get(f) {
                c.each(|h, _| want.push(h));
            }
            want.sort_unstable();
            assert_eq!(held, want, "column {}", self.names.text(*f));
            // The typed time column: every holder's date, start, end and precision, as a fresh build makes it.
            let dates = |c: Option<&crate::fields::FieldCol>| {
                let mut v = Vec::new();
                if let Some(c) = c {
                    c.each_date(|h, d, p| v.push((h, d.start, d.end, p)));
                }
                v
            };
            assert_eq!(
                dates(Some(col)),
                dates(again.f_cols.get(f)),
                "dates of {}",
                self.names.text(*f)
            );
        }
        self.ev_kinds.check(&again.ev_kinds);
        self.tags.check(&again.tags);
        assert_eq!(
            again.duplicate_locators().collect::<Vec<_>>(),
            self.duplicate_locators().collect::<Vec<_>>(),
            "locator index"
        );
        assert_eq!(
            again.unnormalised_locators().collect::<Vec<_>>(),
            self.unnormalised_locators().collect::<Vec<_>>(),
            "unnormalised locators"
        );
        for n in 0..self.names.len() {
            assert_eq!(
                self.names.used_by_node(n as u32),
                again.names.used_by_node(n as u32),
                "type use of {}",
                self.names.text(n as u32)
            );
        }
    }
}
