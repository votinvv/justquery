//! Virtualized SQL code editor.
//!
//! Only the *visible* lines are laid out and painted, so editing and scrolling are O(visible)
//! regardless of file size — a 100k-line buffer renders as fast as a 10-line one. The active tab's
//! text stays the source of truth (`Tab.sql`); this module owns the caret, selection, scroll-to and
//! undo/redo, plus a cached line index. Caret/anchor are CHAR indices (matching find/completion).
//!
//! The pure pieces (index, caret movement, edits) are unit-tested; rendering/input live in
//! [`JustQueryApp::code_editor`].

use crate::theme::p;
use crate::{code_font, code_font_regular, complete, highlight::highlight_sql, JustQueryApp, CODE_SIZE};
use eframe::egui;
use egui::{CornerRadius, Rect, Stroke};

/// A two-generation galley cache keyed by line content. Avoids the periodic "clear everything"
/// stall: when the live generation fills up, it's demoted to `prev` (still served, promoted on hit)
/// and a fresh one starts — so recently-seen lines stay warm and nothing is laid out twice on a
/// scroll-back. Memory is bounded to ~2×CAP galleys.
#[derive(Default)]
pub(crate) struct LineCache {
    cur: std::collections::HashMap<String, std::sync::Arc<egui::Galley>>,
    prev: std::collections::HashMap<String, std::sync::Arc<egui::Galley>>,
}

impl LineCache {
    const CAP: usize = 8192;

    /// Drop both generations. Call on theme switch: cached galleys carry the OLD palette's
    /// colours and point into the old font atlas, which renders as glyph garbage after the
    /// atlas is rebuilt.
    pub fn clear(&mut self) {
        self.cur.clear();
        self.prev.clear();
    }

    pub fn get_or(
        &mut self,
        text: &str,
        make: impl FnOnce() -> std::sync::Arc<egui::Galley>,
    ) -> std::sync::Arc<egui::Galley> {
        if let Some(g) = self.cur.get(text) {
            return g.clone();
        }
        if let Some(g) = self.prev.remove(text) {
            // promote to the live generation
            self.cur.insert(text.to_owned(), g.clone());
            return g;
        }
        let g = make();
        if self.cur.len() >= Self::CAP {
            std::mem::swap(&mut self.cur, &mut self.prev);
            self.cur.clear(); // `cur` is now the old `prev`; drop it, keep old `cur` as `prev`
        }
        self.cur.insert(text.to_owned(), g.clone());
        g
    }
}

/// What the last edit was, for undo coalescing (a run of typing collapses into one undo step).
#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) enum EditKind {
    #[default]
    None,
    Type,
    Delete,
}

/// Per-tab editor state. Driven entirely from within this module; `main.rs` only calls the few
/// `pub` methods below (sync / edit / selection helpers).
pub(crate) struct EditorState {
    caret: usize,            // char index of the caret (moving end of a selection)
    anchor: usize,           // selection anchor (caret == anchor → no selection)
    blink_t0: f64,           // time of the last caret move/edit, for blink
    blink_caret: usize,      // caret pos when the blink last restarted (detect moves)
    pref_col: Option<usize>, // sticky column for Up/Down
    scroll_to_caret: bool,   // bring the caret into view next layout
    // cached line index, rebuilt only when the byte length changes (i.e. on edit / load)
    line_char: Vec<usize>, // char index of each line start
    line_byte: Vec<usize>, // byte index of each line start
    n_chars: usize,        // total char count of the indexed text
    max_cols: usize,       // longest line length in chars (for the horizontal content width)
    indexed_len: usize,    // sql.len() (bytes) when indexed — staleness check
    undo: Vec<(String, usize, usize)>,
    redo: Vec<(String, usize, usize)>,
    last_edit: EditKind,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            caret: 0,
            anchor: 0,
            blink_t0: 0.0,
            blink_caret: usize::MAX,
            pref_col: None,
            scroll_to_caret: false,
            line_char: vec![0],
            line_byte: vec![0],
            n_chars: 0,
            max_cols: 0,
            indexed_len: usize::MAX, // force a rebuild on first use
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: EditKind::None,
        }
    }
}

const MAX_UNDO: usize = 300;
const PAD_L: f32 = 6.0; // gap between gutter and text
const GUT_R: f32 = 8.0; // gutter right padding (number → divider)
const GUT_L: f32 = 6.0; // gutter left padding

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// One word-step left/right of `c` over `chars`.
fn word_step(chars: &[char], c: usize, right: bool) -> usize {
    let n = chars.len();
    let mut i = c.min(n);
    if right {
        while i < n && !is_word(chars[i]) {
            i += 1;
        }
        while i < n && is_word(chars[i]) {
            i += 1;
        }
    } else {
        while i > 0 && !is_word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(chars[i - 1]) {
            i -= 1;
        }
    }
    i
}

