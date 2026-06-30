//! XML Model Manager — the left dock (`LeftPanel::Model`): a list of models from `%APPDATA%\JustQuery\models\`,
//! import (pick a `.jqmodel` → copy into the folder + reload the registry), export the selected model to
//! a file, delete. A double-click on a model opens its description tab (`TabKind::ModelEditor`,
//! Stage 3).
//!
//! All filesystem changes go through a registry reload (`reload_models`) — the manager keeps
//! no separate state of its own, only the index of the selected row.

use crate::theme::{self, p};
use crate::widgets::{
    close_x, empty_hint, focus_field, island_box, island_shadow_under, manager_row, primary_button,
    qbtn_off_sm, qbtn_sm, secondary_button, secondary_button_w, show_modal,
    style_scrollbar, subbar, uniform_button_width,
};
use crate::{dialog, ic, xmlmodel, JustQueryApp, LeftPanel, TabKind};
use crate::{RADIUS_ISLAND, SPACE_2, SPACE_3, TABBAR_H};
use eframe::egui;
use egui::{Align, Layout, Margin, RichText};

impl JustQueryApp {
    /// Reload the registry from the models folder and bump the generation. Call after import/delete/create/
    /// save. Along the way it reassigns models to open XML tabs (the registry changed) — otherwise a
    /// tab would hold a stale/empty `model_id` (the Inspect button stays active but is a no-op).
    pub(crate) fn reload_models(&mut self) {
        // the model set changed → schemas compiled for the old/edited/deleted models are stale;
        // drop the per-process XSD cache so they don't linger (it recompiles lazily on next use)
        crate::xsd::clear_schema_cache();
        self.models = xmlmodel::Registry::load_dir(&crate::models_dir());
        self.models_gen = self.models_gen.wrapping_add(1);
        for i in 0..self.tabs.len() {
            if !self.tabs[i].is_xml() {
                continue;
            }
            let Some(head) = self.tabs[i]
                .doc_mut()
                .map(|d| String::from_utf8_lossy(&d.read_bytes(0, 8192)).into_owned())
            else {
                continue;
            };
            let new_id = self.models.match_doc(&head).map(|m| m.manifest.id.clone());
            self.tabs[i].model_id = new_id;
        }
    }

