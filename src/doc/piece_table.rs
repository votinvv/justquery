//!
//! Piece table над байтами UTF-8.

#![allow(dead_code)] // модель документа: эта сборка использует не весь её API
//!
//! Документ — последовательность «пьес», каждая ссылается на неизменяемый origin-буфер
//! (mmap файла) или на add-буфер (накопитель правок). Координаты — байтовые. Открытие O(1)
//! (одна пьеса), правки затрагивают ≤ 2 пьес.
//!
//! Поиск пьесы по офсету — бинарный по лениво пересчитываемому массиву префиксных сумм длин
//! пьес. Число пьес ≈ числу правок (не зависит от размера файла).

use std::sync::Arc;

/// Неизменяемый origin-буфер: пустой, в памяти или mmap файла.
#[allow(dead_code)] // Mem используется тестами
pub enum OriginBuf {
    Empty,
    Mem(Vec<u8>),
    Mmap(memmap2::Mmap),
}

impl OriginBuf {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            OriginBuf::Empty => &[],
            OriginBuf::Mem(v) => v,
            OriginBuf::Mmap(m) => m,
        }
    }
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Src {
    Origin,
    Add,
}

/// Фрагмент текста: ссылка в буфер `src` на `[start, start+len)`.
#[derive(Clone, Copy, Debug)]
struct Piece {
    src: Src,
    start: usize,
    len: usize,
}

/// Буфер байт с дешёвыми вставками/удалениями в произвольной позиции.
pub struct PieceTable {
    origin: Arc<OriginBuf>,
    add: Vec<u8>,
    pieces: Vec<Piece>,
    total: usize,
    cum: Vec<usize>, // cum[i] = число байт до пьесы i
    cum_dirty: bool,
}

impl PieceTable {
    pub fn new(origin: Arc<OriginBuf>) -> Self {
        let n = origin.len();
        let mut pieces = Vec::new();
        if n > 0 {
            pieces.push(Piece { src: Src::Origin, start: 0, len: n });
        }
        Self { origin, add: Vec::new(), pieces, total: n, cum: Vec::new(), cum_dirty: true }
    }

    pub fn empty() -> Self {
        Self::new(Arc::new(OriginBuf::Empty))
    }

    fn buf(&self, src: Src) -> &[u8] {
        match src {
            Src::Origin => self.origin.as_slice(),
            Src::Add => &self.add,
        }
    }

    fn rebuild_cum(&mut self) {
        self.cum.clear();
        let mut s = 0usize;
        for p in &self.pieces {
            self.cum.push(s);
            s += p.len;
        }
        self.cum_dirty = false;
    }

    /// Вернуть `(индекс_пьесы, смещение_внутри)` для байтового офсета.
    /// Для `offset >= total` возвращает `(pieces.len(), 0)` — позиция «в конце».
    fn find(&mut self, offset: usize) -> (usize, usize) {
        if offset >= self.total {
            return (self.pieces.len(), 0);
        }
        if self.cum_dirty {
            self.rebuild_cum();
        }
        // bisect_right(cum, offset) - 1
        let i = self.cum.partition_point(|&c| c <= offset) - 1;
        (i, offset - self.cum[i])
    }

    #[allow(dead_code)] // используется тестами и full_text
    pub fn len(&self) -> usize {
        self.total
    }

    /// Прочитать `length` байт начиная с `offset` (срез по пьесам).
    pub fn read(&mut self, offset: usize, length: usize) -> Vec<u8> {
        if length == 0 || offset >= self.total {
            return Vec::new();
        }
        let end = (offset + length).min(self.total);
        let mut remaining = end - offset;
        let (mut i, mut off) = self.find(offset);
        let mut out = Vec::with_capacity(remaining);
        while remaining > 0 && i < self.pieces.len() {
            let p = self.pieces[i];
            let avail = p.len - off;
            let take = avail.min(remaining);
            let s = p.start + off;
            out.extend_from_slice(&self.buf(p.src)[s..s + take]);
            remaining -= take;
            i += 1;
            off = 0;
        }
        out
    }

    /// Вставить байты `data` в позицию `offset`.
    pub fn insert(&mut self, offset: usize, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let add_start = self.add.len();
        self.add.extend_from_slice(data);
        let new_piece = Piece { src: Src::Add, start: add_start, len: data.len() };
        let (i, off) = self.find(offset);
        if off == 0 {
            self.pieces.insert(i, new_piece);
        } else {
            let p = self.pieces[i];
            let left = Piece { src: p.src, start: p.start, len: off };
            let right = Piece { src: p.src, start: p.start + off, len: p.len - off };
            self.pieces.splice(i..=i, [left, new_piece, right]);
        }
        self.total += data.len();
        self.cum_dirty = true;
    }

    /// Удалить `length` байт с `offset`. Вернуть удалённые байты (для undo).
    pub fn delete(&mut self, offset: usize, length: usize) -> Vec<u8> {
        if length == 0 || offset >= self.total {
            return Vec::new();
        }
        let length = length.min(self.total - offset);
        if length == 0 {
            return Vec::new();
        }
        let deleted = self.read(offset, length);
        let (i, off) = self.find(offset);
        let (end_i, end_off) = self.find(offset + length);
        let mut new_pieces: Vec<Piece> = Vec::with_capacity(2);
        if off > 0 {
            let p = self.pieces[i];
            new_pieces.push(Piece { src: p.src, start: p.start, len: off });
        }
        let end_slice = if end_off > 0 {
            let p = self.pieces[end_i];
            new_pieces.push(Piece { src: p.src, start: p.start + end_off, len: p.len - end_off });
            end_i + 1
        } else {
            end_i
        };
        self.pieces.splice(i..end_slice, new_pieces);
        self.total -= length;
        self.cum_dirty = true;
        deleted
    }

