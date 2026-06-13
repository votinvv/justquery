//!
//! Виртуальный редактор кода поверх [`crate::doc::Document`].
//!
//! Рендер — только видимые строки, O(видимых) на кадр; модель — piece table с mmap:
//! файл не загружается целиком, координаты каретки — `(строка, колонка)` в кодовых точках.
//! Подсветка стейтфул: состояние лексера на границах строк кэшируется и лениво
//! продвигается с бюджетом на кадр, чтобы прыжок в конец гигабайтного файла не
//! замораживал UI. Редактор ничего не знает ни о приложении, ни о языке: на каждый кадр
//! приложение передаёт [`EditorCtx`] с колбэками подсветки ([`Highlighter`]) и табуляции,
//! а забирает [`EditorOut`].

use crate::doc::{Document, Pos};
use crate::theme::p;
use crate::{code_font, code_font_regular, CODE_SIZE};
use eframe::egui;
use egui::{CornerRadius, Rect, Stroke};
use std::collections::HashMap;

/// Состояние лексера на границе строки. Непрозрачный байт: семантику задаёт подсветчик
/// проекта; `0` — начальное состояние (как в начале документа).
pub(crate) type LexState = u8;

/// Колбэки подсветки конкретного языка (язык задаёт приложение: SQL/XML).
pub(crate) struct Highlighter<'a> {
    /// Полная разметка строки: (текст, состояние на входе) → (job, состояние на выходе).
    pub line: &'a dyn Fn(&str, LexState) -> (egui::text::LayoutJob, LexState),
    /// Только переход состояния — для ленивого долексирования с бюджетом на кадр.
    pub advance: &'a dyn Fn(&str, LexState) -> LexState,
}

/// Двухпоколенный кэш galley по ключу «состояние + текст строки».
#[derive(Default)]
pub(crate) struct LineCache {
    cur: HashMap<String, std::sync::Arc<egui::Galley>>,
    prev: HashMap<String, std::sync::Arc<egui::Galley>>,
}

impl LineCache {
    const CAP: usize = 8192;

    /// Сбросить оба поколения (на смене темы старый атлас шрифтов мёртв).
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

/// Кэш состояний лексера на началах строк, якорный: `states[i]` — состояние ПЕРЕД
/// строкой `base + i`. При дальнем прыжке (инерционный скролл по гигабайтному файлу)
/// кэш мгновенно переякоривается на видимое окно (начало строки считается базовым
/// состоянием — для построчных форматов это верно практически всегда), вместо
/// лексирования миллионов строк.
pub(crate) struct LexCache {
    base: usize,
    states: Vec<LexState>, // непустой; states[0] — состояние перед строкой base
    doc_generation: u64,
}

impl Default for LexCache {
    fn default() -> Self {
        Self { base: 0, states: vec![0], doc_generation: u64::MAX }
    }
}

impl LexCache {
    /// Бюджет долексирования на кадр (строк) — заметно меньше кадра даже на длинных строках.
    const STEP_BUDGET: usize = 2048;
    /// Прыжок дальше этого — переякориваемся, а не лексируем подряд.
    const REANCHOR_GAP: usize = 4096;

    fn reanchor(&mut self, line: usize) {
        self.base = line;
        self.states.clear();
        self.states.push(0);
    }

    /// Инвалидировать состояния, начиная со строки, где произошла правка.
    /// Состояние ПЕРЕД строкой `line` не меняется, поэтому хвост валиден до `line` включительно.
    pub fn invalidate_from(&mut self, line: usize) {
        if line < self.base {
            self.reanchor(line);
        } else {
            let keep = (line - self.base + 1).min(self.states.len());
            self.states.truncate(keep.max(1));
        }
    }

    /// Состояние перед строкой `line`. Если до неё ещё не долексировали в рамках бюджета —
    /// возвращает (0, false): приближение, и вызывающий должен запросить repaint.
    fn state_at(
        &mut self,
        doc: &mut Document,
        line: usize,
        advance: &dyn Fn(&str, LexState) -> LexState,
    ) -> (LexState, bool) {
        if doc.generation != self.doc_generation {
            // содержимое сменилось целиком (открытие/undo формата) — сброс
            self.base = 0;
            self.states.clear();
            self.states.push(0);
            self.doc_generation = doc.generation;
        }
        // запрос выше якоря (скролл вверх) или дальний прыжок вниз → переякорь
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
            let st = *self.states.last().expect("непустой");
            let text = doc.get_line(n);
            self.states.push(advance(&text, st));
            budget -= 1;
        }
        if line < self.base + self.states.len() {
            (self.states[line - self.base], true)
        } else {
            (0, false) // приближение до следующего кадра
        }
    }
}