    /// The XML Model Manager side panel (toggled from the toolbar). Mirrors the layout of the
    /// Connection Manager: header + small toolbar (Import / Export / Delete) + scrollable list.
    pub(crate) fn model_manager_panel(&mut self, ui: &mut egui::Ui) {
        if self.left_panel != Some(LeftPanel::Model) {
            return;
        }
        let mut do_import = false;
        let mut do_create = false; // + → open the new-model creation modal
        let mut do_delete = false; // delete the selected model
        let mut close_panel = false;
        let mut open_idx: Option<usize> = None; // double-click → open the model tab (Stage 3)

        let saved_style = crate::widgets::hush_resize_line(ui);
        egui::Panel::left("left_panel")
            .resizable(true)
            // width range — exactly like the Connection/Metadata managers (one shared span)
            .default_size(220.0)
            .size_range(196.0..=460.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin::ZERO))
            .show(ui, |ui| {
                ui.style_mut().visuals.override_text_color = None;
                // header
                egui::Panel::top("modelmgr_header")
                    .exact_size(TABBAR_H)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: crate::DOCK_TITLE_INDENT as i8,
                        right: crate::CHROME_GUTTER as i8,
                        // centre in the full row to line up with the tab labels (same TABBAR_H)
                        top: 0,
                        bottom: 0,
                    }))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.label(
                                RichText::new("XML Models").size(13.0).color(p().text),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if close_x(ui, "Close panel") {
                                    close_panel = true;
                                }
                            });
                        });
                    });
                // toolbar: New (+, creation modal) · Import (OPEN, file) · Delete (selected).
                // Export has no button here — it's the toolbar's Save As on the open model tab.
                subbar(ui, "modelmgr_toolbar", crate::CHROME_GUTTER as i8, |ui| {
                    if qbtn_sm(ui, ic::PLUS, p().text, "New model").clicked() {
                        do_create = true;
                    }
                    if qbtn_sm(ui, ic::OPEN, p().text, "Import model…").clicked() {
                        do_import = true;
                    }
                    if self.model_sel.is_none() {
                        qbtn_off_sm(ui, ic::DELETE, "Delete (select a model)");
                    } else if qbtn_sm(ui, ic::DELETE, p().text, "Delete selected model").clicked() {
                        do_delete = true;
                    }
                });
                // list island
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin {
                        left: crate::CHROME_GUTTER as i8, // single gutter (window edge)
                        right: crate::CHROME_GUTTER as i8,
                        top: crate::CHROME_GUTTER as i8, // 4px between the sub-toolbar and the data island
                        bottom: 0,
                    }))
                    .show(ui, |ui| {
                        // snapshot of the list for rendering (name + intact flag). We don't show the id —
                        // a flat list, like the Connection Manager; non-intact rows get a "⚠" in the row name.
                        let rows: Vec<(usize, String, bool)> = self
                            .models
                            .models()
                            .iter()
                            .enumerate()
                            .map(|(i, m)| {
                                let label = if m.intact {
                                    m.manifest.id.clone()
                                } else {
                                    format!("⚠ {}", m.manifest.id)
                                };
                                (i, label, m.intact)
                            })
                            .collect();
                        let errors: Vec<String> = self
                            .models
                            .load_errors()
                            .iter()
                            .map(|(p, e)| format!("{}: {e}", p.display()))
                            .collect();
                        // white work-area island (model list), vertically scrollable —
                        // pixel-for-pixel the same skeleton as the Database/Metadata Manager:
                        // Frame(ivory + island_shadow) → ScrollArea(manager_row) → crisp_border
                        // on top (selection/hover reaches the corners with no white triangles).
                        crate::widgets::island_panel(ui, p().ivory, |ui| {
                                ui.set_min_size(ui.available_size());
                                let clip = ui.max_rect();
                                ui.set_clip_rect(clip);
                                // Floating overlay bar (see the Metadata Manager): reserves no width,
                                // so rows fill edge-to-edge and a bar toggling never reflows them.
                                crate::widgets::style_scrollbar_overlay(ui);
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.spacing_mut().item_spacing.y = 0.0; // tight rows — no gap
                                        if rows.is_empty() && errors.is_empty() {
                                            ui.add_space(SPACE_2);
                                            empty_hint(
                                                ui,
                                                "No XML models.\nClick + to create one.",
                                            );
                                        }
                                        for (i, label, _intact) in &rows {
                                            let selected = self.model_sel == Some(*i);
                                            // flat manager row (icon + id); selected →
                                            // tint, as in the Database/Metadata Manager.
                                            let resp = manager_row(
                                                ui,
                                                0.0,
                                                ic::MODEL,
                                                label,
                                                selected,
                                            );
                                            if resp.clicked() {
                                                self.model_sel = Some(*i);
                                            }
                                            if resp.double_clicked() {
                                                open_idx = Some(*i);
                                            }
                                        }
                                        if !errors.is_empty() {
                                            ui.add_space(SPACE_2);
                                            ui.label(
                                                RichText::new("Failed to load:")
                                                    .size(crate::LABEL_SIZE)
                                                    .color(p().danger),
                                            );
                                            for e in &errors {
                                                ui.label(RichText::new(e).size(10.0).color(p().danger));
                                            }
                                        }
                                    });
                        });
                    });
            });
        // restore the style hush_resize_line cloned, like the Connection/Metadata managers — else the
        // suppressed resize-line stroke would leak into later widgets drawn through this same `ui`
        ui.set_style(saved_style);

        // actions — after rendering, so we don't hold the ui borrow longer than needed
        if do_create {
            // open the modal with an empty buffer (if not already open)
            if self.model_create.is_none() {
                self.model_create = Some(crate::ModelCreateBuf {
                    id: String::new(),
                    description: String::new(),
                    xsd: String::new(),
                    focus_id: true,
                    error: None,
                });
            }
        }
        if do_import {
            self.import_model();
        }
        if do_delete {
            // open the delete-confirmation modal (instead of deleting immediately)
            self.model_delete_confirm = self
                .model_sel
                .and_then(|i| self.models.models().get(i).map(|m| m.manifest.id.clone()));
        }
        if open_idx.is_some() {
            self.model_sel = open_idx;
            self.open_model_tab();
        }
        if close_panel {
            self.left_panel = None;
        }
    }

    /// Open the editor tab for the model selected in the manager (double-click on the row). The tab
    /// receives a working copy of the model (`working`) — edits accumulate in it, not in the registry, until
    /// an explicit save.
    pub(crate) fn open_model_tab(&mut self) {
        let Some(idx) = self.model_sel else {
            return;
        };
        let Some(model) = self.models.models().get(idx) else {
            return;
        };
        let id = model.manifest.id.clone();
        // if a tab for this model is already open — switch to it
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| matches!(&t.kind, TabKind::ModelEditor(m) if m.id == id))
        {
            self.active_tab = i;
            return;
        }
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        // tab title = model id (like connections: title = name). File = <id>.jqmodel.
        let mut tab = crate::Tab::new(tab_id, model.manifest.id.clone());
        tab.kind = TabKind::ModelEditor(Box::new(crate::ModelEdit {
            id,
            working: model.clone(),
            dirty: false,
            rule_modal: None,
            match_modal: None,
            pending_delete: None,
        }));
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    /// Save the tab's working copy of the model to disk (`<models>/<id>.jqmodel`) + reload the
    /// registry. File name = id; if the id changed, the old file is deleted. The checksum is
    /// recomputed during serialization — the model becomes `intact` again.
    pub(crate) fn save_model_tab(&mut self) {
        // pull out the working copy + old path (before reload, while the tab still holds the reference)
        let (id, working, old_path) = match self.cur() {
            Some(t) => match &t.kind {
                TabKind::ModelEditor(m) => {
                    (m.working.manifest.id.clone(), m.working.clone(), m.working.path.clone())
                }
                _ => return,
            },
            None => return,
        };
        let dir = crate::models_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error_modal = Some(format!("Cannot create models dir: {e}"));
            return;
        }
        let dest = dir.join(format!("{}.jqmodel", sanitize_filename(&id)));
        // if the id changed — delete the old file (rewriting the same id just overwrites it)
        if let Some(old) = &old_path {
            if old != &dest && old.exists() {
                let _ = std::fs::remove_file(old);
            }
        }
        if let Err(e) = xmlmodel::save_file(&working, &dest) {
            self.error_modal = Some(format!("Save model failed: {e}"));
            return;
        }
        self.reload_models();
        // clear dirty + sync working with the reloaded registry (a fresh intact copy)
        let fresh = self.models.models().iter().find(|m| m.manifest.id == id).cloned();
        if let Some(t) = self.cur_mut() {
            if let TabKind::ModelEditor(m) = &mut t.kind {
                m.dirty = false;
                if let Some(f) = fresh {
                    m.working = f;
                }
            }
        }
    }

    /// Body of the model-editor tab: metadata (read-only), XSD (a replace button that picks from files),
    /// a line-by-line list of validation rules (add/remove). Edits accumulate in the tab's `working`
    /// copy; saving goes through `save_model_tab` (Ctrl+S / toolbar, when `dirty`).
    pub(crate) fn model_tab(&mut self, ui: &mut egui::Ui) {
        // Take a working copy of the model off the tab for rendering. Edits are applied via cur_mut
        // as separate operations, so we don't hold a borrow of self during the render.
        // `model_id` / `dirty` are no longer read here (Save / Export moved to the toolbar); the
        // working snapshot + the two modal-open flags still drive the body render.
        let (_model_id, working_snapshot, _dirty, rule_modal_open, match_modal_open) = match self.cur()
        {
            Some(t) => match &t.kind {
                TabKind::ModelEditor(m) => (
                    m.id.clone(),
                    m.working.clone(),
                    m.dirty,
                    m.rule_modal.is_some(),
                    m.match_modal.is_some(),
                ),
                _ => return,
            },
            None => return,
        };

        // Local editable description buffer: initialized from the snapshot, then after the render it's
        // compared and written back into working via cur_mut (if changed).
        let mut desc_buf = working_snapshot.manifest.description.clone();
        // Snapshot of the identification rules for rendering the list (edits go through the modal, not in-frame).
        let match_rules_view: Vec<(usize, String, String)> = working_snapshot
            .manifest
            .r#match
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let vals = if r.values.is_empty() {
                    "(presence)".to_owned()
                } else {
                    r.values.join(",")
                };
                (i, r.attr.clone(), vals)
            })
            .collect();

        // XSD button: if there's no XSD — "Select XSD", otherwise "Replace XSD" + "Clear".
        // Replacement goes through the native file picker (can be called repeatedly, appending include files).
        let mut ask_rule_delete: Option<usize> = None; // ✕ on a validation rule → delete confirmation
        let mut edit_rule_idx: Option<usize> = None; // edit a validation rule (click on the row)
        let mut open_rule_modal = false; // open the add-validation-rule modal
        let mut ask_match_delete: Option<usize> = None; // ✕ on an identification rule → delete confirmation
        let mut edit_match_idx: Option<usize> = None; // edit an identification rule (click)
        let mut open_match_modal = false; // open the add-identification-rule modal

        // body — the canonical silver sheet + data island + scroll (like About/Scan).
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show(ui, |ui| {
                let sheet = ui.max_rect();
                island_shadow_under(ui.painter(), sheet);
                island_box(ui.painter(), sheet, p().data_bg, RADIUS_ISLAND);
                style_scrollbar(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Content island: shared left edge, single right edge (description, rule ✕,
                        // footer buttons). The width WAS ADAPTIVE: no wider than the design maximum (620), but always
                        // fitting the available width — when the window narrows the content doesn't run off the edge (no
                        // horizontal scroll). inner_w = usable width inside the Frame (minus the
                        // 18×2 padding); we cap max_width by it so the right edge is uniform.
                        // FIXED content width (no adaptive — a varying width looked
                        // out of place): tuned for the minimum window width (1024) with the left panel at its
                        // maximum (460), so the Frame (inner_w + 18×2 padding) fits and the right
                        // ✕ column doesn't run off the edge. On wide windows the island is simply pinned left.
                        let inner_w: f32 = 500.0;
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(18, 16))
                            .show(ui, |ui| {
                                theme::style_modal_widgets(ui);
                                ui.set_max_width(inner_w);
                                // header: id (bold) + intact/dirty status
                                ui.label(
                                    RichText::new(&working_snapshot.manifest.id)
                                        .font(theme::ui_bold_font(crate::HEADING_SIZE))
                                        .color(p().text),
                                );
                                if !working_snapshot.intact {
                                    ui.label(
                                        RichText::new(
                                            "Checksum mismatch — the model was modified \
                                             outside the editor.",
                                        )
                                        .size(crate::GRID_SIZE)
                                        .color(p().danger),
                                    );
                                }
                                ui.add_space(SPACE_3);

                                // ---- XSD schema (read-only, the model's foundation) ----
                                // The XSD is set when the model is created and doesn't change: it's the "foundation",
                                // replacing/clearing it on a finished model makes no sense (easier to create
                                // a new one). So here we only show status, without Replace/Clear.
                                section_label(ui, "XSD schema");
                                ui.label(
                                    RichText::new(if working_snapshot.has_xsd() {
                                        format!("Defined, {} bytes", working_snapshot.xsd.len())
                                    } else {
                                        "  not set".to_owned()
                                    })
                                    .size(crate::BODY_SIZE)
                                    .color(if working_snapshot.has_xsd() {
                                        p().text
                                    } else {
                                        p().warn
                                    }),
                                );
                                ui.add_space(SPACE_3);

                                // description (editable) — fixed height + inner scroll, border/
                                // background/color like a regular field (see multiline_field).
                                section_label(ui, "Description");
                                multiline_field(ui, &mut desc_buf, "model_desc", 60.0, 3, None);
                                ui.add_space(SPACE_3);

                                // ---- Identification rules: rule list (click = edit, ✕ = remove) ----
                                // Each rule = "an attribute of the root element is present / ∈ values".
                                // A document matches if AT LEAST ONE rule holds (OR; see MatchPred::matches).
                                // Attributes come from the XSD (dropdown in the modal). Add — the plus next to
                                // the heading; edit — click on the row.
                                ui.horizontal(|ui| {
                                    section_label(ui, "Identification rules");
                                    ui.add_space(SPACE_2);
                                    if crate::widgets::qbtn_sm(
                                        ui,
                                        ic::PLUS,
                                        p().text,
                                        "Add identification rule",
                                    )
                                    .clicked()
                                    {
                                        open_match_modal = true;
                                    }
                                });
                                ui.add_space(SPACE_2);
                                if match_rules_view.is_empty() {
                                    ui.label(
                                        RichText::new("  no rules — any document matches")
                                            .size(crate::BODY_SIZE)
                                            .color(p().text_dim),
                                    );
                                } else {
                                    for (i, attr, vals) in &match_rules_view {
                                        let line = format!("attr {attr}: {vals}");
                                        let row_id = ui.id().with(("match_row", *i));
                                        let (open, remove) = rule_list_row(
                                            ui,
                                            inner_w,
                                            row_id,
                                            &line,
                                            "Remove this identification rule",
                                        );
                                        if open {
                                            edit_match_idx = Some(*i);
                                        }
                                        if remove {
                                            ask_match_delete = Some(*i);
                                        }
                                    }
                                }
                                ui.add_space(SPACE_3);
                                ui.add_space(SPACE_3);

                                // ---- Validation rules: line-by-line list (click = edit) ----
                                ui.horizontal(|ui| {
                                    section_label(ui, "Validation rules");
                                    ui.add_space(SPACE_2);
                                    if crate::widgets::qbtn_sm(
                                        ui,
                                        ic::PLUS,
                                        p().text,
                                        "Add validation rule",
                                    )
                                    .clicked()
                                    {
                                        open_rule_modal = true;
                                    }
                                });
                                ui.add_space(SPACE_2);

                                let rules_view = parse_rules_view(&working_snapshot.rules);
                                if rules_view.is_empty() {
                                    ui.label(
                                        RichText::new("  no rules yet — click Add rule")
                                            .size(crate::BODY_SIZE)
                                            .color(p().text_dim),
                                    );
                                } else {
                                    for (i, name) in &rules_view {
                                        let line = if name.is_empty() {
                                            "(unnamed)".to_owned()
                                        } else {
                                            name.clone()
                                        };
                                        let row_id = ui.id().with(("rule_row", *i));
                                        let (open, remove) =
                                            rule_list_row(ui, inner_w, row_id, &line, "Remove this rule");
                                        if open {
                                            edit_rule_idx = Some(*i);
                                        }
                                        if remove {
                                            ask_rule_delete = Some(*i);
                                        }
                                    }
                                }
                                // Actions (Save · Export · Close) live on the main toolbar now: Save
                                // = Save verb, Export = Save As verb, and Close is the tab's own ×.
                                // A Page carries only clickable content, never widget buttons.
                            }); // close Frame::show
                    }); // close ScrollArea
            });

        // ---- applying edits to the tab's working copy ----
        // Description: write back if the buffer changed relative to the snapshot.
        let desc_changed = desc_buf != working_snapshot.manifest.description;
        if desc_changed {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    m.working.manifest.description = desc_buf.clone();
                    m.dirty = true;
                }
            }
        }
        // ✕ on an identification rule → request confirmation (the actual delete is in rule_delete_modal)
        if let Some(idx) = ask_match_delete {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    m.pending_delete = Some(crate::PendingRuleDelete::Match(idx));
                }
            }
        }
        // open the add-identification-rule modal
        if open_match_modal {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    if m.match_modal.is_none() {
                        m.match_modal = Some(crate::MatchRuleEditBuf {
                            attr: String::new(),
                            values: String::new(),
                            edit_idx: None,
                            error: None,
                        });
                    }
                }
            }
        }
        // edit an identification rule — fill the buffer from the existing rule
        if let Some(idx) = edit_match_idx {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    if m.match_modal.is_none() {
                        if let Some(rule) = m.working.manifest.r#match.rules.get(idx).cloned() {
                            m.match_modal = Some(crate::MatchRuleEditBuf {
                                attr: rule.attr,
                                values: rule.values.join(","),
                                edit_idx: Some(idx),
                                error: None,
                            });
                        }
                    }
                }
            }
        }
        // ✕ on a validation rule → request confirmation (the actual delete is in rule_delete_modal)
        if let Some(idx) = ask_rule_delete {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    m.pending_delete = Some(crate::PendingRuleDelete::Validation(idx));
                }
            }
        }
        if open_rule_modal {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    if m.rule_modal.is_none() {
                        m.rule_modal = Some(crate::RuleEditBuf {
                            name: String::new(),
                            message: String::new(),
                            severity: "error".to_owned(),
                            check: "{\n  \"type\": \"required_if\",\n  \"condition\": { \"any\": [] }\n}"
                                .to_owned(),
                            error: None,
                            edit_idx: None,
                        });
                    }
                }
            }
        }
        // editing an existing rule: fill the buffer with the rule's values by index
        if let Some(idx) = edit_rule_idx {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    if m.rule_modal.is_none() {
                        if let Some(buf) = rule_to_buf(&m.working.rules, idx) {
                            m.rule_modal = Some(buf);
                        }
                    }
                }
            }
        }

        // add/edit rule modal (drawn on top when the buffer is open)
        if rule_modal_open || open_rule_modal {
            self.rule_edit_modal(ui.ctx());
        }
        if match_modal_open || open_match_modal {
            self.match_rule_edit_modal(ui.ctx());
        }
        // confirm rule deletion (validation/identification), if requested via ✕
        self.rule_delete_modal(ui.ctx());
    }

    /// "Save As" / Export for the active model-editor tab (the Save As toolbar verb): locate the
    /// model in the registry by id and export it to a chosen `.jqmodel` file. Exports the saved
    /// registry copy — Save first to fold in unsaved edits.
    pub(crate) fn export_active_model(&mut self) {
        let Some(id) = self.cur().and_then(|t| match &t.kind {
            TabKind::ModelEditor(m) => Some(m.id.clone()),
            _ => None,
        }) else {
            return;
        };
        self.model_sel = self.models.models().iter().position(|m| m.manifest.id == id);
        self.export_selected_model();
    }

    /// Import a `.jqmodel` via the native dialog → copy into the models folder → reload the registry.
    fn import_model(&mut self) {
        let Some(src) = dialog::open_file() else {
            return;
        };
        // read + validate as a model before copying, so we don't litter the folder with broken files
        let text = match std::fs::read_to_string(&src) {
            Ok(t) => t,
            Err(e) => {
                self.error_modal = Some(format!("Import failed: {e}"));
                return;
            }
        };
        let model = match xmlmodel::parse(&text) {
            Ok(m) => m,
            Err(e) => {
                self.error_modal = Some(format!("Not a valid .jqmodel: {e}"));
                return;
            }
        };
        // The checksum is mandatory and must match. Otherwise a corrupted/hand-edited file
        // would silently get a fresh valid sum on the re-serialization below — import would become a channel
        // for "re-blessing" a forgery. We reject such files with an explicit error.
        if !model.intact {
            let why = if model.checksum.is_empty() {
                "the file has no checksum (---checksum---)"
            } else {
                "checksum mismatch — the file is corrupted or was edited by hand"
            };
            self.error_modal = Some(format!("Import rejected: {why}"));
            return;
        }
        // target file name = model id; create the folder if it doesn't exist
        let dir = crate::models_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error_modal = Some(format!("Cannot create models dir: {e}"));
            return;
        }
        let dest = dir.join(format!("{}.jqmodel", sanitize_filename(&model.manifest.id)));
        // re-serialize — the checksum is guaranteed to match
        if let Err(e) = xmlmodel::save_file(&model, &dest) {
            self.error_modal = Some(format!("Import write failed: {e}"));
            return;
        }
        self.reload_models();
        // select the just-imported one
        self.model_sel = self
            .models
            .models()
            .iter()
            .position(|m| m.manifest.id == model.manifest.id);
    }

    /// Export the selected model to a file via the native save dialog.
    fn export_selected_model(&mut self) {
        let Some(idx) = self.model_sel else {
            return;
        };
        let Some(model) = self.models.models().get(idx) else {
            return;
        };
        let suggested = format!("{}.jqmodel", sanitize_filename(&model.manifest.id));
        let Some(dest) = dialog::save_file(Some(&suggested)) else {
            return;
        };
        if let Err(e) = xmlmodel::save_file(model, &dest) {
            self.error_modal = Some(format!("Export failed: {e}"));
        }
    }

    /// Delete a model by id from disk + reload the registry. Called from the confirmation modal
    /// (`delete_model_modal`), after the user has confirmed the deletion.
    fn delete_model_by_id(&mut self, id: &str) {
        let path = match self
            .models
            .models()
            .iter()
            .find(|m| m.manifest.id == id)
            .and_then(|m| m.path.as_ref())
        {
            Some(p) => p.clone(),
            None => return,
        };
        if let Err(e) = std::fs::remove_file(&path) {
            self.error_modal = Some(format!("Delete failed: {e}"));
            return;
        }
        // close this model's editor tab, if open
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| matches!(&t.kind, TabKind::ModelEditor(m) if m.id == id))
        {
            self.close_tab(i);
            // close_tab may have shifted indices — but we reload the selection anyway
        }
        self.model_sel = None;
        self.reload_models();
    }

    /// Model delete-confirmation modal: "Delete model «<id>»? This action cannot be undone."
    /// with Cancel / Delete (destructive) buttons. Deletion happens only after confirmation.
    pub(crate) fn delete_model_modal(&mut self, ctx: &egui::Context) {
        let Some(id) = self.model_delete_confirm.take() else {
            return;
        };
        let out = crate::widgets::confirm_modal(
            ctx,
            "model_delete",
            crate::widgets::ConfirmKind::Danger,
            "Delete XML model",
            &format!("Delete model «{id}»? The .jqmodel file will be deleted permanently."),
            "Delete",
            "Cancel",
        );
        if out.confirmed {
            self.delete_model_by_id(&id);
            return;
        }
        if out.cancelled {
            return; // closed without deleting (id dropped)
        }
        self.model_delete_confirm = Some(id); // neither → the modal stays open
    }

    /// New-model creation modal: id / name / description fields + Create/Cancel. Creates an empty
    /// model (no XSD/rules) at `%APPDATA%\JustQuery\models\<id>.jqmodel`, reloads the registry, and
    /// opens an editor tab for further filling in.
    pub(crate) fn create_model_modal(&mut self, ctx: &egui::Context) {
        // Closed — draw nothing. We check via take to release the borrow for processing.
        let Some(mut buf) = self.model_create.take() else {
            return;
        };
        let mut commit = false;
        let mut cancel = false;
        let mut pick_xsd_for_create = false;
        let dismiss = show_modal(ctx, "model_create", 380.0, |ui| {
            ui.label(
                egui::RichText::new("New XML model")
                    .size(crate::HEADING_SIZE)
                    .strong()
                    .color(p().text),
            );
            ui.add_space(SPACE_2);
            let field_w = ui.available_width(); // full-width fields → the right edge isn't "wide"

            ui.label(egui::RichText::new("id").size(crate::LABEL_SIZE).color(p().text_dim));
            let resp = focus_field(ui, &mut buf.id, false, field_w);
            if buf.focus_id {
                resp.request_focus();
                buf.focus_id = false;
            }
            ui.label(
                egui::RichText::new("used as the file name; ASCII letters, digits, .-_")
                    .size(10.0)
                    .color(p().disabled),
            );
            ui.add_space(SPACE_2);

            ui.label(egui::RichText::new("description").size(crate::LABEL_SIZE).color(p().text_dim));
            multiline_field(ui, &mut buf.description, "create_desc", 60.0, 3, None);
            ui.add_space(SPACE_2);

            // XSD (required): Select — native dialog, can be invoked repeatedly (Main + includes
            // are concatenated into one block). Create is dimmed while the XSD is empty (a model without a schema doesn't exist).
            ui.label(egui::RichText::new("XSD schema (required)").size(crate::LABEL_SIZE).color(p().text_dim));
            ui.horizontal(|ui| {
                let label = if buf.xsd.is_empty() { "Select XSD…" } else { "Add more…" };
                if secondary_button(ui, label, true) {
                    pick_xsd_for_create = true;
                }
                ui.label(
                    egui::RichText::new(if buf.xsd.is_empty() {
                        "no schema".to_owned()
                    } else {
                        format!("{} bytes", buf.xsd.len())
                    })
                    .size(crate::LABEL_SIZE)
                    .color(if buf.xsd.is_empty() { p().warn } else { p().text_dim }),
                );
            });
            ui.add_space(SPACE_2);

            if let Some(err) = &buf.error {
                ui.label(egui::RichText::new(err).size(crate::LABEL_SIZE).color(p().danger));
                ui.add_space(SPACE_2);
            }

            ui.horizontal(|ui| {
                ui.set_min_width(field_w);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Create is active only when id is non-empty AND an XSD is selected (a model without a schema doesn't exist).
                    let can_create = !buf.id.trim().is_empty() && !buf.xsd.is_empty();
                    if primary_button(ui, "Create", can_create) {
                        commit = true;
                    }
                    ui.add_space(SPACE_2);
                    if secondary_button(ui, "Cancel", true) {
                        cancel = true;
                    }
                });
            });
        });

        // Enter does NOT commit: the modal has a multiline description field where Enter is a line
        // break (otherwise Enter in the description would create the model and lose the rest of the text). Creation —
        // only via the Create button; Escape — cancel.
        if dismiss.escape || cancel {
            return; // modal closed (buffer dropped)
        }

        // pick an XSD via the native dialog (appended to the buffer — can be Main + includes)
        if pick_xsd_for_create {
            if let Some(path) = dialog::open_file() {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        buf.xsd.push_str(&text);
                        if !text.ends_with('\n') {
                            buf.xsd.push('\n');
                        }
                    }
                    Err(e) => {
                        buf.error = Some(format!("Read XSD failed: {e}"));
                    }
                }
            }
        }

        if commit {
            // validate id: non-empty, no invalid characters, not already taken
            let id = buf.id.trim().to_owned();
            if id.is_empty() {
                buf.error = Some("id cannot be empty.".to_owned());
                self.model_create = Some(buf);
                return;
            }
            if !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
                buf.error = Some(
                    "id contains invalid characters (allowed: Latin letters, digits, .-_)."
                        .to_owned(),
                );
                self.model_create = Some(buf);
                return;
            }
            if self.models.models().iter().any(|m| m.manifest.id == id) {
                buf.error = Some(format!("A model with id «{id}» already exists."));
                self.model_create = Some(buf);
                return;
            }
            match self.create_model(&id, buf.description.trim(), buf.xsd.clone()) {
                Ok(()) => {
                    self.reload_models();
                    // select and open the editor for the new model
                    self.model_sel = self.models.models().iter().position(|m| m.manifest.id == id);
                    self.open_model_tab();
                }
                Err(e) => {
                    buf.error = Some(e);
                    self.model_create = Some(buf);
                }
            }
            return;
        }

        // the modal stays open (continue input)
        self.model_create = Some(buf);
    }

    /// Write a new model to disk with the given XSD (required). The checksum is recomputed
    /// during serialization — the model is `intact` right away. codes/rules are empty — the user fills them in the editor.
    fn create_model(&mut self, id: &str, description: &str, xsd: String) -> Result<(), String> {
        let dir = crate::models_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create models dir: {e}"))?;
        // The XSD must compile: the root element used for identification comes from the XSD schema, and
        // rules reference its elements. Without this the model would silently match no
        // document (match_doc checks the root via xsd::compile). We catch it at creation time.
        crate::xsd::compile(&xsd, id).map_err(|e| format!("XSD does not compile: {e}"))?;
        let model = xmlmodel::Model {
            manifest: xmlmodel::Manifest {
                id: id.to_owned(),
                description: description.to_owned(),
                priority: 100,
                r#match: xmlmodel::MatchPred::default(),
            },
            xsd,
            codes: "{}".to_owned(),
            rules: "{\"rules\": []}".to_owned(),
            checksum: String::new(),
            intact: true,
            path: None,
        };
        let dest = dir.join(format!("{id}.jqmodel"));
        xmlmodel::save_file(&model, &dest).map_err(|e| e.to_string())
    }

    /// Add/edit validation-rule modal. Rule fields: id, description (the finding
    /// text), severity (error/warn), and check — a JSON body with ALL the rule's mechanics (`type` +
    /// parameters: codes/block/condition/…). description and check have a fixed height with scroll.
    /// A declarative condition builder is separate work; for now we provide a JSON check editor with validation.
    pub(crate) fn rule_edit_modal(&mut self, ctx: &egui::Context) {
        // Take the buffer off the tab (take, to release the borrow for commit).
        let model_id = self.cur().and_then(|t| match &t.kind {
            TabKind::ModelEditor(m) => Some(m.id.clone()),
            _ => None,
        });
        let Some(model_id) = model_id else {
            return;
        };
        let buf = match self.cur_mut() {
            Some(t) => match &mut t.kind {
                TabKind::ModelEditor(m) => m.rule_modal.take(),
                _ => None,
            },
            None => None,
        };
        let Some(mut buf) = buf else {
            return;
        };
        let mut commit = false;
        let mut cancel = false;
        // Form width — based on the min window size (1024): modal 560 + fields of 260 per column.
        let dismiss = show_modal(ctx, "rule_add", 560.0, |ui| {
            let heading =
                if buf.edit_idx.is_some() { "Edit validation rule" } else { "New validation rule" };
            ui.label(RichText::new(heading).size(crate::HEADING_SIZE).strong().color(p().text));
            ui.add_space(SPACE_3);

            // Rule fields. message and check — fixed height with built-in scroll. Field widths and
            // the buttons' right edge come from the modal's available width. Gaps between blocks are uniform
            // (SPACE_3).
            let field_w = ui.available_width();
            const DESC_H: f32 = 60.0;
            const CHECK_H: f32 = 150.0;

            // name — the key field (the finding's identifier/code).
            ui.label(RichText::new("name").size(crate::LABEL_SIZE).color(p().text_dim));
            let _ = focus_field(ui, &mut buf.name, false, field_w);
            ui.add_space(SPACE_3);

            // message — the finding text; fixed height + scroll.
            ui.label(RichText::new("message").size(crate::LABEL_SIZE).color(p().text_dim));
            multiline_field(ui, &mut buf.message, "rule_msg", DESC_H, 3, None);
            ui.add_space(SPACE_3);

            // severity — the finding's severity (error/warn).
            ui.label(RichText::new("severity").size(crate::LABEL_SIZE).color(p().text_dim));
            ui.horizontal(|ui| {
                ui.radio_value(&mut buf.severity, "error".to_owned(), "error");
                ui.radio_value(&mut buf.severity, "warn".to_owned(), "warn");
            });
            ui.add_space(SPACE_3);

            // check (JSON) — all the rule's mechanics (type + codes/block/scope/condition/…);
            // fixed height + scroll, monospace.
            ui.label(RichText::new("check (JSON)").size(crate::LABEL_SIZE).color(p().text_dim));
            multiline_field(
                ui,
                &mut buf.check,
                "rule_check",
                CHECK_H,
                9,
                Some(egui::FontId::monospace(11.0)),
            );
            ui.add_space(SPACE_3);

            if let Some(err) = &buf.error {
                ui.label(RichText::new(err).size(crate::LABEL_SIZE).color(p().danger));
                ui.add_space(SPACE_2);
            }

            ui.horizontal(|ui| {
                ui.set_min_width(field_w);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // right_to_left: first in code = rightmost on screen (at the right edge of the content).
                    let btn_label = if buf.edit_idx.is_some() { "Save" } else { "Add" };
                    if primary_button(ui, btn_label, !buf.name.trim().is_empty()) {
                        commit = true;
                    }
                    ui.add_space(SPACE_2);
                    if secondary_button(ui, "Cancel", true) {
                        cancel = true;
                    }
                });
            });
        });
        let _ = dismiss;

        if cancel {
            return; // close, buffer dropped
        }
        if commit {
            if buf.name.trim().is_empty() {
                buf.error = Some("name cannot be empty.".to_owned());
                self.return_rule_modal(model_id, buf);
                return;
            }
            // add OR replace the rule in the tab's working.rules (by edit_idx)
            let res = self.cur_mut().and_then(|t| match &mut t.kind {
                TabKind::ModelEditor(m) => {
                    let r = match buf.edit_idx {
                        Some(idx) => replace_rule_at(&mut m.working.rules, idx, &buf),
                        None => add_rule_to_json(&mut m.working.rules, &buf),
                    };
                    if r.is_ok() {
                        m.dirty = true;
                    }
                    Some(r)
                }
                _ => None,
            });
            match res {
                Some(Ok(())) => return, // success — modal closed
                Some(Err(e)) => {
                    buf.error = Some(e);
                    self.return_rule_modal(model_id, buf);
                }
                None => return,
            }
            return;
        }
        // the modal stays open
        self.return_rule_modal(model_id, buf);
    }

    /// Return the rule modal's buffer to the tab (after a failed commit or continued input).
    fn return_rule_modal(&mut self, model_id: String, buf: crate::RuleEditBuf) {
        if let Some(t) = self.cur_mut() {
            if let TabKind::ModelEditor(m) = &mut t.kind {
                if m.id == model_id {
                    m.rule_modal = Some(buf);
                }
            }
        }
    }

    /// Add/edit identification-rule modal: attr (a dropdown of the XSD root type's attributes)
    /// + values (comma-separated; empty = presence). edit_idx = Some(i) → replace.
    pub(crate) fn match_rule_edit_modal(&mut self, ctx: &egui::Context) {
        let model_id = self.cur().and_then(|t| match &t.kind {
            TabKind::ModelEditor(m) => Some(m.id.clone()),
            _ => None,
        });
        let Some(model_id) = model_id else {
            return;
        };
        // take the buffer + xsd attributes (for the attr dropdown)
        let (buf_take, xsd_attrs) = match self.cur_mut() {
            Some(t) => match &mut t.kind {
                TabKind::ModelEditor(m) => {
                    let attrs = xsd_match_candidates(&m.working.xsd).1;
                    (m.match_modal.take(), attrs)
                }
                _ => (None, Vec::new()),
            },
            None => (None, Vec::new()),
        };
        let Some(mut buf) = buf_take else {
            return;
        };
        let mut commit = false;
        let mut cancel = false;
        let attrs_for_combo = xsd_attrs.clone();
        let dismiss = show_modal(ctx, "match_rule", 360.0, |ui| {
            let heading = if buf.edit_idx.is_some() { "Edit identification rule" } else { "New identification rule" };
            ui.label(RichText::new(heading).size(crate::HEADING_SIZE).strong().color(p().text));
            ui.add_space(SPACE_2);

            ui.label(RichText::new("attribute").size(crate::LABEL_SIZE).color(p().text_dim));
            if !attrs_for_combo.is_empty() {
                let cur = attrs_for_combo.iter().position(|a| a == &buf.attr);
                if let Some(picked) = crate::widgets::styled_combo(
                    ui,
                    "match_attr_sel",
                    320.0,
                    crate::BODY_SIZE, // 13, like every other combo/field (was GRID_SIZE 12)
                    true,
                    cur,
                    &attrs_for_combo,
                ) {
                    buf.attr = attrs_for_combo[picked].clone();
                }
            } else {
                ui.add_sized(
                    egui::vec2(320.0, crate::theme::FIELD_H),
                    egui::TextEdit::singleline(&mut buf.attr)
                        // shared field inset + vertical centring (theme.rs)
                        .margin(crate::theme::field_margin())
                        .vertical_align(egui::Align::Center)
                        .hint_text("e.g. schemaVersion"),
                );
            }
            ui.label(
                RichText::new("attribute of the root element (from XSD)")
                    .size(10.0)
                    .color(p().disabled),
            );
            ui.add_space(SPACE_2);

            ui.label(RichText::new("values (comma-separated; empty = presence only)").size(crate::LABEL_SIZE).color(p().text_dim));
            ui.add_sized(
                egui::vec2(320.0, crate::theme::FIELD_H),
                egui::TextEdit::singleline(&mut buf.values)
                    // shared field inset + vertical centring (theme.rs)
                    .margin(crate::theme::field_margin())
                    .vertical_align(egui::Align::Center)
                    .hint_text("e.g. 5.1"),
            );
            ui.add_space(SPACE_2);

            if let Some(err) = &buf.error {
                ui.label(RichText::new(err).size(crate::LABEL_SIZE).color(p().danger));
                ui.add_space(SPACE_2);
            }

            ui.horizontal(|ui| {
                let bw = uniform_button_width(ui, &["Cancel", "Save"]);
                ui.add_space(360.0 - 2.0 * bw - 8.0);
                if secondary_button_w(ui, "Cancel", true, bw) {
                    cancel = true;
                }
                ui.add_space(SPACE_2);
                let lbl = if buf.edit_idx.is_some() { "Save" } else { "Add" };
                if primary_button(ui, lbl, !buf.attr.trim().is_empty()) {
                    commit = true;
                }
            });
        });
        let _ = dismiss;

        if cancel {
            return;
        }
        if commit {
            if buf.attr.trim().is_empty() {
                buf.error = Some("attribute cannot be empty.".to_owned());
                self.return_match_modal(model_id, buf);
                return;
            }
            let new_rule = xmlmodel::MatchRule {
                attr: buf.attr.trim().to_owned(),
                values: buf.values
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect(),
            };
            let res = self.cur_mut().and_then(|t| match &mut t.kind {
                TabKind::ModelEditor(m) => {
                    match buf.edit_idx {
                        Some(i) => {
                            if i < m.working.manifest.r#match.rules.len() {
                                m.working.manifest.r#match.rules[i] = new_rule;
                                m.dirty = true;
                                Some(Ok(()))
                            } else {
                                Some(Err("rule not found".to_owned()))
                            }
                        }
                        None => {
                            m.working.manifest.r#match.rules.push(new_rule);
                            m.dirty = true;
                            Some(Ok(()))
                        }
                    }
                }
                _ => None,
            });
            match res {
                Some(Ok(())) => return,
                Some(Err(e)) => {
                    buf.error = Some(e);
                    self.return_match_modal(model_id, buf);
                }
                None => return,
            }
            return;
        }
        self.return_match_modal(model_id, buf);
    }

    fn return_match_modal(&mut self, model_id: String, buf: crate::MatchRuleEditBuf) {
        if let Some(t) = self.cur_mut() {
            if let TabKind::ModelEditor(m) = &mut t.kind {
                if m.id == model_id {
                    m.match_modal = Some(buf);
                }
            }
        }
    }

    /// Rule delete-confirmation modal (validation/identification). The target is the tab's
    /// `pending_delete`; deletion is applied to the working copy ONLY after confirmation (✕ merely requests it).
    pub(crate) fn rule_delete_modal(&mut self, ctx: &egui::Context) {
        let pending = match self.cur() {
            Some(t) => match &t.kind {
                TabKind::ModelEditor(m) => m.pending_delete,
                _ => None,
            },
            None => None,
        };
        let Some(pending) = pending else {
            return;
        };
        // human-readable label of the target (validation rule id / identification rule attribute)
        let (kind_label, target) = match self.cur().and_then(|t| match &t.kind {
            TabKind::ModelEditor(m) => Some(&m.working),
            _ => None,
        }) {
            Some(w) => match pending {
                crate::PendingRuleDelete::Validation(i) => (
                    "validation rule",
                    parse_rules_view(&w.rules).get(i).map(|(_, name)| name.clone()).unwrap_or_default(),
                ),
                crate::PendingRuleDelete::Match(i) => (
                    "identification rule",
                    w.manifest.r#match.rules.get(i).map(|r| r.attr.clone()).unwrap_or_default(),
                ),
            },
            None => return,
        };
        let msg = if target.is_empty() {
            format!("Delete {kind_label}? This action cannot be undone.")
        } else {
            format!("Delete {kind_label} «{target}»? This action cannot be undone.")
        };
        let out = crate::widgets::confirm_modal(
            ctx,
            "rule_delete",
            crate::widgets::ConfirmKind::Danger,
            "Delete rule",
            &msg,
            "Delete",
            "Cancel",
        );
        if out.cancelled {
            self.clear_pending_delete();
            return;
        }
        if out.confirmed {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    match pending {
                        crate::PendingRuleDelete::Validation(i) => {
                            if remove_rule_at(&mut m.working.rules, i) {
                                m.dirty = true;
                            }
                        }
                        crate::PendingRuleDelete::Match(i) => {
                            if i < m.working.manifest.r#match.rules.len() {
                                m.working.manifest.r#match.rules.remove(i);
                                m.dirty = true;
                            }
                        }
                    }
                    m.pending_delete = None;
                }
            }
        }
        // neither Cancel nor Delete — the modal stays open (we leave pending_delete alone)
    }

    /// Clear the rule-deletion request (Cancel in the confirmation modal).
    fn clear_pending_delete(&mut self) {
        if let Some(t) = self.cur_mut() {
            if let TabKind::ModelEditor(m) = &mut t.kind {
                m.pending_delete = None;
            }
        }
    }
}

