//! The virtualized result grid: the `ResultSet` data model and the hand-rolled, O(visible) data
//! grid that draws it (pinned "#" column + sticky header, mouse cell-selection + TSV copy, and
//! mouse column reorder/resize). Used for both query result sets and the Messages execution log.

use crate::widgets::{crisp_border, style_scrollbar};
use crate::{ACCENT, BORDER, GRID_HEADER, ROWALT, TEXT, TEXTDIM};
use eframe::egui;
use egui::{Color32, CornerRadius, Stroke, Vec2};

/// One result set from a query: column headers + every fetched row (each cell already formatted
/// to a string by the simple-query protocol), plus a precomputed display width per column.
pub(crate) struct ResultSet {
    pub columns: Vec<String>,
    pub widths: Vec<f32>,
    pub rows: Vec<Vec<String>>,
    pub visible: usize, // rows revealed so far (incremental fetch) — lives with the result set
    pub loading: bool,  // a "fetch all" reveal is in progress for this result set
    pub sql: String,    // the statement that produced this result (for per-result Refresh)
    pub col_order: Vec<usize>, // display position → data column index (mouse-reorderable headers)
}

impl ResultSet {
    pub(crate) fn new(columns: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        // width = widest of (header, first 200 values) in chars → points (clamped)
        let mut widths: Vec<f32> = columns.iter().map(|c| c.chars().count() as f32).collect();
        for row in rows.iter().take(200) {
            for (i, v) in row.iter().enumerate() {
                if let Some(w) = widths.get_mut(i) {
                    *w = w.max(v.chars().count() as f32);
                }
            }
        }
        for w in &mut widths {
            *w = (*w * 7.0 + 18.0).clamp(54.0, 380.0);
        }
        let visible = rows.len().min(100); // first page
        let col_order = (0..columns.len()).collect();
        Self { columns, widths, rows, visible, loading: false, sql: String::new(), col_order }
    }
}

/// A rectangular cell selection in the result grid (anchor + focus cell). Column indices are
/// DISPLAY positions (the visible left-to-right order, which the user can reorder by dragging
/// headers); copy maps them back to data columns via `ResultSet::col_order`.
#[derive(Clone, Copy)]
pub(crate) struct GridSel {
    ar: usize,
    ac: usize,
    fr: usize,
    fc: usize,
}

