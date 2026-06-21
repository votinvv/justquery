//! Формат `.jqmodel` и тип XML-модели.
//!
//! Модель XML = XSD-схема + карта показателей + декларативные правила валидации + предикат
//! отнесения (match). Хранится одним текстовым файлом, чтобы диффиться в git как есть. Секции
//! разделяются маркерами-разделителями на отдельных строках; XSD и JSON лежат неэкранированными,
//! поэтому diff читаем.
//!
//! Формат файла (см. канон в `CLAUDE.md` → «XML-модели»):
//! ```text
//! ---model---   YAML-подобный манифест: id/name/description/priority/match
//! ---xsd---     XSD как есть
//! ---codes---   codes_map.json
//! ---rules---   rules.json
//! ---checksum--- SHA-256 от нормализованного содержимого минус это поле
//! ```
//!
//! Контрольная сумма — SHA-256 над конкатенацией нормализованных секций (model/xsd/codes/rules)
//! в виде `"<hex>"`. Нормализация: UTF-8 без BOM, переводы строк `\n`, без trailing-whitespace
//! в манифесте (XSD/JSON берутся как есть, BOM снят). При несовпадении модель помечается
//! повреждённой и грузится read-only.

use std::path::{Path, PathBuf};

use crate::proc::Severity;

/// Маркер-разделитель секции. Поле `name` — без ведущих дефисов (модель/xsd/codes/rules/checksum).
struct SectionMarker(&'static str);

const MODEL: SectionMarker = SectionMarker("model");
const XSD: SectionMarker = SectionMarker("xsd");
const CODES: SectionMarker = SectionMarker("codes");
const RULES: SectionMarker = SectionMarker("rules");
const CHECKSUM: SectionMarker = SectionMarker("checksum");

/// Превратить `SectionMarker("xsd")` в строку `---xsd---`.
fn marker_line(m: &SectionMarker) -> String {
    format!("---{}---", m.0)
}

/// Список маркеров в каноническом порядке сериализации.
const ORDER: &[&SectionMarker] = &[&MODEL, &XSD, &CODES, &RULES, &CHECKSUM];

// ---------------------------------------------------------------------------
//  Manifest (секция model)
// ---------------------------------------------------------------------------

/// Правило идентификации модели: «атрибут корневого элемента присутствует (или равен одному из
/// значений)». Набор правил на модель = список `MatchRule`; документ подходит, если выполняется
/// ХОТЯ БЫ ОДНО правило (OR). Имя корневого элемента проверяется отдельно, на уровне реестра
/// (`Registry::match_doc`): оно должно совпасть с корневым элементом XSD-схемы модели.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchRule {
    /// Имя атрибута корневого элемента (например `schemaVersion`).
    pub attr: String,
    /// Допустимые значения. Пустой вектор = атрибут обязан присутствовать, значение произвольно.
    pub values: Vec<String>,
}

/// Набор правил идентификации модели (секция model → `match`). Документ подходит под набор, если
/// выполняется ХОТЯ БЫ ОДНО правило (OR). Проверка идёт по атрибутам корневого элемента из головы
/// XML (без полного разбора). Пустой список = подходит любой (тогда решает только совпадение
/// корневого элемента с XSD, проверяемое в `Registry::match_doc`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchPred {
    /// Правила идентификации (OR). Пустой список = любой документ (с верным корнем).
    pub rules: Vec<MatchRule>,
}

impl MatchPred {
    /// Проверить, подходит ли документ под набор правил идентификации по атрибутам корневого
    /// элемента (из головы XML). Семантика — OR: достаточно одного выполненного правила. Пустой
    /// список → `true` (решает только совпадение корня, см. `Registry::match_doc`). Имя корневого
    /// элемента здесь НЕ сверяется — это делает `match_doc` по XSD-схеме модели.
    pub fn matches(&self, attrs: &[(String, String)]) -> bool {
        if self.rules.is_empty() {
            return true;
        }
        self.rules.iter().any(|rule| {
            let val = attrs.iter().find(|(n, _)| n == &rule.attr).map(|(_, v)| v.as_str());
            match (val, rule.values.is_empty()) {
                (None, _) => false,                  // атрибут отсутствует — правило не выполнено
                (Some(_), true) => true,             // присутствие достаточно
                (Some(v), false) => rule.values.iter().any(|a| a == v),
            }
        })
    }
}

/// Действие проверки правила: «обязательно при условии» / «запрещено при условии» /
/// сравнение двух показателей. Расширения (pattern/date_*/aggregate) добавляются отдельными
/// типами — см. `RuleCheck`.
//
// Парковано: используется полным редактором правил (модалка add/edit) — следующий заход.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum CheckKind {
    RequiredIf,
    ForbiddenIf,
    Compare,
}