/// Replace characters that are invalid in Windows file names with `_`. The model id = the file name.
fn sanitize_filename(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect()
}

/// Take the rule list for rendering: (index, name). Broken JSON → empty list.
fn parse_rules_view(rules_json: &str) -> Vec<(usize, String)> {
    let arr = serde_json::from_str::<serde_json::Value>(rules_json)
        .ok()
        .and_then(|v| v.get("rules").and_then(|r| r.as_array()).cloned())
        .unwrap_or_default();
    arr.iter()
        .enumerate()
        .map(|(i, r)| {
            let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            (i, name)
        })
        .collect()
}

/// Read a rule by index into the edit modal's buffer (filled-in fields). None if
/// the index is out of range or the JSON is broken. `edit_idx = Some(idx)` marks editing.
fn rule_to_buf(rules_json: &str, idx: usize) -> Option<crate::RuleEditBuf> {
    let root = serde_json::from_str::<serde_json::Value>(rules_json).ok()?;
    let arr = root.get("rules").and_then(|r| r.as_array())?;
    let r = arr.get(idx)?;
    let check = r.get("check").cloned().unwrap_or(serde_json::Value::Null);
    Some(crate::RuleEditBuf {
        name: r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
        message: r.get("message").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
        // normalize to exactly "error"/"warn" — so the radio is always selected and the legacy "warning"
        // (which the engine still accepts) isn't saved back outside the UI.
        severity: match r.get("severity").and_then(|v| v.as_str()) {
            Some("warn") | Some("warning") => "warn",
            _ => "error",
        }
        .to_owned(),
        check: serde_json::to_string_pretty(&check).unwrap_or_default(),
        error: None,
        edit_idx: Some(idx),
    })
}