const PAD_L: f32 = 6.0; // зазор между gutter и текстом
const GUT_R: f32 = 8.0; // правый отступ gutter (номер → разделитель)
const GUT_L: f32 = 6.0;

/// Класс «символ слова» — общий для редактора и поиска.
pub(crate) fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Состояние редактора вкладки: каретка, выделение, blink, sticky-колонка.
/// Текст живёт в [`Document`]; здесь только координаты.
pub(crate) struct EditorState {
    caret: Pos,
    anchor: Pos,
    blink_t0: f64,
    blink_caret: Pos,
    pref_col: Option<usize>,
    scroll_to_caret: bool,
    /// Вертикальная прокрутка в пикселях от начала документа. f64 — на гигабайтных файлах
    /// полотно достигает 5×10⁸ px, где у f32 шаг представления 32px (наезды строк/пустоты).
    scroll_y: f64,
    /// Горизонтальная прокрутка — тоже f64 (та же арифметика, что и по вертикали).
    scroll_x: f64,
    /// Акцентная подсветка строки с ошибкой форматирования: (строка, время установки).
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
            flash_line: None,
        }
    }
}

impl EditorState {
    #[allow(dead_code)] // часть API: эта сборка не читает каретку напрямую
    pub fn caret(&self) -> Pos {
        self.caret
    }

