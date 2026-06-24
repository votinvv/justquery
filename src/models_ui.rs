//! XML Model Manager — левый док (`LeftPanel::Model`): список моделей из `%APPDATA%\JustQuery\models\`,
//! импорт (выбор `.jqmodel` → копирование в папку + перечит реестра), экспорт выбранной модели в
//! файл, удаление. Двойной клик по модели откроет вкладку-описание (`TabKind::ModelEditor`,
//! Этап 3).
//!
//! Все изменения файловой системы идут через перечит реестра (`reload_models`) — менеджер не
//! хранит отдельного состояния, только индекс выбранной строки.

use crate::theme::{self, p};
use crate::widgets::{
    close_x, empty_hint, focus_field, island_box, island_shadow_under, manager_row, primary_button,
    qbtn_off_sm, qbtn_sm, secondary_button, secondary_button_w, show_modal,
    style_scrollbar, subbar, uniform_button_width,
};
use crate::{dialog, ic, xmlmodel, JustQueryApp, LeftPanel, TabKind};
use crate::{RADIUS_ISLAND, SPACE_2, SPACE_3, SPACE_4, TABBAR_H};
use eframe::egui;
use egui::{Align, Layout, Margin, RichText};

impl JustQueryApp {
    /// Перечитать реестр из папки моделей и поднять поколение. Звать после импорта/удаления/создания/
    /// сохранения. Заодно переназначает модель открытым XML-вкладкам (реестр изменился) — иначе
    /// вкладка держала бы устаревший/пустой `model_id` (кнопка Inspect активна, но no-op).
    pub(crate) fn reload_models(&mut self) {
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
        let mut do_create = false; // + → открыть модалку создания новой модели
        let mut do_export = false; // экспорт выбранной модели
        let mut do_delete = false; // удалить выбранную модель
        let mut close_panel = false;
        let mut open_idx: Option<usize> = None; // двойной клик → открыть вкладку модели (Этап 3)

        let saved_style = crate::widgets::hush_resize_line(ui);
        egui::Panel::left("left_panel")
            .resizable(true)
            // диапазон ширины — ровно как у Connection/Metadata менеджеров (единый размах)
            .default_size(220.0)
            .size_range(196.0..=460.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(Margin::ZERO))
            .show_inside(ui, |ui| {
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
                    .show_inside(ui, |ui| {
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
                // toolbar: New (+, модалка создания) · Import (OPEN, файл) · Export (selected) · Delete (selected)
                subbar(ui, "modelmgr_toolbar", |ui| {
                    if qbtn_sm(ui, ic::PLUS, p().text, "New model").clicked() {
                        do_create = true;
                    }
                    if qbtn_sm(ui, ic::OPEN, p().text, "Import model…").clicked() {
                        do_import = true;
                    }
                    if self.model_sel.is_none() {
                        qbtn_off_sm(ui, ic::SAVE, "Export (select a model)");
                    } else if qbtn_sm(ui, ic::SAVE, p().text, "Export selected model…").clicked() {
                        do_export = true;
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
                        left: crate::CHROME_GUTTER as i8, // единый gutter (край окна)
                        right: crate::CHROME_GUTTER as i8,
                        top: 1,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| {
                        // снимок списка для отрисовки (имя + intact-флаг). id не показываем — плоский
                        // список, как Connection Manager; не-intact помечаем «⚠» в имени строки.
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
                        // белый work-area island (список моделей), вертикально прокручиваемый —
                        // пиксель-в-пиксель тот же каркас, что у Database/Metadata Manager:
                        // Frame(ivory + island_shadow) → ScrollArea(manager_row) → crisp_border
                        // поверх (selection/hover доходит до углов без белых треугольников).
                        let island = egui::Frame::new()
                            .fill(p().ivory)
                            .corner_radius(egui::CornerRadius::same(crate::RADIUS_ISLAND))
                            .shadow(crate::theme::island_shadow())
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                let clip = ui.max_rect();
                                ui.set_clip_rect(clip);
                                style_scrollbar(ui);
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
                                            // плоская строка менеджера (иконка + id); выбранный →
                                            // tint, как в Database/Metadata Manager.
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
                                                    .size(11.0)
                                                    .color(p().danger),
                                            );
                                            for e in &errors {
                                                ui.label(RichText::new(e).size(10.0).color(p().danger));
                                            }
                                        }
                                    });
                            });
                        // чёткая граница поверх острова — рамка, единственная со всеми менеджерами.
                        crate::widgets::crisp_border(
                            ui.painter(),
                            island.response.rect,
                            p().border_strong,
                        );
                    });
            });
        let _ = saved_style;

        // действия — после отрисовки, чтобы не держать borrow ui дольше нужного
        if do_create {
            // открыть модалку с пустым буфером (если ещё не открыта)
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
        if do_export {
            self.export_selected_model();
        }
        if do_delete {
            // открыть модалку подтверждения удаления (вместо немедленного удаления)
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

    /// Открыть вкладку-редактор выбранной в менеджере модели (двойной клик по строке). Вкладка
    /// получает рабочую копию модели (`working`) — правки накапливаются в ней, не в реестре, до
    /// явного сохранения.
    pub(crate) fn open_model_tab(&mut self) {
        let Some(idx) = self.model_sel else {
            return;
        };
        let Some(model) = self.models.models().get(idx) else {
            return;
        };
        let id = model.manifest.id.clone();
        // если вкладка для этой модели уже открыта — переключиться на неё
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
        // title вкладки = id модели (как у коннектов: title = name). Файл = <id>.jqmodel.
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

    /// Сохранить рабочую копию модели вкладки на диск (`<models>/<id>.jqmodel`) + перезагрузить
    /// реестр. Имя файла = id; если id менялся, старый файл удаляется. Контрольная сумма
    /// пересчитывается при сериализации — модель снова `intact`.
    pub(crate) fn save_model_tab(&mut self) {
        // вытащить working copy + старый путь (до reload, пока вкладка ещё держит ссылку)
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
        // если id сменился — удалить старый файл (перезапись того же id просто перетрётся)
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
        // сбросить dirty + синхронизировать working с перезагруженным реестром (свежая intact-копия)
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

    /// Тело вкладки-редактора модели: метаданные (read-only), XSD (кнопка замены выбором из файлов),
    /// построчный список правил валидации (add/remove). Правки накапливаются в `working` копии
    /// вкладки; сохранение — через `save_model_tab` (Ctrl+S / тулбар, когда `dirty`).
    pub(crate) fn model_tab(&mut self, ui: &mut egui::Ui) {
        // Снять рабочую копию модели с вкладки для отрисовки. Правки применяем через cur_mut
        // отдельными операциями, чтобы не держать borrow self во время рендера.
        let (model_id, working_snapshot, dirty, rule_modal_open, match_modal_open) = match self.cur()
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

        // Локальный редактируемый буфер description: инициализируется из снимка, после рендера
        // сравнивается и пишется обратно в working через cur_mut (если изменился).
        let mut desc_buf = working_snapshot.manifest.description.clone();
        // Снимок правил идентификации для отрисовки списка (правки — через модалку, не в кадре).
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

        // Кнопка XSD: если XSD отсутствует — «Выбрать XSD», иначе «Заменить XSD» + «Очистить».
        // Замена — через нативный диалог выбора файла (можно вызывать многократно, дописывая include-файлы).
        let mut do_export = false;
        let mut do_close = false;
        let mut ask_rule_delete: Option<usize> = None; // ✕ validation-правила → подтверждение удаления
        let mut edit_rule_idx: Option<usize> = None; // редактировать validation-правило (клик по строке)
        let mut open_rule_modal = false; // открыть модалку добавления validation-правила
        let mut ask_match_delete: Option<usize> = None; // ✕ правила идентификации → подтверждение удаления
        let mut edit_match_idx: Option<usize> = None; // редактировать правило идентификации (клик)
        let mut open_match_modal = false; // открыть модалку добавления правила идентификации

        // тело — каноничный silver sheet + data-island + scroll (как About/Scan).
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p().panel2).inner_margin(self.island_margin()))
            .show_inside(ui, |ui| {
                let sheet = ui.max_rect();
                island_shadow_under(ui.painter(), sheet);
                island_box(ui.painter(), sheet, p().data_bg, RADIUS_ISLAND);
                style_scrollbar(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Контент-остров: общий левый край, единый правый край (description, ✕ правил,
                        // кнопки футера). Ширина АДАПТИВНАЯ: не больше дизайн-максимума (620), но всегда
                        // влезает в доступную ширину — при сужении окна контент не уходит за край (без
                        // горизонтального скролла). inner_w = полезная ширина внутри Frame (за вычетом
                        // padding 18×2); по ней кап-аем max_width, чтобы правый край был единым.
                        // ФИКСИРОВАННАЯ ширина контента (без адаптива — менявшаяся ширина выглядела
                        // чужеродно): подобрана под минимальную ширину окна (1024) при левой панели в
                        // максимуме (460), чтобы Frame (inner_w + padding 18×2) влезал и правый
                        // столбец ✕ не уезжал за край. На широких окнах остров просто прижат влево.
                        let inner_w: f32 = 500.0;
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(18, 16))
                            .show(ui, |ui| {
                                theme::style_modal_widgets(ui);
                                ui.set_max_width(inner_w);
                                // header: id (bold) + intact/dirty-статус
                                ui.label(
                                    RichText::new(&working_snapshot.manifest.id)
                                        .font(theme::ui_bold_font(18.0))
                                        .color(p().text),
                                );
                                if !working_snapshot.intact {
                                    ui.label(
                                        RichText::new(
                                            "Контрольная сумма не совпадает — модель изменена \
                                             вне редактора.",
                                        )
                                        .size(crate::GRID_SIZE)
                                        .color(p().danger),
                                    );
                                }
                                ui.add_space(SPACE_3);

                                // ---- XSD schema (read-only, фундамент модели) ----
                                // XSD задаётся при создании модели и не меняется: это «фундамент»,
                                // замена/очистка готовой модели нецелесообразна (легче создать
                                // новую). Поэтому здесь — только статус, без Replace/Clear.
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

                                // описание (редактируемое) — фикс-высота + внутренний скролл, рамка/
                                // фон/цвет как у обычного поля (см. multiline_field).
                                section_label(ui, "Description");
                                multiline_field(ui, &mut desc_buf, "model_desc", 60.0, 3, None);
                                ui.add_space(SPACE_3);

                                // ---- Identification rules: список правил (клик = edit, ✕ = remove) ----
                                // Каждое правило = «атрибут корневого элемента присутствует / ∈ значений».
                                // Матч документа = все правила (AND). Атрибуты берутся из XSD (dropdown
                                // в модалке). Add — плюсик у заголовка; edit — клик по строке.
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

                                // ---- Правила валидации: построчный список (клик = редактирование) ----
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
                                            "(без имени)".to_owned()
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
                                ui.add_space(SPACE_4);

                                // footer-действия: Save · Export · Close. Кнопки прижаты к правому
                                // краю INNER_W (единая линия с ✕ правил и правым краем Description).
                                // Единая ширина кнопок. Add rule — плюсик у заголовка секции.
                                ui.horizontal(|ui| {
                                    ui.set_min_width(inner_w);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let bw =
                                                uniform_button_width(ui, &["Save", "Export", "Close"]);
                                            let mut do_save = false;
                                            // right_to_left: первая в коде = самая правая на экране.
                                            if secondary_button_w(ui, "Close", true, bw) {
                                                do_close = true;
                                            }
                                            ui.add_space(SPACE_2);
                                            if secondary_button_w(ui, "Export", true, bw) {
                                                do_export = true;
                                            }
                                            ui.add_space(SPACE_2);
                                            if secondary_button_w(ui, "Save", dirty, bw) {
                                                do_save = true;
                                            }
                                            if do_save {
                                                self.save_model_tab();
                                            }
                                        },
                                    );
                                });
                            }); // close Frame::show
                    }); // close ScrollArea
            });

        // ---- применение правок к working копии вкладки ----
        // Description: пишем обратно, если буфер изменился относительно снимка.
        let desc_changed = desc_buf != working_snapshot.manifest.description;
        if desc_changed {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    m.working.manifest.description = desc_buf.clone();
                    m.dirty = true;
                }
            }
        }
        // ✕ правила идентификации → запросить подтверждение (само удаление — в rule_delete_modal)
        if let Some(idx) = ask_match_delete {
            if let Some(t) = self.cur_mut() {
                if let TabKind::ModelEditor(m) = &mut t.kind {
                    m.pending_delete = Some(crate::PendingRuleDelete::Match(idx));
                }
            }
        }
        // открыть модалку добавления правила идентификации
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
        // редактировать правило идентификации — заполнить буфер из существующего правила
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
        // ✕ правила валидации → запросить подтверждение (само удаление — в rule_delete_modal)
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
        // редактирование существующего правила: заполнить буфер значениями правила по индексу
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

        // модалка добавления/редактирования правила (рисуется поверх, когда буфер открыт)
        if rule_modal_open || open_rule_modal {
            self.rule_edit_modal(ui.ctx());
        }
        if match_modal_open || open_match_modal {
            self.match_rule_edit_modal(ui.ctx());
        }
        // подтверждение удаления правила (валидации/идентификации), если запрошено через ✕
        self.rule_delete_modal(ui.ctx());

        if do_export {
            // экспорт рабочей копии: найти индекс в реестре по id и звать существующий путь.
            self.model_sel = self.models.models().iter().position(|m| m.manifest.id == model_id);
            self.export_selected_model();
        }
        if do_close {
            // закрыть активную вкладку — через request_close_tab, чтобы при несохранённых
            // правках (dirty) показалась модалка подтверждения, как у крестика на вкладке.
            self.request_close_tab(self.active_tab);
        }
    }

    /// Импорт `.jqmodel` через нативный диалог → копирование в папку моделей → reload реестра.
    fn import_model(&mut self) {
        let Some(src) = dialog::open_file() else {
            return;
        };
        // читать + валидировать как модель до копирования, чтобы не плодить битые файлы
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
        // Контрольная сумма обязательна и должна биться. Иначе повреждённый/изменённый вручную файл
        // молча получил бы свежую валидную сумму при пересериализации ниже — импорт стал бы каналом
        // «переблагословления» подделки. Отклоняем такие файлы с явной ошибкой.
        if !model.intact {
            let why = if model.checksum.is_empty() {
                "в файле нет контрольной суммы (---checksum---)"
            } else {
                "контрольная сумма не совпадает — файл повреждён или изменён вручную"
            };
            self.error_modal = Some(format!("Import rejected: {why}"));
            return;
        }
        // целевое имя файла = id модели; создать папку, если её нет
        let dir = crate::models_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error_modal = Some(format!("Cannot create models dir: {e}"));
            return;
        }
        let dest = dir.join(format!("{}.jqmodel", sanitize_filename(&model.manifest.id)));
        // сериализуем заново — контрольная сумма гарантированно бьётся
        if let Err(e) = xmlmodel::save_file(&model, &dest) {
            self.error_modal = Some(format!("Import write failed: {e}"));
            return;
        }
        self.reload_models();
        // выбрать только что импортированную
        self.model_sel = self
            .models
            .models()
            .iter()
            .position(|m| m.manifest.id == model.manifest.id);
    }

    /// Экспорт выбранной модели в файл через нативный диалог сохранения.
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

    /// Удалить модель по id с диска + reload реестра. Вызывается из модалки подтверждения
    /// (`delete_model_modal`), после того как пользователь подтвердил удаление.
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
        // закрыть вкладку редактора этой модели, если открыта
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| matches!(&t.kind, TabKind::ModelEditor(m) if m.id == id))
        {
            self.close_tab(i);
            // close_tab мог сместить индексы — но мы всё равно перезагружаем выделение
        }
        self.model_sel = None;
        self.reload_models();
    }

    /// Модалка подтверждения удаления модели: «Удалить модель «<id>»? Действие необратимо.»
    /// с кнопками Cancel / Delete (destructive). Удаление — только после подтверждения.
    pub(crate) fn delete_model_modal(&mut self, ctx: &egui::Context) {
        let Some(id) = self.model_delete_confirm.take() else {
            return;
        };
        let mut do_delete = false;
        let mut cancel = false;
        let id_disp = id.clone();
        show_modal(ctx, "model_delete", 340.0, |ui| {
            ui.label(
                RichText::new("Delete XML model")
                    .size(14.0)
                    .strong()
                    .color(p().text),
            );
            ui.add_space(SPACE_2);
            ui.label(
                RichText::new(format!(
                    "Удалить модель «{id_disp}»? Файл .jqmodel будет удалён безвозвратно."
                ))
                .size(crate::BODY_SIZE)
                .color(p().text),
            );
            ui.add_space(SPACE_2);
            ui.horizontal(|ui| {
                let bw = uniform_button_width(ui, &["Cancel", "Delete"]);
                ui.add_space(340.0 - 2.0 * bw - 8.0);
                if secondary_button_w(ui, "Cancel", true, bw) {
                    cancel = true;
                }
                ui.add_space(SPACE_2);
                if crate::widgets::destructive_button_w(ui, "Delete", true, bw) {
                    do_delete = true;
                }
            });
        });
        if cancel {
            return; // закрыта без удаления (id выброшен)
        }
        if do_delete {
            self.delete_model_by_id(&id);
            return;
        }
        // модалка остаётся открытой
        self.model_delete_confirm = Some(id);
    }

    /// Модалка создания новой модели: поля id / name / description + Create/Cancel. Создаёт пустую
    /// модель (без XSD/правил) в `%APPDATA%\JustQuery\models\<id>.jqmodel`, перезагружает реестр и
    /// открывает вкладку-редактор для дальнейшего наполнения.
    pub(crate) fn create_model_modal(&mut self, ctx: &egui::Context) {
        // Закрыта — ничего не рисуем. Проверяем через take, чтобы освободить borrow для обработки.
        let Some(mut buf) = self.model_create.take() else {
            return;
        };
        let mut commit = false;
        let mut cancel = false;
        let mut pick_xsd_for_create = false;
        let dismiss = show_modal(ctx, "model_create", 380.0, |ui| {
            ui.label(
                egui::RichText::new("New XML model")
                    .size(14.0)
                    .strong()
                    .color(p().text),
            );
            ui.add_space(SPACE_2);
            let field_w = ui.available_width(); // поля на всю ширину → правый край не «широкий»

            ui.label(egui::RichText::new("id").size(12.0).color(p().text_dim));
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

            ui.label(egui::RichText::new("description").size(12.0).color(p().text_dim));
            multiline_field(ui, &mut buf.description, "create_desc", 60.0, 3, None);
            ui.add_space(SPACE_2);

            // XSD (обязателен): Select — нативный диалог, можно вызывать многократно (Main + includes
            // склеиваются в один блок). Create дим, пока XSD пуст (модель без схемы не существует).
            ui.label(egui::RichText::new("XSD schema (required)").size(12.0).color(p().text_dim));
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
                    .size(11.0)
                    .color(if buf.xsd.is_empty() { p().warn } else { p().text_dim }),
                );
            });
            ui.add_space(SPACE_2);

            if let Some(err) = &buf.error {
                ui.label(egui::RichText::new(err).size(11.0).color(p().danger));
                ui.add_space(SPACE_2);
            }

            ui.horizontal(|ui| {
                ui.set_min_width(field_w);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Create активен только когда id непуст И XSD выбран (модель без схемы не существует).
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

        // Enter НЕ коммитит: в модалке есть многострочное поле description, где Enter — перевод
        // строки (иначе Enter в описании создавал бы модель и терял остаток текста). Создание —
        // только кнопкой Create; Escape — отмена.
        if dismiss.escape || cancel {
            return; // модалка закрыта (буфер выброшен)
        }

        // выбор XSD через нативный диалог (дописывается к буферу — можно Main + includes)
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
            // валидация id: непустой, нет недопустимых символов, ещё не занят
            let id = buf.id.trim().to_owned();
            if id.is_empty() {
                buf.error = Some("id не может быть пустым.".to_owned());
                self.model_create = Some(buf);
                return;
            }
            if !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
                buf.error = Some(
                    "id содержит недопустимые символы (разрешены латинские буквы, цифры, .-_)."
                        .to_owned(),
                );
                self.model_create = Some(buf);
                return;
            }
            if self.models.models().iter().any(|m| m.manifest.id == id) {
                buf.error = Some(format!("Модель с id «{id}» уже существует."));
                self.model_create = Some(buf);
                return;
            }
            match self.create_model(&id, buf.description.trim(), buf.xsd.clone()) {
                Ok(()) => {
                    self.reload_models();
                    // выбрать и открыть редактор новой модели
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

        // модалка остаётся открытой (продолжаем ввод)
        self.model_create = Some(buf);
    }

    /// Записать новую модель на диск с заданным XSD (обязателен). Контрольная сумма пересчитывается
    /// при сериализации — модель сразу `intact`. codes/rules пустые — пользователь наполнит в редакторе.
    fn create_model(&mut self, id: &str, description: &str, xsd: String) -> Result<(), String> {
        let dir = crate::models_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create models dir: {e}"))?;
        // XSD обязан компилироваться: корневой элемент для идентификации берётся из XSD-схемы, а
        // правила ссылаются на её элементы. Без этого модель молча не сматчится ни с одним
        // документом (match_doc сверяет корень через xsd::compile). Ловим это на создании.
        crate::xsd::compile(&xsd, id).map_err(|e| format!("XSD не компилируется: {e}"))?;
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

    /// Модалка добавления/редактирования правила валидации. Поля правила: id, description (текст
    /// находки), severity (error/warn) и check — JSON-тело со ВСЕЙ механикой правила (`type` +
    /// параметры: codes/block/condition/…). description и check имеют фикс-высоту со скроллом.
    /// Декларативный конструктор условий — отдельная работа; пока даём JSON-редактор check с валидацией.
    pub(crate) fn rule_edit_modal(&mut self, ctx: &egui::Context) {
        // Снять буфер с вкладки (take, чтобы освободить borrow для commit).
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
        // Ширина формы — по min-размеру окна (1024): модалка 560 + поля по 260 на колонку.
        let dismiss = show_modal(ctx, "rule_add", 560.0, |ui| {
            let heading =
                if buf.edit_idx.is_some() { "Edit validation rule" } else { "New validation rule" };
            ui.label(RichText::new(heading).size(14.0).strong().color(p().text));
            ui.add_space(SPACE_3);

            // Поля правила. message и check — фикс-высота с встроенным скроллом. Ширину полей и
            // правый край кнопок берём из доступной ширины модалки. Отступы между блоками — единые
            // (SPACE_3).
            let field_w = ui.available_width();
            const DESC_H: f32 = 60.0;
            const CHECK_H: f32 = 150.0;

            // name — ключевое поле (идентификатор/код находки).
            ui.label(RichText::new("name").size(12.0).color(p().text_dim));
            let _ = focus_field(ui, &mut buf.name, false, field_w);
            ui.add_space(SPACE_3);

            // message — текст находки; фикс-высота + скролл.
            ui.label(RichText::new("message").size(12.0).color(p().text_dim));
            multiline_field(ui, &mut buf.message, "rule_msg", DESC_H, 3, None);
            ui.add_space(SPACE_3);

            // severity — Severity находки (error/warn).
            ui.label(RichText::new("severity").size(12.0).color(p().text_dim));
            ui.horizontal(|ui| {
                ui.radio_value(&mut buf.severity, "error".to_owned(), "error");
                ui.radio_value(&mut buf.severity, "warn".to_owned(), "warn");
            });
            ui.add_space(SPACE_3);

            // check (JSON) — вся механика правила (type + codes/block/scope/condition/…);
            // фикс-высота + скролл, моноширинный.
            ui.label(RichText::new("check (JSON)").size(12.0).color(p().text_dim));
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
                ui.label(RichText::new(err).size(11.0).color(p().danger));
                ui.add_space(SPACE_2);
            }

            ui.horizontal(|ui| {
                ui.set_min_width(field_w);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // right_to_left: первая в коде = самая правая на экране (у правого края контента).
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
            return; // закрыть, буфер выброшен
        }
        if commit {
            if buf.name.trim().is_empty() {
                buf.error = Some("name не может быть пустым.".to_owned());
                self.return_rule_modal(model_id, buf);
                return;
            }
            // добавить ИЛИ заменить правило в working.rules вкладки (по edit_idx)
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
                Some(Ok(())) => return, // успешно — модалка закрыта
                Some(Err(e)) => {
                    buf.error = Some(e);
                    self.return_rule_modal(model_id, buf);
                }
                None => return,
            }
            return;
        }
        // модалка остаётся открытой
        self.return_rule_modal(model_id, buf);
    }

    /// Вернуть буфер модалки правила на вкладку (после неудачного commit или продолжения ввода).
    fn return_rule_modal(&mut self, model_id: String, buf: crate::RuleEditBuf) {
        if let Some(t) = self.cur_mut() {
            if let TabKind::ModelEditor(m) = &mut t.kind {
                if m.id == model_id {
                    m.rule_modal = Some(buf);
                }
            }
        }
    }

    /// Модалка добавления/редактирования правила идентификации: attr (dropdown из атрибутов XSD
    /// корневого типа) + values (запятая-разделённые; пусто = presence). edit_idx = Some(i) → замена.
    pub(crate) fn match_rule_edit_modal(&mut self, ctx: &egui::Context) {
        let model_id = self.cur().and_then(|t| match &t.kind {
            TabKind::ModelEditor(m) => Some(m.id.clone()),
            _ => None,
        });
        let Some(model_id) = model_id else {
            return;
        };
        // снять буфер + xsd-атрибуты (для dropdown attr)
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
            ui.label(RichText::new(heading).size(14.0).strong().color(p().text));
            ui.add_space(SPACE_2);

            ui.label(RichText::new("attribute").size(12.0).color(p().text_dim));
            if !attrs_for_combo.is_empty() {
                let cur = attrs_for_combo.iter().position(|a| a == &buf.attr);
                if let Some(picked) = crate::widgets::styled_combo(
                    ui,
                    "match_attr_sel",
                    320.0,
                    crate::GRID_SIZE,
                    true,
                    cur,
                    &attrs_for_combo,
                ) {
                    buf.attr = attrs_for_combo[picked].clone();
                }
            } else {
                ui.add(
                    egui::TextEdit::singleline(&mut buf.attr)
                        .desired_width(320.0)
                        .hint_text("e.g. schemaVersion"),
                );
            }
            ui.label(
                RichText::new("attribute of the root element (from XSD)")
                    .size(10.0)
                    .color(p().disabled),
            );
            ui.add_space(SPACE_2);

            ui.label(RichText::new("values (comma-separated; empty = presence only)").size(12.0).color(p().text_dim));
            ui.add(
                egui::TextEdit::singleline(&mut buf.values)
                    .desired_width(320.0)
                    .hint_text("e.g. 5.1"),
            );
            ui.add_space(SPACE_2);

            if let Some(err) = &buf.error {
                ui.label(RichText::new(err).size(11.0).color(p().danger));
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
                buf.error = Some("attribute не может быть пустым.".to_owned());
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
                                Some(Err("правило не найдено".to_owned()))
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

    /// Модалка подтверждения удаления правила (валидации/идентификации). Цель — `pending_delete`
    /// вкладки; удаление применяется к рабочей копии ТОЛЬКО после подтверждения (✕ лишь запрашивает).
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
        // человекочитаемая подпись цели (id правила валидации / атрибут правила идентификации)
        let (kind_label, target) = match self.cur().and_then(|t| match &t.kind {
            TabKind::ModelEditor(m) => Some(&m.working),
            _ => None,
        }) {
            Some(w) => match pending {
                crate::PendingRuleDelete::Validation(i) => (
                    "правило валидации",
                    parse_rules_view(&w.rules).get(i).map(|(_, name)| name.clone()).unwrap_or_default(),
                ),
                crate::PendingRuleDelete::Match(i) => (
                    "правило идентификации",
                    w.manifest.r#match.rules.get(i).map(|r| r.attr.clone()).unwrap_or_default(),
                ),
            },
            None => return,
        };
        let mut do_delete = false;
        let mut cancel = false;
        let dismiss = show_modal(ctx, "rule_delete", 360.0, |ui| {
            ui.label(RichText::new("Delete rule").size(14.0).strong().color(p().text));
            ui.add_space(SPACE_2);
            let msg = if target.is_empty() {
                format!("Удалить {kind_label}? Действие необратимо.")
            } else {
                format!("Удалить {kind_label} «{target}»? Действие необратимо.")
            };
            ui.label(RichText::new(msg).size(crate::BODY_SIZE).color(p().text));
            ui.add_space(SPACE_2);
            ui.horizontal(|ui| {
                let bw = uniform_button_width(ui, &["Cancel", "Delete"]);
                ui.add_space(360.0 - 2.0 * bw - 8.0);
                if secondary_button_w(ui, "Cancel", true, bw) {
                    cancel = true;
                }
                ui.add_space(SPACE_2);
                if crate::widgets::destructive_button_w(ui, "Delete", true, bw) {
                    do_delete = true;
                }
            });
        });
        // в модалке нет полей ввода → Esc = Cancel, Enter = Delete (контракт модалок Design Delta §5)
        if cancel || dismiss.escape {
            self.clear_pending_delete();
            return;
        }
        if do_delete || dismiss.enter {
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
        // ни Cancel, ни Delete — модалка остаётся открытой (pending_delete не трогаем)
    }

    /// Сбросить запрос на удаление правила (Cancel в модалке подтверждения).
    fn clear_pending_delete(&mut self) {
        if let Some(t) = self.cur_mut() {
            if let TabKind::ModelEditor(m) = &mut t.kind {
                m.pending_delete = None;
            }
        }
    }
}

/// Заменить символы, недопустимые в именах файлов Windows, на `_`. id модели = имя файла.
fn sanitize_filename(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect()
}

/// Снять список правил для отрисовки: (индекс, name). Битый JSON → пустой список.
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

/// Прочитать правило по индексу в буфер модалки редактирования (заполненные поля). None, если
/// индекс вне диапазона или JSON битый. `edit_idx = Some(idx)` — признак редактирования.
fn rule_to_buf(rules_json: &str, idx: usize) -> Option<crate::RuleEditBuf> {
    let root = serde_json::from_str::<serde_json::Value>(rules_json).ok()?;
    let arr = root.get("rules").and_then(|r| r.as_array())?;
    let r = arr.get(idx)?;
    let check = r.get("check").cloned().unwrap_or(serde_json::Value::Null);
    Some(crate::RuleEditBuf {
        name: r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
        message: r.get("message").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
        // нормализуем к ровно "error"/"warn" — чтобы радио всегда было выбрано и legacy "warning"
        // (движок его ещё принимает) не сохранялся обратно мимо UI.
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

/// JSON-объект правила из буфера редактора. На верхнем уровне — только id/severity/check/description;
/// вся механика (codes/block/condition/…) живёт ВНУТРИ check. `check` уже распарсен вызывающим.
fn rule_json(rule: &crate::RuleEditBuf, check: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "name": rule.name.trim(),
        "severity": rule.severity.trim(),
        "check": check,
        // message сохраняем КАК ЕСТЬ (без trim) — хвостовые пробелы и переносы строк значимы.
        "message": rule.message,
    })
}

/// Заменить правило по индексу в массиве `rules` значениями из буфера. Возвращает true при успехе.
fn replace_rule_at(rules_json: &mut String, idx: usize, rule: &crate::RuleEditBuf) -> Result<(), String> {
    let check = serde_json::from_str::<serde_json::Value>(&rule.check)
        .map_err(|e| format!("check не валидный JSON: {e}"))?;
    let mut root = serde_json::from_str::<serde_json::Value>(rules_json)
        .unwrap_or_else(|_| serde_json::json!({"rules": []}));
    if root.get("rules").is_none() {
        root["rules"] = serde_json::json!([]);
    }
    let arr = root.get_mut("rules").and_then(|r| r.as_array_mut()).ok_or("rules: не массив")?;
    if idx >= arr.len() {
        return Err("правило не найдено".to_owned());
    }
    arr[idx] = rule_json(rule, check);
    *rules_json = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    Ok(())
}

/// Удалить правило по индексу в массиве `rules`. Возвращает true, если удалено. Пересериализует
/// JSON красиво (2-space indent) — чтобы diff оставался читаемым.
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

/// Добавить правило в массив `rules` секции. `check` парсится из текста буфера; при ошибке парсинга
/// возвращает сообщение об ошибке. Пересериализует JSON красиво.
fn add_rule_to_json(rules_json: &mut String, rule: &crate::RuleEditBuf) -> Result<(), String> {
    let check = serde_json::from_str::<serde_json::Value>(&rule.check)
        .map_err(|e| format!("check не валидный JSON: {e}"))?;
    let mut root = serde_json::from_str::<serde_json::Value>(rules_json)
        .unwrap_or_else(|_| serde_json::json!({"rules": []}));
    if root.get("rules").is_none() {
        root["rules"] = serde_json::json!([]);
    }
    root.get_mut("rules")
        .and_then(|r| r.as_array_mut())
        .ok_or("rules: не массив")?
        .push(rule_json(rule, check));
    *rules_json = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    Ok(())
}

/// Bold-заголовок секции в теле вкладки модели — единый стиль (BODY_SIZE, strong, text).
fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(crate::BODY_SIZE).strong().color(p().text));
}

/// Многострочное поле ФИКСИРОВАННОЙ высоты (`height`, одинаковой у всех вызовов) с ВСТРОЕННЫМ
/// скроллом внутри рамки — как в главном редакторе. Рамку/фон рисует внешний Frame (как у поля id),
/// сам TextEdit идёт `frame(false)`; скроллбар — внутри рамки. Ширина текста = доступная МИНУС
/// ширина скроллбара, причём резервируется ВСЕГДА → перенос строк сразу учитывает скролл (нет
/// «прыжка» при его появлении). `style_scrollbar` затирает визуалы виджетов под скроллбар, поэтому
/// внутри контента возвращаем `style_modal_widgets` (иначе текст красится цветом скроллбара).
fn multiline_field(
    ui: &mut egui::Ui,
    text: &mut String,
    salt: &str,
    height: f32,
    rows: usize,
    font: Option<egui::FontId>,
) {
    const BAR_W: f32 = 8.0; // ширина встроенного скроллбара (style_scrollbar: bar_width = 8)
    egui::Frame::new()
        .fill(p().field_bg)
        .stroke(egui::Stroke::new(1.0, p().border_strong))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CONTROL))
        // inner_margin = 0: ScrollArea заполняет рамку целиком → скроллбар ПРИЖАТ к правому краю и
        // тянется до верхнего/нижнего края (как в главном редакторе). Отступ текста даёт сам TextEdit.
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
                        .frame(egui::Frame::NONE) // рамку рисует внешний Frame; TextEdit без своей
                        .margin(egui::Margin { left: 8, right: 4, top: 4, bottom: 4 }) // отступ текста
                        .desired_width(inner_w - BAR_W)
                        .desired_rows(rows);
                    if let Some(f) = font {
                        te = te.font(f);
                    }
                    ui.add(te);
                });
        });
}