impl EditorState {
    /// Rebuild the line index if the text changed (cheap O(1) length check; O(n) rebuild on edit).
    pub fn sync(&mut self, sql: &str) {
        if self.indexed_len == sql.len() {
            return;
        }
        self.line_char.clear();
        self.line_byte.clear();
        self.line_char.push(0);
        self.line_byte.push(0);
        let mut ci = 0usize;
        for (bi, ch) in sql.char_indices() {
            ci += 1;
            if ch == '\n' {
                self.line_char.push(ci);
                self.line_byte.push(bi + 1);
            }
        }
        self.n_chars = ci;
        self.indexed_len = sql.len();
        self.caret = self.caret.min(self.n_chars);
        self.anchor = self.anchor.min(self.n_chars);
        // longest line (cached so the per-frame content width is O(1), not O(lines))
        let mut maxc = 0usize;
        for w in self.line_char.windows(2) {
            maxc = maxc.max(w[1] - w[0] - 1);
        }
        maxc = maxc.max(self.n_chars - self.line_char.last().copied().unwrap_or(0));
        self.max_cols = maxc;
    }

    fn max_cols(&self) -> usize {
        self.max_cols
    }

    fn n_lines(&self) -> usize {
        self.line_char.len()
    }

    fn line_of(&self, c: usize) -> usize {
        match self.line_char.binary_search(&c) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }
    fn line_len(&self, i: usize) -> usize {
        let start = self.line_char[i];
        let end = self.line_char.get(i + 1).map(|&n| n - 1).unwrap_or(self.n_chars);
        end - start
    }
    fn line_bytes(&self, i: usize, sql: &str) -> (usize, usize) {
        let start = self.line_byte[i];
        let end = self.line_byte.get(i + 1).map(|&n| n - 1).unwrap_or(sql.len());
        (start, end)
    }
    fn line_col(&self, c: usize) -> (usize, usize) {
        let l = self.line_of(c);
        (l, c - self.line_char[l])
    }
    fn pos_of(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.n_lines() - 1);
        self.line_char[line] + col.min(self.line_len(line))
    }
    fn byte_of(&self, c: usize, sql: &str) -> usize {
        let l = self.line_of(c);
        let col = c - self.line_char[l];
        let (lb, le) = self.line_bytes(l, sql);
        sql[lb..le]
            .char_indices()
            .nth(col)
            .map(|(b, _)| lb + b)
            .unwrap_or(le)
    }

    fn sel(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }
    pub fn has_sel(&self) -> bool {
        self.caret != self.anchor
    }
    pub fn selection_text(&self, sql: &str) -> String {
        let (a, b) = self.sel();
        if a == b {
            return String::new();
        }
        let (ba, bb) = (self.byte_of(a, sql), self.byte_of(b, sql));
        sql[ba..bb].to_owned()
    }

    fn snapshot(&mut self, sql: &str, kind: EditKind) {
        if kind != self.last_edit || kind == EditKind::None {
            self.undo.push((sql.to_owned(), self.caret, self.anchor));
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_edit = kind;
    }

    /// Replace the selection (or insert at the caret) with `s`.
    pub fn replace(&mut self, sql: &mut String, s: &str, kind: EditKind) {
        self.snapshot(sql, kind);
        let (a, b) = self.sel();
        let (ba, bb) = (self.byte_of(a, sql), self.byte_of(b, sql));
        sql.replace_range(ba..bb, s);
        let new_caret = a + s.chars().count();
        self.indexed_len = usize::MAX;
        self.sync(sql);
        self.caret = new_caret;
        self.anchor = new_caret;
        self.pref_col = None;
        self.scroll_to_caret = true;
    }

    fn backspace(&mut self, sql: &mut String) {
        if !self.has_sel() {
            if self.caret == 0 {
                return;
            }
            self.anchor = self.caret - 1;
        }
        self.replace(sql, "", EditKind::Delete);
    }
    fn delete(&mut self, sql: &mut String) {
        if !self.has_sel() {
            if self.caret >= self.n_chars {
                return;
            }
            self.anchor = self.caret + 1;
        }
        self.replace(sql, "", EditKind::Delete);
    }

    pub fn undo_op(&mut self, sql: &mut String) -> bool {
        if let Some((t, c, a)) = self.undo.pop() {
            self.redo.push((sql.clone(), self.caret, self.anchor));
            *sql = t;
            self.indexed_len = usize::MAX;
            self.sync(sql);
            self.caret = c.min(self.n_chars);
            self.anchor = a.min(self.n_chars);
            self.last_edit = EditKind::None;
            self.scroll_to_caret = true;
            true
        } else {
            false
        }
    }
    pub fn redo_op(&mut self, sql: &mut String) -> bool {
        if let Some((t, c, a)) = self.redo.pop() {
            self.undo.push((sql.clone(), self.caret, self.anchor));
            *sql = t;
            self.indexed_len = usize::MAX;
            self.sync(sql);
            self.caret = c.min(self.n_chars);
            self.anchor = a.min(self.n_chars);
            self.last_edit = EditKind::None;
            self.scroll_to_caret = true;
            true
        } else {
            false
        }
    }

    fn move_to(&mut self, c: usize, select: bool) {
        self.caret = c.min(self.n_chars);
        if !select {
            self.anchor = self.caret;
        }
        self.last_edit = EditKind::None;
        self.scroll_to_caret = true;
    }

    fn move_h(&mut self, right: bool, select: bool) {
        self.pref_col = None;
        if self.has_sel() && !select {
            let (a, b) = self.sel();
            self.move_to(if right { b } else { a }, false);
            return;
        }
        let c = if right {
            (self.caret + 1).min(self.n_chars)
        } else {
            self.caret.saturating_sub(1)
        };
        self.move_to(c, select);
    }

    fn move_word(&mut self, chars: &[char], right: bool, select: bool) {
        self.pref_col = None;
        let c = word_step(chars, self.caret, right);
        self.move_to(c, select);
    }

    fn move_v(&mut self, down: bool, select: bool) {
        let (line, col) = self.line_col(self.caret);
        let goal = *self.pref_col.get_or_insert(col);
        if down && line + 1 >= self.n_lines() {
            return self.move_keep_pref(self.n_chars, select);
        }
        if !down && line == 0 {
            return self.move_keep_pref(0, select);
        }
        let target = if down { line + 1 } else { line - 1 };
        let c = self.pos_of(target, goal);
        self.move_keep_pref(c, select);
    }

    fn page(&mut self, down: bool, rows: usize, select: bool) {
        let (line, col) = self.line_col(self.caret);
        let goal = *self.pref_col.get_or_insert(col);
        let target = if down {
            (line + rows).min(self.n_lines() - 1)
        } else {
            line.saturating_sub(rows)
        };
        let c = self.pos_of(target, goal);
        self.move_keep_pref(c, select);
    }

    /// Move keeping the sticky preferred column (for vertical motion).
    fn move_keep_pref(&mut self, c: usize, select: bool) {
        self.caret = c.min(self.n_chars);
        if !select {
            self.anchor = self.caret;
        }
        self.last_edit = EditKind::None;
        self.scroll_to_caret = true;
    }

    /// Home: first non-blank, then column 0 (smart Home).
    fn home(&mut self, sql: &str, select: bool) {
        self.pref_col = None;
        let line = self.line_of(self.caret);
        let ls = self.line_char[line];
        let (lb, le) = self.line_bytes(line, sql);
        let indent = sql[lb..le].chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let col = self.caret - ls;
        let target = if col == indent { ls } else { ls + indent };
        self.move_to(target, select);
    }
    fn end(&mut self, select: bool) {
        self.pref_col = None;
        let line = self.line_of(self.caret);
        self.move_to(self.line_char[line] + self.line_len(line), select);
    }
    fn doc_home(&mut self, select: bool) {
        self.pref_col = None;
        self.move_to(0, select);
    }
    fn doc_end(&mut self, select: bool) {
        self.pref_col = None;
        self.move_to(self.n_chars, select);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.n_chars;
    }

    /// Place the caret at a screen pixel offset relative to the text origin (PAD_L already removed).
    fn set_from_pixel(&mut self, rel_x: f32, rel_y: f32, rh: f32, char_w: f32, select: bool) {
        let line = (rel_y / rh).floor().max(0.0) as usize;
        let line = line.min(self.n_lines().saturating_sub(1));
        let col = (rel_x / char_w + 0.5).floor().max(0.0) as usize;
        let c = self.pos_of(line, col);
        self.caret = c;
        if !select {
            self.anchor = c;
        }
        self.pref_col = None;
        self.last_edit = EditKind::None;
    }

    /// Double-click: select the whole word under the pixel. If the click isn't on a word char
    /// (whitespace / punctuation), fall back to placing the caret there with no selection.
    fn select_word_from_pixel(&mut self, rel_x: f32, rel_y: f32, rh: f32, char_w: f32, sql: &str) {
        let line = (rel_y / rh).floor().max(0.0) as usize;
        let line = line.min(self.n_lines().saturating_sub(1));
        // char actually under the pointer (floor, not the +0.5 caret rounding)
        let col = (rel_x / char_w).floor().max(0.0) as usize;
        let c = self.pos_of(line, col);
        let chars: Vec<char> = sql.chars().collect();
        let n = chars.len();
        // pick the word at `c`, or the one ending just before it (click past a word's end)
        let on = if c < n && is_word(chars[c]) {
            Some(c)
        } else if c > 0 && is_word(chars[c - 1]) {
            Some(c - 1)
        } else {
            None
        };
        match on {
            Some(mut s) => {
                let mut e = s;
                while s > 0 && is_word(chars[s - 1]) {
                    s -= 1;
                }
                while e < n && is_word(chars[e]) {
                    e += 1;
                }
                self.anchor = s;
                self.caret = e;
            }
            None => {
                self.caret = c;
                self.anchor = c;
            }
        }
        self.pref_col = None;
        self.last_edit = EditKind::None;
    }
}

