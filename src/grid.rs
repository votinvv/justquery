//!
//! Virtualized result grid: pinned "#" column, sticky header, zebra striping,
//! cell selection + TSV copy, and column reorder/resize by mouse. Scrolling is OUR OWN,
//! in local coordinates: the position is f64 pixels, visible rows are drawn relative to the
//! screen, there is no giant canvas (see [`crate::vscroll`] — on hundreds of thousands of rows
//! egui's f32 canvas jittered on the representation quantum).

use crate::theme::{code_font_regular, p, GRID_SIZE};
use crate::vscroll;
use eframe::egui;
use egui::{CornerRadius, Rect, Stroke, Vec2};

/// Base row height (without wrapping).
pub(crate) const BASE_ROW_H: f32 = 22.0;

/// Column-header band height (4px taller than a data row). Exported so the result panel's
/// auto-size math (which targets an exact whole-row body height) stays in lock-step with the grid.
pub(crate) const HEADER_H: f32 = 26.0;

/// Whole data rows that fit in a grid of TOTAL height `total_h` (header band + data rows + the
/// reserved bottom scroll strip of one [`vscroll::BAR`]). THE single source of truth for the
/// first-page fetch count: [`result_grid`] returns it, and the result panel's auto-size target
/// ([`panel_height_for`]) plus its pre-load `cap` are defined against it — so the one-BAR reservation
/// lives in exactly one place and the three sites can't drift in sign or presence.
pub(crate) fn rows_fit(total_h: f32) -> usize {
    ((total_h - HEADER_H - vscroll::BAR) / BASE_ROW_H).floor().max(0.0) as usize
}

/// Inverse of [`rows_fit`]: the total grid height that fits exactly `n` whole data rows (header band +
/// `n` rows + the reserved bottom scroll strip).
pub(crate) fn panel_height_for(n: usize) -> f32 {
    HEADER_H + n as f32 * BASE_ROW_H + vscroll::BAR
}

/// Grid display model: columns, widths and order (lives with the tab's results).
pub(crate) struct GridModel {
    pub columns: Vec<String>,
    pub widths: Vec<f32>,
    pub col_order: Vec<usize>, // display position → data index
}

impl GridModel {
    /// Create a model: (header, initial width in points).
    pub fn new(cols: &[(&str, f32)]) -> Self {
        Self {
            columns: cols.iter().map(|(c, _)| (*c).to_owned()).collect(),
            widths: cols.iter().map(|(_, w)| *w).collect(),
            col_order: (0..cols.len()).collect(),
        }
    }

    /// Apply the grid output (reorder/resize) to the model.
    pub fn apply(&mut self, out: &GridOutput) {
        if let Some((d, w)) = out.resize {
            if d < self.widths.len() {
                self.widths[d] = w;
            }
        }
        if let Some((from, to)) = out.reorder {
            let n = self.columns.len();
            if self.col_order.len() != n {
                self.col_order = (0..n).collect();
            }
            move_col(&mut self.col_order, from, to);
        }
    }
}

/// Move the column at display position `from` to `to` within `order` (remove + decrement target when
/// `to > from` + clamped insert). No-op if `from` is out of range. Returns the final insertion index
/// (the live drop preview's `skip`). One source of truth for the committed reorder AND the in-frame
/// ghost preview — they must stay in lock-step or the preview diverges from the result.
fn move_col(order: &mut Vec<usize>, from: usize, to: usize) -> Option<usize> {
    if from >= order.len() {
        return None;
    }
    let item = order.remove(from);
    let to = (if to > from { to - 1 } else { to }).min(order.len());
    order.insert(to, item);
    Some(to)
}

/// Rectangular cell selection (anchor + focus). Columns are DISPLAY POSITIONS.
#[derive(Clone, Copy)]
pub(crate) struct GridSel {
    ar: usize,
    ac: usize,
    fr: usize,
    fc: usize,
}

