//!
//! Virtual code editor on top of [`crate::doc::Document`].
//!
//! Render — only the visible lines, O(visible) per frame; the model is a piece table with mmap:
//! the file is not loaded in full, caret coordinates are `(line, column)` in code points.
//! Highlighting is stateful: the lexer state at line boundaries is cached and lazily
//! advanced with a per-frame budget, so jumping to the end of a gigabyte-sized file does not
//! freeze the UI. The editor knows nothing about the application or the language: each frame
//! the application passes an [`EditorCtx`] with highlighting ([`Highlighter`]) and tab callbacks,
//! and gets back an [`EditorOut`].

use crate::doc::{Document, Pos};
use crate::theme::p;
use crate::{code_font, code_font_regular, CODE_SIZE};
use eframe::egui;
use egui::{CornerRadius, Rect, Stroke};
use std::collections::HashMap;

/// Lexer state at a line boundary. An opaque byte: the semantics are defined by the project's
/// highlighter; `0` is the initial state (as at the start of the document).
pub(crate) type LexState = u8;

/// Highlighting callbacks for a specific language (the language is set by the application: SQL).
pub(crate) struct Highlighter<'a> {
    /// Full layout of a line: (text, entry state) → (job, exit state).
    pub line: &'a dyn Fn(&str, LexState) -> (egui::text::LayoutJob, LexState),
    /// State transition only — for lazy re-lexing with a per-frame budget.
    pub advance: &'a dyn Fn(&str, LexState) -> LexState,
}

/// Two-generation galley cache keyed by "state + line text".
#[derive(Default)]
pub(crate) struct LineCache {
    cur: HashMap<String, std::sync::Arc<egui::Galley>>,
    prev: HashMap<String, std::sync::Arc<egui::Galley>>,
}

impl LineCache {
    const CAP: usize = 8192;

    /// Clear both generations (on a theme switch the old font atlas is dead).
    pub fn clear(&mut self) {
        self.cur.clear();
        self.prev.clear();
    }

    pub fn get_or(
        &mut self,
        key: &str,
        make: impl FnOnce() -> std::sync::Arc<egui::Galley>,
    ) -> std::sync::Arc<egui::Galley> {
        if let Some(g) = self.cur.get(key) {
            return g.clone();
        }
        if let Some(g) = self.prev.remove(key) {
            self.cur.insert(key.to_owned(), g.clone());
            return g;
        }
        let g = make();
        if self.cur.len() >= Self::CAP {
            std::mem::swap(&mut self.cur, &mut self.prev);
            self.cur.clear();
        }
        self.cur.insert(key.to_owned(), g.clone());
        g
    }
}

/// Anchored cache of lexer states at line starts: `states[i]` is the state BEFORE
/// line `base + i`. On a far jump (inertial scroll over a gigabyte-sized file) the
/// cache is instantly re-anchored to the visible window (the line start is treated as the
/// base state — for line-oriented formats this holds almost always), instead of
/// lexing millions of lines.
pub(crate) struct LexCache {
    base: usize,
    states: Vec<LexState>, // non-empty; states[0] is the state before line base
}

impl Default for LexCache {
    fn default() -> Self {
        Self { base: 0, states: vec![0] }
    }
}

impl LexCache {
    /// Per-frame re-lexing budget (lines) — comfortably under one frame even on long lines.
    const STEP_BUDGET: usize = 2048;
    /// A jump farther than this — re-anchor instead of lexing line by line.
    const REANCHOR_GAP: usize = 4096;

    fn reanchor(&mut self, line: usize) {
        self.base = line;
        self.states.clear();
        self.states.push(0);
    }

    /// Invalidate states starting from the line where the edit occurred.
    /// The state BEFORE line `line` does not change, so the tail is valid up to and including `line`.
    pub fn invalidate_from(&mut self, line: usize) {
        if line < self.base {
            self.reanchor(line);
        } else {
            let keep = (line - self.base + 1).min(self.states.len());
            self.states.truncate(keep.max(1));
        }
    }

    /// State before line `line`. If re-lexing has not reached it yet within the budget —
    /// returns (0, false): an approximation, and the caller must request a repaint.
    fn state_at(
        &mut self,
        doc: &mut Document,
        line: usize,
        advance: &dyn Fn(&str, LexState) -> LexState,
    ) -> (LexState, bool) {
        // a request above the anchor (scroll up) or a far jump down → re-anchor
        if line < self.base || line > self.base + self.states.len() + Self::REANCHOR_GAP {
            self.reanchor(line);
            return (0, true);
        }
        if line < self.base + self.states.len() {
            return (self.states[line - self.base], true);
        }
        let mut budget = Self::STEP_BUDGET;
        while self.base + self.states.len() <= line && budget > 0 {
            let n = self.base + self.states.len() - 1;
            let st = *self.states.last().expect("non-empty");
            let text = doc.get_line(n);
            self.states.push(advance(&text, st));
            budget -= 1;
        }
        // Keep the cache bounded: a long GRADUAL downward scroll grows `states` one entry per line
        // crossed (a far jump re-anchors instead, but a line-by-line scroll never does). The states
        // above the visible window are no longer needed — drop the front and advance `base`, so the
        // cache stays O(window) regardless of how far the user has scrolled. The retained tail
        // (`REANCHOR_GAP`) comfortably covers any visible window plus the scroll-up slack.
        if self.states.len() > Self::REANCHOR_GAP * 2 {
            let drop = self.states.len() - Self::REANCHOR_GAP;
            self.states.drain(0..drop);
            self.base += drop;
        }
        if line < self.base + self.states.len() {
            (self.states[line - self.base], true)
        } else {
            (0, false) // approximation until the next frame
        }
    }
}

const PAD_L: f32 = crate::theme::TEXT_INSET; // gutter → text: the shared text inset (one knob in theme.rs)
const GUT_R: f32 = crate::theme::GUTTER_PAD_R; // gutter right padding (number → divider)
const GUT_L: f32 = crate::theme::GUTTER_PAD_L; // gutter left padding (edge → number)

