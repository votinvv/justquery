//! SHARED pedant↔justquery — править синхронно (список общих файлов — в README).
//!
//! Ленивый чанкованный индекс строк для файлов до ~1 ГБ.

#![allow(dead_code)] // SHARED-модель: проекты используют разные подмножества её API
//!
//! Хранит позиции начал строк. Чтобы правка не сдвигала весь массив (десятки млн строк),
//! смещения внутри чанка относительные, а база чанка — лениво пересчитываемая префиксная
//! сумма длин чанков. Правка перестраивает только затронутые чанки.
//!
//! Соглашение о строках: начало строки — байт 0 и байт сразу после каждого `\n`.
//! Так `"abc"` → 1 строка; `"abc\n"` → 2 строки (вторая пустая). `\r` не считается
//! разделителем при индексации (CR-only EOL обрабатывается при чтении строки в document).

/// Целевое число строк в чанке.
const TARGET: usize = 8192;

/// Чанк строк: относительные смещения начал строк + длина в байтах и символах.
struct Chunk {
    rel: Vec<u32>, // rel[0] == 0; относительные смещения внутри чанка
    byte_len: u64,
    chars_len: u64, // кодовых точек в чанке (для позиции каретки в символах)
}

/// Найти абсолютные смещения начал строк в `data` (включая первую — `base`).
pub(super) fn scan_starts(data: &[u8], base: u64) -> Vec<u64> {
    let mut starts = Vec::with_capacity(data.len() / 64 + 1);
    starts.push(base);
    for idx in memchr::memchr_iter(b'\n', data) {
        starts.push(base + idx as u64 + 1);
    }
    starts
}

/// Разбить плоский массив абсолютных начал строк на чанки по `TARGET` строк.
/// `data` — байты диапазона `[data_base, data_base + len)`, покрывающего все чанки
/// (для подсчёта символов в каждом чанке).
fn chunk_starts(starts: &[u64], total: u64, data: &[u8], data_base: u64) -> Vec<Chunk> {
    let chars_of = |from: u64, to: u64| -> u64 {
        let a = (from - data_base) as usize;
        let b = ((to - data_base) as usize).min(data.len());
        if a >= b {
            0
        } else {
            super::count_chars(&data[a..b]) as u64
        }
    };
    let mut chunks = Vec::with_capacity(starts.len() / TARGET + 1);
    let nlines = starts.len();
    let mut i = 0usize;
    while i < nlines {
        let j = (i + TARGET).min(nlines);
        let base = starts[i];
        let rel: Vec<u32> = starts[i..j]
            .iter()
            .map(|&s| u32::try_from(s - base).expect("чанк длиннее 4 ГБ не поддерживается"))
            .collect();
        let next_base = if j < nlines { starts[j] } else { total };
        chunks.push(Chunk {
            rel,
            byte_len: next_base - base,
            chars_len: chars_of(base, next_base),
        });
        i = j;
    }
    if chunks.is_empty() {
        chunks.push(Chunk { rel: vec![0], byte_len: total, chars_len: chars_of(0, total) });
    }
    chunks
}

/// Индекс начал строк с ленивыми префиксными суммами по чанкам.
pub struct LineIndex {
    chunks: Vec<Chunk>,
    byte_base: Vec<u64>,   // byte_base[i] = байт до чанка i
    line_base: Vec<usize>, // line_base[i] = строк до чанка i
    char_base: Vec<u64>,   // char_base[i] = символов до чанка i
    total_bytes: u64,
    line_count: usize,
    dirty: bool,
}

impl LineIndex {
    fn from_chunks(chunks: Vec<Chunk>) -> Self {
        Self {
            chunks,
            byte_base: Vec::new(),
            line_base: Vec::new(),
            char_base: Vec::new(),
            total_bytes: 0,
            line_count: 0,
            dirty: true,
        }
    }

    /// Построить индекс, просканировав весь буфер `data` на `\n`.
    #[allow(dead_code)] // используется в тестах; продакшен идёт через from_starts
    pub fn build(data: &[u8]) -> Self {
        let starts = scan_starts(data, 0);
        Self::from_chunks(chunk_starts(&starts, data.len() as u64, data, 0))
    }

    /// Построить индекс из готового массива начал строк (фоновая загрузка с прогрессом).
    pub fn from_starts(starts: &[u64], total: u64, data: &[u8]) -> Self {
        Self::from_chunks(chunk_starts(starts, total, data, 0))
    }

    /// Индекс пустого документа (одна пустая строка).
    pub fn empty() -> Self {
        Self::from_chunks(vec![Chunk { rel: vec![0], byte_len: 0, chars_len: 0 }])
    }

    fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.byte_base.clear();
        self.line_base.clear();
        self.char_base.clear();
        let mut sb = 0u64;
        let mut sl = 0usize;
        let mut sc = 0u64;
        for c in &self.chunks {
            self.byte_base.push(sb);
            self.line_base.push(sl);
            self.char_base.push(sc);
            sb += c.byte_len;
            sl += c.rel.len();
            sc += c.chars_len;
        }
        self.total_bytes = sb;
        self.line_count = sl;
        self.dirty = false;
    }

    /// База чанка, содержащего байт `offset`: (байтовое начало чанка, символов до чанка).
    /// Позиция в символах = база + count_chars(байты от начала чанка до offset).
    pub fn char_base_for_byte(&mut self, offset: u64) -> (u64, u64) {
        let ci = self.chunk_for_byte(offset);
        (self.byte_base[ci], self.char_base[ci])
    }

    pub fn line_count(&mut self) -> usize {
        self.refresh();
        self.line_count
    }

    pub fn total_bytes(&mut self) -> u64 {
        self.refresh();
        self.total_bytes
    }

    fn chunk_for_line(&mut self, n: usize) -> usize {
        self.refresh();
        if n == 0 {
            return 0;
        }
        if n >= self.line_count {
            return self.chunks.len() - 1;
        }
        self.line_base.partition_point(|&l| l <= n) - 1
    }

    fn chunk_for_byte(&mut self, offset: u64) -> usize {
        self.refresh();
        if offset == 0 {
            return 0;
        }
        if offset >= self.total_bytes {
            return self.chunks.len() - 1;
        }
        self.byte_base.partition_point(|&b| b <= offset) - 1
    }

    /// Байтовое смещение начала строки `n` (0-based). Для `n >= line_count` — конец документа.
    pub fn line_start(&mut self, n: usize) -> u64 {
        self.refresh();
        if n == 0 {
            return 0;
        }
        if n >= self.line_count {
            return self.total_bytes;
        }
        let ci = self.chunk_for_line(n);
        let local = n - self.line_base[ci];
        self.byte_base[ci] + self.chunks[ci].rel[local] as u64
    }

    /// Номер строки (0-based), содержащей байт `offset`.
    pub fn line_for_offset(&mut self, offset: u64) -> usize {
        self.refresh();
        if offset == 0 {
            return 0;
        }
        if offset >= self.total_bytes {
            return self.line_count.saturating_sub(1);
        }
        let ci = self.chunk_for_byte(offset);
        let local_off = u32::try_from(offset - self.byte_base[ci]).expect("чанк > 4 ГБ");
        let rel = &self.chunks[ci].rel;
        let local = rel.partition_point(|&r| r <= local_off) - 1;
        self.line_base[ci] + local
    }

    /// Обновить индекс после правки байт `[offset, offset+old_len)` → `new_len` байт.
    ///
    /// `read_fn(o, l)` читает УЖЕ ИЗМЕНЁННЫЕ байты документа. Перестраиваются только чанки,
    /// перекрытые правкой; хвостовые чанки сдвигаются лениво за счёт изменившейся длины
    /// перестроенных чанков.
    pub fn apply_edit(
        &mut self,
        offset: u64,
        old_len: u64,
        new_len: u64,
        read_fn: impl FnOnce(u64, usize) -> Vec<u8>,
    ) {
        self.refresh();
        let delta = new_len as i64 - old_len as i64;
        let old_total = self.total_bytes;
        let old_end = offset + old_len;

        // чанки, перекрытые правкой (в старых координатах). c1 по chunk_for_byte(old_end):
        // если old_end на границе чанка — включаем и его (удаление до границы стирает '\n').
        let c0 = self.chunk_for_byte(offset);
        let c1 = if old_total == 0 { 0 } else { self.chunk_for_byte(old_end.min(old_total)) };

        let b0 = self.byte_base[c0]; // начало первого затронутого чанка (не меняется)
        let is_last = c1 == self.chunks.len() - 1;
        let b1_old =
            if is_last { old_total } else { self.byte_base[c1] + self.chunks[c1].byte_len };
        let b1_new = (b1_old as i64 + delta) as u64;

        let region_len = (b1_new - b0) as usize;
        let region = if region_len > 0 { read_fn(b0, region_len) } else { Vec::new() };

        // новые абсолютные начала строк внутри перестроенной области
        let mut new_starts = scan_starts(&region, b0);
        if !is_last {
            // отбросить ложный «хвостовой» старт ровно на границе следующего чанка
            while new_starts.len() > 1 && *new_starts.last().unwrap() >= b1_new {
                new_starts.pop();
            }
        }
        // последний чанк: старт ровно на b1_new — законная пустая последняя строка.

        let new_chunks = chunk_starts(&new_starts, b1_new, &region, b0);
        self.chunks.splice(c0..=c1, new_chunks);
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Эталонная модель: пересобрать индекс с нуля и сравнить все начала строк.
    fn assert_matches(ix: &mut LineIndex, data: &[u8]) {
        let want = scan_starts(data, 0);
        assert_eq!(ix.line_count(), want.len(), "line_count");
        for (n, &w) in want.iter().enumerate() {
            assert_eq!(ix.line_start(n), w, "line_start({n})");
        }
        assert_eq!(ix.total_bytes(), data.len() as u64);
        // line_for_offset на каждом байте
        for off in 0..=data.len() as u64 {
            let line = want.partition_point(|&s| s <= off).saturating_sub(1);
            let expect = if off >= data.len() as u64 { want.len() - 1 } else { line };
            assert_eq!(ix.line_for_offset(off), expect, "line_for_offset({off})");
        }
    }

    #[test]
    fn build_simple() {
        let data = b"ab\ncd\n\nx";
        let mut ix = LineIndex::build(data);
        assert_eq!(ix.line_count(), 4);
        assert_eq!(ix.line_start(0), 0);
        assert_eq!(ix.line_start(1), 3);
        assert_eq!(ix.line_start(2), 6);
        assert_eq!(ix.line_start(3), 7);
        assert_matches(&mut ix, data);
    }

    #[test]
    fn trailing_newline_makes_empty_line() {
        let data = b"abc\n";
        let mut ix = LineIndex::build(data);
        assert_eq!(ix.line_count(), 2);
        assert_eq!(ix.line_start(1), 4);
    }

    #[test]
    fn empty_doc() {
        let mut ix = LineIndex::empty();
        assert_eq!(ix.line_count(), 1);
        assert_eq!(ix.line_start(0), 0);
        assert_eq!(ix.total_bytes(), 0);
    }

    /// Псевдослучайные правки: модель — Vec<u8>, индекс обновляется через apply_edit.
    #[test]
    fn random_edits_match_model() {
        let mut model: Vec<u8> = b"one\ntwo\nthree\nfour\nfive".to_vec();
        let mut ix = LineIndex::build(&model);
        // простая детерминированная «случайность»
        let mut seed = 0x12345678u64;
        let mut rnd = move |m: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % m.max(1)
        };
        for step in 0..500 {
            let insert = step % 3 != 0 || model.is_empty();
            if insert {
                let pos = rnd(model.len() + 1);
                let what: &[u8] = match rnd(5) {
                    0 => b"\n",
                    1 => b"x",
                    2 => b"ab\ncd",
                    3 => b"\n\n",
                    _ => b"longer text without newline",
                };
                model.splice(pos..pos, what.iter().copied());
                let m = model.clone();
                ix.apply_edit(pos as u64, 0, what.len() as u64, |o, l| {
                    m[o as usize..(o as usize + l).min(m.len())].to_vec()
                });
            } else {
                let pos = rnd(model.len());
                let len = rnd((model.len() - pos).min(7)) + 1;
                model.drain(pos..pos + len);
                let m = model.clone();
                ix.apply_edit(pos as u64, len as u64, 0, |o, l| {
                    m[o as usize..(o as usize + l).min(m.len())].to_vec()
                });
            }
            // сверяться полностью на каждом 25-м шаге (полная сверка дорогая)
            if step % 25 == 0 {
                let want = scan_starts(&model, 0);
                assert_eq!(ix.line_count(), want.len(), "step {step}");
                for (n, &w) in want.iter().enumerate() {
                    assert_eq!(ix.line_start(n), w, "step {step}, line {n}");
                }
            }
        }
        assert_matches(&mut ix, &model.clone());
    }

    /// Правки на границах чанков: документ больше TARGET строк.
    #[test]
    fn edits_across_chunk_boundary() {
        // 3 чанка по TARGET строк
        let mut model: Vec<u8> = Vec::new();
        for i in 0..(TARGET * 2 + 100) {
            model.extend_from_slice(format!("line{i}\n").as_bytes());
        }
        let mut ix = LineIndex::build(&model);
        let total_lines = ix.line_count();

        // удалить '\n' ровно на границе чанка TARGET
        let boundary = ix.line_start(TARGET); // начало строки TARGET
        let pos = (boundary - 1) as usize; // байт '\n' перед ней
        model.remove(pos);
        let m = model.clone();
        ix.apply_edit(pos as u64, 1, 0, |o, l| {
            m[o as usize..(o as usize + l).min(m.len())].to_vec()
        });
        assert_eq!(ix.line_count(), total_lines - 1);
        let want = scan_starts(&model, 0);
        for (n, &w) in want.iter().enumerate() {
            assert_eq!(ix.line_start(n), w, "after boundary delete, line {n}");
        }

        // вставить блок строк в середину первого чанка
        let ins = b"a\nb\nc\n";
        model.splice(10..10, ins.iter().copied());
        let m = model.clone();
        ix.apply_edit(10, 0, ins.len() as u64, |o, l| {
            m[o as usize..(o as usize + l).min(m.len())].to_vec()
        });
        let want = scan_starts(&model, 0);
        assert_eq!(ix.line_count(), want.len());
        for (n, &w) in want.iter().enumerate() {
            assert_eq!(ix.line_start(n), w, "after insert, line {n}");
        }
    }
}