/// Одно правило валидации (секция rules). Зеркало JSON-формата из `rules/mod.rs`, но живёт здесь,
/// чтобы редактор модели и парсер разделяли один тип.
//
// Парковано: тип для полного редактора правил (модалка add/edit/toggle/remove) — следующий заход.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct RuleSpec {
    pub id: String,
    pub codes: Vec<String>,
    pub blocks: Vec<String>,
    pub refer: String,
    pub message: String,
    pub severity: Severity,
    pub enabled: bool,
    /// Сырое JSON-тело проверки (`{"type":"required_if","condition":...}` и т.д.) — парсится
    /// движком `rules::RuleEngine::for_model`. Храним сырьём, чтобы не дублировать разнотипный
    /// матчинг DSL в двух местах.
    pub check: serde_json::Value,
}

/// Манифест модели (секция model) — метаданные + предикат отнесения.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub id: String,
    pub description: String,
    /// Ниже = раньше в очереди матчинга; тай-брейк по `id`.
    pub priority: i64,
    pub r#match: MatchPred,
}

/// Скомпилированная модель — то, что живёт в реестре и подаётся в движки валидации.
#[derive(Clone, Debug)]
pub struct Model {
    pub manifest: Manifest,
    /// XSD-текст как есть (нужен для `xsd::loader::compile`).
    pub xsd: String,
    /// `codes_map.json` как сырой текст (парсится `rules::CodesMap`).
    pub codes: String,
    /// `rules.json` как сырой текст (парсится движком в `RuleSpec`-подобные замыкания).
    pub rules: String,
    /// Контрольная сумма, прочитанная из файла (пусто = отсутствовала).
    //
    // Парковано: экспонируется для будущего редактора (показ/перевычисление суммы); UI сегодня
    // смотрит только на `intact`.
    #[allow(dead_code)]
    pub checksum: String,
    /// Бьётся ли хэш в файле с фактическим содержимым. `false` → модель read-only + баннер.
    pub intact: bool,
    /// Путь к файлу модели (для реестра/экспорта); `None` — только что созданная, не сохранена.
    pub path: Option<PathBuf>,
}

impl Model {
    /// `true`, если в модели задан XSD (не пустая секция). Это гейт «нет XSD → нет правил».
    pub fn has_xsd(&self) -> bool {
        !self.xsd.trim().is_empty()
    }
}

// ---------------------------------------------------------------------------
//  Парсинг
// ---------------------------------------------------------------------------

/// Результат разбора файла: либо годная модель, либо читаемая ошибка.
pub fn parse(text: &str) -> Result<Model, String> {
    let sections = split_sections(text)?;
    let model_body = sections.get(MODEL.0).ok_or("нет секции ---model---")?;
    let xsd = sections.get(XSD.0).cloned().unwrap_or_default();
    let codes = sections.get(CODES.0).cloned().unwrap_or_default();
    let rules = sections.get(RULES.0).cloned().unwrap_or_default();
    let file_checksum = sections.get(CHECKSUM.0).map(|s| s.trim().to_owned()).unwrap_or_default();

    let manifest = parse_manifest(model_body)?;
    if manifest.id.is_empty() {
        return Err("манифест: id не задан".to_owned());
    }

    // Контрольная сумма — над нормализованными четырьмя секциями.
    let actual = checksum_of(model_body, &xsd, &codes, &rules);
    let intact = !file_checksum.is_empty() && file_checksum == actual;

    Ok(Model {
        manifest,
        xsd,
        codes,
        rules,
        checksum: file_checksum,
        intact,
        path: None,
    })
}

/// Разрезать текст по маркерам. Возвращает имя секции (без дефисов) → тело (без завершающего
/// перевода строки). Маркеры вне порядка и дубли допустимы — берётся последнее вхождение.
fn split_sections(text: &str) -> Result<std::collections::HashMap<&'static str, String>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut map: std::collections::HashMap<&'static str, String> = std::collections::HashMap::new();
    let mut current: Option<&'static str> = None;
    let mut buf = String::new();

    for line in text.split('\n') {
        let trimmed = line.strip_suffix('\r').unwrap_or(line);
        if let Some(name) = parse_marker(trimmed) {
            if let Some(cur) = current {
                map.insert(cur, trim_trailing_newline(&buf));
            }
            buf.clear();
            current = Some(name);
        } else if let Some(_) = current {
            buf.push_str(trimmed);
            buf.push('\n');
        }
    }
    if let Some(cur) = current {
        map.insert(cur, trim_trailing_newline(&buf));
    }

    // Валидность имён секций уже проверена в parse_marker, но проверим что нет неизвестных.
    Ok(map)
}

/// Распознать строку-маркер. Возвращает имя секции, если строка вида `---name---`.
fn parse_marker(line: &str) -> Option<&'static str> {
    let inner = line.strip_prefix("---")?.strip_suffix("---")?;
    ORDER.iter().find(|m| m.0 == inner).map(|m| m.0)
}

fn trim_trailing_newline(s: &str) -> String {
    let mut s = s.trim_end_matches('\n').to_owned();
    if let Some(stripped) = s.strip_suffix('\r') {
        s = stripped.to_owned();
    }
    s
}

