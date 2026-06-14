//!
//! Фасад документа [`Document`] — буфер, индекс строк, undo, снапшоты.
//!
//! Координаты позиции — `Pos = (line, col)`, 0-based, `col` в кодовых точках.
//! Внутреннее представление — байты UTF-8 (piece table + line index). Модель не держит
//! весь текст одной строкой: чтение только построчно/диапазонно, файл маппится через mmap
//! и НЕ загружается целиком.

#![allow(dead_code)] // модель документа: эта сборка использует не весь её API

pub mod encodings;
pub mod line_index;
pub mod piece_table;

pub use encodings::Eol;
use line_index::LineIndex;
use piece_table::{OriginBuf, PieceSnapshot, PieceTable};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub type Pos = (usize, usize);

const MAX_RANGE_BYTES: usize = 256 * 1024 * 1024; // лимит get_text_range / копирования
const LINE_CACHE_MAX: usize = 10_000;
const MERGE_WINDOW_S: f64 = 1.0; // окно слияния последовательного набора
/// Файлы крупнее — открываются в фоне (с прогрессом в статус-баре).
pub const ASYNC_THRESHOLD: u64 = 4 * 1024 * 1024;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Каталог временных файлов приложения.
pub fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(crate::brand::EXE_BASE);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Уникальное имя временного файла.
pub fn temp_file(ext: &str) -> PathBuf {
    let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    temp_dir().join(format!("{pid}-{t}-{n}.{ext}"))
}

/// Описание правки для слушателей (редактор инвалидирует кэш с `start_line`).
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // поля — информационный API события
pub struct ChangeEvent {
    pub start_line: usize,
    pub removed_lines: usize,
    pub added_lines: usize,
}

/// Примитивная обратимая правка / полная замена содержимого (форматирование).
enum EditItem {
    Edit { offset: usize, old: Vec<u8>, new: Vec<u8> },
    FullReplace { old_origin: PathBuf, new_origin: PathBuf, old_encoding: String, old_eol: Eol },
}

/// Подсчёт кодовых точек в UTF-8 (байты, не являющиеся continuation).
/// `\r` и `\n` — обычные символы: CRLF считается двумя.
pub fn count_chars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| (**b & 0xC0) != 0x80).count()
}

/// Документ с дешёвыми правками для файлов до 1 ГБ.
pub struct Document {
    pt: PieceTable,
    index: LineIndex,
    origin_path: Option<PathBuf>, // файл под mmap (оригинал или temp)
    owns_origin: bool,            // origin_path — наш временный файл
    pub path: Option<PathBuf>,    // логический путь документа
    modified: bool,
    pub encoding_label: String,
    pub eol: Eol,
    char_count: usize,
    /// Верхняя оценка длины самой длинной строки в байтах (для ширины скролла).
    /// Монотонно растёт при правках; пересчитывается при полной замене содержимого.
    max_line_bytes: usize,

    undo: Vec<Vec<EditItem>>,
    redo: Vec<Vec<EditItem>>,
    open_txn: Option<Vec<EditItem>>,
    compound_depth: usize,
    last_edit_time: Option<Instant>,
    last_edit_was_typing: bool,

    line_cache: HashMap<usize, (String, usize)>, // строка n → (текст без EOL, число символов)
    temp_files: Vec<PathBuf>,
    /// Поколение содержимого: растёт ТОЛЬКО при полной замене (открытие/формат/undo формата).
    pub generation: u64,
    /// Аккумулятор «первая изменённая строка» с последнего съёма (для инвалидации кэшей).
    change_start: Option<usize>,
}

/// Сообщения фоновой загрузки файла.
pub enum LoadMsg {
    Progress(u8),
    Done(Box<Document>),
    Failed(String),
}

impl Document {
    /// Пустой документ.
    pub fn new_empty() -> Self {
        Self {
            pt: PieceTable::empty(),
            index: LineIndex::empty(),
            origin_path: None,
            owns_origin: false,
            path: None,
            modified: false,
            encoding_label: "UTF-8".to_owned(),
            eol: Eol::Crlf, // дефолт нового файла на Windows
            char_count: 0,
            max_line_bytes: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            open_txn: None,
            compound_depth: 0,
            last_edit_time: None,
            last_edit_was_typing: false,
            line_cache: HashMap::new(),
            temp_files: Vec::new(),
            generation: 0,
            change_start: None,
        }
    }

