//! Универсальный шреддинг XML → набор плоских таблиц для табличного («человеческого») просмотра —
//! это и есть результат кнопки **Run** на XML-вкладке. Не привязан к 758-П: работает с любым XML.
//!
//! Универсальные правила:
//! - Элемент становится ТАБЛИЦЕЙ, если он структурный (есть дети-элементы) ИЛИ встречается >1 раза.
//!   Одна строка — один экземпляр элемента; экземпляры агрегируются по имени по всему документу.
//! - Скалярные дети-листья (без детей-элементов и не сами таблицы) идут КОЛОНКАМИ строки родителя;
//!   атрибуты — колонками `@имя`; собственный текст листа — колонкой `значение`.
//! - Структурные / повторяющиеся дети в строку родителя НЕ вкладываются — у них своя таблица.
//!
//! Таблицы идут в порядке первого появления имени элемента в документе.

use crate::doc::piece_table::PieceSnapshot;
use crate::proc::ProcMsg;
use crate::xsd::xmltree::{parse_str, XNode};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Служебный корень для оборачивания выделенного фрагмента (может быть несколькими элементами).
const SEL_ROOT: &str = "__selection__";

/// Источник Run: весь документ (снимок — читается в фоне) или выделенный фрагмент (текст).
pub enum ShredSource {
    Snap(PieceSnapshot),
    Selection(String),
}

/// Запустить фоновое преобразование XML → таблицы. Шлёт `ProcMsg::Tables` (успех) либо
/// `Failed` (не well-formed / ошибка чтения) / `Cancelled`.
pub fn spawn_shred(src: ShredSource, cancel: Arc<AtomicBool>, tx: Sender<ProcMsg>) {
    std::thread::spawn(move || {
        // 1) получить XML-текст
        let (xml, synthetic) = match src {
            ShredSource::Snap(snap) => {
                let mut buf = Vec::new();
                if snap.reader().read_to_end(&mut buf).is_err() {
                    let _ = tx.send(ProcMsg::Failed("could not read the document".to_owned()));
                    return;
                }
                (String::from_utf8_lossy(&buf).into_owned(), false)
            }
            // фрагмент может быть несколькими элементами → оборачиваем в служебный корень
            ShredSource::Selection(s) => (format!("<{SEL_ROOT}>{s}</{SEL_ROOT}>"), true),
        };
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(ProcMsg::Cancelled);
            return;
        }
        // 2) разобрать и развернуть
        match parse_str(&xml, &[]) {
            Ok(tree) => {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(ProcMsg::Cancelled);
                    return;
                }
                let mut tables = shred(&tree);
                if synthetic {
                    tables.retain(|t| t.name != SEL_ROOT); // не показывать служебную обёртку
                }
                let _ = tx.send(ProcMsg::Tables(tables));
            }
            Err(e) => {
                let _ = tx.send(ProcMsg::Failed(format!("XML parse error: {e}")));
            }
        }
    });
}