/// "Word character" class — shared by the editor and search.
pub(crate) fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Next word boundary from `pos` in `doc`, moving `right` or left. Skips any run of non-word
/// characters, then the adjacent word — the same rule used by Ctrl+Arrow navigation and by
/// Ctrl+Backspace/Ctrl+Delete word deletion. Crosses one line break when at a line edge.
fn word_boundary(doc: &mut Document, pos: Pos, right: bool) -> Pos {
    let (l, c) = pos;
    let chars: Vec<char> = doc.get_line(l).chars().collect();
    let n = chars.len();
    if right {
        if c >= n {
            if l + 1 < doc.line_count() {
                (l + 1, 0)
            } else {
                (l, c)
            }
        } else {
            let mut i = c;
            while i < n && !is_word(chars[i]) {
                i += 1;
            }
            while i < n && is_word(chars[i]) {
                i += 1;
            }
            (l, i)
        }
    } else if c == 0 {
        if l > 0 {
            (l - 1, doc.line_length(l - 1))
        } else {
            (l, c)
        }
    } else {
        let mut i = c.min(n);
        while i > 0 && !is_word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(chars[i - 1]) {
            i -= 1;
        }
        (l, i)
    }
}

/// Editor state of a tab: caret, selection, blink, sticky column.
/// The text lives in [`Document`]; only coordinates are here.
pub(crate) struct EditorState {
    caret: Pos,
    anchor: Pos,
    blink_t0: f64,
    blink_caret: Pos,
    pref_col: Option<usize>,
    scroll_to_caret: bool,
    /// Vertical scroll in pixels from the start of the document. f64 — on gigabyte-sized files
    /// the canvas reaches 5×10⁸ px, where f32 has a representation step of 32px (overlapping lines/gaps).
    scroll_y: f64,
    /// Horizontal scroll — also f64 (same arithmetic as the vertical one).
    scroll_x: f64,
    /// Disappearing-overlay scrollbar fade — lives next to the scroll offset.
    fade: crate::vscroll::Fade,
    /// Accent highlight of the line with a formatting error: (line, time it was set).
    pub flash_line: Option<(usize, f64)>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            caret: (0, 0),
            anchor: (0, 0),
            blink_t0: 0.0,
            blink_caret: (usize::MAX, usize::MAX),
            pref_col: None,
            scroll_to_caret: false,
            scroll_y: 0.0,
            scroll_x: 0.0,
            fade: crate::vscroll::Fade::default(),
            flash_line: None,
        }
    }
}

impl EditorState {
    pub fn caret(&self) -> Pos {
        self.caret
    }

    pub fn has_sel(&self) -> bool {
        self.caret != self.anchor
    }

    /// 0-based line of the selection's start (top-most end) — to offset per-statement source lines
    /// when only a selection is executed/validated.
    pub fn sel_start_line(&self) -> usize {
        self.sel().0 .0
    }

    fn sel(&self) -> (Pos, Pos) {
        if self.caret <= self.anchor {
            (self.caret, self.anchor)
        } else {
            (self.anchor, self.caret)
        }
    }

    pub fn selection_text(&self, doc: &mut Document) -> Result<String, String> {
        let (a, b) = self.sel();
        if a == b {
            return Ok(String::new());
        }
        doc.get_text_range(a, b)
    }

    fn clamp(&mut self, doc: &mut Document) {
        let last = doc.line_count() - 1;
        for pos in [&mut self.caret, &mut self.anchor] {
            if pos.0 > last {
                *pos = (last, doc.line_length(last));
            } else {
                pos.1 = pos.1.min(doc.line_length(pos.0));
            }
        }
    }

    /// Replace the selection (or insert at the caret) with the text `s`.
    /// Line breaks in `s` must already be normalized to the document's EOL.
    pub fn replace(&mut self, doc: &mut Document, s: &str) {
        self.clamp(doc);
        let (a, b) = self.sel();
        let start_byte = doc.pos_to_byte(a);
        doc.replace_range(a, b, s);
        let end = doc.byte_to_pos(start_byte + s.len() as u64);
        self.caret = end;
        self.anchor = end;
        self.pref_col = None;
        self.scroll_to_caret = true;
    }

    fn backspace(&mut self, doc: &mut Document) {
        if !self.has_sel() {
            let (l, c) = self.caret;
            if c > 0 {
                self.anchor = (l, c - 1);
            } else if l > 0 {
                self.anchor = (l - 1, doc.line_length(l - 1));
            } else {
                return;
            }
        }
        self.replace(doc, "");
    }

    fn delete(&mut self, doc: &mut Document) {
        if !self.has_sel() {
            let (l, c) = self.caret;
            if c < doc.line_length(l) {
                self.anchor = (l, c + 1);
            } else if l + 1 < doc.line_count() {
                self.anchor = (l + 1, 0);
            } else {
                return;
            }
        }
        self.replace(doc, "");
    }

    /// Ctrl+Backspace: delete the selection, or the word before the caret (up to the previous
    /// word boundary). No-op at the document start.
    fn backspace_word(&mut self, doc: &mut Document) {
        if !self.has_sel() {
            let pos = word_boundary(doc, self.caret, false);
            if pos == self.caret {
                return;
            }
            self.anchor = pos;
        }
        self.replace(doc, "");
    }

    /// Ctrl+Delete: delete the selection, or the word after the caret (up to the next word
    /// boundary). No-op at the document end.
    fn delete_word(&mut self, doc: &mut Document) {
        if !self.has_sel() {
            let pos = word_boundary(doc, self.caret, true);
            if pos == self.caret {
                return;
            }
            self.anchor = pos;
        }
        self.replace(doc, "");
    }

    pub fn undo_op(&mut self, doc: &mut Document) -> bool {
        if let Some(pos) = doc.undo() {
            self.caret = pos;
            self.anchor = pos;
            self.clamp(doc);
            self.scroll_to_caret = true;
            true
        } else {
            false
        }
    }

    pub fn redo_op(&mut self, doc: &mut Document) -> bool {
        if let Some(pos) = doc.redo() {
            self.caret = pos;
            self.anchor = pos;
            self.clamp(doc);
            self.scroll_to_caret = true;
            true
        } else {
            false
        }
    }

    pub fn move_to(&mut self, pos: Pos, select: bool) {
        self.caret = pos;
        if !select {
            self.anchor = pos;
        }
        self.scroll_to_caret = true;
    }