    /// Снять и сбросить аккумулятор «первая изменённая строка».
    pub fn take_change_start(&mut self) -> Option<usize> {
        self.change_start.take()
    }

    // ====================================================================
    //  Открытие / загрузка
    // ====================================================================

    /// Синхронное открытие (малые файлы). `progress` — колбэк 0..100.
    pub fn open_sync(
        path: &Path,
        mut progress: Option<&mut dyn FnMut(u8)>,
    ) -> std::io::Result<Self> {
        let mut doc = Self::new_empty();
        doc.path = Some(path.to_owned());
        let det = encodings::detect(path)?;
        doc.encoding_label = det.label.clone();
        let (origin_path, owns) = if det.is_utf8_no_bom {
            (path.to_owned(), false)
        } else {
            let dst = temp_file("utf8");
            let enc = det.encoding.unwrap_or(encoding_rs::UTF_8);
            let mut cb = progress.as_deref_mut().map(|cb| {
                move |p: u8| cb((p as f32 * 0.4) as u8)
            });
            encodings::transcode_to_utf8(
                path,
                &dst,
                enc,
                cb.as_mut().map(|c| c as &mut dyn FnMut(u8)),
            )?;
            doc.temp_files.push(dst.clone());
            (dst, true)
        };
        doc.attach_origin(&origin_path, owns)?;
        // EOL по образцу первых ~1 МБ UTF-8 содержимого
        let sample = doc.pt.read(0, 1024 * 1024);
        doc.eol = encodings::detect_eol(&sample);
        // индекс строк + подсчёт символов одним проходом по mmap
        doc.build_index_and_chars(progress);
        Ok(doc)
    }

