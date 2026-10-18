//! The result-panel data model: [`Row`] (one compact result row with real NULLs), [`ResultSet`]
//! (one grid sheet — rows, grid model, lazy-stream flags, client-side sort) and [`ResultTab`]
//! (one sheet of the bottom panel: a data grid or the Find results).

use crate::theme::p;
use crate::{grid, proc, vscroll};
use eframe::egui;
use std::cmp::Ordering;

/// The NULL flag in a [`Row`]'s cell-end offsets (a row's text is therefore capped at 2 GiB —
/// far past PostgreSQL's 1 GB field limit for any realistic row).
const NULL_BIT: u32 = 1 << 31;

/// One result row in compact form: every cell's text in ONE buffer plus the cell boundaries — two
/// allocations per row instead of one per cell (a million-row fetch used to cost a heap block per
/// cell). SQL NULL is a flag, distinct from every text value, the empty string included.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct Row {
    text: Box<str>,
    /// Per cell: its end offset into `text`, with [`NULL_BIT`] set for SQL NULL.
    ends: Box<[u32]>,
}

impl Row {
    /// A row of non-NULL text cells.
    pub fn from_texts<S: AsRef<str>>(cells: impl IntoIterator<Item = S>) -> Self {
        let mut b = RowBuilder::default();
        for c in cells {
            b.push(c.as_ref());
        }
        b.finish()
    }

    /// A row of cells, `None` = SQL NULL.
    pub fn from_cells<'a>(cells: impl IntoIterator<Item = Option<&'a str>>) -> Self {
        let mut b = RowBuilder::default();
        for c in cells {
            match c {
                Some(s) => b.push(s),
                None => b.push_null(),
            }
        }
        b.finish()
    }

    /// Number of cells.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// The text of cell `i`; `None` for SQL NULL or a column past the row's end.
    pub fn cell(&self, i: usize) -> Option<&str> {
        let end = *self.ends.get(i)?;
        if end & NULL_BIT != 0 {
            return None;
        }
        let start = if i == 0 {
            0
        } else {
            self.ends[i - 1] & !NULL_BIT
        };
        Some(&self.text[start as usize..end as usize])
    }

    /// Whether cell `i` is SQL NULL.
    pub fn is_null(&self, i: usize) -> bool {
        self.ends.get(i).is_some_and(|e| e & NULL_BIT != 0)
    }
}

/// Builds a [`Row`] cell by cell without a per-cell allocation (the COPY parser unescapes straight
/// into the shared buffer).
#[derive(Default)]
pub(crate) struct RowBuilder {
    text: String,
    ends: Vec<u32>,
}

impl RowBuilder {
    fn end_cell(&mut self, null: bool) {
        // clamp a (pathological) row past 2 GiB: the overflow is dropped rather than corrupting
        // the offsets
        if self.text.len() >= NULL_BIT as usize {
            let mut cut = NULL_BIT as usize - 1;
            while !self.text.is_char_boundary(cut) {
                cut -= 1;
            }
            self.text.truncate(cut);
        }
        let end = self.text.len() as u32;
        self.ends.push(if null { end | NULL_BIT } else { end });
    }

    /// Append a text cell.
    pub fn push(&mut self, s: &str) {
        self.text.push_str(s);
        self.end_cell(false);
    }

    /// Append a cell written by `f` straight into the row buffer.
    pub fn push_with(&mut self, f: impl FnOnce(&mut String)) {
        f(&mut self.text);
        self.end_cell(false);
    }

    /// Append a SQL NULL cell.
    pub fn push_null(&mut self) {
        self.end_cell(true);
    }

    /// Cells pushed so far.
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn finish(self) -> Row {
        Row {
            text: self.text.into_boxed_str(),
            ends: self.ends.into_boxed_slice(),
        }
    }
}

/// The text the grid shows (and measures) for a NULL cell.
pub(crate) const NULL_DISPLAY: &str = "—";