    /// External jump/selection: caret at `caret`, anchor at `anchor` (equal — no selection),
    /// both positions are clamped; scroll to the caret on the next frame.
    pub fn select_range(&mut self, doc: &mut Document, anchor: Pos, caret: Pos) {
        self.pref_col = None;
        self.anchor = anchor;
        self.caret = caret;
        self.clamp(doc);
        self.scroll_to_caret = true;
    }

    fn move_h(&mut self, doc: &mut Document, right: bool, select: bool) {
        self.pref_col = None;
        if self.has_sel() && !select {
            let (a, b) = self.sel();
            self.move_to(if right { b } else { a }, false);
            return;
        }
        let (l, c) = self.caret;
        let pos = if right {
            if c < doc.line_length(l) {
                (l, c + 1)
            } else if l + 1 < doc.line_count() {
                (l + 1, 0)
            } else {
                (l, c)
            }
        } else if c > 0 {
            (l, c - 1)
        } else if l > 0 {
            (l - 1, doc.line_length(l - 1))
        } else {
            (l, c)
        };
        self.move_to(pos, select);
    }

    fn move_word(&mut self, doc: &mut Document, right: bool, select: bool) {
        self.pref_col = None;
        let pos = word_boundary(doc, self.caret, right);
        self.move_to(pos, select);
    }

    fn move_v(&mut self, doc: &mut Document, down: bool, select: bool) {
        let (line, col) = self.caret;
        let goal = *self.pref_col.get_or_insert(col);
        let nl = doc.line_count();
        if down && line + 1 >= nl {
            return self.move_keep_pref((line, doc.line_length(line)), select);
        }
        if !down && line == 0 {
            return self.move_keep_pref((0, 0), select);
        }
        let target = if down { line + 1 } else { line - 1 };
        let c = goal.min(doc.line_length(target));
        self.move_keep_pref((target, c), select);
    }

    fn page(&mut self, doc: &mut Document, down: bool, rows: usize, select: bool) {
        let (line, col) = self.caret;
        let goal = *self.pref_col.get_or_insert(col);
        let nl = doc.line_count();
        let target =
            if down { (line + rows).min(nl - 1) } else { line.saturating_sub(rows) };
        let c = goal.min(doc.line_length(target));
        self.move_keep_pref((target, c), select);
    }

    fn move_keep_pref(&mut self, pos: Pos, select: bool) {
        self.caret = pos;
        if !select {
            self.anchor = pos;
        }
        self.scroll_to_caret = true;
    }

    /// Home: first non-whitespace character, then column 0 (smart Home).
    fn home(&mut self, doc: &mut Document, select: bool) {
        self.pref_col = None;
        let (line, col) = self.caret;
        let text = doc.get_line(line);
        let indent = text.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let target = if col == indent { 0 } else { indent };
        self.move_to((line, target), select);
    }

    fn end(&mut self, doc: &mut Document, select: bool) {
        self.pref_col = None;
        let (line, _) = self.caret;
        self.move_to((line, doc.line_length(line)), select);
    }

    fn doc_home(&mut self, select: bool) {
        self.pref_col = None;
        self.move_to((0, 0), select);
    }

    fn doc_end(&mut self, doc: &mut Document, select: bool) {
        self.pref_col = None;
        let last = doc.line_count() - 1;
        self.move_to((last, doc.line_length(last)), select);
    }

    pub fn select_all(&mut self, doc: &mut Document) {
        let last = doc.line_count() - 1;
        self.anchor = (0, 0);
        self.caret = (last, doc.line_length(last));
    }

    /// Place the caret: the line is computed by the caller (in f64, exactly), the column by pixel.
    fn set_from_line_x(
        &mut self,
        doc: &mut Document,
        line: usize,
        rel_x: f32,
        char_w: f32,
        select: bool,
    ) {
        let line = line.min(doc.line_count() - 1);
        let col = ((rel_x / char_w + 0.5).floor().max(0.0) as usize).min(doc.line_length(line));
        self.caret = (line, col);
        if !select {
            self.anchor = self.caret;
        }
        self.pref_col = None;
    }

    /// Double click: select the word in line `line` under pixel `rel_x`.
    fn select_word_at(
        &mut self,
        doc: &mut Document,
        line: usize,
        rel_x: f32,
        char_w: f32,
    ) {
        let line = line.min(doc.line_count() - 1);
        let col = (rel_x / char_w).floor().max(0.0) as usize;
        // bounded word lookup — no whole-line materialization (1GB double-click lag fix)
        match doc.word_bounds_at(line, col, is_word) {
            Some((s, e)) => {
                self.anchor = (line, s);
                self.caret = (line, e);
            }
            None => {
                let col = col.min(doc.line_length(line));
                self.caret = (line, col);
                self.anchor = self.caret;
            }
        }
        self.pref_col = None;
    }
}

/// Everything the editor needs from the application for one frame.
pub(crate) struct EditorCtx<'a> {
    pub doc: &'a mut Document,
    pub ed: &'a mut EditorState,
    pub lex: &'a mut LexCache,
    pub line_cache: &'a mut LineCache,
    /// Highlighting of search matches: line → [(column, length in characters)].
    /// Cleared by the editor on the first edit (the positions go stale).
    pub search_hl: &'a mut HashMap<usize, Vec<(usize, usize)>>,
    /// External jump/selection (anchor, caret) — applied in this frame.
    pub pending_goto: &'a mut Option<(Pos, Pos)>,
    /// External focus request + grace window (live in the application, survive across frames).
    pub focus_request: &'a mut bool,
    pub focus_grace: &'a mut u8,
    pub ed_id: egui::Id,
    pub hl: Highlighter<'a>,
    /// Text inserted on Tab at the caret position (per-project indentation).
    pub tab_insert: &'a dyn Fn(&mut Document, Pos) -> String,
}