    pub fn has_sel(&self) -> bool {
        self.caret != self.anchor
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

    /// Заменить выделение (или вставить в каретку) текстом `s`.
    /// Переводы строки в `s` должны быть уже нормализованы к EOL документа.
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

    /// Внешний прыжок/выделение: каретка в `caret`, якорь в `anchor` (равны — без выделения),
    /// обе позиции клампятся; прокрутка к каретке на следующем кадре.
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
        let (l, c) = self.caret;
        let chars: Vec<char> = doc.get_line(l).chars().collect();
        let n = chars.len();
        let pos = if right {
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
        };
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

    /// Home: первый непробельный символ, затем колонка 0 (smart Home).
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

    /// Поставить каретку: строка вычислена вызывающим (в f64, точно), колонка — по пикселю.
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

    /// Двойной клик: выделить слово в строке `line` под пикселем `rel_x`.
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

/// Всё, что редактору нужно от приложения на один кадр.
pub(crate) struct EditorCtx<'a> {
    pub doc: &'a mut Document,
    pub ed: &'a mut EditorState,
    pub lex: &'a mut LexCache,
    pub line_cache: &'a mut LineCache,
    /// Подсветка совпадений поиска: строка → [(колонка, длина в символах)].
    /// Чистится редактором при первой правке (позиции устаревают).
    pub search_hl: &'a mut HashMap<usize, Vec<(usize, usize)>>,
    /// Внешний прыжок/выделение (anchor, caret) — применяется в этом кадре.
    pub pending_goto: &'a mut Option<(Pos, Pos)>,
    /// Запрос фокуса извне + окно грации (живут у приложения, переживают кадры).
    pub focus_request: &'a mut bool,
    pub focus_grace: &'a mut u8,
    /// true → правки заблокированы (фоновый процесс), навигация и копирование работают.
    pub read_only: bool,
    pub ed_id: egui::Id,
    pub hl: Highlighter<'a>,
    /// Текст вставки по Tab в позиции каретки (per-project табуляция).
    pub tab_insert: &'a dyn Fn(&mut Document, Pos) -> String,
}

/// Что приложение получает от кадра редактора.
#[allow(dead_code)] // часть API: эта сборка не читает все поля
pub(crate) struct EditorOut {
    /// Буфер изменился в этом кадре.
    pub edited: bool,
    /// Первая изменённая строка (для инвалидации per-project кэшей).
    pub changed_from: Option<usize>,
    /// Позиция каретки (0-based) — для статус-бара.
    pub caret: Pos,
    /// Ошибка операции (например, копирование диапазона больше лимита) — в модалку.
    pub error: Option<String>,
    /// Экранный x колонки 0 и y строки 0 (с учётом прокрутки) — якорь для попапов.
    pub origin: egui::Pos2,
    pub char_w: f32,
    pub row_h: f32,
}

/// Отрисовать и обслужить виртуальный редактор внутри `sheet`.
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
        read_only,
        ed_id,
        hl,
        tab_insert,
    } = cx;
    let ectx = &ui.ctx().clone();
    let mut error: Option<String> = None;

    let rh = ectx.fonts_mut(|f| f.row_height(&code_font(CODE_SIZE)));
    let char_w = ectx.fonts_mut(|f| f.glyph_width(&code_font(CODE_SIZE), '0'));

    // геометрия gutter
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

    // внешний запрос фокуса (новая вкладка / открытие / меню)
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

    // ---- клавиатура ----
    let mut edited = false;
    if focused && !read_only {
        edited |= editor_input(
            doc,
            ed,
            ectx,
            rows_vis.saturating_sub(2),
            tab_insert,
            &mut error,
        );
    } else if focused {
        // только навигация/копирование в read-only
        editor_nav_input(doc, ed, ectx, rows_vis.saturating_sub(2), &mut error);
    }
    // инвалидация кэшей от первой изменённой строки (правка/вставка/undo/redo)
    let changed_from = doc.take_change_start();
    if let Some(start) = changed_from {
        lex.invalidate_from(start);
        search_hl.clear(); // позиции совпадений устарели
    }

    ed.clamp(doc);

    // ---- собственная виртуальная прокрутка + отрисовка ----
    // Никакого гигантского полотна: позиция скролла — f64-пиксели, видимые строки
    // рисуются в локальных координатах окна. На 1 ГБ (5×10⁸ px «полотна») f32-координаты
    // egui квантуются по 32px — отсюда были наезды строк, пустые ряды и клики мимо.
    ui.painter().rect_filled(sheet, sheet_cr, p().field_bg);
    let n_lines = doc.line_count();
    let rh64 = rh as f64;
    let content_h = n_lines as f64 * rh64;
    let caret_galley = hl_line(doc, lex, line_cache, &hl, ed.caret.0, ui);
    let max_cols = doc.max_line_bytes(); // байты ≥ символы — верхняя оценка
    let total_w = (max_cols as f32 * char_w).max(caret_galley.rect.width()) + PAD_L + 12.0;

    // прыжок/выделение, запрошенные извне (результаты/поиск/ошибка форматирования)
    if let Some((a, c)) = pending_goto.take() {
        ed.select_range(doc, a, c);
    }

    // резервируем место под полосы (двухпроходно: видимость зависит от внутренней области)
    let mut need_v = content_h > view.height() as f64;
    let mut need_h = total_w > view.width() - if need_v { crate::vscroll::BAR } else { 0.0 };
    need_v = content_h
        > (view.height() - if need_h { crate::vscroll::BAR } else { 0.0 }) as f64;
    need_h = total_w > view.width() - if need_v { crate::vscroll::BAR } else { 0.0 };
    let inner = Rect::from_min_max(
        view.min,
        egui::pos2(
            view.right() - if need_v { crate::vscroll::BAR } else { 0.0 },
            view.bottom() - if need_h { crate::vscroll::BAR } else { 0.0 },
        ),
    );
    let rows_vis = (inner.height() / rh).ceil() as usize + 1;

    // колесо/тачпад (включая инжектированную кинетику) — над всем листом, и gutter тоже
    let d = crate::vscroll::wheel_delta(ui, sheet);
    if d != egui::Vec2::ZERO {
        ed.scroll_y -= d.y as f64;
        ed.scroll_x -= d.x as f64;
        ectx.request_repaint();
    }

    let want_scroll = std::mem::take(&mut ed.scroll_to_caret);
    if want_scroll {
        // вертикально: строка каретки в видимой области
        let cy = ed.caret.0 as f64 * rh64;
        if cy < ed.scroll_y {
            ed.scroll_y = cy;
        } else if cy + rh64 > ed.scroll_y + inner.height() as f64 {
            ed.scroll_y = cy + rh64 - inner.height() as f64;
        }
        // горизонтально: каретка с запасом в 3 символа от краёв
        let cx = (PAD_L
            + caret_galley.pos_from_cursor(egui::text::CCursor::new(ed.caret.1)).min.x)
            as f64;
        let margin = (char_w * 3.0) as f64;
        if cx - margin < ed.scroll_x {
            ed.scroll_x = (cx - margin).max(0.0);
        } else if cx + margin > ed.scroll_x + inner.width() as f64 {
            ed.scroll_x = cx + margin - inner.width() as f64;
        }
    }
    ed.scroll_y = ed.scroll_y.clamp(0.0, (content_h - inner.height() as f64).max(0.0));
    ed.scroll_x = ed.scroll_x.clamp(0.0, (total_w as f64 - inner.width() as f64).max(0.0));

    let now_time = ui.input(|i| i.time);
    let flash = ed.flash_line.filter(|(_, t0)| now_time - t0 < 2.4);
    ed.flash_line = flash;

    // снапшот смещения: ВЕСЬ кадр (текст, каретка, gutter) рисуется одним offset;
    // полосы внизу меняют ed.scroll_* уже для следующего кадра
    let sy = ed.scroll_y;
    let sx = ed.scroll_x;
    // экранный y строки: разность в f64 (точно на любой глубине файла), затем привязка
    // к физическим пикселям — высота строки шрифта дробная, и без снэпа каждая строка
    // пересекала пиксельную границу в свой момент («волна» при медленном скролле)
    let ppp = ectx.pixels_per_point() as f64;
    let top64 = inner.top() as f64;
    let line_y = move |line: usize| -> f32 {
        let y = top64 + line as f64 * rh64 - sy;
        ((y * ppp).round() / ppp) as f32
    };
    let ox = (inner.left() as f64 + PAD_L as f64 - sx) as f32; // x нулевой колонки

    // ---- мышь ----
    let resp = ui.interact(inner, ed_id, egui::Sense::click_and_drag());
    if resp.hovered() {
        ectx.set_cursor_icon(egui::CursorIcon::Text);
    }
    if let Some(pp) = resp.interact_pointer_pos() {
        // строка — в f64 (точно на любых глубинах файла), колонка — локальный пиксель
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

    // ---- строки ----
    let pt = ui.painter().with_clip_rect(inner);
    let first = (sy / rh64).floor().max(0.0) as usize;
    let last = (first + rows_vis).min(n_lines.saturating_sub(1));
    let (sa, sb) = ed.sel();

    for line in first..=last {
        let y = line_y(line);
        let text = doc.get_line(line);
        let llen = text.chars().count();

        // акцентная вспышка строки с ошибкой форматирования (затухает)
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

        // активная строка (без выделения)
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
        // выделение
        if sb > sa && line >= sa.0 && line <= sb.0 {
            let s0 = if line == sa.0 { sa.1.min(llen) } else { 0 };
            let s1 = if line == sb.0 { sb.1.min(llen) } else { llen };
            let cont = line < sb.0; // выделение продолжается на следующей строке
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
        // подсветка совпадений поиска на видимой строке
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
        // текст
        if !text.is_empty() {
            let galley = hl_line(doc, lex, line_cache, &hl, line, ui);
            ui.painter().with_clip_rect(inner).galley(egui::pos2(ox, y), galley, p().text);
        }
    }

    // каретка (свой blink) — galley пересчитываем от ЖИВОЙ строки (клик этого кадра
    // мог сменить строку, а caret_galley выше считан до обработки мыши)
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
            // (экранный x каретки, левый/правый край вью, x каретки в строке, ширина строки)
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
                Stroke::new(1.0, p().accent),
            );
        }
        ectx.request_repaint_after(std::time::Duration::from_secs_f32(
            if cyc < 0.5 { 0.5 - cyc } else { 1.0 - cyc },
        ));
    }

    // ---- полосы прокрутки (свои, в локальных координатах) ----
    if need_v {
        let track = Rect::from_min_max(
            egui::pos2(view.right() - crate::vscroll::BAR, inner.top()),
            egui::pos2(view.right(), inner.bottom()),
        );
        crate::vscroll::vbar(ui, track, ed_id.with("vbar"), &mut ed.scroll_y, content_h);
    }
    if need_h {
        let track = Rect::from_min_max(
            egui::pos2(inner.left(), view.bottom() - crate::vscroll::BAR),
            egui::pos2(inner.right(), view.bottom()),
        );
        crate::vscroll::hbar(ui, track, ed_id.with("hbar"), &mut ed.scroll_x, total_w as f64);
    }

    // ---- gutter (та же scroll_y → параллельность с текстом конструктивна) ----
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
    painter.vline(text_left - 0.5, sheet.y_range(), Stroke::new(1.0, p().border));
    painter.rect_stroke(
        sheet,
        sheet_cr,
        Stroke::new(1.0, p().border_strong),
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

/// Кэшированная galley подсветки строки `line` с учётом состояния лексера на её начале.
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
        ui.ctx().request_repaint(); // долексируем в следующих кадрах
    }
    let text = doc.get_line(line);
    let mut key = String::with_capacity(text.len() + 2);
    key.push(char::from(state));
    key.push('\u{1}');
    key.push_str(&text);
    line_cache.get_or(&key, || {
        let (mut job, _) = (hl.line)(&text, state);
        job.wrap.max_width = f32::INFINITY;
        ui.fonts_mut(|f| f.layout_job(job))
    })
}