/// A rule's JSON object built from the editor buffer. At the top level — only id/severity/check/description;
/// all the mechanics (codes/block/condition/…) live INSIDE check. `check` is already parsed by the caller.
fn rule_json(rule: &crate::RuleEditBuf, check: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "name": rule.name.trim(),
        "severity": rule.severity.trim(),
        "check": check,
        // we keep message AS IS (no trim) — trailing spaces and line breaks are significant.
        "message": rule.message,
    })
}

/// Replace the rule at the given index in the `rules` array with the buffer's values. Returns true on success.
fn replace_rule_at(rules_json: &mut String, idx: usize, rule: &crate::RuleEditBuf) -> Result<(), String> {
    let check = serde_json::from_str::<serde_json::Value>(&rule.check)
        .map_err(|e| format!("check is not valid JSON: {e}"))?;
    let mut root = serde_json::from_str::<serde_json::Value>(rules_json)
        .unwrap_or_else(|_| serde_json::json!({"rules": []}));
    if root.get("rules").is_none() {
        root["rules"] = serde_json::json!([]);
    }
    let arr = root.get_mut("rules").and_then(|r| r.as_array_mut()).ok_or("rules: not an array")?;
    if idx >= arr.len() {
        return Err("rule not found".to_owned());
    }
    arr[idx] = rule_json(rule, check);
    *rules_json = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    Ok(())
}