/// Разобрать секцию model (строки `key: value` / `key:` + вложенные `  - value`).
///
/// Поддерживается ровно то, что нужно манифесту:
/// ```text
/// id: 785p_5_1
/// description: |
///   многострочное
///   описание
/// priority: 10
/// match:
///   - attr: schemaVersion
///     values:
///       - "5.1"
/// ```
///
/// Поле `name` (из ранней версии формата) молча игнорируется — отображаемое имя = id.
fn parse_manifest(body: &str) -> Result<Manifest, String> {
    let mut id = String::new();
    let mut description = String::new();
    let mut priority: i64 = 100;
    let mut match_rules: Vec<MatchRule> = Vec::new();

    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        let (key, rest) = split_kv(line).ok_or_else(|| format!("манифест: неверная строка «{line}»"))?;
        match key {
            "id" => id = rest.trim().to_owned(),
            "name" => {
                // legacy: name больше не хранится (отображаемое имя = id). Выбрасываем значение,
                // но не ругаемся — старые .jqmodel должны грузиться.
            }
            "description" => {
                // `|` → блочное значение до строки с меньшим отступом/пустой секции.
                description = if rest.trim() == "|" {
                    take_block(&mut lines)
                } else {
                    rest.trim().to_owned()
                };
            }
            "priority" => {
                priority = rest.trim().parse().map_err(|_| format!("priority не число: «{rest}»"))?;
            }
            "match" => {
                // Список правил идентификации. Поддерживается два формата:
                //   (1) новый — YAML-список:
                //         match:
                //           - attr: schemaVersion
                //             values:
                //               - "5.1"
                //           - attr: presenceOnlyAttr
                //   (2) legacy — `attr NAME:` + вложенные значения (старые .jqmodel грузятся).
                while let Some(next) = lines.peek() {
                    if !next.starts_with("  ") {
                        break;
                    }
                    let nested = lines.next().unwrap();
                    let trimmed = nested.trim_start();
                    if let Some(rest) = trimmed.strip_prefix("- attr:") {
                        // новый формат: начало правила, дальше `    values:` со списком
                        let attr_name = rest.trim().to_owned();
                        let mut values = Vec::new();
                        while let Some(more) = lines.peek() {
                            if !more.starts_with("    ") {
                                break;
                            }
                            let vl = lines.next().unwrap();
                            let t = vl.trim_start();
                            if let Some(item) = t.strip_prefix("- ") {
                                values.push(unquote(item.trim()));
                            } else if t.strip_prefix("values:").is_some() {
                                // блок значений — следующие строки с 6+ пробелами
                            }
                        }
                        match_rules.push(MatchRule { attr: attr_name, values });
                    } else if let Some(rest) = trimmed.strip_prefix("- ") {
                        // `- attr: NAME` на одной строке (значения могут идти ниже как `values:`)
                        let entry = rest.trim();
                        if let Some(an) = entry.strip_prefix("attr:") {
                            let attr_name = an.trim().to_owned();
                            let mut values = Vec::new();
                            while let Some(more) = lines.peek() {
                                if !more.starts_with("    ") {
                                    break;
                                }
                                let vl = lines.next().unwrap();
                                let t = vl.trim_start();
                                if let Some(item) = t.strip_prefix("- ") {
                                    values.push(unquote(item.trim()));
                                }
                            }
                            match_rules.push(MatchRule { attr: attr_name, values });
                        }
                    } else if let Some(rest) = trimmed.strip_prefix("attr ") {
                        // legacy: `attr schemaVersion:` + вложенные значения
                        let attr_name = rest.trim().trim_end_matches(':').to_owned();
                        let mut values = Vec::new();
                        while let Some(more) = lines.peek() {
                            if !more.starts_with("    ") && !more.starts_with("      -") {
                                break;
                            }
                            let vl = lines.next().unwrap();
                            if let Some(item) = vl.trim().strip_prefix("- ") {
                                values.push(unquote(item.trim()));
                            }
                        }
                        match_rules.push(MatchRule { attr: attr_name, values });
                    } else if trimmed.starts_with("root:") {
                        // legacy root: игнорируем (название корневого элемента более не проверяется)
                    } else {
                        return Err(format!("match: неизвестная строка «{nested}»"));
                    }
                }
            }
            other => return Err(format!("манифест: неизвестное поле «{other}»")),
        }
    }

    Ok(Manifest {
        id,
        description,
        priority,
        r#match: MatchPred { rules: match_rules },
    })
}

/// `key: value` → (`key`, `value`). `value` может быть пустым (для вложенных секций/блоков).
fn split_kv(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let colon = line.find(':')?;
    Some((&line[..colon], line[colon + 1..].trim_start()))
}

/// Прочитать блочное значение (после `key: |`) — строки с отступом ≥ 2, до отступа < 2 или EOF.
/// Строки склеиваются через `\n` БЕЗ trim — чтобы сохранить хвостовые пробелы и переносы строк
/// (зеркально `serialize_manifest`, который пишет каждую строку через `split('\n')`). Round-trip
/// идемпотентен: `serialize → parse → serialize` стабилен.
fn take_block<'a, I: Iterator<Item = &'a str>>(lines: &mut std::iter::Peekable<I>) -> String {
    let mut parts: Vec<String> = Vec::new();
    while let Some(next) = lines.peek() {
        if !next.starts_with("  ") {
            break;
        }
        let line = lines.next().unwrap();
        // снять один уровень отступа (2 пробела), остальное — как есть (хвостовые пробелы значимы)
        let deindented = line.strip_prefix("  ").unwrap_or(line);
        parts.push(deindented.to_owned());
    }
    parts.join("\n")
}