    /// Фоновое открытие: воркер строит документ целиком и шлёт его по каналу.
    pub fn spawn_open(path: PathBuf) -> std::sync::mpsc::Receiver<LoadMsg> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let mut cb = move |p: u8| {
                let _ = tx2.send(LoadMsg::Progress(p));
            };
            match Document::open_sync(&path, Some(&mut cb)) {
                Ok(doc) => {
                    let _ = tx.send(LoadMsg::Done(Box::new(doc)));
                }
                Err(e) => {
                    let _ = tx.send(LoadMsg::Failed(e.to_string()));
                }
            }
        });
        rx
    }

    /// Открыть origin-файл read-only И заблокировать его от записи/удаления другими процессами
    /// (`FILE_SHARE_READ`). Хэндл держится живым в [`OriginBuf::Mmap`] всё время жизни вкладки;
    /// другие смогут только читать. Если файл прямо сейчас открыт кем-то на запись — открытие
    /// упадёт со sharing violation (это безопаснее, чем mmap-ить меняющийся файл).
    fn open_origin_locked(path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
    }

    fn attach_origin(&mut self, origin_path: &Path, owns: bool) -> std::io::Result<()> {
        self.detach_origin();
        let size = std::fs::metadata(origin_path)?.len();
        if size > 0 {
            let fh = Self::open_origin_locked(origin_path)?;
            // SAFETY: файл открыт read-only и заблокирован от внешней записи (open_origin_locked),
            // поэтому содержимое mmap стабильно на всё время жизни буфера.
            let mm = unsafe { memmap2::Mmap::map(&fh)? };
            self.pt = PieceTable::new(Arc::new(OriginBuf::Mmap(mm, fh)));
        } else {
            self.pt = PieceTable::empty();
        }
        self.origin_path = Some(origin_path.to_owned());
        self.owns_origin = owns;
        Ok(())
    }

    fn detach_origin(&mut self) {
        // сбрасываем piece table, чтобы снять ссылку на mmap перед его закрытием
        self.pt = PieceTable::empty();
    }

    /// Один проход по origin: начала строк (прогресс 40..100) + число символов.
    fn build_index_and_chars(&mut self, mut progress: Option<&mut dyn FnMut(u8)>) {
        let origin = self.pt.origin_arc();
        let data = origin.as_slice();
        let total = data.len().max(1);
        const BLOCK: usize = 8 * 1024 * 1024;
        let mut starts: Vec<u64> = Vec::with_capacity(data.len() / 64 + 1);
        starts.push(0);
        let mut chars = 0usize;
        let mut pos = 0usize;
        while pos < data.len() {
            let end = (pos + BLOCK).min(data.len());
            let block = &data[pos..end];
            for idx in memchr::memchr_iter(b'\n', block) {
                starts.push((pos + idx + 1) as u64);
            }
            chars += count_chars(block);
            pos = end;
            if let Some(cb) = progress.as_deref_mut() {
                cb(40 + ((pos * 60) / total).min(60) as u8);
            }
        }
        self.index = LineIndex::from_starts(&starts, data.len() as u64, data);
        self.char_count = chars;
        // самая длинная строка: максимальный зазор между началами строк (включая хвост)
        let mut maxb = 0usize;
        for w in starts.windows(2) {
            maxb = maxb.max((w[1] - w[0]) as usize);
        }
        maxb = maxb.max(data.len() - *starts.last().unwrap_or(&0) as usize);
        self.max_line_bytes = maxb;
    }

    /// Верхняя оценка длины самой длинной строки в байтах.
    pub fn max_line_bytes(&self) -> usize {
        self.max_line_bytes
    }

    // ====================================================================
    //  Свойства
    // ====================================================================

    pub fn modified(&self) -> bool {
        self.modified
    }

    pub fn line_count(&mut self) -> usize {
        self.index.line_count()
    }

    #[allow(dead_code)] // публичный API модели — пригодится (goto byte, диагностика)
    pub fn total_bytes(&mut self) -> u64 {
        self.index.total_bytes()
    }

    pub fn char_count(&self) -> usize {
        self.char_count
    }

    #[allow(dead_code)]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[allow(dead_code)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ====================================================================
    //  Чтение
    // ====================================================================

    pub fn read_bytes(&mut self, offset: u64, length: usize) -> Vec<u8> {
        self.pt.read(offset as usize, length)
    }

    fn strip_eol(raw: &[u8]) -> &[u8] {
        if raw.ends_with(b"\r\n") {
            &raw[..raw.len() - 2]
        } else if raw.ends_with(b"\n") || raw.ends_with(b"\r") {
            &raw[..raw.len() - 1]
        } else {
            raw
        }
    }

    /// Прочитать строку `n` в кэш (если её там нет) и вернуть ССЫЛКУ — без клонирования. Горячий
    /// путь для измерений/скана: на гигабайтных строках клон строки в `get_line` (даже из кэша)
    /// был основным источником лага (двойной клик дёргал строку несколько раз за кадр).
    fn ensure_line_cached(&mut self, n: usize) -> &str {
        if !self.line_cache.contains_key(&n) {
            let start = self.index.line_start(n);
            let end = self.index.line_start(n + 1);
            let raw = self.pt.read(start as usize, (end - start) as usize);
            let text = String::from_utf8_lossy(Self::strip_eol(&raw)).into_owned();
            let chars = text.chars().count(); // считаем длину один раз и кэшируем рядом
            if self.line_cache.len() >= LINE_CACHE_MAX {
                self.line_cache.clear(); // простая стратегия: переполнился — сбросили
            }
            self.line_cache.insert(n, (text, chars));
        }
        self.line_cache.get(&n).map(|(s, _)| s.as_str()).unwrap_or("")
    }

    /// Текст строки `n` (0-based) без перевода строки.
    pub fn get_line(&mut self, n: usize) -> String {
        if n >= self.line_count() {
            return String::new();
        }
        self.ensure_line_cached(n).to_owned()
    }

    /// Длина строки `n` в кодовых точках (без EOL). O(1) после первого доступа — счётчик символов
    /// кэшируется рядом со строкой (важно для тач-драга: `set_from_line_x` зовёт это каждый кадр).
    pub fn line_length(&mut self, n: usize) -> usize {
        if n >= self.line_count() {
            return 0;
        }
        self.ensure_line_cached(n);
        self.line_cache.get(&n).map_or(0, |(_, c)| *c)
    }

    /// Границы слова (в кодовых точках) вокруг колонки `col` в строке `n`, либо `None`, если под
    /// кликом не слово. Скан ограничен окном вокруг клика, поэтому двойной клик в очень длинной
    /// строке НЕ материализует и не сканирует её целиком (фикс лага на ~1 ГБ-файлах).
    pub fn word_bounds_at(
        &mut self,
        n: usize,
        col: usize,
        is_word: impl Fn(char) -> bool,
    ) -> Option<(usize, usize)> {
        if n >= self.line_count() {
            return None;
        }
        const W: usize = 1024; // окно скана слова вокруг клика (символов в каждую сторону)
        let lo = col.saturating_sub(W);
        let hi = col.saturating_add(W);
        let s = self.ensure_line_cached(n);
        // окно символов [lo..=hi]; win[j] соответствует колонке `lo + j`
        let win: Vec<char> = s
            .chars()
            .enumerate()
            .skip_while(|(i, _)| *i < lo)
            .take_while(|(i, _)| *i <= hi)
            .map(|(_, ch)| ch)
            .collect();
        let c = col - lo; // индекс кликнутого символа внутри окна
        // слово под кликом: символ на `c` или (как в редакторах) слева от него
        let start_local = if c < win.len() && is_word(win[c]) {
            c
        } else if c > 0 && c <= win.len() && is_word(win[c - 1]) {
            c - 1
        } else {
            return None;
        };
        let (mut s0, mut e0) = (start_local, start_local);
        while s0 > 0 && is_word(win[s0 - 1]) {
            s0 -= 1;
        }
        while e0 < win.len() && is_word(win[e0]) {
            e0 += 1;
        }
        Some((lo + s0, lo + e0))
    }

    /// Байтовый диапазон строки `n` вместе с её переводом строки.
    #[allow(dead_code)] // публичный API модели
    pub fn line_byte_span(&mut self, n: usize) -> (u64, u64) {
        (self.index.line_start(n), self.index.line_start(n + 1))
    }

    /// Текст между позициями (для копирования). Диапазон > 256 МБ запрещён.
    pub fn get_text_range(&mut self, start: Pos, end: Pos) -> Result<String, String> {
        let a = self.pos_to_byte(start);
        let b = self.pos_to_byte(end);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        if (b - a) as usize > MAX_RANGE_BYTES {
            return Err("Диапазон слишком велик для копирования (> 256 МБ)".to_owned());
        }
        let raw = self.pt.read(a as usize, (b - a) as usize);
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    /// Снимок содержимого для фоновых процессов (поиск/валидация/форматирование).
    pub fn snapshot(&self) -> PieceSnapshot {
        self.pt.snapshot()
    }

    // ====================================================================
    //  Конвертация координат
    // ====================================================================

    pub fn pos_to_byte(&mut self, pos: Pos) -> u64 {
        let (line, col) = pos;
        let line = line.min(self.line_count().saturating_sub(1));
        let start = self.index.line_start(line);
        if col == 0 {
            return start;
        }
        let text = self.get_line(line);
        let byte_col: usize = text
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(text.len());
        start + byte_col as u64
    }

    /// Позиция в символах от начала документа (0-based) для `(строка, колонка)`.
    /// Дёшево: символьная база чанка индекса + скан хвоста внутри чанка.
    pub fn char_pos(&mut self, pos: Pos) -> usize {
        let line = pos.0.min(self.line_count().saturating_sub(1));
        let line_start = self.index.line_start(line);
        let (byte_base, char_base) = self.index.char_base_for_byte(line_start);
        let tail = self.pt.read(byte_base as usize, (line_start - byte_base) as usize);
        char_base as usize + count_chars(&tail) + pos.1
    }

    /// Перевести байтовое смещение в `(line, col)`.
    pub fn byte_to_pos(&mut self, offset: u64) -> Pos {
        let offset = offset.min(self.index.total_bytes());
        let line = self.index.line_for_offset(offset);
        let start = self.index.line_start(line);
        let raw = self.pt.read(start as usize, (offset - start) as usize);
        let stripped = Self::strip_eol(&raw);
        (line, count_chars(stripped))
    }

    // ====================================================================
    //  Правки
    // ====================================================================

    /// Начать составную операцию (группируется в один шаг undo).
    #[allow(dead_code)] // публичный API модели (используется в тестах)
    pub fn begin_compound(&mut self) {
        if self.compound_depth == 0 {
            self.open_txn = Some(Vec::new());
        }
        self.compound_depth += 1;
    }

    #[allow(dead_code)]
    pub fn end_compound(&mut self) {
        if self.compound_depth == 0 {
            return;
        }
        self.compound_depth -= 1;
        if self.compound_depth == 0 {
            if let Some(txn) = self.open_txn.take() {
                if !txn.is_empty() {
                    self.undo.push(txn);
                    self.redo.clear();
                }
            }
        }
    }

    /// Заменить текст в диапазоне `[start, end)` на `text`. Вернуть событие изменения.
    pub fn replace_range(&mut self, start: Pos, end: Pos, text: &str) -> ChangeEvent {
        let a = self.pos_to_byte(start);
        let b = self.pos_to_byte(end);
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let new = text.as_bytes().to_vec();
        let (ev, old) = self.apply_primitive(a as usize, (b - a) as usize, &new);
        self.record(EditItem::Edit { offset: a as usize, old, new });
        ev
    }

    /// Применить правку к буферу и индексу; вернуть (событие, старые байты).
    fn apply_primitive(
        &mut self,
        offset: usize,
        old_len: usize,
        new_bytes: &[u8],
    ) -> (ChangeEvent, Vec<u8>) {
        let first_line = self.index.line_for_offset(offset as u64);
        let old = if old_len > 0 { self.pt.delete(offset, old_len) } else { Vec::new() };
        let removed_lines = if old_len > 0 {
            // строки в старых координатах — считаем по удалённым байтам
            memchr::memchr_iter(b'\n', &old).count()
        } else {
            0
        };
        if !new_bytes.is_empty() {
            self.pt.insert(offset, new_bytes);
        }
        let pt = &mut self.pt;
        self.index.apply_edit(offset as u64, old_len as u64, new_bytes.len() as u64, |o, l| {
            pt.read(o as usize, l)
        });
        let added_lines = memchr::memchr_iter(b'\n', new_bytes).count();
        self.char_count = self.char_count + count_chars(new_bytes) - count_chars(&old);
        // оценка самой длинной строки: максимальный зазор в новых байтах + граничные строки
        let mut maxb = self.max_line_bytes;
        let mut prev = 0usize;
        for idx in memchr::memchr_iter(b'\n', new_bytes) {
            maxb = maxb.max(idx - prev);
            prev = idx + 1;
        }
        maxb = maxb.max(new_bytes.len() - prev);
        for line in [first_line, first_line + added_lines] {
            let (s, e) = (self.index.line_start(line), self.index.line_start(line + 1));
            maxb = maxb.max((e - s) as usize);
        }
        self.max_line_bytes = maxb;
        self.invalidate_cache_from(first_line);
        self.modified = true;
        self.change_start =
            Some(self.change_start.map_or(first_line, |c| c.min(first_line)));
        (ChangeEvent { start_line: first_line, removed_lines, added_lines }, old)
    }

    /// Положить правку в журнал undo (с авто-слиянием набора текста).
    fn record(&mut self, item: EditItem) {
        if let Some(txn) = self.open_txn.as_mut() {
            txn.push(item);
            return;
        }
        let now = Instant::now();
        let merged = self.try_merge_typing(&item, now);
        if !merged {
            // «набор текста» — вставка короткого фрагмента без перевода строки
            self.last_edit_was_typing = matches!(
                &item,
                EditItem::Edit { old, new, .. }
                    if old.is_empty() && !new.contains(&b'\n') && new.len() <= 4
            );
            self.undo.push(vec![item]);
        }
        self.redo.clear();
        self.last_edit_time = Some(now);
    }

    fn try_merge_typing(&mut self, item: &EditItem, now: Instant) -> bool {
        if !self.last_edit_was_typing || self.undo.is_empty() {
            return false;
        }
        let Some(t0) = self.last_edit_time else { return false };
        if now.duration_since(t0).as_secs_f64() > MERGE_WINDOW_S {
            return false;
        }
        let EditItem::Edit { offset, old, new } = item else { return false };
        if !old.is_empty() || new.contains(&b'\n') || new.len() > 4 {
            return false;
        }
        let last_txn = self.undo.last_mut().unwrap();
        if last_txn.len() != 1 {
            return false;
        }
        let EditItem::Edit { offset: poff, old: pold, new: pnew } = &mut last_txn[0] else {
            return false;
        };
        if !pold.is_empty() || *poff + pnew.len() != *offset {
            return false;
        }
        pnew.extend_from_slice(new);
        true
    }

    fn invalidate_cache_from(&mut self, line: usize) {
        self.line_cache.retain(|&k, _| k < line);
    }

    // ====================================================================
    //  Undo / Redo
    // ====================================================================

    /// Откатить последнюю операцию. Вернуть позицию курсора после отката.
    pub fn undo(&mut self) -> Option<Pos> {
        let txn = self.undo.pop()?;
        let mut cursor = None;
        let mut new_txn = Vec::with_capacity(txn.len());
        for item in txn.into_iter().rev() {
            let (item, pos) = self.revert(item);
            cursor = Some(pos);
            new_txn.push(item);
        }
        new_txn.reverse();
        self.redo.push(new_txn);
        if self.undo.is_empty() {
            self.modified = false;
        }
        self.last_edit_was_typing = false;
        cursor
    }

    /// Повторить отменённую операцию.
    pub fn redo(&mut self) -> Option<Pos> {
        let txn = self.redo.pop()?;
        let mut cursor = None;
        let mut new_txn = Vec::with_capacity(txn.len());
        for item in txn {
            let (item, pos) = self.reapply(item);
            cursor = Some(pos);
            new_txn.push(item);
        }
        self.undo.push(new_txn);
        self.modified = true;
        self.last_edit_was_typing = false;
        cursor
    }

    fn revert(&mut self, item: EditItem) -> (EditItem, Pos) {
        match item {
            EditItem::FullReplace { old_origin, new_origin, old_encoding, old_eol } => {
                let cur_enc = self.encoding_label.clone();
                let cur_eol = self.eol;
                let _ = self.set_origin_file(&old_origin, true);
                self.encoding_label = old_encoding.clone();
                self.eol = old_eol;
                (
                    EditItem::FullReplace {
                        old_origin,
                        new_origin,
                        old_encoding: cur_enc,
                        old_eol: cur_eol,
                    },
                    (0, 0),
                )
            }
            EditItem::Edit { offset, old, new } => {
                // убрать new, вернуть old
                let (_, _) = self.apply_primitive(offset, new.len(), &old);
                let pos = self.byte_to_pos((offset + old.len()) as u64);
                (EditItem::Edit { offset, old, new }, pos)
            }
        }
    }

    fn reapply(&mut self, item: EditItem) -> (EditItem, Pos) {
        match item {
            EditItem::FullReplace { old_origin, new_origin, old_encoding, old_eol } => {
                let _ = self.set_origin_file(&new_origin, true);
                (
                    EditItem::FullReplace { old_origin, new_origin, old_encoding, old_eol },
                    (0, 0),
                )
            }
            EditItem::Edit { offset, old, new } => {
                let (_, _) = self.apply_primitive(offset, old.len(), &new);
                let pos = self.byte_to_pos((offset + new.len()) as u64);
                (EditItem::Edit { offset, old, new }, pos)
            }
        }
    }

    // ====================================================================
    //  Снапшоты / форматирование / сохранение
    // ====================================================================

    /// Заменить всё содержимое результатом форматирования (одна операция undo).
    pub fn swap_origin(&mut self, new_utf8_path: &Path) -> std::io::Result<()> {
        // снимок текущего содержимого для отката
        let old_snap = temp_file("utf8");
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&old_snap)?);
            self.pt.write_to(&mut f)?;
            use std::io::Write;
            f.flush()?;
        }
        self.temp_files.push(old_snap.clone());
        self.temp_files.push(new_utf8_path.to_owned());
        let record = EditItem::FullReplace {
            old_origin: old_snap,
            new_origin: new_utf8_path.to_owned(),
            old_encoding: self.encoding_label.clone(),
            old_eol: self.eol,
        };
        self.set_origin_file(new_utf8_path, true)?;
        self.undo.push(vec![record]);
        self.redo.clear();
        self.modified = true;
        Ok(())
    }

    /// Переключить origin на содержимое файла `path` и перестроить индекс.
    fn set_origin_file(&mut self, path: &Path, owns: bool) -> std::io::Result<()> {
        self.attach_origin(path, owns)?;
        let sample = self.pt.read(0, 1024 * 1024);
        self.eol = encodings::detect_eol(&sample);
        self.build_index_and_chars(None);
        self.line_cache.clear();
        self.generation += 1;
        self.change_start = Some(0);
        Ok(())
    }

    /// Сохранить документ в UTF-8 без BOM. Без `path` — по текущему пути.
    pub fn save(&mut self, path: Option<&Path>) -> std::io::Result<()> {
        let target: PathBuf = match path.or(self.path.as_deref()) {
            Some(p) => p.to_owned(),
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Не задан путь сохранения",
                ))
            }
        };
        let same_as_origin = self
            .origin_path
            .as_ref()
            .is_some_and(|op|

                std::path::absolute(&target).ok() == std::path::absolute(op).ok());
        let tmp = target.with_extension(format!(
            "tmp-{}",
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        {
            let f = std::fs::File::create(&tmp)?;
            let mut w = std::io::BufWriter::new(f);
            self.pt.write_to(&mut w)?;
            use std::io::Write;
            w.flush()?;
            w.get_ref().sync_all()?;
        }
        if same_as_origin {
            // освободить mmap, подменить файл, переоткрыть; содержимое идентично,
            // поэтому индекс строк и журнал undo остаются валидными
            self.detach_origin();
            std::fs::rename(&tmp, &target)?;
            self.reattach_same_content(&target)?;
        } else {
            std::fs::rename(&tmp, &target)?;
            // origin остаётся прежним (содержимое то же), путь документа меняется
        }
        self.path = Some(target);
        // после сохранения исходная кодировка нерелевантна — на диске UTF-8
        self.encoding_label = "UTF-8".to_owned();
        self.modified = false;
        Ok(())
    }

    /// Переоткрыть origin на файл с тем же содержимым (после сохранения).
    fn reattach_same_content(&mut self, path: &Path) -> std::io::Result<()> {
        let size = std::fs::metadata(path)?.len();
        if size > 0 {
            let fh = Self::open_origin_locked(path)?;
            // SAFETY: см. attach_origin
            let mm = unsafe { memmap2::Mmap::map(&fh)? };
            self.pt = PieceTable::new(Arc::new(OriginBuf::Mmap(mm, fh)));
        } else {
            self.pt = PieceTable::empty();
        }
        self.origin_path = Some(path.to_owned());
        self.owns_origin = false;
        self.line_cache.clear();
        // индекс строк не перестраиваем: содержимое не изменилось
        Ok(())
    }

    /// Полный текст (только для тестов/малых файлов).
    #[cfg(test)]
    pub fn full_text(&mut self) -> String {
        let n = self.pt.len();
        String::from_utf8_lossy(&self.pt.read(0, n)).into_owned()
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.detach_origin();
        for p in &self.temp_files {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Удалить старые временные файлы из прошлых запусков (best effort).
pub fn cleanup_temp_dir(max_age_s: u64) {
    let d = temp_dir();
    let now = std::time::SystemTime::now();
    let Ok(entries) = std::fs::read_dir(&d) else { return };
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if now.duration_since(modified).map(|d| d.as_secs()).unwrap_or(0) > max_age_s {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_from(text: &str) -> Document {
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), text);
        // сбрасываем undo/modified, имитируя «свежеоткрытый» документ
        d.undo.clear();
        d.modified = false;
        d
    }

    #[test]
    fn empty_doc_basics() {
        let mut d = Document::new_empty();
        assert_eq!(d.line_count(), 1);
        assert_eq!(d.get_line(0), "");
        assert_eq!(d.char_count(), 0);
    }

    #[test]
    fn insert_and_read_lines() {
        let mut d = doc_from("ab\nсд\nef");
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.get_line(0), "ab");
        assert_eq!(d.get_line(1), "сд");
        assert_eq!(d.get_line(2), "ef");
        assert_eq!(d.char_count(), 8);
    }

    #[test]
    fn replace_range_multibyte() {
        let mut d = doc_from("привет мир");
        d.replace_range((0, 7), (0, 10), "world");
        assert_eq!(d.full_text(), "привет world");
        assert_eq!(d.char_count(), 12);
    }

    #[test]
    fn crlf_lines_strip_eol() {
        let mut d = doc_from("ab\r\ncd\r\n");
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.get_line(0), "ab");
        assert_eq!(d.get_line(1), "cd");
        assert_eq!(d.get_line(2), "");
    }

    #[test]
    fn crlf_counts_as_two_chars() {
        // \r и \n — обычные кодовые точки: CRLF = два символа
        let mut d = Document::new_empty(); // EOL по умолчанию CRLF
        d.replace_range((0, 0), (0, 0), "\r\n");
        assert_eq!(d.char_count(), 2);
        assert_eq!(d.char_pos((1, 0)), 2);
        let mut d = doc_from("a\r\nb");
        assert_eq!(d.char_count(), 4);
        assert_eq!(d.char_pos((1, 1)), 4);
    }

    #[test]
    fn char_pos_counts_codepoints() {
        let mut d = doc_from("аб\nвгд\ne");
        assert_eq!(d.char_pos((0, 0)), 0);
        assert_eq!(d.char_pos((0, 2)), 2);
        assert_eq!(d.char_pos((1, 0)), 3); // после "аб\n"
        assert_eq!(d.char_pos((1, 3)), 6);
        assert_eq!(d.char_pos((2, 1)), 8);
        // после правки кэш чанков пересчитывается
        d.replace_range((0, 0), (0, 1), "xy");
        assert_eq!(d.get_line(0), "xyб");
        assert_eq!(d.char_pos((1, 0)), 4);
    }

    #[test]
    fn pos_byte_round_trip() {
        let mut d = doc_from("аб\nвгд\ne");
        for (line, col) in [(0, 0), (0, 1), (0, 2), (1, 0), (1, 3), (2, 1)] {
            let b = d.pos_to_byte((line, col));
            assert_eq!(d.byte_to_pos(b), (line, col), "({line},{col})");
        }
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut d = doc_from("hello");
        d.replace_range((0, 5), (0, 5), " world");
        assert_eq!(d.full_text(), "hello world");
        assert!(d.modified());
        let pos = d.undo();
        assert_eq!(d.full_text(), "hello");
        assert_eq!(pos, Some((0, 5)));
        let pos = d.redo();
        assert_eq!(d.full_text(), "hello world");
        assert_eq!(pos, Some((0, 11)));
    }

    #[test]
    fn typing_merges_into_one_undo() {
        let mut d = doc_from("");
        d.replace_range((0, 0), (0, 0), "a");
        d.replace_range((0, 1), (0, 1), "b");
        d.replace_range((0, 2), (0, 2), "c");
        assert_eq!(d.full_text(), "abc");
        d.undo();
        assert_eq!(d.full_text(), "");
    }

    #[test]
    fn compound_groups_edits() {
        let mut d = doc_from("abc");
        d.begin_compound();
        d.replace_range((0, 0), (0, 1), "X");
        d.replace_range((0, 2), (0, 3), "Y");
        d.end_compound();
        assert_eq!(d.full_text(), "XbY");
        d.undo();
        assert_eq!(d.full_text(), "abc");
        d.redo();
        assert_eq!(d.full_text(), "XbY");
    }

    #[test]
    fn char_count_tracks_edits() {
        let mut d = doc_from("привет");
        assert_eq!(d.char_count(), 6);
        d.replace_range((0, 0), (0, 2), "x");
        assert_eq!(d.full_text(), "xивет");
        assert_eq!(d.char_count(), 5);
        d.undo();
        assert_eq!(d.char_count(), 6);
    }

    #[test]
    fn open_save_round_trip() {
        let dir = temp_dir();
        let p = dir.join("test-open-save.xml");
        std::fs::write(&p, "<a>\nпривет\n</a>").unwrap();
        let mut d = Document::open_sync(&p, None).unwrap();
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.get_line(1), "привет");
        assert_eq!(d.encoding_label, "UTF-8");
        d.replace_range((1, 0), (1, 6), "hello");
        d.save(None).unwrap();
        drop(d);
        let out = std::fs::read_to_string(&p).unwrap();
        assert_eq!(out, "<a>\nhello\n</a>");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn open_cp1251_transcodes() {
        let dir = temp_dir();
        let p = dir.join("test-cp1251.xml");
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("<а>текст</а>");
        std::fs::write(&p, &bytes).unwrap();
        let mut d = Document::open_sync(&p, None).unwrap();
        assert_eq!(d.encoding_label, "Windows-1251");
        assert_eq!(d.get_line(0), "<а>текст</а>");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn save_over_origin_keeps_working() {
        let dir = temp_dir();
        let p = dir.join("test-save-origin.xml");
        std::fs::write(&p, "line1\nline2").unwrap();
        let mut d = Document::open_sync(&p, None).unwrap();
        d.replace_range((0, 0), (0, 5), "first");
        d.save(None).unwrap(); // сохранение поверх mmap-нутого файла
        assert_eq!(d.get_line(0), "first");
        // документ остаётся редактируемым после переоткрытия mmap
        d.replace_range((1, 0), (1, 5), "second");
        assert_eq!(d.get_line(1), "second");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn swap_origin_is_undoable() {
        let mut d = doc_from("old content");
        let formatted = temp_file("utf8");
        std::fs::write(&formatted, "new\nformatted").unwrap();
        d.swap_origin(&formatted).unwrap();
        assert_eq!(d.full_text(), "new\nformatted");
        assert_eq!(d.line_count(), 2);
        d.undo();
        assert_eq!(d.full_text(), "old content");
        d.redo();
        assert_eq!(d.full_text(), "new\nformatted");
    }
}