/// Remove the rule at the given index in the `rules` array. Returns true if removed. Re-serializes
/// the JSON prettily (2-space indent) — to keep the diff readable.
fn remove_rule_at(rules_json: &mut String, idx: usize) -> bool {
    let mut root = match serde_json::from_str::<serde_json::Value>(rules_json) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let rules = match root.get_mut("rules").and_then(|r| r.as_array_mut()) {
        Some(a) => a,
        None => return false,
    };
    if idx >= rules.len() {
        return false;
    }
    rules.remove(idx);
    *rules_json = serde_json::to_string_pretty(&root).unwrap_or_else(|_| rules_json.clone());
    true
}

/// Add a rule to the section's `rules` array. `check` is parsed from the buffer text; on a parse error
/// it returns an error message. Re-serializes the JSON prettily.
fn add_rule_to_json(rules_json: &mut String, rule: &crate::RuleEditBuf) -> Result<(), String> {
    let check = serde_json::from_str::<serde_json::Value>(&rule.check)
        .map_err(|e| format!("check is not valid JSON: {e}"))?;
    let mut root = serde_json::from_str::<serde_json::Value>(rules_json)
        .unwrap_or_else(|_| serde_json::json!({"rules": []}));
    if root.get("rules").is_none() {
        root["rules"] = serde_json::json!([]);
    }
    root.get_mut("rules")
        .and_then(|r| r.as_array_mut())
        .ok_or("rules: not an array")?
        .push(rule_json(rule, check));
    *rules_json = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    Ok(())
}