/// Одна таблица: имя элемента, колонки и строки (значения уже как строки для грида).
pub struct ShredTable {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Развернуть дерево `root` в набор таблиц.
pub fn shred(root: &XNode) -> Vec<ShredTable> {
    // 1) «структурный?», «повторяется среди детей одного родителя?» и порядок первого появления
    let mut structured: HashSet<&str> = HashSet::new();
    let mut repeats_in_parent: HashSet<&str> = HashSet::new();
    let mut order: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for n in root.iter() {
        let name = n.name.as_str();
        if seen.insert(name) {
            order.push(name);
        }
        if !n.children.is_empty() {
            structured.insert(name);
        }
        // повтор имени именно СРЕДИ детей одного родителя (а не по всему документу): иначе
        // обычное поле-скаляр, встречающееся в каждой записи, ошибочно стало бы своей таблицей
        let mut local: HashMap<&str, usize> = HashMap::new();
        for c in &n.children {
            let e = local.entry(c.name.as_str()).or_insert(0);
            *e += 1;
            if *e == 2 {
                repeats_in_parent.insert(c.name.as_str());
            }
        }
    }
    // имя — таблица, если оно структурное ИЛИ повторяется среди детей одного родителя
    let is_table: HashSet<&str> = order
        .iter()
        .copied()
        .filter(|n| structured.contains(n) || repeats_in_parent.contains(n))
        .collect();

    // 2) для каждого имени-таблицы собрать строки (агрегируя все экземпляры по документу)
    let mut tables = Vec::new();
    for &name in &order {
        if !is_table.contains(name) {
            continue;
        }
        let mut cols: Vec<String> = Vec::new();
        let mut col_idx: HashMap<String, usize> = HashMap::new();
        let mut rows: Vec<Vec<String>> = Vec::new();
        for inst in root.iter().filter(|n| n.name == name) {
            let mut row: Vec<String> = Vec::new();
            for (k, v) in record_fields(inst, &is_table) {
                let i = match col_idx.get(&k) {
                    Some(&i) => i,
                    None => {
                        let i = cols.len();
                        col_idx.insert(k.clone(), i);
                        cols.push(k);
                        i
                    }
                };
                if i >= row.len() {
                    row.resize(i + 1, String::new());
                }
                if row[i].is_empty() {
                    row[i] = v;
                } else {
                    row[i].push_str("; ");
                    row[i].push_str(&v);
                }
            }
            rows.push(row);
        }
        // выровнять все строки по числу колонок
        for r in &mut rows {
            r.resize(cols.len(), String::new());
        }
        tables.push(ShredTable { name: name.to_string(), columns: cols, rows });
    }
    tables
}

/// Поля одной строки: атрибуты (`@k`) + скалярные дети-листья (не-таблицы) + текст листа.
fn record_fields(n: &XNode, is_table: &HashSet<&str>) -> Vec<(String, String)> {
    let mut f: Vec<(String, String)> = Vec::new();
    for (k, v) in &n.attrs {
        f.push((format!("@{k}"), v.clone()));
    }
    for c in &n.children {
        if is_table.contains(c.name.as_str()) {
            continue; // структурный/повторяющийся ребёнок — у него своя таблица
        }
        f.push((c.name.clone(), c.text.trim().to_owned()));
    }
    let txt = n.text.trim();
    if !txt.is_empty() && f.iter().all(|(k, _)| k.starts_with('@')) {
        f.push(("value".to_owned(), txt.to_owned()));
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xsd::xmltree::parse_str;

    #[test]
    fn repeating_elements_become_a_table() {
        let xml = r#"<root>
            <item id="1"><n>a</n></item>
            <item id="2"><n>b</n></item>
        </root>"#;
        let tree = parse_str(xml, &[]).unwrap();
        let tables = shred(&tree);
        let item = tables.iter().find(|t| t.name == "item").expect("таблица item");
        assert_eq!(item.rows.len(), 2);
        assert!(item.columns.iter().any(|c| c == "@id"), "колонка @id: {:?}", item.columns);
        assert!(item.columns.iter().any(|c| c == "n"), "колонка n: {:?}", item.columns);
    }

    #[test]
    fn scalar_children_are_columns_not_tables() {
        let xml = r#"<doc><title>Hi</title><year>2026</year></doc>"#;
        let tree = parse_str(xml, &[]).unwrap();
        let tables = shred(&tree);
        // doc — структурный (одна строка); title/year — скаляры-колонки, своих таблиц нет
        assert!(tables.iter().any(|t| t.name == "doc"));
        assert!(!tables.iter().any(|t| t.name == "title"));
        let doc = tables.iter().find(|t| t.name == "doc").unwrap();
        assert_eq!(doc.rows.len(), 1);
        assert!(doc.columns.iter().any(|c| c == "title"));
        assert!(doc.columns.iter().any(|c| c == "year"));
    }
}