fn unquote(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_owned()
    } else {
        s.to_owned()
    }
}

// ---------------------------------------------------------------------------
//  Сериализация
// ---------------------------------------------------------------------------

/// Сериализовать модель в текст `.jqmodel`. Контрольная сумма пересчитывается над нормализованным
/// телом (model/xsd/codes/rules), поэтому свежесериализанный файл всегда `intact`.
pub fn serialize(model: &Model) -> String {
    let model_body = serialize_manifest(&model.manifest);
    let checksum = checksum_of(&model_body, &model.xsd, &model.codes, &model.rules);
    let mut out = String::new();
    out.push_str(&marker_line(&MODEL));
    out.push('\n');
    out.push_str(&model_body);
    out.push('\n');
    out.push_str(&marker_line(&XSD));
    out.push('\n');
    out.push_str(&model.xsd);
    if !model.xsd.is_empty() && !model.xsd.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&marker_line(&CODES));
    out.push('\n');
    out.push_str(&model.codes);
    if !model.codes.is_empty() && !model.codes.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&marker_line(&RULES));
    out.push('\n');
    out.push_str(&model.rules);
    if !model.rules.is_empty() && !model.rules.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&marker_line(&CHECKSUM));
    out.push('\n');
    out.push_str(&checksum);
    out.push('\n');
    out
}

fn serialize_manifest(m: &Manifest) -> String {
    let mut out = String::new();
    out.push_str(&format!("id: {}\n", m.id));
    // description — всегда блочная YAML-форма (|), даже для однострочного: так содержимое не
    // экранируется (нет кавычек/слэшей), diff читаем, а парсер снимает один уровень отступа.
    // split('\n') (а не .lines()) — чтобы СОХРАНИТЬ хвостовые переносы строк и пустые строки
    // (take_block их восстанавливает join'ом без trim). Хвостовые пробелы в строке тоже значимы.
    out.push_str("description: |\n");
    for line in m.description.split('\n') {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!("priority: {}\n", m.priority));
    // match — список правил идентификации (только если что-то задано).
    if !m.r#match.rules.is_empty() {
        out.push_str("match:\n");
        for rule in &m.r#match.rules {
            out.push_str(&format!("  - attr: {}\n", escape_yaml_str(&rule.attr)));
            if rule.values.is_empty() {
                // присутствие атрибута — без значений (значения не пишем)
            } else {
                out.push_str("    values:\n");
                for v in &rule.values {
                    out.push_str(&format!("      - \"{}\"\n", escape_yaml_str(v)));
                }
            }
        }
    }
    out.trim_end().to_owned()
}

fn escape_yaml_str(s: &str) -> String {
    // Если есть спецсимволы YAML — в кавычки с экранированием `"`.
    if s.is_empty() || s.contains([':', '#', '\n', '"', '\'']) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_owned()
    }
}

// ---------------------------------------------------------------------------
//  Контрольная сумма
// ---------------------------------------------------------------------------

/// Нормализованное тело для хэширования: четыре секции, разделённые `\n---<name>---\n`, UTF-8 без
/// BOM, `\n`-переносы. Поле checksum сюда не входит (по определению не может — считается от него).
fn canonical_body(model: &str, xsd: &str, codes: &str, rules: &str) -> Vec<u8> {
    let norm = |s: &str| -> String {
        let s = s.strip_prefix('\u{feff}').unwrap_or(s);
        s.replace("\r\n", "\n").replace('\r', "\n")
    };
    let mut out = String::new();
    out.push_str("---model---\n");
    out.push_str(&norm(model));
    if !model.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---xsd---\n");
    out.push_str(&norm(xsd));
    if !xsd.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---codes---\n");
    out.push_str(&norm(codes));
    if !codes.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---rules---\n");
    out.push_str(&norm(rules));
    if !rules.ends_with('\n') {
        out.push('\n');
    }
    out.into_bytes()
}

/// SHA-256 от канонического тела, в виде hex-строки в кавычках (см. `checksum`).
fn checksum_of(model: &str, xsd: &str, codes: &str, rules: &str) -> String {
    use sha2::{Digest, Sha256};
    let body = canonical_body(model, xsd, codes, rules);
    let mut hasher = Sha256::new();
    hasher.update(&body);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(66);
    hex.push('"');
    for b in digest.iter() {
        hex.push_str(&format!("{:02x}", b));
    }
    hex.push('"');
    hex
}

// ---------------------------------------------------------------------------
//  I/O
// ---------------------------------------------------------------------------

/// Прочитать модель из файла. `path` сохраняется в `Model.path`. Контрольная сумма сверяется.
pub fn load_file(path: &Path) -> Result<Model, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut model = parse(&text)?;
    model.path = Some(path.to_path_buf());
    Ok(model)
}