/// Hand-rolled, virtualized data grid for one result set. The scroll area spans the whole
/// island, so the vertical scrollbar runs the full height; the header is pinned to the top of
/// the viewport (fixed vertically, scrolls horizontally in sync with the body). A synthetic
/// "#" row-number is the first column. Returns `(new selection, copied TSV, reorder, resize)`
/// for the caller to apply to the real result set.
pub(crate) fn result_grid(
    ui: &mut egui::Ui,
    rs: &ResultSet,
    rows: usize,
    sel: Option<GridSel>,
) -> (Option<GridSel>, Option<String>, Option<(usize, usize)>, Option<(usize, f32)>) {
    let full = ui.max_rect();
    let header_h = 26.0;
    let row_h = 22.0;
    let pad = 8.0;
    let num_w = 56.0; // "#" row-number column
    let mono = egui::FontId::monospace(12.0);
    let ncols = rs.columns.len();
    // display position → data column index (mouse-reorderable headers); fall back to identity
    let order: Vec<usize> = if rs.col_order.len() == ncols {
        rs.col_order.clone()
    } else {
        (0..ncols).collect()
    };
    let dwidths: Vec<f32> = order.iter().map(|&d| rs.widths[d]).collect();
    let total_w: f32 = num_w + dwidths.iter().sum::<f32>();
    let content_h = header_h + rows as f32 * row_h;

    ui.scope_builder(egui::UiBuilder::new().max_rect(full), |ui| {
        ui.set_clip_rect(full);
        style_scrollbar(ui);
        // egui paints the scrollbar track with extreme_bg_color — tint it like the header
        ui.style_mut().visuals.extreme_bg_color = GRID_HEADER;
        ui.painter().rect_filled(full, CornerRadius::ZERO, GRID_HEADER);
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show_viewport(ui, |ui, viewport| {
                let (content, _) = ui.allocate_exact_size(
                    Vec2::new(total_w, content_h),
                    egui::Sense::hover(),
                );
                // stable id (only one grid is shown at a time) → keyboard focus persists across
                // frames, so the Ctrl+C below actually fires (an auto-id response loses focus)
                let resp = ui.interact(
                    content,
                    egui::Id::new("result_grid_body"),
                    egui::Sense::click_and_drag(),
                );
                let origin = content.left_top();
                // strictly clip to the island: egui expands the ScrollArea's content clip by
                // clip_rect_margin (~3px), which would let rows bleed above the sticky header
                let painter = ui.painter().with_clip_rect(full);

                // display-column geometry helpers (data columns start after the "#" column)
                let col_left = |disp: usize| -> f32 {
                    origin.x + num_w + dwidths.iter().take(disp).sum::<f32>()
                };
                let col_at = |px: f32| -> Option<usize> {
                    let mut cx = origin.x + num_w;
                    for (disp, w) in dwidths.iter().enumerate() {
                        if px >= cx && px < cx + *w {
                            return Some(disp);
                        }
                        cx += *w;
                    }
                    None
                };
                // insertion index 0..=n for a drop at px (by column midpoints)
                let insert_at = |px: f32| -> usize {
                    let mut cx = origin.x + num_w;
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
                let mut ghost: Option<(usize, f32)> = None; // (source display col, floating left x)
                let mut drop: Option<(usize, usize)> = None; // (source display, insertion target)
                let hdr_drag = ui.interact(
                    egui::Rect::from_min_max(
                        egui::pos2(full.left() + num_w, full.top()),
                        egui::pos2(full.right(), full.top() + header_h),
                    ),
                    egui::Id::new("result_grid_header"),
                    egui::Sense::click_and_drag(),
                );
                // store (source display col, grab offset) so the column tracks the cursor naturally
                let drag_id = egui::Id::new("result_grid_col_drag");
                if hdr_drag.drag_started() {
                    if let Some(p) = hdr_drag.interact_pointer_pos() {
                        if let Some(src) = col_at(p.x) {
                            ui.memory_mut(|m| m.data.insert_temp(drag_id, (src, p.x - col_left(src))));
                        }
                    }
                }
                let drag: Option<(usize, f32)> = ui.memory(|m| m.data.get_temp(drag_id));
                if hdr_drag.dragged() {
                    if let (Some((src, dx)), Some(p)) = (drag, hdr_drag.interact_pointer_pos()) {
                        ghost = Some((src, p.x - dx));
                        drop = Some((src, insert_at(p.x)));
                    }
                }
                if hdr_drag.drag_stopped() {
                    if let (Some((src, _)), Some(p)) = (drag, hdr_drag.interact_pointer_pos()) {
                        reorder = Some((src, insert_at(p.x)));
                    }
                }
                let dragging = ghost.is_some();
                // live preview layout: the other columns reflow to make room, and the dragged
                // column's slot (`skip`) becomes an empty gap at the current drop target. The
                // dragged column itself floats as the ghost below. Interaction (col_at/insert_at)
                // still snaps to the RESTING `order`, so the reflow can't feed back on itself.
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
                let lwidths: Vec<f32> = layout.iter().map(|&d| rs.widths[d]).collect();

                // --- column resize handles (thin strips on each header's right edge) ----
                // Header band only, so they never clash with cell selection in the body; placed
                // after the reorder hdr_drag so a grab on the edge resizes (and the middle of a
                // header still reorders). Live: the width is committed every dragged frame.
                let mut resize: Option<(usize, f32)> = None;
                if !dragging {
                    let mut x = origin.x + num_w;
                    for (lidx, &d) in layout.iter().enumerate() {
                        x += lwidths[lidx];
                        let handle = egui::Rect::from_min_max(
                            egui::pos2(x - 3.0, full.top()),
                            egui::pos2(x + 3.0, full.top() + header_h),
                        );
                        let rh = ui.interact(
                            handle,
                            egui::Id::new(("result_grid_colsize", d)),
                            egui::Sense::drag(),
                        );
                        if rh.hovered() || rh.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                        }
                        if rh.dragged() {
                            resize = Some((d, (rs.widths[d] + rh.drag_delta().x).clamp(40.0, 2000.0)));
                        }
                    }
                }

                // --- cell selection (click / drag in the body) -------------------------
                let mut new_sel = sel;
                let mut copy = None;
                if let Some(p) = resp.interact_pointer_pos() {
                    // ignore the pinned "#" column and the header band
                    if p.y >= full.top() + header_h && p.x >= full.left() + num_w {
                        let r = ((p.y - (origin.y + header_h)) / row_h).floor() as i64;
                        if r >= 0 && (r as usize) < rows {
                            let r = r as usize;
                            if let Some(c) = col_at(p.x) {
                                if resp.drag_started() || resp.clicked() {
                                    new_sel = Some(GridSel { ar: r, ac: c, fr: r, fc: c });
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
                // copy on Ctrl/Cmd+C (also accept egui's synthetic Copy event) while focused
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
                            for c in c0..=c1 {
                                if c > c0 {
                                    out.push('\t');
                                }
                                let d = order[c]; // display → data column
                                out.push_str(rs.rows[r].get(d).map_or("", |v| v.as_str()));
                            }
                            out.push('\n');
                        }
                        copy = Some(out);
                    }
                }
                let selr = new_sel.map(|s| {
                    (s.ar.min(s.fr), s.ar.max(s.fr), s.ac.min(s.fc), s.ac.max(s.fc))
                });

                // white sheet behind the data rows
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(origin.x, origin.y + header_h),
                        Vec2::new(total_w, rows as f32 * row_h),
                    ),
                    CornerRadius::ZERO,
                    Color32::WHITE,
                );

                let first = (((viewport.min.y - header_h) / row_h).floor() as i64).max(0) as usize;
                let last = ((((viewport.max.y - header_h) / row_h).ceil() as i64).max(0) as usize)
                    .min(rows);
                for i in first..last {
                    let y = origin.y + header_h + i as f32 * row_h;
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(origin.x, y),
                        Vec2::new(total_w, row_h),
                    );
                    if i % 2 == 1 {
                        painter.rect_filled(rect, CornerRadius::ZERO, ROWALT);
                    }
                    // ("#" row numbers are drawn later as a pinned column overlay)
                    let mut x = rect.left() + num_w;
                    for (lidx, &d) in layout.iter().enumerate() {
                        let w = lwidths[lidx];
                        // the dragged column's slot is left as a plain background gap
                        if Some(lidx) == skip {
                            x += w;
                            continue;
                        }
                        let cell = egui::Rect::from_min_size(
                            egui::pos2(x, rect.top()),
                            Vec2::new(w, row_h),
                        );
                        // selected-cell highlight (drawn under the text); hidden while dragging
                        if !dragging {
                            if let Some((r0, r1, c0, c1)) = selr {
                                if i >= r0 && i <= r1 && lidx >= c0 && lidx <= c1 {
                                    painter.rect_filled(
                                        cell,
                                        CornerRadius::ZERO,
                                        Color32::from_rgba_unmultiplied(0xaa, 0xcc, 0xf0, 150),
                                    );
                                }
                            }
                        }
                        let val = rs.rows[i].get(d).map_or("", |v| v.as_str());
                        let col = if val == "(null)" { TEXTDIM } else { TEXT };
                        painter.with_clip_rect(cell).text(
                            egui::pos2(cell.left() + pad, cell.center().y),
                            egui::Align2::LEFT_CENTER,
                            val,
                            mono.clone(),
                            col,
                        );
                        x += w;
                    }
                }

                // the "torn out" gap shows the grid's own backdrop (the deepest layer under the
                // rows) instead of the white/striped sheet — as if the column were ripped out
                if let Some(g) = skip {
                    let gx = origin.x + num_w + lwidths.iter().take(g).sum::<f32>();
                    let gap = egui::Rect::from_min_size(
                        egui::pos2(gx, origin.y + header_h),
                        Vec2::new(lwidths[g], rows as f32 * row_h),
                    );
                    painter.rect_filled(gap, CornerRadius::ZERO, GRID_HEADER);
                }

                // sticky header — pinned to the (constant) top of the island, follows h-scroll.
                // Using the fixed island top instead of origin.y + viewport.min.y avoids the
                // sub-pixel jitter that made the header shimmer while scrolling.
                let hy = full.top();
                let header_rect = egui::Rect::from_min_size(
                    egui::pos2(full.left(), hy),
                    Vec2::new(full.width(), header_h),
                );
                painter.rect_filled(header_rect, CornerRadius::ZERO, GRID_HEADER);
                painter.vline(origin.x + num_w, header_rect.y_range(), Stroke::new(1.0, BORDER));
                let mut x = origin.x + num_w;
                for (lidx, &d) in layout.iter().enumerate() {
                    let w = lwidths[lidx];
                    // the dragged column's header slot stays an empty gap (just background)
                    if Some(lidx) == skip {
                        painter.vline(x + w, header_rect.y_range(), Stroke::new(1.0, BORDER));
                        x += w;
                        continue;
                    }
                    let cell = egui::Rect::from_min_size(
                        egui::pos2(x, hy),
                        Vec2::new(w, header_h),
                    );
                    painter.with_clip_rect(cell.intersect(header_rect)).text(
                        egui::pos2(cell.left() + pad, cell.center().y),
                        egui::Align2::LEFT_CENTER,
                        &rs.columns[d],
                        mono.clone(),
                        TEXT,
                    );
                    painter.vline(x + w, header_rect.y_range(), Stroke::new(1.0, BORDER));
                    x += w;
                }
                painter.hline(header_rect.x_range(), hy + header_h, Stroke::new(1.0, BORDER));

                // pinned "#" row-number column — fixed at the island's left edge, so it stays
                // put while the data scrolls horizontally (only scrolls vertically with rows)
                let nx = full.left();
                let nclip = egui::Rect::from_min_max(
                    egui::pos2(nx, full.top() + header_h),
                    egui::pos2(nx + num_w, full.bottom()),
                );
                let np = painter.with_clip_rect(nclip);
                for i in first..last {
                    let y = origin.y + header_h + i as f32 * row_h;
                    let cell =
                        egui::Rect::from_min_size(egui::pos2(nx, y), Vec2::new(num_w, row_h));
                    let bg = if i % 2 == 1 { ROWALT } else { Color32::WHITE };
                    np.rect_filled(cell, CornerRadius::ZERO, bg);
                    np.text(
                        egui::pos2(cell.right() - pad, cell.center().y),
                        egui::Align2::RIGHT_CENTER,
                        (i + 1).to_string(),
                        mono.clone(),
                        TEXTDIM,
                    );
                }
                // "#" header corner + the fixed divider between "#" and the data columns
                let nhdr = egui::Rect::from_min_size(
                    egui::pos2(nx, full.top()),
                    Vec2::new(num_w, header_h),
                );
                painter.rect_filled(nhdr, CornerRadius::ZERO, GRID_HEADER);
                painter.vline(nx + num_w, full.y_range(), Stroke::new(1.0, BORDER));
                painter.hline(nhdr.x_range(), full.top() + header_h, Stroke::new(1.0, BORDER));

                // live floating ghost of the column being dragged (drawn last, on top). Its
                // resting slot has already been filled by the reflow; the empty `skip` gap shows
                // where it will land.
                if let Some((src, gleft)) = ghost {
                    let w = dwidths[src];
                    let d = order[src];
                    let gp = painter.with_clip_rect(full);
                    let gh = egui::Rect::from_min_max(
                        egui::pos2(gleft, full.top()),
                        egui::pos2(gleft + w, full.bottom()),
                    );
                    // soft shadow, then the column's own header + white body
                    gp.rect_filled(
                        gh.translate(Vec2::new(2.0, 0.0)),
                        CornerRadius::ZERO,
                        Color32::from_black_alpha(30),
                    );
                    let gh_hdr = egui::Rect::from_min_size(egui::pos2(gleft, full.top()), Vec2::new(w, header_h));
                    gp.rect_filled(gh_hdr, CornerRadius::ZERO, GRID_HEADER);
                    gp.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(gleft, full.top() + header_h),
                            egui::pos2(gleft + w, full.bottom()),
                        ),
                        CornerRadius::ZERO,
                        Color32::WHITE,
                    );
                    for i in first..last {
                        let y = origin.y + header_h + i as f32 * row_h;
                        let cell = egui::Rect::from_min_size(egui::pos2(gleft, y), Vec2::new(w, row_h));
                        if i % 2 == 1 {
                            gp.rect_filled(cell, CornerRadius::ZERO, ROWALT);
                        }
                        let val = rs.rows[i].get(d).map_or("", |v| v.as_str());
                        let col = if val == "(null)" { TEXTDIM } else { TEXT };
                        gp.with_clip_rect(cell).text(
                            egui::pos2(cell.left() + pad, cell.center().y),
                            egui::Align2::LEFT_CENTER,
                            val,
                            mono.clone(),
                            col,
                        );
                    }
                    gp.with_clip_rect(gh_hdr).text(
                        egui::pos2(gh_hdr.left() + pad, gh_hdr.center().y),
                        egui::Align2::LEFT_CENTER,
                        &rs.columns[d],
                        mono.clone(),
                        ACCENT,
                    );
                    crisp_border(&gp, gh, ACCENT);
                }
                (new_sel, copy, reorder, resize)
            })
            .inner
    })
    .inner
}