    /// Потоково записать всё содержимое в `w`.
    pub fn write_to<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        for p in &self.pieces {
            w.write_all(&self.buf(p.src)[p.start..p.start + p.len])?;
        }
        Ok(())
    }

    /// Текущее число пьес (диагностика фрагментации).
    #[allow(dead_code)]
    pub fn piece_count(&self) -> usize {
        self.pieces.len()
    }

    /// Неизменяемый снимок содержимого для фоновых процессов (поиск/валидация).
    /// Origin разделяется по Arc; add-буфер копируется (он мал — это набранные правки).
    pub fn snapshot(&self) -> PieceSnapshot {
        PieceSnapshot {
            origin: Arc::clone(&self.origin),
            add: Arc::from(self.add.as_slice()),
            pieces: Arc::from(self.pieces.as_slice()),
            total: self.total,
        }
    }

    pub(super) fn origin_arc(&self) -> Arc<OriginBuf> {
        Arc::clone(&self.origin)
    }
}

/// Снимок piece table: читается из любого потока, не блокируя правки в UI.
#[derive(Clone)]
pub struct PieceSnapshot {
    origin: Arc<OriginBuf>,
    add: Arc<[u8]>,
    pieces: Arc<[Piece]>,
    total: usize,
}

impl PieceSnapshot {
    pub fn len(&self) -> usize {
        self.total
    }

    fn buf(&self, src: Src) -> &[u8] {
        match src {
            Src::Origin => self.origin.as_slice(),
            Src::Add => &self.add,
        }
    }

    /// Последовательный читатель всего содержимого (для quick-xml / поиска).
    pub fn reader(&self) -> SnapReader<'_> {
        SnapReader { snap: self, piece: 0, off: 0 }
    }

    /// Обойти содержимое кусками-слайсами (нулевое копирование).
    pub fn for_each_chunk(&self, mut f: impl FnMut(&[u8])) {
        for p in self.pieces.iter() {
            f(&self.buf(p.src)[p.start..p.start + p.len]);
        }
    }
}

/// `std::io::Read` поверх снимка piece table.
pub struct SnapReader<'a> {
    snap: &'a PieceSnapshot,
    piece: usize,
    off: usize,
}

impl std::io::Read for SnapReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0usize;
        while written < buf.len() && self.piece < self.snap.pieces.len() {
            let p = self.snap.pieces[self.piece];
            let src = &self.snap.buf(p.src)[p.start + self.off..p.start + p.len];
            if src.is_empty() {
                self.piece += 1;
                self.off = 0;
                continue;
            }
            let take = src.len().min(buf.len() - written);
            buf[written..written + take].copy_from_slice(&src[..take]);
            written += take;
            self.off += take;
            if self.off >= p.len {
                self.piece += 1;
                self.off = 0;
            }
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(origin: &[u8]) -> PieceTable {
        PieceTable::new(Arc::new(OriginBuf::Mem(origin.to_vec())))
    }

    fn text(p: &mut PieceTable) -> String {
        let n = p.len();
        String::from_utf8(p.read(0, n)).unwrap()
    }

    #[test]
    fn insert_middle() {
        let mut p = pt(b"hello world");
        p.insert(5, b",");
        assert_eq!(text(&mut p), "hello, world");
        assert_eq!(p.len(), 12);
    }

    #[test]
    fn insert_at_ends() {
        let mut p = pt(b"bc");
        p.insert(0, b"a");
        p.insert(3, b"d");
        assert_eq!(text(&mut p), "abcd");
    }

    #[test]
    fn delete_returns_old() {
        let mut p = pt(b"abcdef");
        let old = p.delete(1, 3);
        assert_eq!(old, b"bcd");
        assert_eq!(text(&mut p), "aef");
    }

    #[test]
    fn delete_across_pieces() {
        let mut p = pt(b"abc");
        p.insert(1, b"XY"); // aXYbc
        let old = p.delete(0, 4); // удаляем через 3 пьесы
        assert_eq!(old, b"aXYb");
        assert_eq!(text(&mut p), "c");
    }

    #[test]
    fn empty_origin() {
        let mut p = PieceTable::empty();
        assert_eq!(p.len(), 0);
        p.insert(0, b"hi");
        assert_eq!(text(&mut p), "hi");
    }

    #[test]
    fn snapshot_is_stable() {
        let mut p = pt(b"abc");
        p.insert(3, b"def");
        let snap = p.snapshot();
        p.delete(0, 6);
        assert_eq!(p.len(), 0);
        let mut out = Vec::new();
        use std::io::Read;
        snap.reader().read_to_end(&mut out).unwrap();
        assert_eq!(out, b"abcdef");
    }

    #[test]
    fn read_partial() {
        let mut p = pt(b"0123456789");
        p.insert(5, b"abc"); // 01234abc56789
        assert_eq!(p.read(3, 7), b"34abc56");
    }
}