/// One grid in the result panel — a SQL result set or a one-row command/error status. The grid
/// display model (columns/widths/order) + every fetched row. `title` is its tab label; `err`
/// paints rows in danger; `goto_line` links a click back to a 1-based editor line; `sql`
/// (non-empty) makes it Refresh-able.
pub(crate) struct ResultSet {
    pub title: String,
    pub gm: grid::GridModel, // columns / widths / order — the grid model
    pub rows: Vec<Row>,      // every row fetched SO FAR (a lazy grid grows this on demand)
    // ---- lazy fetch (a statement streamed on demand via COPY TO STDOUT) ----
    pub lazy: bool, // this grid is a live lazy stream (fetch buttons apply) vs a fully-buffered grid
    pub more: bool, // more rows remain on the server (the stream isn't exhausted) — buttons armed
    pub fetching: bool, // a fetch command is in flight (buttons disabled until it settles)
    pub stale: bool, // this result advertises more server-side rows but has NO live stream to fetch
    // them — a live stream lost before EOF (idle timeout / a Refresh or new run reclaiming the
    // connection / a disconnect / a dead worker → `disarm_lazy_grids`; or a stream error at `LazyEnd`),
    // OR a non-last statement's initial buffer that was capped (`copy_head`: the tail was discarded).
    // The fetch buttons stay ACTIVE — together with the tab's `…` they are the standing indicator of a
    // partial grid; every click shows a modal explaining there's nothing more to fetch here (re-run to
    // load the full result). Distinct from a fully-fetched result (clean EOF) and from a live stream.
    pub sql: String, // the statement that produced this result (for per-result Refresh; "" = none)
    pub scroll: (f64, f64), // grid scroll (f64 px on both axes)
    pub fade: vscroll::Fade, // disappearing-overlay scrollbar fade — per result-tab, next to `scroll`
    pub err: bool,           // paint rows as an error (the error status grid)
    pub goto_line: Option<usize>, // clicking a row jumps to this 1-based editor line
    pub truncated: bool, // only part is shown (a non-last batch SELECT capped to a page, a stream error)
    // ---- live-stream routing ----
    pub stream: u32, // the lazy stream this grid is served by (routes LazyRows / fetch commands;
    // 0 = the session run's single stream, or none at all for a buffered grid)
    pub fetch_started: Option<std::time::Instant>, // this sheet's in-flight incremental-fetch start
    // (folded into its fetch_elapsed when the fetch settles)
    // ---- client-side sort (over the rows fetched so far) ----
    pub sort: Vec<(usize, bool)>, // active sort keys (data column index, descending); order = priority
    pub view: Vec<usize>,         // visible row → index into `rows`; empty = identity (unsorted)
    pub view_dirty: bool,         // `view` needs rebuilding (the sort changed)
    // ---- status model (result-tab glyph / count / timer) ----
    pub is_status: bool, // a synthetic command/error status grid (not real data → counts as "0 rows")
    pub load_elapsed: Option<std::time::Duration>, // CUMULATIVE wall time (run start → this result's first page)
    pub fetch_elapsed: std::time::Duration, // incremental-fetch time accrued on THIS sheet (status-bar timer)
}

/// Width of each column = widest of (header, first 200 values) in chars → points (clamped).
pub(crate) fn grid_widths(columns: &[String], rows: &[Row]) -> Vec<f32> {
    let mut widths: Vec<f32> = columns.iter().map(|c| c.chars().count() as f32).collect();
    for row in rows.iter().take(200) {
        for (i, w) in widths.iter_mut().enumerate() {
            let n = row.cell(i).map_or(1, |v| v.chars().count());
            *w = w.max(n as f32);
        }
    }
    for w in &mut widths {
        *w = (*w * 7.0 + 18.0).clamp(54.0, 380.0);
    }
    widths
}

impl ResultSet {
    pub(crate) fn new(columns: Vec<String>, rows: Vec<Row>) -> Self {
        let widths = grid_widths(&columns, &rows);
        let col_order = (0..columns.len()).collect();
        Self {
            title: String::new(),
            gm: grid::GridModel {
                columns,
                widths,
                col_order,
            },
            rows,
            lazy: false,
            more: false,
            fetching: false,
            stale: false,
            sql: String::new(),
            scroll: (0.0, 0.0),
            fade: vscroll::Fade::default(),
            err: false,
            goto_line: None,
            truncated: false,
            stream: 0,
            fetch_started: None,
            sort: Vec::new(),
            view: Vec::new(),
            view_dirty: false,
            is_status: false,
            load_elapsed: None,
            fetch_elapsed: std::time::Duration::ZERO,
        }
    }

    /// The tab-strip label base: the statement's entity, or "Result".
    pub(crate) fn label(&self) -> &str {
        if self.title.is_empty() {
            "Result"
        } else {
            &self.title
        }
    }

    /// Whole-pill tint for this sheet's result tab: **error → red**; everything else → **neutral**
    /// (`None` = the default active/inactive colour, like the editor tabs — no green).
    pub(crate) fn status_color(&self) -> Option<egui::Color32> {
        self.err.then(|| p().danger)
    }