/// Bold section heading in the model tab body — one shared style (BODY_SIZE, strong, text).
fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(crate::BODY_SIZE).strong().color(p().text));
}

/// A multiline field of FIXED height (`height`, the same for all calls) with a BUILT-IN
/// scroll inside the border — like the main editor. The border/background is drawn by an outer Frame (like the id field),
/// the TextEdit itself uses `frame(false)`; the scrollbar is inside the border. Text width = available MINUS
/// the scrollbar width, and it's reserved ALWAYS → line wrapping accounts for the scroll immediately (no
/// "jump" when it appears). `style_scrollbar` overwrites the widget visuals for the scrollbar, so
/// inside the content we restore `style_modal_widgets` (otherwise the text is painted the scrollbar color).
fn multiline_field(
    ui: &mut egui::Ui,
    text: &mut String,
    salt: &str,
    height: f32,
    rows: usize,
    font: Option<egui::FontId>,
) {
    const BAR_W: f32 = 8.0; // width of the built-in scrollbar (style_scrollbar: bar_width = 8)
    egui::Frame::new()
        .fill(p().field_bg)
        .stroke(egui::Stroke::new(1.0 / ui.ctx().pixels_per_point(), p().border_strong)) // crisp 1 device px
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CONTROL))
        // inner_margin = 0: the ScrollArea fills the border entirely → the scrollbar is PINNED to the right edge and
        // runs to the top/bottom edge (like the main editor). The text padding is provided by the TextEdit itself.
        .inner_margin(egui::Margin::ZERO)
        .show(ui, |ui| {
            let inner_w = ui.available_width();
            ui.set_min_height(height);
            style_scrollbar(ui);
            egui::ScrollArea::vertical()
                .id_salt(salt)
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    theme::style_modal_widgets(ui);
                    let mut te = egui::TextEdit::multiline(text)
                        // The outer Frame draws the border; this inner frame is invisible (no fill /
                        // stroke) and exists only to carry the shared text inset. NOTE: once a custom
                        // frame is set, egui IGNORES `TextEdit::margin`, so the inset MUST live on the
                        // frame's inner_margin (a bare `.margin()` here would silently do nothing —
                        // that's why Description used to hug the left edge).
                        .frame(egui::Frame::new().inner_margin(egui::Margin {
                            left: crate::theme::TEXT_INSET as i8,
                            right: 4, // tight on the right for the scrollbar gutter
                            top: 4,
                            bottom: 4,
                        }))
                        .desired_width(inner_w - BAR_W)
                        .desired_rows(rows);
                    if let Some(f) = font {
                        te = te.font(f);
                    }
                    ui.add(te);
                });
        });
}

