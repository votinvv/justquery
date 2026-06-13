//! Каркас фоновых процессов вкладки: поиск / валидация / форматирование.
//!
//! На вкладке может выполняться не более одного процесса; вкладки независимы. Воркер живёт
//! в своём потоке, шлёт сообщения по каналу (опрашивается в update-цикле), останавливается
//! по `AtomicBool`. Результаты копятся в памяти; при превышении лимита 100 МБ процесс
//! останавливается, накопленное показывается, в статус-бар уходит ошибка.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Лимит памяти на накопленные результаты процесса (только результаты, не данные файла).
pub const RESULTS_CAP_BYTES: usize = 100 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcKind {
    Search,
    Validate,
    Format,
}

impl ProcKind {
    pub fn title(self) -> &'static str {
        match self {
            ProcKind::Search => "Поиск",
            ProcKind::Validate => "Валидация",
            ProcKind::Format => "Форматирование",
        }
    }
    /// «завершён/завершена/завершено» по роду.
    pub fn finished_word(self) -> &'static str {
        match self {
            ProcKind::Search => "завершён",
            ProcKind::Validate => "завершена",
            ProcKind::Format => "завершено",
        }
    }
    /// «остановлен/остановлена/остановлено» по роду.
    pub fn stopped_word(self) -> &'static str {
        match self {
            ProcKind::Search => "остановлен",
            ProcKind::Validate => "остановлена",
            ProcKind::Format => "остановлено",
        }
    }
}

/// Совпадение поиска. `line`/`col` — 0-based, `col`/`len` в кодовых точках.
pub struct SearchMatch {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub preview: String,
}

impl SearchMatch {
    /// Оценка памяти записи (для лимита 100 МБ).
    pub fn approx_bytes(&self) -> usize {
        self.preview.capacity() + std::mem::size_of::<Self>()
    }
}

/// Серьёзность находки валидации.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "Ошибка",
            Severity::Warning => "Предупр.",
        }
    }
}

/// Находка валидации (XSD или правила разделов 5/6).
pub struct Finding {
    pub severity: Severity,
    /// 1-based строка; 0 — неизвестна.
    pub line: usize,
    pub code: String,
    pub message: String,
    /// Источник: «XSD», «п. 5.6», «р. 6, стр. 83»… (в гриде не выводится — дублирует «Код»).
    #[allow(dead_code)]
    pub source: String,
}

impl Finding {
    pub fn approx_bytes(&self) -> usize {
        self.code.capacity()
            + self.message.capacity()
            + self.source.capacity()
            + std::mem::size_of::<Self>()
    }
}

/// Сообщения воркера процесса.
pub enum ProcMsg {
    Progress(f32), // 0..100
    SearchBatch(Vec<SearchMatch>),
    Findings(Vec<Finding>),
    /// Форматирование успешно: путь к временному UTF-8 файлу результата.
    FormatOk { out_path: PathBuf, changed: bool },
    /// Ошибка форматирования: непригодный XML. `line`/`col` — 1-based.
    FormatErr { line: usize, col: usize, msg: String },
    /// Успешное завершение (поиск/валидация).
    Done,
    /// Воркер увидел флаг отмены и вышел.
    Cancelled,
    /// Фатальная ошибка процесса.
    Failed(String),
}

/// Выполняющийся процесс вкладки.
pub struct RunningProc {
    pub kind: ProcKind,
    pub rx: std::sync::mpsc::Receiver<ProcMsg>,
    pub cancel: Arc<AtomicBool>,
    pub started: std::time::Instant,
    pub progress: f32,
    /// Версия схемы для сообщений (валидация); пусто для остальных.
    pub schema: String,
    /// Лимит результатов превышен — процесс остановлен принудительно.
    pub capped: bool,
}

impl RunningProc {
    /// Подпись процесса для статус-бара: «Валидация (схема 5.1)» / «Поиск».
    pub fn label(&self) -> String {
        if self.schema.is_empty() {
            self.kind.title().to_owned()
        } else {
            format!("{} (схема {})", self.kind.title(), self.schema)
        }
    }
}

/// Накопленные результаты последнего поиска/валидации на вкладке.
pub enum ResultsKind {
    Search(Vec<SearchMatch>),
    Validation(Vec<Finding>),
}

pub struct Results {
    pub kind: ResultsKind,
    /// Модель отображения грида (колонки/ширины/порядок) — у каждого вида своя таблица.
    pub grid: crate::grid::GridModel,
    /// Оценка занятой памяти (для лимита).
    pub bytes: usize,
    /// Лимит был превышен — показана только часть.
    pub truncated: bool,
}

impl Results {
    pub fn new_search() -> Self {
        let grid = crate::grid::GridModel::new(&[
            ("Строка", 80.0),
            ("Колонка", 80.0),
            ("Фрагмент", 640.0),
        ]);
        Self { kind: ResultsKind::Search(Vec::new()), grid, bytes: 0, truncated: false }
    }
    pub fn new_validation() -> Self {
        let grid = crate::grid::GridModel::new(&[
            ("Тип", 90.0),
            ("Строка", 80.0),
            ("Код", 100.0),
            ("Сообщение", 640.0),
        ]);
        Self { kind: ResultsKind::Validation(Vec::new()), grid, bytes: 0, truncated: false }
    }
    pub fn len(&self) -> usize {
        match &self.kind {
            ResultsKind::Search(v) => v.len(),
            ResultsKind::Validation(v) => v.len(),
        }
    }

    /// Значения ячеек строки `r` в порядке колонок (единый источник для грида и высот).
    pub fn row_values(&self, r: usize) -> Vec<String> {
        match &self.kind {
            ResultsKind::Search(v) => {
                let m = &v[r];
                vec![(m.line + 1).to_string(), (m.col + 1).to_string(), m.preview.clone()]
            }
            ResultsKind::Validation(v) => {
                let f = &v[r];
                vec![
                    f.severity.label().to_owned(),
                    if f.line > 0 { f.line.to_string() } else { "—".to_owned() },
                    f.code.clone(),
                    f.message.clone(),
                ]
            }
        }
    }

    /// Строка `r` с ошибкой (для красной полосы/текста в гриде валидации).
    pub fn row_is_err(&self, r: usize) -> bool {
        matches!(&self.kind, ResultsKind::Validation(v) if v[r].severity == Severity::Error)
    }
}