    /// A one-row status grid: a command outcome (`ok`) or an error (`!ok`). Columns Status / Line /
    /// Message; the row links back to `line` (1-based; 0 = unknown). Tab label = `label`.
    pub(crate) fn status(ok: bool, label: &str, line: usize, text: &str) -> Self {
        let cols = vec!["Status".to_owned(), "Line".to_owned(), "Message".to_owned()];
        let ln = if line > 0 {
            line.to_string()
        } else {
            NULL_DISPLAY.to_owned()
        };
        let st = if ok { "OK" } else { "Error" };
        let mut rs = Self::new(cols, vec![Row::from_texts([st, ln.as_str(), text])]);
        rs.err = !ok;
        rs.is_status = true; // synthetic status grid → its tab reads "0 rows", not the synthetic row count
        rs.goto_line = (line > 0).then_some(line);
        rs.title = label.to_owned();
        rs
    }

    /// Cycle the sort on a column. Plain click (`!additive`): a fresh column → ascending; the same
    /// single column → asc → desc → cleared. Ctrl/Cmd click (`additive`): toggle just this column
    /// asc → desc → removed, keeping the other keys (a new column joins at the lowest priority).
    pub(crate) fn toggle_sort(&mut self, col: usize, additive: bool) {
        let existing = self.sort.iter().position(|&(c, _)| c == col);
        if additive {
            match existing {
                Some(i) if !self.sort[i].1 => self.sort[i].1 = true,
                Some(i) => {
                    self.sort.remove(i);
                }
                None => self.sort.push((col, false)),
            }
        } else if self.sort.len() == 1 && existing == Some(0) {
            if !self.sort[0].1 {
                self.sort[0].1 = true;
            } else {
                self.sort.clear();
            }
        } else {
            self.sort.clear();
            self.sort.push((col, false));
        }
        self.view_dirty = true;
    }

    /// Rebuild the display permutation from the active sort keys (stable; NULLs last on
    /// ascending). Each key column is classified ONCE — integers if every non-NULL cell parses as
    /// one, else floats, else text — and its numbers are parsed once up front, so the comparator
    /// is a cheap total order (a per-pair numeric-or-text guess is not transitive: "10" < "1a" <
    /// "9" < "10", which `sort_by` may panic on). Empty `sort` → identity. Clears the dirty flag.
    pub(crate) fn rebuild_view(&mut self) {
        self.view_dirty = false;
        if self.sort.is_empty() {
            self.view.clear();
            return;
        }
        let rows = &self.rows;
        let keys: Vec<(SortCol, bool)> = self
            .sort
            .iter()
            .map(|&(c, desc)| (SortCol::classify(rows, c), desc))
            .collect();
        let mut idx: Vec<usize> = (0..rows.len()).collect();
        idx.sort_by(|&a, &b| {
            for (key, desc) in &keys {
                let ord = key.cmp(rows, a, b);
                let ord = if *desc { ord.reverse() } else { ord };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });
        self.view = idx;
    }

    /// Map a visible row index to its underlying `rows` index (identity when unsorted, and for
    /// rows appended after the last sort — they follow in arrival order).
    #[inline]
    pub(crate) fn data_row(&self, r: usize) -> usize {
        self.view.get(r).copied().unwrap_or(r)
    }
}

/// One sort key column, pre-parsed for a cheap total-order comparator. `None` = NULL.
enum SortCol {
    Int(Vec<Option<i128>>),
    Float(Vec<Option<f64>>),
    Text(usize),
}

impl SortCol {
    fn classify(rows: &[Row], c: usize) -> Self {
        fn parse_all<T: std::str::FromStr>(rows: &[Row], c: usize) -> Option<Vec<Option<T>>> {
            rows.iter()
                .map(|r| match r.cell(c) {
                    None => Some(None),
                    Some(s) => s.trim().parse::<T>().ok().map(Some),
                })
                .collect()
        }
        if let Some(v) = parse_all::<i128>(rows, c) {
            SortCol::Int(v)
        } else if let Some(v) = parse_all::<f64>(rows, c) {
            SortCol::Float(v)
        } else {
            SortCol::Text(c)
        }
    }