/// A single row of the rule list. Split into TWO non-overlapping areas so clicks don't conflict:
/// on the left — a wide "drill into the rule" zone (clickable AS A WHOLE, not only on the text; returns
/// `.0`), on the right — a separate ✕ delete column (returns `.1`). The whole row highlights on hover
/// (a clickability affordance); the text is truncated to the zone width. The rectangles are computed by hand
/// (the `manager_row` style); `id` must be unique per row.
fn rule_list_row(
    ui: &mut egui::Ui,
    inner_w: f32,
    id: egui::Id,
    text: &str,
    remove_tip: &str,
) -> (bool, bool) {
    const COL_W: f32 = 24.0; // width of the ✕ column
    let (rect, row_resp) =
        ui.allocate_exact_size(egui::vec2(inner_w, crate::theme::CONTROL_H), egui::Sense::hover());
    // hover background for the whole row
    if row_resp.contains_pointer() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
            p().hover,
        );
    }
    let x_left = rect.right() - COL_W;
    let label_rect = egui::Rect::from_min_max(rect.min, egui::pos2(x_left, rect.bottom()));
    let x_rect = egui::Rect::from_min_max(egui::pos2(x_left, rect.top()), rect.max);

    // LEFT zone — "drill in": clickable as a whole (not only on the text glyphs). The cursor is
    // the regular arrow (we don't use the pointing hand in the app).
    let open = ui.interact(label_rect, id.with("open"), egui::Sense::click());
    // label text, truncated to the zone width (+ a hard clip on narrow windows)
    let budget =
        (((label_rect.width() - crate::theme::TEXT_INSET) / (crate::BODY_SIZE * 0.5)).max(4.0)) as usize;
    ui.painter().with_clip_rect(label_rect).text(
        egui::pos2(label_rect.left() + crate::theme::TEXT_INSET, label_rect.center().y),
        egui::Align2::LEFT_CENTER,
        ellipsize(text, budget),
        egui::FontId::proportional(crate::BODY_SIZE),
        p().text,
    );

    // RIGHT column — delete: a separate area with its own hover box (like qbtn_sm).
    let x = ui.interact(x_rect, id.with("remove"), egui::Sense::click());
    let t = ui.ctx().animate_bool(x.id, x.hovered());
    if t > 0.0 {
        ui.painter().rect_filled(
            x_rect.shrink2(egui::vec2(1.0, 2.0)),
            egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
            p().acc_bg.gamma_multiply(t),
        );
    }
    ui.painter().text(
        x_rect.center(),
        egui::Align2::CENTER_CENTER,
        ic::CLOSE,
        egui::FontId::proportional(15.0),
        p().danger,
    );
    let x = x.on_hover_text(remove_tip);
    (open.clicked(), x.clicked())
}

/// Truncate a string to `max` characters with an ellipsis — for single-line list labels.
fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Compile the model working copy's XSD and pull out candidates for the match predicate:
/// the root element (one) + the root type's attribute names. On a compile error — empty lists
/// (then Identification stays editable text rather than a dropdown).
fn xsd_match_candidates(xsd: &str) -> (Vec<String>, Vec<String>) {
    use crate::xsd::model::TypeRef;
    let schema = match crate::xsd::compile(xsd, "match-candidates") {
        Ok(s) => s,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let root = schema.root_name.clone();
    // attributes of the root type (if it's complex)
    let mut attrs = Vec::new();
    if let TypeRef::Complex(idx) = &schema.root_type {
        if let Some(ct) = schema.complexes.get(*idx) {
            attrs = ct.attrs.iter().map(|a| a.name.clone()).collect();
        }
    }
    (vec![root], attrs)
}
