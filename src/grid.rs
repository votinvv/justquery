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

/// Grid display model: columns, widths and order (lives with the tab's results).
pub(crate) struct GridModel {
    pub columns: Vec<String>,
    pub widths: Vec<f32>,
    pub col_order: Vec<usize>, // display position → data index
}

impl GridModel {
    /// Create a model: (header, initial width in points).
    #[allow(dead_code)] // part of the API: this build does not construct the model from literals
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
            if from < self.col_order.len() {
                let item = self.col_order.remove(from);
                let to = if to > from { to - 1 } else { to };
                self.col_order.insert(to.min(self.col_order.len()), item);
            }
        }
    }
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
    #[allow(dead_code)] // part of the API: this build does not handle row clicks
    pub clicked_row: Option<usize>,
}

/// Draw the grid: `rows` rows, cells are requested from `row(i)` (a vector of values in data
/// order), `row_err(i)` — whether the row has an error (red bar + red text in `err_col`).
/// `wrap` — line-wrap mode (the caller must pass `row_tops` — cumulative f64 row-top offsets
/// of length `rows+1`). `offset` — scroll (f64 px on both axes), lives in the caller.
#[allow(clippy::too_many_arguments)]
pub(crate) fn result_grid(
    ui: &mut egui::Ui,
    gm: &GridModel,
    rows: usize,
    sel: Option<GridSel>,
    row: &dyn Fn(usize) -> Vec<String>,
    row_err: &dyn Fn(usize) -> bool,
    err_col: Option<usize>,
    wrap: bool,
    row_tops: Option<&[f64]>,
    offset: &mut (f64, f64),
) -> GridOutput {
    let full = ui.max_rect();
    let header_h = 26.0;
    let row_h = BASE_ROW_H;
    let pad = 8.0;
    let num_w = 56.0; // "#" column
    // The result grid shares the editor's font family (JetBrains Mono + the icon-font fallback, so
    // PUA glyphs in cells render correctly), one point smaller than the editor (`GRID_SIZE` = 12):
    // dense data grids read cleaner, and the column-width heuristic is calibrated to 12pt.
    let mono = code_font_regular(GRID_SIZE);
    let ncols = gm.columns.len();
    let order: Vec<usize> = if gm.col_order.len() == ncols {
        gm.col_order.clone()
    } else {
        (0..ncols).collect()
    };
    let dwidths: Vec<f32> = order.iter().map(|&d| gm.widths[d]).collect();
    let cols_w: f32 = dwidths.iter().sum();

    // row geometry (variable when wrapping, fixed otherwise)
    let valid_tops = row_tops.filter(|t| t.len() == rows + 1);
    let row_top = |i: usize| -> f64 {
        valid_tops.map_or(i as f64 * row_h as f64, |t| t[i.min(rows)])
    };
    let row_h_at = |i: usize| -> f32 {
        valid_tops.map_or(row_h, |t| ((t[i + 1] - t[i]).max(1.0)) as f32)
    };
    let rows_h: f64 = valid_tops.map_or(rows as f64 * row_h as f64, |t| t[rows]);
    // row number by y (f64) relative to the top of the data area
    let row_at = |py: f64| -> Option<usize> {
        if py < 0.0 || rows == 0 {
            return None;
        }
        let r = match valid_tops {
            Some(t) => t.partition_point(|&v| v <= py).saturating_sub(1),
            None => (py / row_h as f64).floor() as usize,
        };
        if r < rows {
            Some(r)
        } else {
            None
        }
    };

    // we ALWAYS reserve room for the scrollbars: while streaming the row count grows, and
    // bars appearing/disappearing would jolt the layout
    let bar = vscroll::BAR;
    let data = Rect::from_min_max(
        egui::pos2(full.left(), full.top() + header_h),
        egui::pos2(full.right() - bar, full.bottom() - bar),
    );
    let cols_view_w = (data.width() - num_w).max(0.0);

    // wheel/touchpad over the whole grid area
    let d = vscroll::wheel_delta(ui, full);
    if d != Vec2::ZERO {
        offset.0 -= d.y as f64;
        offset.1 -= d.x as f64;
        ui.ctx().request_repaint();
    }
    offset.0 = offset.0.clamp(0.0, (rows_h - data.height() as f64).max(0.0));
    offset.1 = offset.1.clamp(0.0, (cols_w as f64 - cols_view_w as f64).max(0.0));

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
    ui.painter().rect_filled(full, CornerRadius::same(crate::RADIUS_ISLAND), p().grid_header);

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
    let dragging = ghost.is_some();
    let mut layout = order.clone();
    let mut skip = None;
    if let Some((src, tgt)) = drop {
        if src < layout.len() {
            let item = layout.remove(src);
            let t = (if tgt > src { tgt - 1 } else { tgt }).min(layout.len());
            layout.insert(t, item);
            skip = Some(t);
        }
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

    // --- cell selection + row click ------------------------------------
    let mut new_sel = sel;
    let mut copy = None;
    let mut clicked_row = None;
    if let Some(pp) = resp.interact_pointer_pos() {
        if pp.y >= data.top() && pp.x >= data.left() + num_w {
            if let Some(r) = row_at((pp.y - data.top()) as f64 + offset.0) {
                if let Some(c) = col_at(pp.x) {
                    if resp.drag_started() || resp.clicked() {
                        new_sel = Some(GridSel { ar: r, ac: c, fr: r, fc: c });
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
        if let Some(s) = new_sel {
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
    for i in first..last {
        let rhh = row_h_at(i);
        let y = row_y(i);
        let rect = Rect::from_min_size(
            egui::pos2(data.left(), y),
            Vec2::new(data.width().max(num_w + cols_w), rhh),
        );
        if i % 2 == 1 {
            dp.rect_filled(rect, CornerRadius::ZERO, p().row_alt);
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
            let cell = Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, rhh));
            if !dragging {
                if let Some((r0, r1, c0, c1)) = selr {
                    if i >= r0 && i <= r1 && lidx >= c0 && lidx <= c1 {
                        dp.rect_filled(cell, CornerRadius::ZERO, p().editor_sel);
                    }
                }
            }
            let val = vals.get(d).map_or("", |v| v.as_str());
            let col = if Some(d) == err_col && is_err {
                p().danger
            } else if val == "—" {
                p().text_dim
            } else {
                p().text
            };
            if wrap {
                // cell wrapped to the column width; short text stays on a single line
                let galley =
                    dp.layout(val.to_owned(), mono.clone(), col, (w - pad * 2.0).max(20.0));
                dp.with_clip_rect(cell.intersect(data)).galley(
                    egui::pos2(cell.left() + pad, cell.top() + 3.0),
                    galley,
                    col,
                );
            } else {
                dp.with_clip_rect(cell.intersect(data)).text(
                    egui::pos2(cell.left() + pad, cell.center().y),
                    egui::Align2::LEFT_CENTER,
                    val,
                    mono.clone(),
                    col,
                );
            }
            x += w;
        }
    }

    // border around the selected block
    if !dragging {
        if let Some((r0, r1, c0, c1)) = selr {
            let x0 = colx0 + lwidths.iter().take(c0).sum::<f32>();
            let x1 = colx0 + lwidths.iter().take(c1 + 1).sum::<f32>();
            let y0 = row_y(r0);
            let y1 = row_y(r1) + row_h_at(r1);
            dp.rect_stroke(
                Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1)),
                CornerRadius::ZERO,
                Stroke::new(1.0, p().accent),
                egui::StrokeKind::Inside,
            );
        }
    }

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
    painter.rect_filled(header_rect, CornerRadius::ZERO, p().grid_header);
    painter.vline(data.left() + num_w, header_rect.y_range(), Stroke::new(1.0, p().border));
    let hp = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(data.left() + num_w, hy),
        egui::pos2(data.right(), hy + header_h),
    ));
    let mut x = colx0;
    for (lidx, &d) in layout.iter().enumerate() {
        let w = lwidths[lidx];
        if Some(lidx) == skip {
            hp.vline(x + w, header_rect.y_range(), Stroke::new(1.0, p().border));
            x += w;
            continue;
        }
        let cell = Rect::from_min_size(egui::pos2(x, hy), Vec2::new(w, header_h));
        hp.with_clip_rect(cell.intersect(header_rect)).text(
            egui::pos2(cell.left() + pad, cell.center().y),
            egui::Align2::LEFT_CENTER,
            &gm.columns[d],
            // header = same font/weight as the data; the `grid_header` background tint alone marks it
            mono.clone(),
            p().text,
        );
        hp.vline(x + w, header_rect.y_range(), Stroke::new(1.0, p().border));
        x += w;
    }
    painter.hline(header_rect.x_range(), hy + header_h, Stroke::new(1.0, p().border));

    // pinned "#" column
    let nx = full.left();
    let nclip = Rect::from_min_max(egui::pos2(nx, data.top()), egui::pos2(nx + num_w, data.bottom()));
    let np = painter.with_clip_rect(nclip);
    for i in first..last {
        let rhh = row_h_at(i);
        let y = row_y(i);
        let cell = Rect::from_min_size(egui::pos2(nx, y), Vec2::new(num_w, rhh));
        let bg = if i % 2 == 1 { p().row_alt } else { p().field_bg };
        np.rect_filled(cell, CornerRadius::ZERO, bg);
        if row_err(i) {
            np.rect_filled(
                Rect::from_min_size(cell.left_top(), Vec2::new(2.0, rhh)),
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
    let nhdr = Rect::from_min_size(egui::pos2(nx, full.top()), Vec2::new(num_w, header_h));
    painter.rect_filled(nhdr, CornerRadius::ZERO, p().grid_header);
    // "#" divider — only down to the last data row, not across the whole panel
    painter.vline(nx + num_w, full.top()..=sheet_bottom, Stroke::new(1.0, p().border));
    painter.hline(nhdr.x_range(), full.top() + header_h, Stroke::new(1.0, p().border));

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
            let rhh = row_h_at(i);
            let y = row_y(i);
            let cell = Rect::from_min_size(egui::pos2(gleft, y), Vec2::new(w, rhh));
            if i % 2 == 1 {
                gp.rect_filled(cell, CornerRadius::ZERO, p().row_alt);
            }
            let vals = row(i);
            let val = vals.get(d).map_or("", |v| v.as_str());
            gp.with_clip_rect(cell).text(
                egui::pos2(cell.left() + pad, cell.top() + 3.0),
                egui::Align2::LEFT_TOP,
                val,
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

    // --- scrollbars (our own; registered AFTER the body — they win the hit-test) ----
    let vtrack = Rect::from_min_max(
        egui::pos2(full.right() - bar, full.top() + header_h),
        egui::pos2(full.right(), full.bottom() - bar),
    );
    vscroll::vbar(ui, vtrack, ui.id().with("grid_vbar"), &mut offset.0, rows_h);
    let htrack = Rect::from_min_max(
        egui::pos2(data.left() + num_w, full.bottom() - bar),
        egui::pos2(full.right() - bar, full.bottom()),
    );
    vscroll::hbar(ui, htrack, ui.id().with("grid_hbar"), &mut offset.1, cols_w as f64);

    GridOutput { sel: new_sel, copy, reorder, resize, clicked_row }
}

/// Number of lines after wrapping for monospace text in a column `cols` characters wide
/// (greedy word wrap, as egui does for a monospace font). Cheap — no galley layout.
#[allow(dead_code)] // part of the API: this build does not enable line-wrap mode
pub(crate) fn mono_wrap_lines(text: &str, cols: usize) -> usize {
    let cols = cols.max(1);
    let mut total = 0usize;
    for para in text.split('\n') {
        let mut lines = 1usize;
        let mut col = 0usize; // characters on the current line
        for word in para.split(' ') {
            let wl = word.chars().count();
            let sep = if col == 0 { 0 } else { 1 };
            if col + sep + wl <= cols {
                col += sep + wl;
            } else {
                // wrap to a new line
                if col != 0 {
                    lines += 1;
                }
                if wl <= cols {
                    col = wl;
                } else {
                    // word longer than a line — break it by characters
                    let extra = (wl - 1) / cols;
                    lines += extra;
                    col = wl - extra * cols;
                }
            }
        }
        total += lines;
    }
    total.max(1)
}

#[cfg(test)]
mod tests {
    use super::mono_wrap_lines;

    #[test]
    fn wrap_lines_basic() {
        assert_eq!(mono_wrap_lines("short", 80), 1);
        assert_eq!(mono_wrap_lines("", 80), 1);
        assert_eq!(mono_wrap_lines("aaaaa aaaaa aaaaa aaaaa", 11), 2);
        assert_eq!(mono_wrap_lines("aaaaaaaaaa", 4), 3); // 4+4+2
        assert_eq!(mono_wrap_lines("a\nb\nc", 80), 3);
    }
}