/// Навигация и копирование (read-only режим во время процесса).
fn editor_nav_input(
    doc: &mut Document,
    ed: &mut EditorState,
    ctx: &egui::Context,
    page_rows: usize,
    error: &mut Option<String>,
) {
    use egui::{Key, Modifiers};
    let cmd = Modifiers::COMMAND;
    if ctx.input_mut(|i| i.consume_key(cmd, Key::A)) {
        ed.select_all(doc);
    }
    let events = ctx.input(|i| i.events.clone());
    for ev in &events {
        match ev {
            egui::Event::Copy => {
                if ed.has_sel() {
                    match ed.selection_text(doc) {
                        Ok(s) => ctx.copy_text(s),
                        Err(e) => *error = Some(e),
                    }
                }
            }
            egui::Event::Key { key, pressed: true, modifiers, .. } => {
                let sh = modifiers.shift;
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
                    Key::PageUp => ed.page(doc, false, page_rows.max(1), sh),
                    Key::PageDown => ed.page(doc, true, page_rows.max(1), sh),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Обычные клавиши редактирования (true, если буфер изменился).
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
    if ctx.input_mut(|i| i.consume_key(cmd, Key::Z)) {
        changed |= ed.undo_op(doc);
    }
    if ctx.input_mut(|i| {
        i.consume_key(cmd | Modifiers::SHIFT, Key::Z) || i.consume_key(cmd, Key::Y)
    }) {
        changed |= ed.redo_op(doc);
    }
    let events = ctx.input(|i| i.events.clone());
    for ev in &events {
        match ev {
            egui::Event::Copy => {
                if ed.has_sel() {
                    match ed.selection_text(doc) {
                        Ok(s) => ctx.copy_text(s),
                        Err(e) => *error = Some(e),
                    }
                }
            }
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
                let sh = modifiers.shift;
                match key {
                    Key::Enter => {
                        // перевод строки + отступ текущей строки
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
                    Key::Backspace => {
                        ed.backspace(doc);
                        changed = true;
                    }
                    Key::Delete => {
                        ed.delete(doc);
                        changed = true;
                    }
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
                    Key::PageUp => ed.page(doc, false, page_rows.max(1), sh),
                    Key::PageDown => ed.page(doc, true, page_rows.max(1), sh),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    // тесты ставят каретку/якорь прямо в приватные поля — литерал тут не собрать
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    use crate::doc::Document;

    fn doc_from(text: &str) -> Document {
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), text);
        d
    }

    #[test]
    fn replace_moves_caret() {
        let mut d = doc_from("abcd");
        let mut e = EditorState::default();
        e.anchor = (0, 1);
        e.caret = (0, 3); // выделено "bc"
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
        assert!(e.undo_op(&mut d)); // слитый набор откатывается одним шагом
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
        // игрушечный лексер: «[[» открывает блок (состояние 1), «]]» закрывает (0)
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
        // правка строки 0: состояния после неё инвалидированы, но пересчитываются
        lex.invalidate_from(0);
        assert_eq!(lex.state_at(&mut d, 1, &advance), (1, true));
    }
}