/// What the grid frame asks the caller to apply.
pub(crate) struct GridOutput {
    pub sel: Option<GridSel>,
    /// TSV of the selected cells — if Ctrl+C was pressed this frame.
    pub copy: Option<String>,
    /// Completed column drag: (source position, insertion position).
    pub reorder: Option<(usize, usize)>,
    /// Live resize: (data index, new width).
    pub resize: Option<(usize, f32)>,
    /// Click on a data row (for jumping to the document line).
    pub clicked_row: Option<usize>,
    /// Whole data rows that fit in the visible data area (floor) — for sizing the lazy first page so
    /// it fills the panel exactly without a vertical scrollbar.
    pub rows_fit: usize,
    /// A header was clicked to sort: (data column index, additive — Ctrl/Cmd held for multi-sort).
    pub sort_click: Option<(usize, bool)>,
    /// The "#" gutter was clicked to select a whole row: (visible row, ctrl held, alt held).
    pub row_click: Option<(usize, bool, bool)>,
    /// A fresh cell interaction happened in the data area → the caller should drop any row selection.
    pub clear_rows: bool,
}

/// Collapse control whitespace (tab / newline / CR) to a single space for single-line cell display,
/// so an embedded tab/newline can't inflate the row's width or break its layout. Borrows when the
/// value is already clean (the common case). The stored value (used for copy) is left untouched.
fn cell_display(s: &str) -> std::borrow::Cow<'_, str> {
    if s.bytes().any(|b| b == b'\t' || b == b'\n' || b == b'\r') {
        std::borrow::Cow::Owned(
            s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// Draw the grid: `rows` rows, cells are requested from `row(i)` (the values in data order),
/// `row_err(i)` — whether the row has an error (red bar + red text in `err_col`).
/// `row` returns a [`Cow`]: the data path borrows the stored row (no per-frame allocation on the
/// draw hot path), the probe path synthesizes and owns it.
/// `offset` — scroll (f64 px on both axes), lives in the caller. `fade` — the
/// disappearing-overlay scrollbar fade state, also caller-owned (per result-tab).
#[allow(clippy::too_many_arguments)]
pub(crate) fn result_grid<'r>(
    ui: &mut egui::Ui,
    gm: &GridModel,
    rows: usize,
    sel: Option<GridSel>,
    row: &dyn Fn(usize) -> std::borrow::Cow<'r, [String]>,
    row_err: &dyn Fn(usize) -> bool,
    err_col: Option<usize>,
    // active sort keys in priority order: (data column index, descending) — drives the header marker
    sort: &[(usize, bool)],
    // selected whole rows (visible indices) — painted highlighted; the "#" gutter drives this
    row_sel: &std::collections::BTreeSet<usize>,
    offset: &mut (f64, f64),
    fade: &mut vscroll::Fade,
) -> GridOutput {
    let full = ui.max_rect();
    let header_h = HEADER_H;
    let row_h = BASE_ROW_H;
    let pad = crate::theme::TEXT_INSET; // cell text inset — the shared knob (theme.rs)
    // The result grid shares the editor's font family (JetBrains Mono + the icon-font fallback, so
    // PUA glyphs in cells render correctly), one point smaller than the editor (`GRID_SIZE` = 12):
    // dense data grids read cleaner, and the column-width heuristic is calibrated to 12pt.
    let mono = code_font_regular(GRID_SIZE);
    // "#" column: width tracks the digit count of the largest row number, exactly like the editor's
    // gutter (6 px left + 8 px right padding), so it grows as the lazy stream scrolls in more rows.
    let glyph_w = ui.ctx().fonts_mut(|f| f.glyph_width(&mono, '0'));
    // digit count of the largest row number, without the per-frame String allocation
    let num_digits = (rows.max(1).ilog10() as usize + 1).max(3);
    let num_w =
        (glyph_w * num_digits as f32).ceil() + crate::theme::GUTTER_PAD_L + crate::theme::GUTTER_PAD_R;
    let ncols = gm.columns.len();
    let order: Vec<usize> = if gm.col_order.len() == ncols {
        gm.col_order.clone()
    } else {
        (0..ncols).collect()
    };
    let dwidths: Vec<f32> = order.iter().map(|&d| gm.widths[d]).collect();
    let cols_w: f32 = dwidths.iter().sum();

    // row geometry: fixed-height rows
    let row_top = |i: usize| -> f64 { i as f64 * row_h as f64 };
    let rows_h: f64 = rows as f64 * row_h as f64;
    // row number by y (f64) relative to the top of the data area
    let row_at = |py: f64| -> Option<usize> {
        if py < 0.0 || rows == 0 {
            return None;
        }
        let r = (py / row_h as f64).floor() as usize;
        if r < rows {
            Some(r)
        } else {
            None
        }
    };

    // Disappearing overlay scrollbars: the data fills the WHOLE island (no permanent reserve) and the
    // handles float on top of it. `need_*` (single pass — an overlay handle doesn't shrink the show
    // decision) says whether each axis scrolls. Only when BOTH axes scroll do the handles overlap each
    // other's far edge, so we shorten each viewport by one BAR (`vview`/`hview`): that extends the scroll
    // range just enough that the last row / column can slide out from under the perpendicular handle at
    // the very end — the clearance strip appears only there, never as a permanent gutter. (This same
    // one-bar shortening keeps the two tracks from meeting at the shared corner.)
    let bar = vscroll::BAR;
    let data = Rect::from_min_max(
        egui::pos2(full.left(), full.top() + header_h),
        egui::pos2(full.right(), full.bottom()),
    );
    let cols_view_w = (data.width() - num_w).max(0.0);
    let need_v = rows_h > data.height() as f64;
    let need_h = cols_w as f64 > cols_view_w as f64;
    let clear = if need_v && need_h { bar } else { 0.0 };
    let vview = (data.height() - clear).max(0.0);
    let hview = (cols_view_w - clear).max(0.0);
    // first-page fetch count: whole rows the data area shows, with the bottom scroll strip (one BAR)
    // reserved so a freshly-run result leaves the horizontal bar's home clear below the last row — it
    // lies UNDER the last row, never over it. Reserved unconditionally (even with no h-overflow) so the
    // default panel height is static regardless of column count. `rows_fit(full.height())` is the shared
    // definition the panel's auto-size + pre-load `cap` are named against.
    let rows_fit = rows_fit(full.height());

    // wheel/touchpad over the whole grid area
    let d = vscroll::wheel_delta(ui, full);
    if d != Vec2::ZERO {
        offset.0 -= d.y as f64;
        offset.1 -= d.x as f64;
        ui.ctx().request_repaint();
    }
    offset.0 = offset.0.clamp(0.0, (rows_h - vview as f64).max(0.0));
    offset.1 = offset.1.clamp(0.0, (cols_w as f64 - hview as f64).max(0.0));

    // screen coordinates as local numbers; y snaps to physical pixels
    // (row heights are fractional → without snapping a "wave" appears during slow scrolling)
    let colx0 = (data.left() as f64 + num_w as f64 - offset.1) as f32;
    let ppp = ui.ctx().pixels_per_point() as f64;
    let data_top64 = data.top() as f64;
    let off_y = offset.0;
    let row_y = move |i: usize| -> f32 {
        let y = data_top64 + row_top(i) - off_y;
        ((y * ppp).round() / ppp) as f32
    };

    ui.set_clip_rect(full);
    // Base sheet uses the editor's SURFACE tone (light), not the darker CHROME header tint: the
    // header band repaints its own grid_header on top, while the area past the last column and below
    // the last row stays light, matching the code editor.
    ui.painter().rect_filled(full, CornerRadius::same(crate::RADIUS_ISLAND), p().field_bg);

    // body: interaction over the data area (the bars are registered later — they win the hit)
    let resp = ui.interact(data, ui.id().with("grid_body"), egui::Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(full);

    let col_left = |disp: usize| -> f32 { colx0 + dwidths.iter().take(disp).sum::<f32>() };
    let col_at = |px: f32| -> Option<usize> {
        let mut cx = colx0;
        for (disp, w) in dwidths.iter().enumerate() {
            if px >= cx && px < cx + *w {
                return Some(disp);
            }
            cx += *w;
        }
        None
    };
    let insert_at = |px: f32| -> usize {
        let mut cx = colx0;
        for (disp, w) in dwidths.iter().enumerate() {
            if px < cx + *w * 0.5 {
                return disp;
            }
            cx += *w;
        }
        dwidths.len()
    };

    // --- header drag → live reflow + floating ghost ------------------------
    let mut reorder = None;
    let mut ghost: Option<(usize, f32)> = None;
    let mut drop: Option<(usize, usize)> = None;
    let hdr_drag = ui.interact(
        Rect::from_min_max(
            egui::pos2(data.left() + num_w, full.top()),
            egui::pos2(data.right(), full.top() + header_h),
        ),
        ui.id().with("grid_header"),
        egui::Sense::click_and_drag(),
    );
    let drag_id = ui.id().with("grid_col_drag");
    if hdr_drag.drag_started() {
        if let Some(pp) = hdr_drag.interact_pointer_pos() {
            if let Some(src) = col_at(pp.x) {
                ui.memory_mut(|m| m.data.insert_temp(drag_id, (src, pp.x - col_left(src))));
            }
        }
    }
    let drag: Option<(usize, f32)> = ui.memory(|m| m.data.get_temp(drag_id));
    if hdr_drag.dragged() {
        if let (Some((src, dx)), Some(pp)) = (drag, hdr_drag.interact_pointer_pos()) {
            ghost = Some((src, pp.x - dx));
            drop = Some((src, insert_at(pp.x)));
        }
    }
    if hdr_drag.drag_stopped() {
        if let (Some((src, _)), Some(pp)) = (drag, hdr_drag.interact_pointer_pos()) {
            reorder = Some((src, insert_at(pp.x)));
        }
    }
    // plain click on a header (no drag) → sort that column; Ctrl/Cmd makes it additive (multi-sort)
    let mut sort_click: Option<(usize, bool)> = None;
    if hdr_drag.clicked() {
        if let Some(disp) = hdr_drag.interact_pointer_pos().and_then(|pp| col_at(pp.x)) {
            let additive = ui.input(|i| i.modifiers.command);
            sort_click = Some((order[disp], additive));
        }
    }
    let dragging = ghost.is_some();
    let mut layout = order.clone();
    let mut skip = None;
    if let Some((src, tgt)) = drop {
        skip = move_col(&mut layout, src, tgt);
    }
    let lwidths: Vec<f32> = layout.iter().map(|&d| gm.widths[d]).collect();

    // --- column resize (handles on the right edge of the headers) -----------------
    let mut resize: Option<(usize, f32)> = None;
    if !dragging {
        let mut x = colx0;
        for (lidx, &d) in layout.iter().enumerate() {
            x += lwidths[lidx];
            let handle = Rect::from_min_max(
                egui::pos2(x - 3.0, full.top()),
                egui::pos2(x + 3.0, full.top() + header_h),
            );
            let rh = ui.interact(
                handle,
                ui.id().with(("grid_colsize", d)),
                egui::Sense::drag(),
            );
            if rh.hovered() || rh.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            }
            if rh.dragged() {
                resize = Some((d, (gm.widths[d] + rh.drag_delta().x).clamp(40.0, 2000.0)));
            }
        }
    }

    // --- cell selection (data area) + whole-row selection ("#" gutter) -----------------
    let mut new_sel = sel;
    let mut copy = None;
    let mut clicked_row = None;
    let mut row_click: Option<(usize, bool, bool)> = None;
    let mut clear_rows = false;
    if let Some(pp) = resp.interact_pointer_pos() {
        if pp.y >= data.top() {
            let r = row_at((pp.y - data.top()) as f64 + offset.0);
            if pp.x >= data.left() + num_w {
                // data area → rectangular cell selection
                if let (Some(r), Some(c)) = (r, col_at(pp.x)) {
                    if resp.drag_started() || resp.clicked() {
                        new_sel = Some(GridSel { ar: r, ac: c, fr: r, fc: c });
                        clear_rows = true; // a fresh cell interaction supersedes any row selection
                    }
                    if resp.clicked() {
                        clicked_row = Some(r);
                    } else if resp.dragged() {
                        new_sel = new_sel.map(|mut s| {
                            s.fr = r;
                            s.fc = c;
                            s
                        });
                    }
                }
            } else if resp.clicked() {
                // "#" gutter → select the whole row; Ctrl toggles, Alt extends a range
                if let Some(r) = r {
                    let (ctrl, alt) = ui.input(|i| (i.modifiers.command, i.modifiers.alt));
                    row_click = Some((r, ctrl, alt));
                }
            }
        }
    }
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    let want_copy = resp.has_focus()
        && ui.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.modifiers.command && i.key_pressed(egui::Key::C))
        });
    if want_copy {
        if !row_sel.is_empty() {
            // whole selected rows (every column, display order), ascending by visible index
            let mut out = String::new();
            for &r in row_sel.iter().filter(|&&r| r < rows) {
                let vals = row(r);
                for (i, &d) in order.iter().enumerate() {
                    if i > 0 {
                        out.push('\t');
                    }
                    out.push_str(vals.get(d).map_or("", |v| v.as_str()));
                }
                out.push('\n');
            }
            copy = Some(out);
        } else if let Some(s) = new_sel {
            let r0 = s.ar.min(s.fr);
            let r1 = s.ar.max(s.fr).min(rows.saturating_sub(1));
            let c0 = s.ac.min(s.fc);
            let c1 = s.ac.max(s.fc).min(order.len().saturating_sub(1));
            let mut out = String::new();
            for r in r0..=r1 {
                let vals = row(r);
                for (i, &d) in order[c0..=c1].iter().enumerate() {
                    if i > 0 {
                        out.push('\t');
                    }
                    out.push_str(vals.get(d).map_or("", |v| v.as_str()));
                }
                out.push('\n');
            }
            copy = Some(out);
        }
    }
    let selr =
        new_sel.map(|s| (s.ar.min(s.fr), s.ar.max(s.fr), s.ac.min(s.fc), s.ac.max(s.fc)));

    // white sheet under the rows (down to the end of the data or to the bottom of the area)
    let sheet_bottom = (data.top() + (rows_h - offset.0).max(0.0) as f32).min(data.bottom());
    painter.rect_filled(
        Rect::from_min_max(data.left_top(), egui::pos2(data.right(), sheet_bottom)),
        CornerRadius::ZERO,
        p().field_bg,
    );

    let dp = painter.with_clip_rect(data);
    let first = row_at(offset.0).unwrap_or(0);
    let last = row_at(offset.0 + data.height() as f64).map_or(rows, |r| (r + 1).min(rows));
    // right edge of the actual table (last column's right edge), clamped to the visible area: zebra,
    // row-selection AND the body row rules all stop here — never at the panel edge — so empty space to
    // the right of the table stays blank (a 1-column `select 1` doesn't drag a rule across the panel)
    let data_right = (colx0 + cols_w).min(data.right());
    for i in first..last {
        let y = row_y(i);
        // zebra / whole-row selection fill the row band from the left edge to the end of the table
        let rect = Rect::from_min_max(egui::pos2(data.left(), y), egui::pos2(data_right, y + row_h));
        if i % 2 == 1 {
            dp.rect_filled(rect, CornerRadius::ZERO, p().row_alt);
        }
        if row_sel.contains(&i) {
            dp.rect_filled(rect, CornerRadius::ZERO, p().editor_sel); // whole-row selection
        }
        let vals = row(i);
        let is_err = row_err(i);
        let mut x = colx0;
        for (lidx, &d) in layout.iter().enumerate() {
            let w = lwidths[lidx];
            if Some(lidx) == skip {
                x += w;
                continue;
            }
            let cell = Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, row_h));
            if !dragging {
                if let Some((r0, r1, c0, c1)) = selr {
                    if i >= r0 && i <= r1 && lidx >= c0 && lidx <= c1 {
                        dp.rect_filled(cell, CornerRadius::ZERO, p().editor_sel);
                    }
                }
            }
            let raw = vals.get(d).map_or("", |v| v.as_str());
            let col = if Some(d) == err_col && is_err {
                p().danger
            } else if raw == "—" {
                p().text_dim
            } else {
                p().text
            };
            let val = cell_display(raw); // collapse tab/newline so a cell stays single-line
            dp.with_clip_rect(cell.intersect(data)).text(
                egui::pos2(cell.left() + pad, cell.center().y),
                egui::Align2::LEFT_CENTER,
                val.as_ref(),
                mono.clone(),
                col,
            );
            x += w;
        }
    }

    // (no outline around the selected block — the accent cell fill alone marks the selection; the
    // clipped 1px border read inconsistently, showing on 1–3 sides near the data edges)

    // the "pulled-out" column during drag — an empty slot
    if let Some(g) = skip {
        let gx = colx0 + lwidths.iter().take(g).sum::<f32>();
        let gap = Rect::from_min_max(
            egui::pos2(gx, data.top()),
            egui::pos2(gx + lwidths[g], sheet_bottom),
        );
        dp.rect_filled(gap, CornerRadius::ZERO, p().grid_header);
    }

    // sticky header
    let hy = full.top();
    let header_rect =
        Rect::from_min_size(egui::pos2(full.left(), hy), Vec2::new(full.width(), header_h));
    // The header fills the FULL island width (its top corners rounded to the island radius) — the
    // chrome owns the whole top of the island; the overlay v-bar rides over the data below it, never
    // over the header.
    painter.rect_filled(
        Rect::from_min_max(egui::pos2(full.left(), hy), egui::pos2(full.right(), hy + header_h)),
        CornerRadius { nw: crate::RADIUS_ISLAND, ne: crate::RADIUS_ISLAND, sw: 0, se: 0 },
        p().grid_header,
    );
    painter.vline(
        painter.round_to_pixel_center(data.left() + num_w),
        header_rect.y_range(),
        Stroke::new(crate::widgets::hairline(painter.ctx()), p().border),
    );
    // Column dividers (and the header text/markers, which set their own clip) stay within the content
    // area — they never bleed into the reserved scrollbar band. The last column's divider reaching the
    // header is preserved: it is drawn here up to data.right(), and when a vertical bar is present the
    // content↔bar separator (drawn later, full height incl. the header) carries that closing line.
    let hp = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(data.left() + num_w, hy),
        egui::pos2(data.right(), hy + header_h),
    ));
    let mut x = colx0;
    for (lidx, &d) in layout.iter().enumerate() {
        let w = lwidths[lidx];
        if Some(lidx) == skip {
            hp.vline(
                hp.round_to_pixel_center(x + w),
                header_rect.y_range(),
                Stroke::new(crate::widgets::hairline(hp.ctx()), p().border),
            );
            x += w;
            continue;
        }
        let cell = Rect::from_min_size(egui::pos2(x, hy), Vec2::new(w, header_h));
        // sort marker: arrow ↑/↓ for this column, plus its 1-based priority when sorting on several
        let sort_mark = sort.iter().position(|&(c, _)| c == d).map(|pos| {
            let arrow = if sort[pos].1 { "↓" } else { "↑" };
            if sort.len() > 1 { format!("{arrow}{}", pos + 1) } else { arrow.to_owned() }
        });
        // leave room on the right for the marker so it doesn't overlap a long header name
        let name_right = match &sort_mark {
            Some(m) => (cell.right() - pad - m.chars().count() as f32 * glyph_w - 4.0).max(cell.left()),
            None => cell.right() - pad,
        };
        hp.with_clip_rect(
            Rect::from_min_max(cell.left_top(), egui::pos2(name_right, cell.bottom()))
                .intersect(header_rect),
        )
        .text(
            egui::pos2(cell.left() + pad, cell.center().y),
            egui::Align2::LEFT_CENTER,
            &gm.columns[d],
            // header = same font/weight as the data; the `grid_header` background tint alone marks it
            mono.clone(),
            p().text,
        );
        if let Some(m) = sort_mark {
            hp.with_clip_rect(cell.intersect(header_rect)).text(
                egui::pos2(cell.right() - pad, cell.center().y),
                egui::Align2::RIGHT_CENTER,
                &m,
                mono.clone(),
                p().accent,
            );
        }
        hp.vline(
            hp.round_to_pixel_center(x + w),
            header_rect.y_range(),
            Stroke::new(crate::widgets::hairline(hp.ctx()), p().border),
        );
        x += w;
    }
    painter.hline(
        full.left()..=full.right(),
        painter.round_to_pixel_center(hy + header_h),
        Stroke::new(crate::widgets::hairline(painter.ctx()), p().border),
    );

    // pinned "#" column
    let nx = full.left();
    // The "#" gutter fills the FULL island height (its bottom-left corner rounded to the island radius)
    // — the chrome owns the whole left of the island; the overlay h-bar rides over the data to its
    // right, never over the "#" gutter.
    painter
        .with_clip_rect(Rect::from_min_max(egui::pos2(nx, data.top()), egui::pos2(nx + num_w, full.bottom())))
        .rect_filled(
            Rect::from_min_max(egui::pos2(nx, data.top()), egui::pos2(nx + num_w, full.bottom())),
            CornerRadius { nw: 0, ne: 0, sw: crate::RADIUS_ISLAND, se: 0 },
            p().gutter,
        );
    // row tints + numbers clip to the DATA area only, so a partially-scrolled last row's number isn't
    // painted down in the bottom-scrollbar band (the flat fill above already covers it).
    let nclip = Rect::from_min_max(egui::pos2(nx, data.top()), egui::pos2(nx + num_w, data.bottom()));
    let np = painter.with_clip_rect(nclip);
    for i in first..last {
        let y = row_y(i);
        let cell = Rect::from_min_size(egui::pos2(nx, y), Vec2::new(num_w, row_h));
        // flat single tone like the editor's gutter (no zebra here); selected rows tint accent-soft
        let bg = if row_sel.contains(&i) { p().editor_sel } else { p().gutter };
        np.rect_filled(cell, CornerRadius::ZERO, bg);
        if row_err(i) {
            np.rect_filled(
                Rect::from_min_size(cell.left_top(), Vec2::new(2.0, row_h)),
                CornerRadius::ZERO,
                p().danger,
            );
        }
        // row number — top-aligned (like the other columns)
        np.text(
            egui::pos2(cell.right() - pad, cell.top() + 3.0),
            egui::Align2::RIGHT_TOP,
            (i + 1).to_string(),
            mono.clone(),
            p().text_dim,
        );
    }
    // the "#" header cap is already covered by the full-width header fill above (chrome), so no
    // separate fill here; `nhdr` is kept only to anchor the header underline below.
    let nhdr = Rect::from_min_size(egui::pos2(nx, full.top()), Vec2::new(num_w, header_h));
    // "#" divider runs the full island height (the "#" column owns the whole left edge).
    painter.vline(
        painter.round_to_pixel_center(nx + num_w),
        full.top()..=full.bottom(),
        Stroke::new(crate::widgets::hairline(painter.ctx()), p().border),
    );
    painter.hline(
        nhdr.x_range(),
        painter.round_to_pixel_center(full.top() + header_h),
        Stroke::new(crate::widgets::hairline(painter.ctx()), p().border),
    );

    // --- thin 1px grid over the data body, painted AFTER the zebra/cells AND the "#" gutter so
    // nothing repaints over it. Horizontal row rules run from the left edge (across the "#" gutter)
    // to the END OF THE TABLE (`data_right`), not the panel edge. Vertical column rules sit at each
    // column's right edge (the last one closes the table) and run down to the last data row. Same
    // hairline + pixel-snap as the header separators → crisp at any DPI.
    let line = Stroke::new(crate::widgets::hairline(painter.ctx()), p().border);
    // the clip extends to the island bottom so the rule under a panel-filling last row (which lands
    // exactly on data.bottom()) isn't shaved off by the clip edge; each rule's y is clamped to it.
    let hgrid = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(data.left(), data.top()),
        egui::pos2(data.right(), full.bottom()),
    ));
    for i in first..last {
        let y = (row_y(i) + row_h).min(data.bottom());
        hgrid.hline(data.left()..=data_right, hgrid.round_to_pixel_center(y), line);
    }
    // vertical column rules sit at each column's right edge; they stay inside the content area (clip to
    // data.right()) so nothing bleeds into a reserved scrollbar band. The content's right/bottom border
    // is the separator drawn just below.
    let vgrid = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(data.left() + num_w, data.top()),
        egui::pos2(data.right(), data.bottom()),
    ));
    let mut gx = colx0;
    for w in &lwidths {
        gx += *w;
        vgrid.vline(vgrid.round_to_pixel_center(gx), data.top()..=sheet_bottom, line);
    }
    // (no content↔band separators: the overlay bars reserve no permanent strip. The chrome L-frame's own
    // hairlines (the header underline and the "#" divider) frame the top and left, and the island border
    // (widgets::island) draws the outer edges.)

    // floating ghost of the dragged column
    if let Some((src, gleft)) = ghost {
        let w = dwidths[src];
        let d = order[src];
        let gp = painter.with_clip_rect(full);
        let gh = Rect::from_min_max(
            egui::pos2(gleft, full.top()),
            egui::pos2(gleft + w, sheet_bottom),
        );
        gp.rect_filled(gh.translate(Vec2::new(2.0, 0.0)), CornerRadius::ZERO, p().shadow);
        let gh_hdr =
            Rect::from_min_size(egui::pos2(gleft, full.top()), Vec2::new(w, header_h));
        gp.rect_filled(gh_hdr, CornerRadius::ZERO, p().grid_header);
        gp.rect_filled(
            Rect::from_min_max(egui::pos2(gleft, full.top() + header_h), gh.right_bottom()),
            CornerRadius::ZERO,
            p().field_bg,
        );
        for i in first..last {
            let y = row_y(i);
            let cell = Rect::from_min_size(egui::pos2(gleft, y), Vec2::new(w, row_h));
            if i % 2 == 1 {
                gp.rect_filled(cell, CornerRadius::ZERO, p().row_alt);
            }
            let vals = row(i);
            let val = cell_display(vals.get(d).map_or("", |v| v.as_str()));
            gp.with_clip_rect(cell).text(
                egui::pos2(cell.left() + pad, cell.top() + 3.0),
                egui::Align2::LEFT_TOP,
                val.as_ref(),
                mono.clone(),
                p().text,
            );
        }
        gp.with_clip_rect(gh_hdr).text(
            egui::pos2(gh_hdr.left() + pad, gh_hdr.center().y),
            egui::Align2::LEFT_CENTER,
            &gm.columns[d],
            mono.clone(),
            p().accent,
        );
        crate::widgets::crisp_border_r(&gp, gh, p().accent, 0);
    }

    // --- disappearing overlay scrollbars (our own; registered AFTER the body — they win the hit-test).
    // The handles are semi-transparent overlays ON the data — they reserve NO permanent space; the fade
    // (shown on activity, easing out after an idle) is `vscroll::Fade`, whose state lives in the caller.
    // Only when BOTH axes scroll is each viewport shortened by one BAR (`vview`/`hview`), extending the
    // scroll range just enough that the last row / column slides clear of the perpendicular handle at the
    // very end — a clearance strip only there. Tracks are confined to the DATA region (below the header,
    // right of the "#" gutter) so a handle never rides onto the chrome, each stopping one bar short of the
    // shared corner. Below a whisker of opacity we skip drawing/interaction so a hidden bar can't eat a click.
    let handle_a = fade.alpha(ui, full, d != Vec2::ZERO);
    if handle_a > 0.003 {
        if need_v {
            let vtrack = Rect::from_min_max(
                egui::pos2(full.right() - bar, data.top()),
                egui::pos2(full.right(), data.top() + vview),
            );
            vscroll::vbar(ui, vtrack, ui.id().with("grid_vbar"), &mut offset.0, rows_h, vview as f64, handle_a);
        }
        if need_h {
            let hx0 = data.left() + num_w;
            let htrack = Rect::from_min_max(
                egui::pos2(hx0, full.bottom() - bar),
                egui::pos2(hx0 + hview, full.bottom()),
            );
            vscroll::hbar(ui, htrack, ui.id().with("grid_hbar"), &mut offset.1, cols_w as f64, hview as f64, handle_a);
        }
    }

    GridOutput {
        sel: new_sel,
        copy,
        reorder,
        resize,
        clicked_row,
        rows_fit,
        sort_click,
        row_click,
        clear_rows,
    }
}