impl JustQueryApp {
    /// Render + drive the virtualized SQL editor for the active tab inside `sheet`.
    pub(crate) fn code_editor(&mut self, ui: &mut egui::Ui, sheet: Rect) {
        let ctx = &ui.ctx().clone();
        let idx = self.active_tab.min(self.tabs.len() - 1);
        let tab_id = self.tabs[idx].id;
        let ed_id = egui::Id::new(("code_editor", tab_id));

        let mut sql = std::mem::take(&mut self.tabs[idx].sql);
        let mut ed = std::mem::take(&mut self.tabs[idx].ed);
        ed.sync(&sql);

        let rh = ui.ctx().fonts_mut(|f| f.row_height(&code_font(CODE_SIZE)));
        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&code_font(CODE_SIZE), '0'));

        // gutter geometry
        let digits = ed.n_lines().to_string().len().max(3);
        let gw = (char_w * digits as f32).ceil() + GUT_L + GUT_R;
        let text_left = sheet.left() + gw;
        let gutter_rect = Rect::from_min_max(sheet.min, egui::pos2(text_left, sheet.max.y));
        // The editor island's OUTER frame rounds on all four corners (including the left, where the
        // gutter sits). What must stay square is the INNER seam between the gutter and the text — so
        // the gutter rounds only its outer-left corners and is flush (square) against the text.
        let r = crate::RADIUS_ISLAND;
        let sheet_cr = CornerRadius::same(r);
        let gutter_cr = CornerRadius { nw: r, ne: 0, sw: r, se: 0 };
        // the first line sits flush at the top of the sheet (no top padding); the active-line
        // highlight rounds its top corners so it never spills past the rounded frame
        let view = Rect::from_min_max(egui::pos2(text_left, sheet.top()), sheet.max);
        let rows_vis = (view.height() / rh).ceil() as usize + 1;

        // external focus request (new tab / open file / menu). A mouse click on the triggering
        // button is a "click elsewhere", so egui clears the editor focus at end of frame; the native
        // file dialog likewise steals window focus for a frame. Re-request over a short grace window
        // so the focus survives those frames.
        if std::mem::take(&mut self.focus_editor) {
            self.focus_grace = 4;
        }
        if self.focus_grace > 0 {
            ctx.memory_mut(|m| m.request_focus(ed_id));
            self.focus_grace -= 1;
            ctx.request_repaint(); // ensure the grace frames actually run even if otherwise idle
        }
        let focused = ctx.memory(|m| m.has_focus(ed_id));
        // keep arrows / Tab / Esc for the editor instead of egui's focus navigation (begin_pass
        // would otherwise steal them, since this isn't a built-in focusable widget)
        if focused {
            ctx.memory_mut(|m| {
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

        // a pending find result → select it + scroll to it
        if let Some((s, e)) = self.pending_find.take() {
            ed.anchor = s.min(ed.n_chars);
            ed.caret = e.min(ed.n_chars);
            ed.scroll_to_caret = true;
        }

        // ---- keyboard: completion popup first, then normal editing ----
        let mut edited = false;
        if focused {
            edited |= self.editor_completion(&mut sql, &mut ed, ctx, tab_id);
            edited |= self.editor_input(&mut sql, &mut ed, ctx, rows_vis.saturating_sub(2));
        }

        // status bar + find mirror
        let (cl, cc) = ed.line_col(ed.caret);
        self.cursor_ln = cl + 1;
        self.cursor_col = cc + 1;
        self.caret = ed.caret;

        // ---- virtualized scroll + paint ----
        // Fill the WHOLE sheet (not just `view`) with the editor surface so the app chrome never
        // shows through the 4px top padding above line 1 (the "app background on the editor" bleed).
        // The gutter is painted over the left of this below.
        ui.painter().rect_filled(sheet, sheet_cr, p().field_bg);
        // content size = actual text extent only. Do NOT pad either dimension up to the viewport:
        // forcing one to the viewport size makes the other scrollbar shrink the inner area below the
        // content and pops a spurious second scrollbar (h-bar → spurious v-bar and vice-versa).
        let total_h = ed.n_lines() as f32 * rh;
        // Caret x comes from the ACTUAL line galley, not `col * char_w`: for monospace text the two
        // agree to the sub-pixel, but if a line holds an odd-width glyph (a pasted symbol the code
        // font lacks, drawn from a proportional fallback) `col * char_w` drifts off the real text
        // and the caret lands a character short — the horizontal twin of trusting the galley's
        // geometry (not a metric) for vertical positioning. The content width also covers that
        // line's real width, so End can always scroll the true end into view.
        // (the caret's own x is measured the same way, from the live caret, in the draw block below)
        let caret_line = ed.line_of(ed.caret);
        let (clb, cle) = ed.line_bytes(caret_line, &sql);
        let caret_galley = self.hl_line(ui, &sql[clb..cle]);
        let total_w = (ed.max_cols() as f32 * char_w).max(caret_galley.rect.width()) + PAD_L + 12.0;

        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(view));
        child.set_clip_rect(view);
        crate::style_scrollbar(&mut child);
        child.style_mut().visuals.extreme_bg_color = p().grid_header;

        let want_scroll = std::mem::take(&mut ed.scroll_to_caret);
        let out = egui::ScrollArea::both()
            .auto_shrink([false, false]) // fill `view`; don't shrink the area to the content width
            .id_salt(("code_scroll", tab_id))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show_viewport(&mut child, |ui, _viewport| {
                ui.set_height(total_h);
                ui.set_width(total_w);
                let origin = ui.min_rect().min; // content top-left (scrolled) in screen coords
                let off_y = view.top() - origin.y; // current vertical scroll offset

                // mouse: one interaction over the visible area (inside the scroll content, so it
                // doesn't compete with the ScrollArea's own wheel handling). Sets the I-beam cursor,
                // focuses on click, and maps clicks/drags to the caret using the live `origin`.
                let resp = ui.interact(view, ed_id, egui::Sense::click_and_drag());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                }
                if let Some(p) = resp.interact_pointer_pos() {
                    let rel_x = p.x - origin.x - PAD_L;
                    let rel_y = p.y - origin.y;
                    // Move the caret on the button PRESS (`primary_pressed`), not on the click's
                    // release: a tap should reposition the caret the instant the button goes down so
                    // it never feels like it's waiting for the click to "complete" (egui defers
                    // `clicked()` to the release, and to tell a click apart from a drag). The caret
                    // is then drawn from this live position in the same frame (see the draw block).
                    let pressed = ui.input(|i| i.pointer.primary_pressed());
                    if resp.double_clicked() {
                        ed.select_word_from_pixel(rel_x, rel_y, rh, char_w, &sql);
                    } else if pressed {
                        resp.request_focus();
                        let extend = ui.input(|i| i.modifiers.shift);
                        ed.set_from_pixel(rel_x, rel_y, rh, char_w, extend);
                    } else if resp.dragged() {
                        ed.set_from_pixel(rel_x, rel_y, rh, char_w, true);
                    }
                }
                let caret_line = ed.line_of(ed.caret);

                let first = (off_y / rh).floor().max(0.0) as usize;
                let last = (first + rows_vis).min(ed.n_lines().saturating_sub(1));
                let (sa, sb) = ed.sel();

                for line in first..=last {
                    let y = origin.y + line as f32 * rh;
                    let lc0 = ed.line_char[line];
                    let llen = ed.line_len(line);
                    let (lb, le) = ed.line_bytes(line, &sql);
                    let text = &sql[lb..le];

                    // active line (no selection) — span the full visible width (clipped to `view`).
                    // On the first line, round the top-right so it meets the frame's rounded corner
                    // instead of spilling a square corner past it.
                    if !ed.has_sel() && line == caret_line {
                        let cr = if line == 0 {
                            CornerRadius { nw: 0, ne: crate::RADIUS_ISLAND, sw: 0, se: 0 }
                        } else {
                            CornerRadius::ZERO
                        };
                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                egui::pos2(view.left(), y),
                                egui::pos2(view.right(), y + rh),
                            ),
                            cr,
                            p().active_line,
                        );
                    }
                    // selection
                    if sb > sa {
                        let le2 = lc0 + llen;
                        if sb > lc0 && sa <= le2 {
                            let s0 = sa.saturating_sub(lc0).min(llen);
                            let s1 = (sb - lc0).min(llen);
                            let x0 = origin.x + PAD_L + s0 as f32 * char_w;
                            let extra = if sb > le2 { char_w * 0.5 } else { 0.0 };
                            let x1 = origin.x + PAD_L + s1 as f32 * char_w + extra;
                            ui.painter().rect_filled(
                                Rect::from_min_max(egui::pos2(x0, y), egui::pos2(x1, y + rh)),
                                CornerRadius::ZERO,
                                p().editor_sel,
                            );
                        }
                    }
                    // text
                    if !text.is_empty() {
                        let galley = self.hl_line(ui, text);
                        ui.painter().galley(egui::pos2(origin.x + PAD_L, y), galley, p().text);
                    }
                }

                // caret (with our own blink)
                if focused {
                    // recompute the caret geometry from the LIVE caret: a click this frame moved it
                    // inside the closure above, but the outer `caret_local_x` was measured from the
                    // pre-click caret. Without this, a click snaps to the new LINE immediately (that
                    // `caret_line` is recomputed post-click) yet the horizontal x is one frame stale,
                    // so the caret visibly "runs" across to the clicked column on the next frame.
                    let caret_line = ed.line_of(ed.caret);
                    let caret_col = ed.caret - ed.line_char[caret_line];
                    let (clb, cle) = ed.line_bytes(caret_line, &sql);
                    let caret_galley = self.hl_line(ui, &sql[clb..cle]);
                    let caret_local_x =
                        caret_galley.pos_from_cursor(egui::text::CCursor::new(caret_col)).min.x;
                    let now = ui.input(|i| i.time);
                    // restart the blink (solid on) whenever the caret moves or the text changes —
                    // including a click this frame — so it never reappears mid-"off" and feels laggy
                    if edited || ed.caret != ed.blink_caret {
                        ed.blink_t0 = now;
                        ed.blink_caret = ed.caret;
                    }
                    let cyc = ((now - ed.blink_t0) % 1.0) as f32;
                    let cx = (origin.x + PAD_L + caret_local_x).round() + 0.5;
                    let cy = origin.y + caret_line as f32 * rh;
                    #[cfg(test)]
                    ui.ctx().data_mut(|d| {
                        // (caret screen x, view left, view right, caret x within its line, line width)
                        d.insert_temp(
                            egui::Id::new("dbg_caret"),
                            (cx, view.left(), view.right(), caret_local_x, caret_galley.rect.width()),
                        )
                    });
                    if cyc < 0.5 {
                        // coral caret — the studio signature (Design System v2 §6 Editor)
                        ui.painter().line_segment(
                            [egui::pos2(cx, cy), egui::pos2(cx, cy + rh)],
                            Stroke::new(1.0, p().accent),
                        );
                    }
                    if want_scroll {
                        // keep a few characters of breathing room on each side of the caret so it
                        // never glues to the right edge and "runs away" off-screen — the horizontal
                        // analogue of the extra row kept above/below for vertical scrolling.
                        let margin = char_w * 3.0;
                        ui.scroll_to_rect(
                            Rect::from_min_max(
                                egui::pos2(cx - margin, cy),
                                egui::pos2(cx + margin, cy + rh),
                            ),
                            None,
                        );
                    }
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32(
                        if cyc < 0.5 { 0.5 - cyc } else { 1.0 - cyc },
                    ));
                }
            });

        // ---- gutter (painted over the chrome) ----
        let painter = ui.painter();
        painter.rect_filled(gutter_rect, gutter_cr, p().gutter);
        let top0 = view.top() - out.state.offset.y; // screen y of line 0
        let caret_line = ed.line_of(ed.caret);
        if !ed.has_sel() {
            let y = top0 + caret_line as f32 * rh;
            if y + rh > sheet.top() && y < sheet.bottom() {
                // first line rounds its top-left to meet the gutter's rounded frame corner
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
        let first = (out.state.offset.y / rh).floor().max(0.0) as usize;
        let last = (first + rows_vis).min(ed.n_lines().saturating_sub(1));
        for line in first..=last {
            let y = top0 + line as f32 * rh;
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
        painter.vline(text_left - 0.5, sheet.y_range(), Stroke::new(1.0, p().border));
        painter.rect_stroke(
            sheet,
            sheet_cr, // square left (against the gutter divider), rounded right
            Stroke::new(1.0, p().border_strong),
            egui::StrokeKind::Inside,
        );

        // ---- completion popup ----
        if self.ac.open && !self.ac.items.is_empty() && self.ac.tab == tab_id {
            let (line, scol) = ed.line_col(self.ac.start.min(ed.n_chars));
            let ax = text_left + PAD_L + scol as f32 * char_w - out.state.offset.x;
            let ay = top0 + (line + 1) as f32 * rh + 2.0;
            self.completion_popup(ctx, tab_id, egui::pos2(ax, ay));
        }
        if let Some(i) = self.ac.accept.take() {
            if i < self.ac.items.len() {
                let ins = self.ac.items[i].insert.clone();
                let chars: Vec<char> = sql.chars().collect();
                let mut e = self.ac.start.min(chars.len());
                while e < chars.len() && is_word(chars[e]) {
                    e += 1;
                }
                ed.caret = self.ac.start;
                ed.anchor = e;
                ed.replace(&mut sql, &ins, EditKind::None);
                self.focus_editor = true;
                edited = true;
            }
            self.ac.close();
        }

        if edited {
            self.tabs[idx].dirty = true;
        }
        self.caret = ed.caret;
        self.tabs[idx].sql = sql;
        self.tabs[idx].ed = ed;
    }

    /// Cached single-line highlight galley (keyed by the line's text).
    fn hl_line(&mut self, ui: &egui::Ui, text: &str) -> std::sync::Arc<egui::Galley> {
        self.line_cache.get_or(text, || {
            let mut job = highlight_sql(text, CODE_SIZE);
            job.wrap.max_width = f32::INFINITY;
            ui.fonts_mut(|f| f.layout_job(job))
        })
    }

    /// Normal editing keys (returns true if the buffer changed).
    fn editor_input(
        &mut self,
        sql: &mut String,
        ed: &mut EditorState,
        ctx: &egui::Context,
        page_rows: usize,
    ) -> bool {
        use egui::{Key, Modifiers};
        let cmd = Modifiers::COMMAND;
        let mut changed = false;

        if ctx.input_mut(|i| i.consume_key(cmd, Key::A)) {
            ed.select_all();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::Z)) {
            changed |= ed.undo_op(sql);
        }
        if ctx.input_mut(|i| {
            i.consume_key(cmd | Modifiers::SHIFT, Key::Z) || i.consume_key(cmd, Key::Y)
        }) {
            changed |= ed.redo_op(sql);
        }
        // Clipboard: egui-winit turns Ctrl+C/X/V into semantic Copy/Cut/Paste events (the raw
        // Key::C/V/X don't arrive), so handle those — that's also how the OS clipboard text reaches
        // us on paste. (Ctrl+A / Ctrl+Z / Ctrl+Y above DO arrive as key events.)
        let events = ctx.input(|i| i.events.clone());
        for ev in &events {
            match ev {
                egui::Event::Copy => {
                    if ed.has_sel() {
                        ctx.copy_text(ed.selection_text(sql));
                    }
                }
                egui::Event::Cut => {
                    if ed.has_sel() {
                        ctx.copy_text(ed.selection_text(sql));
                        ed.replace(sql, "", EditKind::Delete);
                        changed = true;
                    }
                }
                egui::Event::Paste(t) => {
                    let norm = t.replace("\r\n", "\n").replace('\r', "\n");
                    ed.replace(sql, &norm, EditKind::None);
                    changed = true;
                }
                egui::Event::Text(t) if !t.chars().any(|c| c.is_control()) && !t.is_empty() => {
                    ed.replace(sql, t, EditKind::Type);
                    changed = true;
                }
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    let sh = modifiers.shift;
                    match key {
                        Key::Enter => {
                            let a = ed.caret.min(ed.anchor);
                            let ins = complete::enter_indent(sql, a);
                            ed.replace(sql, &ins, EditKind::None);
                            changed = true;
                        }
                        Key::Tab => {
                            let ins = complete::tab_spaces(sql, ed.caret.min(ed.anchor));
                            ed.replace(sql, &ins, EditKind::None);
                            changed = true;
                        }
                        Key::Backspace => {
                            ed.backspace(sql);
                            changed = true;
                        }
                        Key::Delete => {
                            ed.delete(sql);
                            changed = true;
                        }
                        Key::ArrowLeft if modifiers.command => {
                            let chars: Vec<char> = sql.chars().collect();
                            ed.move_word(&chars, false, sh);
                        }
                        Key::ArrowRight if modifiers.command => {
                            let chars: Vec<char> = sql.chars().collect();
                            ed.move_word(&chars, true, sh);
                        }
                        Key::ArrowLeft => ed.move_h(false, sh),
                        Key::ArrowRight => ed.move_h(true, sh),
                        Key::ArrowUp => ed.move_v(false, sh),
                        Key::ArrowDown => ed.move_v(true, sh),
                        Key::Home if modifiers.command => ed.doc_home(sh),
                        Key::End if modifiers.command => ed.doc_end(sh),
                        Key::Home => ed.home(sql, sh),
                        Key::End => ed.end(sh),
                        Key::PageUp => ed.page(false, page_rows.max(1), sh),
                        Key::PageDown => ed.page(true, page_rows.max(1), sh),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        changed
    }

    /// F6 completion: build on request, track the prefix, drive the popup keys, accept on Enter/Tab.
    /// Returns true if it edited the buffer (accept). Consumes the keys it uses.
    fn editor_completion(
        &mut self,
        sql: &mut String,
        ed: &mut EditorState,
        ctx: &egui::Context,
        tab_id: u64,
    ) -> bool {
        use egui::{Key, Modifiers};
        if self.ac.open && self.ac.tab != tab_id {
            self.ac.close();
        }
        if std::mem::take(&mut self.ac.request) {
            self.ac_build(sql, ed.caret, tab_id);
        }
        if !self.ac.open {
            return false;
        }
        // keep the prefix in sync; close on a context change
        let chars: Vec<char> = sql.chars().collect();
        let mut ws = ed.caret.min(chars.len());
        while ws > 0 && is_word(chars[ws - 1]) {
            ws -= 1;
        }
        if ws != self.ac.start || ed.has_sel() {
            self.ac.close();
            return false;
        }
        let prefix: String = chars[self.ac.start..ed.caret.min(chars.len())].iter().collect();
        self.ac_refilter(&prefix);
        if self.ac.items.is_empty() {
            self.ac.close();
            return false;
        }
        let (up, down, enter, tab, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if up {
            self.ac.sel = self.ac.sel.saturating_sub(1);
        }
        if down {
            self.ac.sel = (self.ac.sel + 1).min(self.ac.items.len().saturating_sub(1));
        }
        if esc {
            self.ac.close();
        }
        if (enter || tab) && self.ac.sel < self.ac.items.len() {
            let ins = self.ac.items[self.ac.sel].insert.clone();
            let mut e = self.ac.start.min(chars.len());
            while e < chars.len() && is_word(chars[e]) {
                e += 1;
            }
            ed.caret = self.ac.start;
            ed.anchor = e;
            ed.replace(sql, &ins, EditKind::None);
            self.ac.close();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(s: &str) -> (EditorState, String) {
        let mut e = EditorState::default();
        let t = s.to_string();
        e.sync(&t);
        (e, t)
    }

    #[test]
    fn index_and_line_col() {
        let (e, _) = st("ab\ncd\n");
        assert_eq!(e.n_lines(), 3);
        assert_eq!(e.line_col(0), (0, 0));
        assert_eq!(e.line_col(3), (1, 0));
        assert_eq!(e.line_col(5), (1, 2));
        assert_eq!(e.line_of(6), 2);
    }

    #[test]
    fn insert_replaces_selection() {
        let (mut e, mut t) = st("abcd");
        e.anchor = 1;
        e.caret = 3; // select "bc"
        e.replace(&mut t, "X", EditKind::Type);
        assert_eq!(t, "aXd");
        assert_eq!(e.caret, 2);
        assert!(!e.has_sel());
    }

    #[test]
    fn backspace_joins_lines() {
        let (mut e, mut t) = st("ab\ncd");
        e.caret = 3; // start of line 2
        e.anchor = 3;
        e.backspace(&mut t);
        assert_eq!(t, "abcd");
        assert_eq!(e.caret, 2);
    }

    #[test]
    fn delete_forward() {
        let (mut e, mut t) = st("abc");
        e.caret = 1;
        e.anchor = 1;
        e.delete(&mut t);
        assert_eq!(t, "ac");
        assert_eq!(e.caret, 1);
    }

    #[test]
    fn vertical_keeps_preferred_column() {
        let (mut e, _t) = st("abcd\nxy\nzzzz");
        e.caret = 4; // end of line 0 (col 4)
        e.anchor = 4;
        e.move_v(true, false); // → line 1, clamped to col 2
        assert_eq!(e.line_col(e.caret), (1, 2));
        e.move_v(true, false); // → line 2, restored to col 4
        assert_eq!(e.line_col(e.caret), (2, 4));
    }

    #[test]
    fn undo_coalesces_typing_run() {
        let (mut e, mut t) = st("");
        e.replace(&mut t, "a", EditKind::Type);
        e.replace(&mut t, "b", EditKind::Type);
        assert_eq!(t, "ab");
        assert!(e.undo_op(&mut t)); // one step undoes the whole run
        assert_eq!(t, "");
        assert!(e.redo_op(&mut t));
        assert_eq!(t, "ab");
    }

    #[test]
    fn word_step_moves_by_word() {
        let c: Vec<char> = "foo bar".chars().collect();
        assert_eq!(word_step(&c, 0, true), 3);
        assert_eq!(word_step(&c, 3, true), 7);
        assert_eq!(word_step(&c, 7, false), 4);
        assert_eq!(word_step(&c, 4, false), 0);
    }

    #[test]
    fn doc_home_end() {
        let (mut e, _t) = st("ab\ncd\nef");
        e.caret = 4;
        e.anchor = 4;
        e.doc_end(false);
        assert_eq!(e.caret, 8);
        e.doc_home(false);
        assert_eq!(e.caret, 0);
    }

    #[test]
    fn home_smart_toggles() {
        let (mut e, t) = st("    select");
        e.caret = 10; // end
        e.anchor = 10;
        e.home(&t, false); // → first non-blank (col 4)
        assert_eq!(e.line_col(e.caret), (0, 4));
        e.home(&t, false); // → column 0
        assert_eq!(e.line_col(e.caret), (0, 0));
    }
}