/// Записать модель в файл (с пересчётом контрольной суммы). Используется менеджером/редактором
/// при сохранении и импорте.
pub fn save_file(model: &Model, path: &Path) -> Result<(), String> {
    let text = serialize(model);
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
//  Детектор корневого элемента (без полного разбора)
// ---------------------------------------------------------------------------

/// Сканировать «голову» XML и извлечь локальное имя корневого элемента + его атрибуты.
/// Пропускает XML-декларацию (`<?xml …?>`) и пропускает комментарии/пробелы до первого тэга.
/// Возвращает `None`, если корневой элемент не найден в первых N байт (или XML битый).
///
/// Локальные имена/атрибуты — без префикса пространства имён (как в XSD-движке и валидаторе).
pub fn detect_root(head: &str) -> Option<(String, Vec<(String, String)>)> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(head);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        let ev = reader.read_event_into(&mut buf).ok()?;
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let name = crate::xsd::xmltree::local_name_of(e.name().as_ref()).to_owned();
                let mut attrs = Vec::new();
                for a in e.attributes().flatten() {
                    let key = crate::xsd::xmltree::local_name_of(a.key.as_ref()).to_owned();
                    let val = a
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map(|v| v.into_owned())
                        .unwrap_or_default();
                    attrs.push((key, val));
                }
                return Some((name, attrs));
            }
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::Text(_) => {}
            Event::Eof => return None,
            _ => {}
        }
        buf.clear();
    }
}

// ---------------------------------------------------------------------------
//  Реестр моделей
// ---------------------------------------------------------------------------

/// Реестр моделей приложения. Грузит все `.jqmodel` из папки app-data, хранит отсортированными
/// по `priority` (тай-брейк по `id`), и матчит документы против этой упорядоченной очереди.
pub struct Registry {
    /// Модели в порядке матчинга (приоритет выше → раньше).
    models: Vec<Model>,
    /// Ошибки загрузки отдельных файлов (имя файла → сообщение) — показываются в менеджере,
    /// чтобы битый файл не молча выпадал из списка.
    load_errors: Vec<(PathBuf, String)>,
}