/// Одна строка списка правил. Делится на ДВЕ непересекающиеся области, чтобы клики не конфликтовали:
/// слева — широкая зона «провалиться в правило» (кликабельна ЦЕЛИКОМ, не только по тексту; возврат
/// `.0`), справа — отдельный столбец ✕ удаления (возврат `.1`). Вся строка подсвечивается на hover
/// (аффорданс кликабельности); текст обрезается по ширине зоны. Прямоугольники считаются вручную
/// (стиль `manager_row`), `id` должен быть уникален на строку.
fn rule_list_row(
    ui: &mut egui::Ui,
    inner_w: f32,
    id: egui::Id,
    text: &str,
    remove_tip: &str,
) -> (bool, bool) {
    const COL_W: f32 = 24.0; // ширина столбца ✕
    let (rect, row_resp) =
        ui.allocate_exact_size(egui::vec2(inner_w, crate::theme::CONTROL_H), egui::Sense::hover());
    // hover-фон всей строки
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

    // ЛЕВАЯ зона — «провалиться»: кликабельна целиком (а не только по глифам текста). Курсор —
    // обычная стрелка (палец в приложении не используем).
    let open = ui.interact(label_rect, id.with("open"), egui::Sense::click());
    // текст метки, обрезанный под ширину зоны (+ жёсткий клип на узких окнах)
    let budget = (((label_rect.width() - 8.0) / (crate::BODY_SIZE * 0.5)).max(4.0)) as usize;
    ui.painter().with_clip_rect(label_rect).text(
        egui::pos2(label_rect.left() + 2.0, label_rect.center().y),
        egui::Align2::LEFT_CENTER,
        ellipsize(text, budget),
        egui::FontId::proportional(crate::BODY_SIZE),
        p().text,
    );

    // ПРАВЫЙ столбец — удаление: отдельная область со своим hover-боксом (как qbtn_sm).
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

/// Обрезать строку до `max` символов с многоточием — для однострочных меток списка.
fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Скомпилировать XSD рабочей копии модели и достать кандидатов для match-предиката:
/// корневой элемент (один) + имена атрибутов корневого типа. При ошибке компиляции — пустые списки
/// (тогда Identification остаётся редактируемым текстом, а не dropdown).
fn xsd_match_candidates(xsd: &str) -> (Vec<String>, Vec<String>) {
    use crate::xsd::model::TypeRef;
    let schema = match crate::xsd::compile(xsd, "match-candidates") {
        Ok(s) => s,
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let root = schema.root_name.clone();
    // атрибуты корневого типа (если он complex)
    let mut attrs = Vec::new();
    if let TypeRef::Complex(idx) = &schema.root_type {
        if let Some(ct) = schema.complexes.get(*idx) {
            attrs = ct.attrs.iter().map(|a| a.name.clone()).collect();
        }
    }
    (vec![root], attrs)
}