    /// Order rows `a` and `b` (indices into `rows`) by this column; NULL sorts greatest, matching
    /// PostgreSQL's default NULLS LAST on ascending.
    fn cmp(&self, rows: &[Row], a: usize, b: usize) -> Ordering {
        fn nulls_last<T>(a: Option<T>, b: Option<T>, f: impl FnOnce(T, T) -> Ordering) -> Ordering {
            match (a, b) {
                (Some(x), Some(y)) => f(x, y),
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
            }
        }
        match self {
            SortCol::Int(v) => nulls_last(v[a], v[b], |x, y| x.cmp(&y)),
            SortCol::Float(v) => nulls_last(v[a], v[b], |x, y| x.total_cmp(&y)),
            SortCol::Text(c) => nulls_last(rows[a].cell(*c), rows[b].cell(*c), |x, y| x.cmp(y)),
        }
    }
}

/// One sheet of the bottom result panel — all kinds live in a single `Vec` shown by one tab strip.
/// Run (SQL Execute) clears the list; Find owns a single "Find" sheet
/// (re-running replaces it, never piles up).
pub(crate) enum ResultTab {
    /// A data grid: SQL result set, or a one-row command/error status.
    Data(ResultSet),
    /// The search-results sheet (labelled "Find") — clicking a row jumps to that editor line.
    Probe(proc::Results),
}

/// The live lazy grid served by stream `stream` in a panel, if any — the target for that
/// stream's `LazyRows` and fetch-state flags. (One lazy grid per stream: the session run parks
/// exactly one, a multisession run one per parked statement.)
pub(crate) fn lazy_grid_by_stream(panel: &mut [ResultTab], stream: u32) -> Option<&mut ResultSet> {
    panel.iter_mut().rev().find_map(|s| match s {
        ResultTab::Data(rs) if rs.lazy && rs.stream == stream => Some(rs),
        _ => None,
    })
}

/// Whether any sheet of the panel still has a fetch in flight — with several parked streams
/// (multisession) one settling doesn't make the tab idle.
pub(crate) fn panel_fetching(panel: &[ResultTab]) -> bool {
    panel
        .iter()
        .any(|s| matches!(s, ResultTab::Data(rs) if rs.fetching))
}

/// Whether any sheet of the panel is a live lazy grid with a fetch in flight (Stop then PAUSES
/// that fetch instead of cancelling a query).
pub(crate) fn panel_lazy_fetching(panel: &[ResultTab]) -> bool {
    panel
        .iter()
        .any(|s| matches!(s, ResultTab::Data(rs) if rs.lazy && rs.fetching))
}

/// Disarm every live lazy grid in a panel when its stream ends. A grid still flagged `lazy` is a stream
/// being cut off BEFORE its natural EOF (an idle timeout, a Refresh/new run reclaiming the connection, a
/// disconnect, or a dead worker) — its remaining server rows can no longer be fetched here, so mark it
/// `stale` (the fetch buttons stay live permanently as the partial-grid indicator; every click explains
/// in a modal). A grid that reached EOF is already `lazy == false` (cleared at `LazyEnd`) and is left
/// inert — that is the "fetched everything" case. Then clear the live-stream flags. One place so every
/// close path agrees.
pub(crate) fn disarm_lazy_grids(panel: &mut [ResultTab]) {
    for sheet in panel {
        if let ResultTab::Data(rs) = sheet {
            if rs.lazy {
                rs.stale = true;
            }
            rs.lazy = false;
            rs.more = false;
            rs.fetching = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_keeps_cells_and_nulls_apart() {
        let r = Row::from_cells([Some("a"), None, Some(""), Some("—")]);
        assert_eq!(r.len(), 4);
        assert_eq!(r.cell(0), Some("a"));
        assert_eq!(r.cell(1), None);
        assert!(r.is_null(1));
        assert_eq!(r.cell(2), Some(""), "the empty string is not NULL");
        assert_eq!(r.cell(3), Some("—"), "a literal dash is not NULL");
        assert!(!r.is_null(3));
        assert_eq!(r.cell(9), None);
    }

    fn sorted(cells: &[Option<&str>], desc: bool) -> Vec<Option<String>> {
        let rows = cells.iter().map(|c| Row::from_cells([*c])).collect();
        let mut rs = ResultSet::new(vec!["c".into()], rows);
        rs.sort = vec![(0, desc)];
        rs.rebuild_view();
        (0..rs.rows.len())
            .map(|r| rs.rows[rs.data_row(r)].cell(0).map(str::to_owned))
            .collect()
    }

    #[test]
    fn sort_is_a_total_order_on_mixed_text() {
        // a per-pair numeric guess cycles here ("10" < "1a" < "9" < "10"); the column is text
        let got = sorted(&[Some("10"), Some("9"), Some("1a")], false);
        let got: Vec<_> = got.iter().map(|c| c.as_deref().unwrap()).collect();
        assert_eq!(got, ["10", "1a", "9"]);
    }

    #[test]
    fn sort_numeric_columns_with_nulls_last() {
        let got = sorted(&[Some("10"), None, Some("2"), Some("-1.5")], false);
        assert_eq!(
            got,
            [
                Some("-1.5".into()),
                Some("2".into()),
                Some("10".into()),
                None
            ]
        );
        // big integers stay exact (no f64 rounding)
        let got = sorted(&[Some("9007199254740993"), Some("9007199254740992")], false);
        assert_eq!(got[0].as_deref(), Some("9007199254740992"));
        // descending reverses, NULL first (PostgreSQL's NULLS FIRST on DESC)
        let got = sorted(&[Some("1"), None, Some("3")], true);
        assert_eq!(got, [None, Some("3".into()), Some("1".into())]);
    }
}