/// What the application gets back from an editor frame.
#[allow(dead_code)] // part of the API: this build does not read all fields
pub(crate) struct EditorOut {
    /// The buffer changed in this frame.
    pub edited: bool,
    /// First changed line (for invalidating per-project caches).
    pub changed_from: Option<usize>,
    /// Caret position (0-based) — for the status bar.
    pub caret: Pos,
    /// Operation error (e.g. copying a range larger than the limit) — to a modal.
    pub error: Option<String>,
    /// Screen x of column 0 and y of line 0 (accounting for scroll) — anchor for popups.
    pub origin: egui::Pos2,
    pub char_w: f32,
    pub row_h: f32,
}

/// Render and service the virtual editor inside `sheet`.
pub(crate) fn code_editor(ui: &mut egui::Ui, sheet: Rect, cx: EditorCtx) -> EditorOut {
    let EditorCtx {
        doc,
        ed,
        lex,
        line_cache,
        search_hl,
        pending_goto,
        focus_request,
        focus_grace,
        ed_id,
        hl,
        tab_insert,
    } = cx;
    let ectx = &ui.ctx().clone();
    let mut error: Option<String> = None;

    let rh = ectx.fonts_mut(|f| f.row_height(&code_font(CODE_SIZE)));
    let char_w = ectx.fonts_mut(|f| f.glyph_width(&code_font(CODE_SIZE), '0'));

    // gutter geometry
    let n_lines = doc.line_count();
    let digits = n_lines.to_string().len().max(3);
    let gw = (char_w * digits as f32).ceil() + GUT_L + GUT_R;
    let text_left = sheet.left() + gw;
    let gutter_rect = Rect::from_min_max(sheet.min, egui::pos2(text_left, sheet.max.y));
    let r = crate::RADIUS_ISLAND;
    let sheet_cr = CornerRadius::same(r);
    let gutter_cr = CornerRadius { nw: r, ne: 0, sw: r, se: 0 };
    let view = Rect::from_min_max(egui::pos2(text_left, sheet.top()), sheet.max);
    let rows_vis = (view.height() / rh).ceil() as usize + 1;

    // external focus request (new tab / open / menu)
    if std::mem::take(focus_request) {
        *focus_grace = 4;
    }
    if *focus_grace > 0 {
        ectx.memory_mut(|m| m.request_focus(ed_id));
        *focus_grace -= 1;
        ectx.request_repaint();
    }
    let focused = ectx.memory(|m| m.has_focus(ed_id));
    if focused {
        ectx.memory_mut(|m| {
            m.set_focus_lock_filter(
                ed_id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
    }

    // ---- keyboard ----
    let mut edited = false;
    if focused {
        edited |= editor_input(
            doc,
            ed,
            ectx,
            rows_vis.saturating_sub(2),
            tab_insert,
            &mut error,
        );
    }
    // invalidate caches from the first changed line (edit/paste/undo/redo)
    let changed_from = doc.take_change_start();
    if let Some(start) = changed_from {
        lex.invalidate_from(start);
        search_hl.clear(); // match positions are stale
    }

    ed.clamp(doc);

    // ---- own virtual scroll + rendering ----
    // No giant canvas: the scroll position is f64 pixels, the visible lines
    // are drawn in window-local coordinates. At 1 GB (5×10⁸ px of "canvas") egui's f32
    // coordinates are quantized by 32px — that caused overlapping lines, empty rows and misclicks.
    ui.painter().rect_filled(sheet, sheet_cr, p().field_bg);
    let n_lines = doc.line_count();
    let rh64 = rh as f64;
    let content_h = n_lines as f64 * rh64;
    let caret_galley = hl_line(doc, lex, line_cache, &hl, ed.caret.0, ui);
    // horizontal extent = the longest line in CHARACTERS × the monospace advance (chars, NOT bytes, which
    // over-count multibyte UTF-8 and tripped the h-scroll at half the width) + the left inset + a few px
    // of caret air (I-beam 2px + 1px) so the end-of-line caret clears the frame. The air is part of the
    // extent, NOT a separate scroll-only range — so the bar and the scroll always appear together (a line
    // that fits never scrolls a couple phantom px with no bar), and while the text+air still fits the
    // end-of-line caret already has its air with no scroll at all.
    let max_cols = doc.max_line_chars();
    const CARET_AIR: f32 = 3.0;
    let total_w = (max_cols as f32 * char_w).max(caret_galley.rect.width()) + PAD_L + CARET_AIR;

    // jump/selection requested from outside (results / search / formatting error)
    if let Some((a, c)) = pending_goto.take() {
        ed.select_range(doc, a, c);
    }

    // Overlay scrollbars: no reserved space — the text fills the whole area and the fading handles float
    // over it. `need_*` (single pass) says which axis scrolls; only when BOTH do we shorten each usable
    // viewport by one BAR (`vview`/`hview`) so the last line / column slides clear of the perpendicular
    // handle at the very end (and the two tracks stop one bar short of the shared corner).
    let bar = crate::vscroll::BAR;
    let inner = view; // text fills the whole area; the handles overlay it
    let need_v = content_h > inner.height() as f64;
    let need_h = total_w > inner.width();
    let clear = if need_v && need_h { bar } else { 0.0 };
    let vview = (inner.height() - clear) as f64; // usable height (minus the h-bar it clears at the end)
    let hview = (inner.width() - clear) as f64;
    let rows_vis = (inner.height() / rh).ceil() as usize + 1;

    // wheel/touchpad (including injected kinetics) — over the whole sheet, gutter included
    let d = crate::vscroll::wheel_delta(ui, sheet);
    if d != egui::Vec2::ZERO {
        ed.scroll_y -= d.y as f64;
        ed.scroll_x -= d.x as f64;
        ectx.request_repaint();
    }

    let want_scroll = std::mem::take(&mut ed.scroll_to_caret);
    if want_scroll {
        // vertically: keep the caret line within the usable area (clear of the overlay h-bar)
        let cy = ed.caret.0 as f64 * rh64;
        if cy < ed.scroll_y {
            ed.scroll_y = cy;
        } else if cy + rh64 > ed.scroll_y + vview {
            ed.scroll_y = cy + rh64 - vview;
        }
        // horizontally: keep the caret 3 characters clear of the edges (and the overlay v-bar)
        let cx = (PAD_L
            + caret_galley.pos_from_cursor(egui::text::CCursor::new(ed.caret.1)).min.x)
            as f64;
        let margin = (char_w * 3.0) as f64;
        if cx - margin < ed.scroll_x {
            ed.scroll_x = (cx - margin).max(0.0);
        } else if cx + margin > ed.scroll_x + hview {
            ed.scroll_x = cx + margin - hview;
        }
    }
    ed.scroll_y = ed.scroll_y.clamp(0.0, (content_h - vview).max(0.0));
    ed.scroll_x = ed.scroll_x.clamp(0.0, (total_w as f64 - hview).max(0.0));

    let now_time = ui.input(|i| i.time);
    let flash = ed.flash_line.filter(|(_, t0)| now_time - t0 < 2.4);
    ed.flash_line = flash;

    // offset snapshot: the WHOLE frame (text, caret, gutter) is drawn with one offset;
    // the scrollbars below change ed.scroll_* for the next frame already
    let sy = ed.scroll_y;
    let sx = ed.scroll_x;
    // screen y of a line: the difference in f64 (exact at any depth in the file), then snapped
    // to physical pixels — the font's line height is fractional, and without snapping each line
    // crossed a pixel boundary at its own moment (a "wave" during slow scrolling)
    let ppp = ectx.pixels_per_point() as f64;
    let top64 = inner.top() as f64;
    let line_y = move |line: usize| -> f32 {
        let y = top64 + line as f64 * rh64 - sy;
        ((y * ppp).round() / ppp) as f32
    };
    let ox = (inner.left() as f64 + PAD_L as f64 - sx) as f32; // x of column zero

    // ---- mouse ----
    let resp = ui.interact(inner, ed_id, egui::Sense::click_and_drag());
    if resp.hovered() {
        ectx.set_cursor_icon(egui::CursorIcon::Text);
    }
    if let Some(pp) = resp.interact_pointer_pos() {
        // the line in f64 (exact at any depth in the file), the column by local pixel
        let line_f = ((pp.y - inner.top()) as f64 + sy) / rh64;
        let line = line_f.max(0.0) as usize;
        let rel_x = pp.x - ox;
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        if resp.double_clicked() {
            ed.select_word_at(doc, line, rel_x, char_w);
        } else if pressed {
            resp.request_focus();
            let extend = ui.input(|i| i.modifiers.shift);
            ed.set_from_line_x(doc, line, rel_x, char_w, extend);
        } else if resp.dragged() {
            ed.set_from_line_x(doc, line, rel_x, char_w, true);
        }
    }
    let caret_line = ed.caret.0;

    // ---- lines ----
    let pt = ui.painter().with_clip_rect(inner);
    let first = (sy / rh64).floor().max(0.0) as usize;
    let last = (first + rows_vis).min(n_lines.saturating_sub(1));
    let (sa, sb) = ed.sel();

    for line in first..=last {
        let y = line_y(line);
        let llen = doc.line_length(line); // O(1) from cache — no line clone (hl_line reads it itself)

        // accent flash of the line with a formatting error (fades out)
        if let Some((fl, t0)) = flash {
            if fl == line {
                let age = (now_time - t0) as f32;
                let k = (1.0 - age / 2.4).clamp(0.0, 1.0);
                let col = crate::theme::tint(p().field_bg, p().danger, 0.10 + 0.35 * k);
                pt.rect_filled(
                    Rect::from_min_max(
                        egui::pos2(inner.left(), y),
                        egui::pos2(inner.right(), y + rh),
                    ),
                    CornerRadius::ZERO,
                    col,
                );
                ectx.request_repaint();
            }
        }

        // active line (no selection)
        if !ed.has_sel() && line == caret_line && flash.is_none() {
            let cr = if line == 0 {
                CornerRadius { nw: 0, ne: crate::RADIUS_ISLAND, sw: 0, se: 0 }
            } else {
                CornerRadius::ZERO
            };
            pt.rect_filled(
                Rect::from_min_max(
                    egui::pos2(inner.left(), y),
                    egui::pos2(inner.right(), y + rh),
                ),
                cr,
                p().active_line,
            );
        }
        // selection
        if sb > sa && line >= sa.0 && line <= sb.0 {
            let s0 = if line == sa.0 { sa.1.min(llen) } else { 0 };
            let s1 = if line == sb.0 { sb.1.min(llen) } else { llen };
            let cont = line < sb.0; // selection continues onto the next line
            let x0 = ox + s0 as f32 * char_w;
            let extra = if cont { char_w * 0.5 } else { 0.0 };
            let x1 = ox + s1 as f32 * char_w + extra;
            if x1 > x0 || cont {
                pt.rect_filled(
                    Rect::from_min_max(egui::pos2(x0, y), egui::pos2(x1, y + rh)),
                    CornerRadius::ZERO,
                    p().editor_sel,
                );
            }
        }
        // highlight search matches on the visible line
        if let Some(hls) = search_hl.get(&line) {
            for &(c0, len) in hls {
                let x0 = ox + c0 as f32 * char_w;
                let x1 = x0 + len as f32 * char_w;
                pt.rect_filled(
                    Rect::from_min_max(egui::pos2(x0, y), egui::pos2(x1, y + rh)),
                    CornerRadius::ZERO,
                    p().find_hl,
                );
            }
        }
        // text
        if llen > 0 {
            let galley = hl_line(doc, lex, line_cache, &hl, line, ui);
            ui.painter().with_clip_rect(inner).galley(egui::pos2(ox, y), galley, p().text);
        }
    }

    // caret (own blink) — recompute the galley from the LIVE line (a click this frame
    // may have changed the line, while caret_galley above was read before the mouse was handled)
    if focused {
        let caret_line = ed.caret.0;
        let caret_col = ed.caret.1;
        let live_galley = hl_line(doc, lex, line_cache, &hl, caret_line, ui);
        let caret_local_x =
            live_galley.pos_from_cursor(egui::text::CCursor::new(caret_col)).min.x;
        let now = ui.input(|i| i.time);
        if edited || ed.caret != ed.blink_caret {
            ed.blink_t0 = now;
            ed.blink_caret = ed.caret;
        }
        let cyc = ((now - ed.blink_t0) % 1.0) as f32;
        let cx = (ox + caret_local_x).round() + 0.5;
        let cy = line_y(caret_line);
        #[cfg(test)]
        ectx.data_mut(|d| {
            // (screen x of the caret, left/right edge of the view, caret x within the line, line width)
            d.insert_temp(
                egui::Id::new("dbg_caret"),
                (cx, inner.left(), inner.right(), caret_local_x, live_galley.rect.width()),
            )
        });
        if cyc < 0.5
            && inner.contains(egui::pos2(cx.clamp(inner.left(), inner.right()), cy + rh * 0.5))
        {
            pt.line_segment(
                [egui::pos2(cx, cy), egui::pos2(cx, cy + rh)],
                // 2px matches the theme's global text-cursor width (theme.rs::text_cursor);
                // the brand accent — visible in both light and dark themes.
                Stroke::new(2.0, p().accent),
            );
        }
        ectx.request_repaint_after(std::time::Duration::from_secs_f32(
            if cyc < 0.5 { 0.5 - cyc } else { 1.0 - cyc },
        ));
    }

    // ---- scrollbars (own, in local coordinates): disappearing overlays over the text — `vscroll::Fade`
    // state lives on EditorState next to the scroll offset. Tracks are confined + one-bar clearance like
    // the result grid; below a whisker of opacity we skip drawing/interaction so a hidden bar can't eat a click.
    let handle_a = ed.fade.alpha(ui, sheet, d != egui::Vec2::ZERO);
    if handle_a > 0.003 {
        if need_v {
            let track = Rect::from_min_max(
                egui::pos2(view.right() - bar, inner.top()),
                egui::pos2(view.right(), inner.top() + vview as f32),
            );
            crate::vscroll::vbar(ui, track, ed_id.with("vbar"), &mut ed.scroll_y, content_h, vview, handle_a);
        }
        if need_h {
            let track = Rect::from_min_max(
                egui::pos2(inner.left(), view.bottom() - bar),
                egui::pos2(inner.left() + hview as f32, view.bottom()),
            );
            crate::vscroll::hbar(ui, track, ed_id.with("hbar"), &mut ed.scroll_x, total_w as f64, hview, handle_a);
        }
    }

    // ---- gutter (same scroll_y → staying in lockstep with the text is structural) ----
    let painter = ui.painter();
    painter.rect_filled(gutter_rect, gutter_cr, p().gutter);
    if !ed.has_sel() {
        let y = line_y(caret_line);
        if y + rh > sheet.top() && y < sheet.bottom() {
            let cr = if caret_line == 0 {
                CornerRadius { nw: r, ne: 0, sw: 0, se: 0 }
            } else {
                CornerRadius::ZERO
            };
            painter.with_clip_rect(gutter_rect).rect_filled(
                Rect::from_min_max(
                    egui::pos2(gutter_rect.left(), y),
                    egui::pos2(gutter_rect.right(), y + rh),
                ),
                cr,
                p().active_line,
            );
        }
    }
    let gp = painter.with_clip_rect(gutter_rect);
    for line in first..=last {
        let y = line_y(line);
        if y + rh < sheet.top() || y > sheet.bottom() {
            continue;
        }
        gp.text(
            egui::pos2(text_left - GUT_R, y),
            egui::Align2::RIGHT_TOP,
            line + 1,
            code_font_regular(CODE_SIZE),
            p().text_dim,
        );
    }
    painter.vline(
        painter.round_to_pixel_center(text_left),
        sheet.y_range(),
        Stroke::new(crate::widgets::hairline(painter.ctx()), p().border),
    );
    // `sheet` is already pixel-snapped upstream (main.rs), so a 1-device-px inside stroke is crisp.
    painter.rect_stroke(
        sheet,
        sheet_cr,
        Stroke::new(crate::widgets::hairline(painter.ctx()), p().border_strong),
        egui::StrokeKind::Inside,
    );

    EditorOut {
        edited,
        changed_from,
        caret: ed.caret,
        error,
        origin: egui::pos2(ox, (top64 - sy) as f32),
        char_w,
        row_h: rh,
    }
}

/// Cached highlighting galley for line `line`, accounting for the lexer state at its start.
fn hl_line(
    doc: &mut Document,
    lex: &mut LexCache,
    line_cache: &mut LineCache,
    hl: &Highlighter,
    line: usize,
    ui: &egui::Ui,
) -> std::sync::Arc<egui::Galley> {
    let (state, exact) = lex.state_at(doc, line, hl.advance);
    if !exact {
        ui.ctx().request_repaint(); // finish re-lexing in the next frames
    }
    let text = doc.line_ref(line);
    let mut key = String::with_capacity(text.len() + 2);
    key.push(char::from(state));
    key.push('\u{1}');
    key.push_str(text);
    line_cache.get_or(&key, || {
        let (mut job, _) = (hl.line)(text, state);
        job.wrap.max_width = f32::INFINITY;
        ui.fonts_mut(|f| f.layout_job(job))
    })
}

/// Copy the selection to the clipboard.
fn handle_copy(doc: &mut Document, ed: &EditorState, ctx: &egui::Context, error: &mut Option<String>) {
    if ed.has_sel() {
        match ed.selection_text(doc) {
            Ok(s) => ctx.copy_text(s),
            Err(e) => *error = Some(e),
        }
    }
}

/// Apply a navigation key (arrows/Home/End/PageUp/PageDown + Cmd variants) to `ed`.
fn apply_nav_key(
    doc: &mut Document,
    ed: &mut EditorState,
    key: &egui::Key,
    modifiers: &egui::Modifiers,
    page_rows: usize,
) {
    use egui::Key;
    let sh = modifiers.shift;
    let rows = page_rows.max(1);
    match key {
        Key::ArrowLeft if modifiers.command => ed.move_word(doc, false, sh),
        Key::ArrowRight if modifiers.command => ed.move_word(doc, true, sh),
        Key::ArrowLeft => ed.move_h(doc, false, sh),
        Key::ArrowRight => ed.move_h(doc, true, sh),
        Key::ArrowUp => ed.move_v(doc, false, sh),
        Key::ArrowDown => ed.move_v(doc, true, sh),
        Key::Home if modifiers.command => ed.doc_home(sh),
        Key::End if modifiers.command => ed.doc_end(doc, sh),
        Key::Home => ed.home(doc, sh),
        Key::End => ed.end(doc, sh),
        Key::PageUp => ed.page(doc, false, rows, sh),
        Key::PageDown => ed.page(doc, true, rows, sh),
        _ => {}
    }
}

/// Regular editing keys (true if the buffer changed).
fn editor_input(
    doc: &mut Document,
    ed: &mut EditorState,
    ctx: &egui::Context,
    page_rows: usize,
    tab_insert: &dyn Fn(&mut Document, Pos) -> String,
    error: &mut Option<String>,
) -> bool {
    use egui::{Key, Modifiers};
    let cmd = Modifiers::COMMAND;
    let mut changed = false;
    let eol = doc.eol.bytes();
    let eol_str = std::str::from_utf8(eol).unwrap_or("\n").to_owned();

    if ctx.input_mut(|i| i.consume_key(cmd, Key::A)) {
        ed.select_all(doc);
    }
    // Undo / Redo via Ctrl+Z / Ctrl+Shift+Z. We distinguish Shift manually (not with two
    // consecutive consume_key calls): the first consume_key(cmd, Z) would otherwise also catch
    // Ctrl+Shift+Z. Ctrl+Y removed (it will return as a user setting).
    let mut do_undo = false;
    let mut do_redo = false;
    ctx.input_mut(|i| {
        i.events.retain(|ev| match ev {
            egui::Event::Key { key: Key::Z, pressed: true, modifiers, .. }
                if modifiers.command && !modifiers.alt =>
            {
                if modifiers.shift {
                    do_redo = true;
                } else {
                    do_undo = true;
                }
                false // event consumed
            }
            _ => true,
        });
    });
    if do_redo {
        changed |= ed.redo_op(doc);
    } else if do_undo {
        changed |= ed.undo_op(doc);
    }
    let events = ctx.input(|i| i.events.clone());
    for ev in &events {
        match ev {
            egui::Event::Copy => handle_copy(doc, ed, ctx, error),
            egui::Event::Cut => {
                if ed.has_sel() {
                    match ed.selection_text(doc) {
                        Ok(s) => {
                            ctx.copy_text(s);
                            ed.replace(doc, "");
                            changed = true;
                        }
                        Err(e) => *error = Some(e),
                    }
                }
            }
            egui::Event::Paste(t) => {
                let norm = t.replace("\r\n", "\n").replace('\r', "\n").replace('\n', &eol_str);
                ed.replace(doc, &norm);
                changed = true;
            }
            egui::Event::Text(t) if !t.chars().any(|c| c.is_control()) && !t.is_empty() => {
                ed.replace(doc, t);
                changed = true;
            }
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                match key {
                    Key::Enter => {
                        // line break + indentation of the current line
                        let (l, c) = if ed.caret <= ed.anchor { ed.caret } else { ed.anchor };
                        let text = doc.get_line(l);
                        let indent: String = text
                            .chars()
                            .take(c)
                            .take_while(|ch| *ch == ' ' || *ch == '\t')
                            .collect();
                        ed.replace(doc, &format!("{eol_str}{indent}"));
                        changed = true;
                    }
                    Key::Tab => {
                        let pos = if ed.caret <= ed.anchor { ed.caret } else { ed.anchor };
                        let ins = tab_insert(doc, pos);
                        ed.replace(doc, &ins);
                        changed = true;
                    }
                    Key::Backspace if modifiers.command => {
                        ed.backspace_word(doc);
                        changed = true;
                    }
                    Key::Delete if modifiers.command => {
                        ed.delete_word(doc);
                        changed = true;
                    }
                    Key::Backspace => {
                        ed.backspace(doc);
                        changed = true;
                    }
                    Key::Delete => {
                        ed.delete(doc);
                        changed = true;
                    }
                    _ => apply_nav_key(doc, ed, key, modifiers, page_rows),
                }
            }
            _ => {}
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    // the tests set the caret/anchor directly in private fields — a struct literal won't compile here
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    use crate::doc::Document;

    fn doc_from(text: &str) -> Document {
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), text);
        d
    }

    #[test]
    fn lex_cache_stays_bounded_on_long_scroll() {
        // Gradually "scroll" down a long document; the lexer-state cache must stay O(window) instead
        // of growing one entry per line crossed (a far jump re-anchors, but a line-by-line scroll
        // never did). Regression for the unbounded `LexCache.states` growth.
        let n_lines = 30_000usize;
        let mut d = doc_from(&"x\n".repeat(n_lines));
        let mut lex = LexCache::default();
        let advance = |_t: &str, st: LexState| st; // no-op lexer: state never changes
        let mut target = 0usize;
        while target < n_lines {
            // pump until the budget-limited cache reaches `target` (simulates per-frame progress)
            for _ in 0..64 {
                if lex.state_at(&mut d, target, &advance).1 {
                    break;
                }
            }
            target += 500;
        }
        assert!(
            lex.states.len() <= LexCache::REANCHOR_GAP * 2 + 1,
            "LexCache.states grew unbounded: {}",
            lex.states.len()
        );
    }

    #[test]
    fn replace_moves_caret() {
        let mut d = doc_from("abcd");
        let mut e = EditorState::default();
        e.anchor = (0, 1);
        e.caret = (0, 3); // "bc" selected
        e.replace(&mut d, "X");
        assert_eq!(d.get_line(0), "aXd");
        assert_eq!(e.caret(), (0, 2));
        assert!(!e.has_sel());
    }

    #[test]
    fn backspace_joins_lines() {
        let mut d = doc_from("ab\ncd");
        let mut e = EditorState::default();
        e.caret = (1, 0);
        e.anchor = (1, 0);
        e.backspace(&mut d);
        assert_eq!(d.get_line(0), "abcd");
        assert_eq!(e.caret(), (0, 2));
    }

    #[test]
    fn delete_at_line_end_joins() {
        let mut d = doc_from("ab\ncd");
        let mut e = EditorState::default();
        e.caret = (0, 2);
        e.anchor = (0, 2);
        e.delete(&mut d);
        assert_eq!(d.get_line(0), "abcd");
    }

    #[test]
    fn vertical_keeps_preferred_column() {
        let mut d = doc_from("abcd\nxy\nzzzz");
        let mut e = EditorState::default();
        e.caret = (0, 4);
        e.anchor = (0, 4);
        e.move_v(&mut d, true, false);
        assert_eq!(e.caret(), (1, 2));
        e.move_v(&mut d, true, false);
        assert_eq!(e.caret(), (2, 4));
    }

    #[test]
    fn horizontal_wraps_lines() {
        let mut d = doc_from("ab\ncd");
        let mut e = EditorState::default();
        e.caret = (0, 2);
        e.anchor = (0, 2);
        e.move_h(&mut d, true, false);
        assert_eq!(e.caret(), (1, 0));
        e.move_h(&mut d, false, false);
        assert_eq!(e.caret(), (0, 2));
    }

    #[test]
    fn word_step_moves_by_word() {
        let mut d = doc_from("foo bar");
        let mut e = EditorState::default();
        e.move_word(&mut d, true, false);
        assert_eq!(e.caret(), (0, 3));
        e.move_word(&mut d, true, false);
        assert_eq!(e.caret(), (0, 7));
        e.move_word(&mut d, false, false);
        assert_eq!(e.caret(), (0, 4));
        e.move_word(&mut d, false, false);
        assert_eq!(e.caret(), (0, 0));
    }

    #[test]
    fn backspace_word_deletes_preceding_word() {
        let mut d = doc_from("foo bar baz");
        let mut e = EditorState::default();
        e.caret = (0, 11);
        e.anchor = (0, 11);
        e.backspace_word(&mut d); // removes "baz"
        assert_eq!(d.get_line(0), "foo bar ");
        assert_eq!(e.caret(), (0, 8));
        e.backspace_word(&mut d); // removes "bar" plus the trailing space
        assert_eq!(d.get_line(0), "foo ");
        assert_eq!(e.caret(), (0, 4));
    }

    #[test]
    fn backspace_word_removes_selection_first() {
        let mut d = doc_from("foo bar baz");
        let mut e = EditorState::default();
        e.anchor = (0, 0);
        e.caret = (0, 7); // "foo bar" selected
        e.backspace_word(&mut d);
        assert_eq!(d.get_line(0), " baz");
        assert_eq!(e.caret(), (0, 0));
    }

    #[test]
    fn backspace_word_joins_lines_at_start() {
        let mut d = doc_from("ab\ncd");
        let mut e = EditorState::default();
        e.caret = (1, 0);
        e.anchor = (1, 0);
        e.backspace_word(&mut d);
        assert_eq!(d.get_line(0), "abcd");
        assert_eq!(e.caret(), (0, 2));
    }

    #[test]
    fn backspace_word_noop_at_doc_start() {
        let mut d = doc_from("foo");
        let mut e = EditorState::default();
        e.caret = (0, 0);
        e.anchor = (0, 0);
        e.backspace_word(&mut d);
        assert_eq!(d.get_line(0), "foo");
    }

    #[test]
    fn delete_word_deletes_following_word() {
        let mut d = doc_from("foo bar baz");
        let mut e = EditorState::default();
        e.caret = (0, 0);
        e.anchor = (0, 0);
        e.delete_word(&mut d); // removes "foo"
        assert_eq!(d.get_line(0), " bar baz");
        assert_eq!(e.caret(), (0, 0));
        e.delete_word(&mut d); // removes leading space + "bar"
        assert_eq!(d.get_line(0), " baz");
    }

    #[test]
    fn delete_word_noop_at_doc_end() {
        let mut d = doc_from("foo");
        let mut e = EditorState::default();
        e.caret = (0, 3);
        e.anchor = (0, 3);
        e.delete_word(&mut d);
        assert_eq!(d.get_line(0), "foo");
    }

    #[test]
    fn home_smart_toggles() {
        let mut d = doc_from("    select");
        let mut e = EditorState::default();
        e.caret = (0, 10);
        e.anchor = (0, 10);
        e.home(&mut d, false);
        assert_eq!(e.caret(), (0, 4));
        e.home(&mut d, false);
        assert_eq!(e.caret(), (0, 0));
    }

    #[test]
    fn undo_via_editor() {
        let mut d = doc_from("");
        let mut e = EditorState::default();
        e.replace(&mut d, "a");
        e.replace(&mut d, "b");
        assert_eq!(d.get_line(0), "ab");
        assert!(e.undo_op(&mut d)); // the merged typing is undone in a single step
        assert_eq!(d.get_line(0), "");
    }

    #[test]
    fn selection_text_multiline() {
        let mut d = doc_from("ab\ncd\nef");
        let mut e = EditorState::default();
        e.anchor = (0, 1);
        e.caret = (2, 1);
        assert_eq!(e.selection_text(&mut d).unwrap(), "b\ncd\ne");
    }

    #[test]
    fn select_range_clamps_and_scrolls() {
        let mut d = doc_from("ab\ncd");
        let mut e = EditorState::default();
        e.select_range(&mut d, (9, 9), (9, 9));
        assert_eq!(e.caret(), (1, 2));
        assert!(!e.has_sel());
        assert!(e.scroll_to_caret);
    }

    #[test]
    fn lex_cache_advances_and_invalidates() {
        // toy lexer: "[[" opens a block (state 1), "]]" closes it (0)
        let advance = |text: &str, st: LexState| -> LexState {
            let mut s = st;
            let b = text.as_bytes();
            let mut i = 0;
            while i + 1 < b.len() {
                match (s, &b[i..i + 2]) {
                    (0, b"[[") => {
                        s = 1;
                        i += 2;
                    }
                    (1, b"]]") => {
                        s = 0;
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            s
        };
        let mut d = doc_from("a [[ x\nstill\n]] b\ntext");
        let mut lex = LexCache::default();
        assert_eq!(lex.state_at(&mut d, 0, &advance), (0, true));
        assert_eq!(lex.state_at(&mut d, 1, &advance), (1, true));
        assert_eq!(lex.state_at(&mut d, 2, &advance), (1, true));
        assert_eq!(lex.state_at(&mut d, 3, &advance), (0, true));
        // editing line 0: states after it are invalidated, but get recomputed
        lex.invalidate_from(0);
        assert_eq!(lex.state_at(&mut d, 1, &advance), (1, true));
    }
}