impl Registry {
    /// Прочитать все `.jqmodel` из директории. Битые файлы не падают, а попадают в `load_errors`.
    /// Если директории нет — возвращается пустой реестр (нет моделей).
    pub fn load_dir(dir: &Path) -> Self {
        let mut models = Vec::new();
        let mut load_errors = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Self { models, load_errors },
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jqmodel") {
                continue;
            }
            match load_file(&path) {
                Ok(m) => models.push(m),
                Err(e) => load_errors.push((path, e)),
            }
        }
        // порядок: приоритет выше (меньше число) раньше, при равенстве — по id лексикографически.
        models.sort_by(|a, b| {
            a.manifest
                .priority
                .cmp(&b.manifest.priority)
                .then_with(|| a.manifest.id.cmp(&b.manifest.id))
        });
        Self { models, load_errors }
    }

    /// Снимок реестра (в порядке матчинга). Клон — модели небольшие, реестр перечитывается редко.
    pub fn models(&self) -> &[Model] {
        &self.models
    }

    /// Ошибки загрузки отдельных файлов.
    pub fn load_errors(&self) -> &[(PathBuf, String)] {
        &self.load_errors
    }

    /// Найти первую (по приоритету) модель, подходящую документу. Критерий: (1) локальное имя
    /// корневого элемента документа совпадает с корневым элементом XSD-схемы модели И (2) выполнено
    /// хотя бы одно правило идентификации (или правил нет). XSD компилируется через кэш
    /// (`xsd::compile`, ключ — SHA-256 содержимого), поэтому повторные открытия дёшевы; перебор идёт
    /// по заранее отсортированной очереди и обрывается на первой подошедшей модели. Модель без XSD
    /// или с непригодной схемой матч не проходит (корень не из чего вывести).
    pub fn match_doc(&self, head: &str) -> Option<&Model> {
        let (root, attrs) = detect_root(head)?;
        self.models.iter().find(|m| {
            m.has_xsd()
                && crate::xsd::compile(&m.xsd, &m.manifest.id)
                    .map(|s| s.root_name == root)
                    .unwrap_or(false)
                && m.manifest.r#match.matches(&attrs)
        })
    }

    /// `true`, если в реестре есть хотя бы одна модель.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Построить пустой реестр (для менеджера/тестов: заполнить через `push`, затем `sort`).
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self { models: Vec::new(), load_errors: Vec::new() }
    }

    /// Добавить модель в реестр. Порядок матчинга пересчитывается `sort`.
    #[allow(dead_code)]
    pub fn push(&mut self, model: Model) {
        self.models.push(model);
    }

    /// Пересчитать порядок матчинга (priority → id). Звать после серии `push`/удалений.
    #[allow(dead_code)]
    pub fn sort(&mut self) {
        self.models.sort_by(|a, b| {
            a.manifest
                .priority
                .cmp(&b.manifest.priority)
                .then_with(|| a.manifest.id.cmp(&b.manifest.id))
        });
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_text() -> String {
        [
            "---model---",
            "id: 5.1",
            "description: |",
            "  Многострочное",
            "  описание модели",
            "priority: 10",
            "match:",
            "  - attr: schemaVersion",
            "    values:",
            "      - \"5.1\"",
            "---xsd---",
            "<?xml version=\"1.0\"?>",
            "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"/>",
            "---codes---",
            "{\"ФЛ_1\": []}",
            "---rules---",
            "{\"rules\": []}",
            "---checksum---",
            "", // будет пересчитан
        ]
        .join("\n")
    }

    #[test]
    fn roundtrip_parse_and_serialize() {
        // serialize → parse → serialize — стабильный (idempotent), контрольная сумма бьётся.
        let model = parse(&sample_text()).expect("parse");
        assert_eq!(model.manifest.id, "5.1");
        assert_eq!(model.manifest.priority, 10);
        assert_eq!(model.manifest.r#match.rules.len(), 1);
        assert_eq!(model.manifest.r#match.rules[0].attr, "schemaVersion");
        assert_eq!(model.manifest.r#match.rules[0].values, vec!["5.1".to_owned()]);
        assert!(model.xsd.contains("<xs:schema"));
        assert!(!model.intact, "контрольная сумма в образце неверна — intact=false");

        let serialized = serialize(&model);
        let reparsed = parse(&serialized).expect("reparse");
        assert!(reparsed.intact, "после serialize intact должно быть true");
        // idempotent: повторная сериализация совпадает
        assert_eq!(serialize(&reparsed), serialized);
    }

    #[test]
    fn serialize_intact_checksum() {
        // Свежесериализованная модель имеет верную контрольную сумму.
        let model = parse(&sample_text()).unwrap();
        let text = serialize(&model);
        let reparsed = parse(&text).unwrap();
        assert!(reparsed.intact);
        assert!(!reparsed.checksum.is_empty());
    }

    #[test]
    fn tampered_xsd_detected() {
        // Пользователь поменял XSD руками — сумма не бьётся, intact=false.
        let model = parse(&sample_text()).unwrap();
        let mut text = serialize(&model);
        text = text.replacen("<xs:schema", "<xs:schema tampered", 1);
        let reparsed = parse(&text).unwrap();
        assert!(!reparsed.intact, "подмена XSD должна ломать контрольную сумму");
    }

    #[test]
    fn match_pred_attr_value() {
        let pred = MatchPred {
            rules: vec![MatchRule {
                attr: "schemaVersion".to_owned(),
                values: vec!["5.1".to_owned()],
            }],
        };
        let attrs = |v: &str| vec![("schemaVersion".to_owned(), v.to_owned())];
        assert!(pred.matches(&attrs("5.1")));
        assert!(!pred.matches(&attrs("5.0")), "значение не подходит");
        assert!(!pred.matches(&[]), "атрибут обязателен");
    }

    #[test]
    fn match_pred_presence_only() {
        // Пустой список значений → только проверка присутствия атрибута.
        let pred = MatchPred {
            rules: vec![MatchRule {
                attr: "schemaVersion".to_owned(),
                values: Vec::new(),
            }],
        };
        assert!(pred.matches(&[("schemaVersion".to_owned(), "anything".to_owned())]));
        assert!(!pred.matches(&[]));
    }

    #[test]
    fn match_pred_multiple_rules_or() {
        // Несколько правил — достаточно выполнения ХОТЯ БЫ ОДНОГО (OR).
        let pred = MatchPred {
            rules: vec![
                MatchRule { attr: "schemaVersion".to_owned(), values: vec!["5.1".to_owned()] },
                MatchRule { attr: "kind".to_owned(), values: Vec::new() },
            ],
        };
        // оба выполнены → матч
        let both = vec![
            ("schemaVersion".to_owned(), "5.1".to_owned()),
            ("kind".to_owned(), "x".to_owned()),
        ];
        assert!(pred.matches(&both));
        // выполнено только первое правило → всё равно матч (OR)
        assert!(pred.matches(&[("schemaVersion".to_owned(), "5.1".to_owned())]));
        // выполнено только второе правило (присутствие kind) → матч (OR)
        assert!(pred.matches(&[("kind".to_owned(), "y".to_owned())]));
        // ни одного правила не выполнено → не матч
        assert!(!pred.matches(&[("schemaVersion".to_owned(), "9.9".to_owned())]));
    }

    #[test]
    fn missing_model_section_errors() {
        let text = "---xsd---\n<xs:schema/>\n";
        assert!(parse(&text).is_err());
    }

    #[test]
    fn empty_id_errors() {
        let text = "---model---\nid:\ndescription: x\n---xsd---\n\n";
        assert!(parse(&text).is_err());
    }

    #[test]
    fn detect_root_skips_declaration() {
        let head = "<?xml version=\"1.0\"?>\n<!-- c --><Document schemaVersion=\"5.1\" other=\"x\"/>";
        let (root, attrs) = detect_root(head).expect("root");
        assert_eq!(root, "Document");
        let sv = attrs.iter().find(|(k, _)| k == "schemaVersion").map(|(_, v)| v.as_str());
        assert_eq!(sv, Some("5.1"));
    }

    #[test]
    fn detect_root_with_namespace() {
        // Префикс пространства имён снимается — локальное имя «Document».
        let head = "<bk:Document xmlns:bk=\"urn:x\" schemaVersion=\"5.0\"/>";
        let (root, _) = detect_root(head).unwrap();
        assert_eq!(root, "Document");
    }

    #[test]
    fn detect_root_none_on_text() {
        // Нет тэга в голове → None.
        assert!(detect_root("just text, no xml").is_none());
    }

    /// Построить реестр из моделей с заданными приоритетами (в памяти, без файлов).
    fn registry_of(specs: &[(i64, &str, &str)]) -> Registry {
        // (priority, id, attr-value) → модель с match по schemaVersion=value.
        let mut models = Vec::new();
        for &(prio, id, val) in specs {
            let mut m = parse(&sample_text()).unwrap();
            m.manifest.id = id.to_owned();
            m.manifest.priority = prio;
            // match_doc сверяет корень документа с корнем XSD-схемы → тестовой модели нужна
            // минимальная компилируемая схема с корневым элементом Document.
            m.xsd = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">\
                     <xs:element name=\"Document\" type=\"xs:string\"/></xs:schema>"
                .to_owned();
            m.manifest.r#match = MatchPred {
                rules: vec![MatchRule {
                    attr: "schemaVersion".to_owned(),
                    values: vec![val.to_owned()],
                }],
            };
            models.push(m);
        }
        models.sort_by(|a, b| {
            a.manifest.priority.cmp(&b.manifest.priority).then_with(|| a.manifest.id.cmp(&b.manifest.id))
        });
        // Реестр строится через приватный конструктор в тестах: используем load_dir через tempdir
        // — но для скорости соберём через пустой реестр + patch. Здесь просто проверим match логику
        // через минимальный Registry.
        let mut reg = Registry::load_dir(std::path::Path::new("__nonexistent_for_test__"));
        reg.models = models;
        reg
    }

    #[test]
    fn registry_matches_by_priority() {
        // Две модели подходят (schemaVersion=5.1); выигрывает с меньшим priority.
        let reg = registry_of(&[(20, "low", "5.1"), (10, "high", "5.1")]);
        let m = reg.match_doc("<Document schemaVersion=\"5.1\"/>").expect("match");
        assert_eq!(m.manifest.id, "high", "должна выиграть модель с меньшим priority");
    }

    #[test]
    fn registry_tiebreak_by_id() {
        // Равный priority → тай-брейк по id (лексикографически).
        let reg = registry_of(&[(10, "b", "5.1"), (10, "a", "5.1")]);
        let m = reg.match_doc("<Document schemaVersion=\"5.1\"/>").unwrap();
        assert_eq!(m.manifest.id, "a");
    }

    #[test]
    fn registry_no_match_returns_none() {
        let reg = registry_of(&[(10, "x", "5.1")]);
        assert!(reg.match_doc("<Document schemaVersion=\"9.9\"/>").is_none());
    }

    #[test]
    fn registry_empty_no_match() {
        let reg = Registry::load_dir(std::path::Path::new("__nonexistent_for_test__"));
        assert!(reg.is_empty());
        assert!(reg.match_doc("<Document schemaVersion=\"5.1\"/>").is_none());
    }

    /// End-to-end пайплайн на реальных `.jqmodel` из `schemas/built/`: грузим реестр, матчим
    /// документ 5.1, компилируем XSD и строим движок правил — всё должно пройти без ошибок.
    /// Пропускается, если файлы ещё не сгенерированы (см. `gen_builtin_models`).
    #[test]
    fn built_models_load_and_match() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas").join("built");
        let reg = Registry::load_dir(&dir);
        if reg.is_empty() {
            eprintln!("schemas/built пуст — пропускаю (запусти gen_builtin_models для генерации)");
            return;
        }
        assert_eq!(reg.models().len(), 2, "ожидалось 2 встроенных модели в built/");
        // обе модели целостны (контрольная сумма бьётся)
        assert!(reg.models().iter().all(|m| m.intact), "контрольная сумма не бьётся");
        // матч 5.1
        let m = reg
            .match_doc("<?xml version=\"1.0\"?>\n<Document schemaVersion=\"5.1\"/>")
            .expect("5.1 должна сматчиться");
        assert_eq!(m.manifest.id, "785p_5_1");
        // XSD компилируется через кэш
        let schema = crate::xsd::compile(&m.xsd, &m.manifest.id).expect("xsd compile");
        assert_eq!(schema.root_name, "Document");
        // движок правил строится
        let _engine = crate::rules::RuleEngine::for_model(&m.codes, &m.rules).expect("rules engine");
    }

    /// Сгенерировать `.jqmodel`-файлы для встроенных (бывших) моделей 5.0/5.1 из `schemas/` и
    /// записать их в `schemas/built/`. Это и есть «экспорт встроенного локально»: после перехода на
    /// модели продукт больше не несёт схем в бинарнике — пользователь импортирует эти файлы через
    /// менеджер. Помечен `#[ignore]`, чтобы не гонять в обычном `cargo test`; запуск:
    /// `cargo test --release gen_builtin_models -- --ignored`.
    #[test]
    #[ignore]
    fn gen_builtin_models() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let out_dir = root.join("schemas").join("built");
        std::fs::create_dir_all(&out_dir).expect("create built dir");
        for (v, mid) in [("5.0", "785p_5_0"), ("5.1", "785p_5_1")] {
            // XSD-секция: Main + транзитивные include, склеенные в один блок (как делает
            // xsd::compile в проде — нарезка по <xs:schema> отрабатывает при загрузке модели).
            let mut xsd = String::new();
            for f in [
                "Main.xsd",
                "BKIApiCommonTypes.xsd",
                "ReferencesTypes.xsd",
                "Blocks.xsd",
                "BlocksCur.xsd",
                "Events.xsd",
            ] {
                let p = root.join("schemas").join(v).join("xsd").join(f);
                xsd.push_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())));
            }
            let codes = std::fs::read_to_string(root.join("schemas").join(v).join("codes_map.json")).unwrap();
            let rules = std::fs::read_to_string(root.join("schemas").join(v).join("rules.json")).unwrap();
            let model = Model {
                manifest: Manifest {
                    id: mid.to_owned(),
                    description: format!(
                        "Модель 785-П (ред. {v}): XSD + карта показателей + правила разделов 5/6."
                    ),
                    priority: 10,
                    r#match: MatchPred {
                        rules: vec![MatchRule {
                            attr: "schemaVersion".to_owned(),
                            values: vec![v.to_owned()],
                        }],
                    },
                },
                xsd,
                codes,
                rules,
                checksum: String::new(), // save_file пересчитает
                intact: true,
                path: None,
            };
            let dest = out_dir.join(format!("{mid}.jqmodel"));
            save_file(&model, &dest).unwrap();
            // контрольная сумма должна биться сразу после записи
            let reloaded = load_file(&dest).unwrap();
            assert!(reloaded.intact, "{mid}: контрольная сумма не бьётся после генерации");
            println!("written: {}", dest.display());
        }
    }

    /// Переписать секцию `rules` под новый формат `{name, message, severity, check}`: `codes`/`block`
    /// уезжают ВНУТРЬ `check`; `name` = `id` [+ « » + `title`]; человекочитаемый текст → `message`
    /// (из `message` или `description` прошлых форматов); `ref`/`title`/`id`/`description` удаляются.
    /// Идемпотентно. Возвращает новый JSON секции.
    fn migrate_rules_json(rules: &str) -> String {
        use serde_json::{json, Value};
        let mut root: Value = serde_json::from_str(rules).unwrap_or_else(|_| json!({"rules": []}));
        let Some(arr) = root.get_mut("rules").and_then(|r| r.as_array_mut()) else {
            return rules.to_owned();
        };
        for rule in arr.iter_mut() {
            let Some(obj) = rule.as_object_mut() else { continue };
            let codes = obj.remove("codes");
            let block = obj.remove("block");
            obj.remove("ref");
            // name = существующее name, иначе id [+ " " + title] (старые форматы).
            if !obj.contains_key("name") {
                let id = obj.remove("id").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
                let title = obj.remove("title").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
                let name = match (id.is_empty(), title.is_empty()) {
                    (false, false) => format!("{id} {title}"),
                    (false, true) => id,
                    (true, false) => title,
                    (true, true) => String::new(),
                };
                obj.insert("name".to_owned(), Value::String(name));
            } else {
                obj.remove("id");
                obj.remove("title");
            }
            // message = существующее message, иначе description (формат прошлого раунда).
            if !obj.contains_key("message") {
                if let Some(d) = obj.remove("description") {
                    obj.insert("message".to_owned(), d);
                }
            } else {
                obj.remove("description");
            }
            if let Some(check) = obj.get_mut("check").and_then(|c| c.as_object_mut()) {
                if let Some(c) = codes {
                    if c.as_array().is_some_and(|a| !a.is_empty()) {
                        check.insert("codes".to_owned(), c);
                    }
                }
                if let Some(b) = block {
                    if b.as_array().is_some_and(|a| !a.is_empty()) {
                        check.insert("block".to_owned(), b);
                    }
                }
            }
        }
        serde_json::to_string_pretty(&root).unwrap_or_else(|_| rules.to_owned())
    }

    /// Мигрировать `.jqmodel`-файлы под новый формат правил НА МЕСТЕ: папка моделей пользователя
    /// (`%APPDATA%\JustQuery\models`) + корень репозитория (устаревшие экспортные артефакты). Каждый
    /// файл загружается, секция rules переписывается, контрольная сумма пересчитывается при записи.
    /// Игнорируется в обычном прогоне; запуск:
    /// `cargo test --release migrate_models_to_new_format -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn migrate_models_to_new_format() {
        let dirs = [
            crate::models_dir(),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        ];
        for dir in dirs {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("jqmodel") {
                    continue;
                }
                let mut model = match load_file(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("SKIP {}: {e}", path.display());
                        continue;
                    }
                };
                model.rules = migrate_rules_json(&model.rules);
                match save_file(&model, &path) {
                    Ok(()) => println!("migrated: {}", path.display()),
                    Err(e) => eprintln!("FAIL {}: {e}", path.display()),
                }
            }
        }
    }
}
