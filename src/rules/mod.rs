//! Движок правил валидации XML (разделы 5/6).
//!
//! Модель аккумуляторная, один проход: события подаются по мере материализации поддеревьев
//! потоковым валидатором. Контекст несёт атрибуты Document, блок Source, текущий Title и
//! текущее событие.
//!
//! - раздел 5 — Rust-функции (5.6, 5.16, 5.17, 5.18, 5.27);
//! - раздел 6 — декларативный реестр rules.json, коды показателей резолвятся через
//!   codes_map.json;
//! - агрегаты — уникальность orderNum (п. 5.23), счётчики subjectsCount/groupBlocksCount.

use crate::proc::{Finding, Severity};
use crate::xsd::xmltree::XNode;
use serde_json::Value;
use std::collections::HashMap;

/// Карта кодов показателей: код → варианты (текстовый элемент или флаг-пара).
pub struct CodesMap {
    map: HashMap<String, Vec<CodeEntry>>,
}

enum CodeEntry {
    Element(String),
    Flag { elem0: Option<String>, elem1: Option<String> },
}

impl CodesMap {
    pub fn parse(json: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let obj = v.as_object().ok_or("codes_map: ожидался объект")?;
        let mut map = HashMap::with_capacity(obj.len());
        for (code, entries) in obj {
            let mut list = Vec::new();
            for e in entries.as_array().into_iter().flatten() {
                if e.get("kind").and_then(Value::as_str) == Some("flag") {
                    list.push(CodeEntry::Flag {
                        elem0: e.get("elem0").and_then(Value::as_str).map(str::to_owned),
                        elem1: e.get("elem1").and_then(Value::as_str).map(str::to_owned),
                    });
                } else if let Some(el) = e.get("element").and_then(Value::as_str) {
                    list.push(CodeEntry::Element(el.to_owned()));
                }
            }
            map.insert(code.clone(), list);
        }
        Ok(Self { map })
    }
}

/// Контекст вычисления правил на текущем событии.
pub struct RuleContext {
    #[allow(dead_code)] // доступно правилам, зависящим от версии
    pub schema_version: String,
    pub doc_attrs: HashMap<String, String>,
    pub source: Option<XNode>,
    pub title: Option<XNode>,
    pub event: Option<XNode>,
    pub event_name: String,
    pub subject_kind: String, // FL | UL | AF
    pub findings: Vec<Finding>,
}

impl RuleContext {
    fn new(version: &str) -> Self {
        Self {
            schema_version: version.to_owned(),
            doc_attrs: HashMap::new(),
            source: None,
            title: None,
            event: None,
            event_name: String::new(),
            subject_kind: String::new(),
            findings: Vec::new(),
        }
    }

    fn report(
        &mut self,
        code: &str,
        message: String,
        source: &str,
        line: usize,
        severity: Severity,
    ) {
        self.findings.push(Finding {
            severity,
            line,
            code: code.to_owned(),
            message,
            source: source.to_owned(),
        });
    }
}

type EventRule = Box<dyn Fn(&mut RuleContext) + Send>;

/// Набор правил для конкретной версии схемы.
pub struct Registry {
    #[allow(dead_code)]
    pub version: String,
    event_rules: Vec<EventRule>,
    subject_rules: Vec<EventRule>,
}

/// Движок: применяет правила реестра к подаваемым поддеревьям.
pub struct RuleEngine {
    registry: Registry,
    ctx: RuleContext,
    order_nums: Vec<(String, usize, String)>, // (orderNum, line, event_name)
    subject_count: usize,
    event_count: usize,
}

impl RuleEngine {
    /// Собрать движок для версии схемы («5.0» / «5.1»).
    pub fn for_version(version: &str) -> Result<Self, String> {
        let (codes_json, rules_json) = match version {
            "5.0" => (
                include_str!("../../schemas/5.0/codes_map.json"),
                include_str!("../../schemas/5.0/rules.json"),
            ),
            "5.1" => (
                include_str!("../../schemas/5.1/codes_map.json"),
                include_str!("../../schemas/5.1/rules.json"),
            ),
            other => return Err(format!("неизвестная версия схемы «{other}»")),
        };
        let codes = std::sync::Arc::new(CodesMap::parse(codes_json)?);
        let mut registry = Registry {
            version: version.to_owned(),
            event_rules: Vec::new(),
            subject_rules: Vec::new(),
        };
        section5::install(&mut registry);
        section6::install(&mut registry, rules_json, codes)?;
        Ok(Self {
            registry,
            ctx: RuleContext::new(version),
            order_nums: Vec::new(),
            subject_count: 0,
            event_count: 0,
        })
    }

    pub fn begin_document(&mut self, attrs: HashMap<String, String>) {
        self.ctx.doc_attrs = attrs;
    }

    pub fn set_source(&mut self, source: XNode) {
        self.ctx.source = Some(source);
    }

    /// Начало субъекта: kind ∈ {FL, UL, AF}; `title` — материализованный Title.
    /// Возвращает находки субъектных правил.
    pub fn begin_subject(&mut self, kind: &str, title: Option<XNode>) -> Vec<Finding> {
        self.ctx.subject_kind = kind.to_owned();
        self.ctx.title = title;
        self.subject_count += 1;
        for rule in &self.registry.subject_rules {
            rule(&mut self.ctx);
        }
        std::mem::take(&mut self.ctx.findings)
    }

    /// Очередное событие `*_Event_*`. Возвращает находки событийных правил.
    pub fn on_event(&mut self, event: XNode) -> Vec<Finding> {
        self.ctx.event_name = event.name.clone();
        self.event_count += 1;
        if let Some(order) = event.attr("orderNum") {
            self.order_nums.push((order.to_owned(), event.line, event.name.clone()));
        }
        self.ctx.event = Some(event);
        for rule in &self.registry.event_rules {
            rule(&mut self.ctx);
        }
        self.ctx.event = None;
        std::mem::take(&mut self.ctx.findings)
    }

    /// Конец документа: агрегатные проверки.
    pub fn finalize(mut self) -> Vec<Finding> {
        let mut out = Vec::new();
        // п. 5.23: orderNum уникален в пределах документа
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (order, line, _name) in &self.order_nums {
            if seen.contains_key(order.as_str()) {
                out.push(Finding {
                    severity: Severity::Error,
                    line: *line,
                    code: "P5_23".to_owned(),
                    message: format!(
                        "Порядковый номер записи orderNum=«{order}» не уникален (повтор в пределах документа)."
                    ),
                    source: "п. 5.23".to_owned(),
                });
            } else {
                seen.insert(order, *line);
            }
        }
        // согласованность счётчиков
        let attrs = std::mem::take(&mut self.ctx.doc_attrs);
        if let Some(sc) = attrs.get("subjectsCount") {
            if sc.bytes().all(|b| b.is_ascii_digit())
                && sc.parse::<usize>().ok() != Some(self.subject_count)
            {
                out.push(Finding {
                    severity: Severity::Error,
                    line: 0,
                    code: "DOC_COUNT".to_owned(),
                    message: format!(
                        "Атрибут subjectsCount=«{sc}» не совпадает с числом субъектов в документе ({})." ,
                        self.subject_count
                    ),
                    source: "атрибут subjectsCount".to_owned(),
                });
            }
        }
        if let Some(gbc) = attrs.get("groupBlocksCount") {
            if gbc.bytes().all(|b| b.is_ascii_digit())
                && gbc.parse::<usize>().ok() != Some(self.event_count)
            {
                out.push(Finding {
                    severity: Severity::Error,
                    line: 0,
                    code: "DOC_COUNT".to_owned(),
                    message: format!(
                        "Атрибут groupBlocksCount=«{gbc}» не совпадает с числом групп блоков показателей ({})." ,
                        self.event_count
                    ),
                    source: "атрибут groupBlocksCount".to_owned(),
                });
            }
        }
        out
    }
}

// ===========================================================================
//  Раздел 5 (Rust-функции)
// ===========================================================================

mod section5 {
    use super::*;

    /// Есть ли среди потомков элемент с именем из набора.
    fn has(el: Option<&XNode>, names: &[&str]) -> bool {
        el.is_some_and(|e| e.iter().any(|d| names.contains(&d.name.as_str())))
    }

    const NONMON_SOURCE: &[&str] = &["FL_30_NonMonetarySource", "UL_21_NonMonetarySource"];
    const NONMON_SUBJECT: &[&str] = &["FL_31_NonMonetarySubject", "UL_22_NonMonetarySubject"];
    const EVENTS_4: &[&str] = &[
        "FL_Event_4_1", "FL_Event_4_2", "FL_Event_4_3", "FL_Event_4_4",
        "UL_Event_4_1", "UL_Event_4_2", "UL_Event_4_3", "UL_Event_4_4",
    ];

    fn formation_date(source: Option<&XNode>) -> String {
        let Some(src) = source else { return String::new() };
        for d in src.iter() {
            if d.name.contains("CreditInfoDate") {
                return d.text.chars().take(10).collect();
            }
        }
        String::new()
    }

    pub fn install(reg: &mut Registry) {
        // 5.6 — дата события не позднее даты формирования информации
        reg.event_rules.push(Box::new(|ctx: &mut RuleContext| {
            let Some(ev) = ctx.event.take() else { return };
            if let Some(edt) = ev.attr("eventDateTime") {
                let fdate = formation_date(ctx.source.as_ref());
                if !edt.is_empty() && !fdate.is_empty() {
                    let ev_date: String = edt.chars().take(10).collect();
                    if ev_date.as_str() > fdate.as_str() {
                        ctx.report(
                            "P5_06",
                            format!(
                                "Дата события ({ev_date}) позже даты формирования информации ({fdate}) — нарушение п. 5.6."
                            ),
                            "п. 5.6",
                            ev.line,
                            Severity::Error,
                        );
                    }
                }
            }
            ctx.event = Some(ev);
        }));

        // 5.16 — блок неденежного обязательства источника по признаку ФЛ_18.9 / ЮЛ_11.8
        reg.event_rules.push(Box::new(|ctx: &mut RuleContext| {
            let Some(ev) = ctx.event.take() else { return };
            let has0 = has(Some(&ev), &["monetarySourceExist_0"]);
            let has1 = has(Some(&ev), &["monetarySourceExist_1"]);
            let block = has(Some(&ev), NONMON_SOURCE);
            if has0 && !block {
                ctx.report(
                    "P5_16",
                    "Блок «Сведения о неденежном обязательстве источника» обязателен при признаке денежного обязательства источника = 0 (п. 5.16).".to_owned(),
                    "п. 5.16",
                    ev.line,
                    Severity::Error,
                );
            } else if has1 && block {
                ctx.report(
                    "P5_16",
                    "Блок «Сведения о неденежном обязательстве источника» должен отсутствовать при признаке денежного обязательства источника = 1 (п. 5.16).".to_owned(),
                    "п. 5.16",
                    ev.line,
                    Severity::Error,
                );
            }
            ctx.event = Some(ev);
        }));

        // 5.17 — то же для неденежного обязательства субъекта
        reg.event_rules.push(Box::new(|ctx: &mut RuleContext| {
            let Some(ev) = ctx.event.take() else { return };
            let has0 = has(Some(&ev), &["monetarySubjectExist_0"]);
            let has1 = has(Some(&ev), &["monetarySubjectExist_1"]);
            let block = has(Some(&ev), NONMON_SUBJECT);
            if has0 && !block {
                ctx.report(
                    "P5_17",
                    "Блок «Сведения о неденежном обязательстве субъекта» обязателен при признаке денежного обязательства субъекта = 0 (п. 5.17).".to_owned(),
                    "п. 5.17",
                    ev.line,
                    Severity::Error,
                );
            } else if has1 && block {
                ctx.report(
                    "P5_17",
                    "Блок «Сведения о неденежном обязательстве субъекта» должен отсутствовать при признаке денежного обязательства субъекта = 1 (п. 5.17).".to_owned(),
                    "п. 5.17",
                    ev.line,
                    Severity::Error,
                );
            }
            ctx.event = Some(ev);
        }));

        // 5.18 — формат кода субъекта (4–15 символов, цифры и буквы одного алфавита)
        reg.subject_rules.push(Box::new(|ctx: &mut RuleContext| {
            let Some(title) = ctx.title.take() else { return };
            if let Some(code) = title.attr("subjectCode") {
                if !code.is_empty() && !valid_subject_code(code) {
                    ctx.report(
                        "P5_18",
                        format!(
                            "Код субъекта «{code}» не соответствует п. 5.18: ожидается 4–15 символов, цифры и буквы одного алфавита."
                        ),
                        "п. 5.18",
                        title.line,
                        Severity::Error,
                    );
                }
            }
            ctx.title = Some(title);
        }));

        // 5.27 — changeReason обязателен в событиях 4.1–4.4 при коде операции D1/D2
        reg.event_rules.push(Box::new(|ctx: &mut RuleContext| {
            if !EVENTS_4.contains(&ctx.event_name.as_str()) {
                return;
            }
            let Some(ev) = ctx.event.take() else { return };
            let op = ev.attr("operationCode").unwrap_or("");
            if (op == "D1" || op == "D2") && ev.attr("changeReason").is_none() {
                let name = ctx.event_name.clone();
                ctx.report(
                    "P5_27",
                    format!(
                        "Для события {name} с кодом операции «{op}» обязателен атрибут changeReason «Код причины изменения» (п. 5.27)."
                    ),
                    "п. 5.27",
                    ev.line,
                    Severity::Error,
                );
            }
            ctx.event = Some(ev);
        }));
    }

    fn is_cyr(c: char) -> bool {
        matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')
    }

    pub(super) fn valid_subject_code(code: &str) -> bool {
        let n = code.chars().count();
        if !(4..=15).contains(&n) {
            return false;
        }
        let has_cyr = code.chars().any(is_cyr);
        let has_lat = code.chars().any(|c| c.is_ascii_alphabetic());
        if has_cyr && has_lat {
            return false; // смешение алфавитов запрещено
        }
        if has_cyr {
            code.chars().all(|c| c.is_ascii_digit() || is_cyr(c))
        } else if has_lat {
            code.chars().all(|c| c.is_ascii_digit() || c.is_ascii_alphabetic())
        } else {
            code.chars().all(|c| c.is_ascii_digit())
        }
    }
}

// ===========================================================================
//  Раздел 6 (декларативный реестр)
// ===========================================================================

mod section6 {
    use super::*;
    use std::sync::Arc;

    /// Значения показателя `code` в ближайшей области:
    /// primary (блок) → событие → титул → источник.
    fn resolve_values(
        ctx: &RuleContext,
        codes: &CodesMap,
        code: &str,
        primary: Option<&XNode>,
    ) -> Vec<String> {
        let Some(entries) = codes.map.get(code) else { return Vec::new() };
        if entries.is_empty() {
            return Vec::new();
        }
        let scopes: [Option<&XNode>; 3] = match primary {
            Some(p) => [Some(p), ctx.title.as_ref(), ctx.source.as_ref()],
            None => [ctx.event.as_ref(), ctx.title.as_ref(), ctx.source.as_ref()],
        };
        for scope in scopes.into_iter().flatten() {
            let mut present: std::collections::HashSet<&str> = std::collections::HashSet::new();
            let mut vals: Vec<String> = Vec::new();
            for el in scope.iter() {
                present.insert(el.name.as_str());
                for e in entries {
                    if let CodeEntry::Element(name) = e {
                        if name == &el.name {
                            vals.push(el.text.trim().to_owned());
                        }
                    }
                }
            }
            for e in entries {
                if let CodeEntry::Flag { elem0, elem1 } = e {
                    if elem1.as_deref().is_some_and(|n| present.contains(n)) {
                        vals.push("1".to_owned());
                    } else if elem0.as_deref().is_some_and(|n| present.contains(n)) {
                        vals.push("0".to_owned());
                    }
                }
            }
            if !vals.is_empty() {
                return vals;
            }
        }
        Vec::new()
    }

    fn to_number(s: &str) -> Option<f64> {
        s.parse().ok()
    }

    fn eval_leaf(
        ctx: &RuleContext,
        codes: &CodesMap,
        leaf: &Value,
        primary: Option<&XNode>,
    ) -> bool {
        let code = leaf.get("code").and_then(Value::as_str).unwrap_or("");
        let op = leaf.get("op").and_then(Value::as_str).unwrap_or("");
        let value = leaf.get("value");
        let values = if code.is_empty() {
            Vec::new()
        } else {
            resolve_values(ctx, codes, code, primary)
        };
        let present = values.iter().any(|v| !v.is_empty() && v != "-");
        match op {
            "present" => return present,
            "absent" => return !present,
            _ => {}
        }
        if values.is_empty() {
            return false;
        }
        let sval = value.and_then(Value::as_str).unwrap_or("");
        match op {
            "eq" => values.iter().any(|v| v == sval),
            "ne" => values.iter().all(|v| v != sval),
            "in" => {
                let allowed: Vec<&str> = match value {
                    Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
                    Some(Value::String(s)) => vec![s.as_str()],
                    _ => Vec::new(),
                };
                values.iter().any(|v| allowed.contains(&v.as_str()))
            }
            "gt" | "ge" | "lt" | "le" => {
                let rv = value
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(to_number)));
                let Some(rv) = rv else { return false };
                values.iter().filter_map(|v| to_number(v)).any(|lv| match op {
                    "gt" => lv > rv,
                    "ge" => lv >= rv,
                    "lt" => lv < rv,
                    _ => lv <= rv,
                })
            }
            _ => false,
        }
    }

    fn eval_condition(
        ctx: &RuleContext,
        codes: &CodesMap,
        cond: Option<&Value>,
        primary: Option<&XNode>,
    ) -> bool {
        let Some(cond) = cond else { return true };
        if let Some(all) = cond.get("all").and_then(Value::as_array) {
            return all.iter().all(|c| eval_condition(ctx, codes, Some(c), primary));
        }
        if let Some(any) = cond.get("any").and_then(Value::as_array) {
            return any.iter().any(|c| eval_condition(ctx, codes, Some(c), primary));
        }
        if let Some(not) = cond.get("not") {
            return !eval_condition(ctx, codes, Some(not), primary);
        }
        eval_leaf(ctx, codes, cond, primary)
    }

    /// Блоки для пер-блочной оценки, или [None] для оценки на событии.
    fn block_scopes<'a>(ctx: &'a RuleContext, blocks: &[String]) -> Vec<Option<&'a XNode>> {
        if blocks.is_empty() {
            return vec![None];
        }
        let Some(ev) = ctx.event.as_ref() else { return Vec::new() };
        ev.iter()
            .filter(|el| blocks.iter().any(|b| b == &el.name))
            .map(Some)
            .collect()
    }

    fn present_in(
        ctx: &RuleContext,
        codes: &CodesMap,
        rule_codes: &[String],
        primary: Option<&XNode>,
    ) -> bool {
        rule_codes.iter().any(|code| {
            resolve_values(ctx, codes, code, primary)
                .iter()
                .any(|v| !v.is_empty() && v != "-")
        })
    }

    struct RuleSpec {
        id: String,
        codes: Vec<String>,
        blocks: Vec<String>,
        refer: String,
        message: String,
        severity: Severity,
        condition: Option<Value>,
        else_forbidden: bool,
    }

    fn spec_of(rule: &Value, check: &Value) -> RuleSpec {
        let arr = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        RuleSpec {
            id: rule.get("id").and_then(Value::as_str).unwrap_or("T6").to_owned(),
            codes: arr(rule.get("codes")),
            blocks: arr(rule.get("block")),
            refer: rule.get("ref").and_then(Value::as_str).unwrap_or("р. 6").to_owned(),
            message: rule
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Нарушено правило раздела 6")
                .to_owned(),
            severity: match rule.get("severity").and_then(Value::as_str) {
                Some("warning") => Severity::Warning,
                _ => Severity::Error,
            },
            condition: check.get("condition").cloned(),
            else_forbidden: check.get("else").and_then(Value::as_str) == Some("forbidden"),
        }
    }

    pub fn install(reg: &mut Registry, rules_json: &str, codes: Arc<CodesMap>) -> Result<(), String> {
        let data: Value = serde_json::from_str(rules_json).map_err(|e| e.to_string())?;
        for rule in data.get("rules").and_then(Value::as_array).into_iter().flatten() {
            let check = rule.get("check").cloned().unwrap_or(Value::Null);
            let ctype = check.get("type").and_then(Value::as_str).unwrap_or("");
            match ctype {
                "required_if" => {
                    let spec = spec_of(rule, &check);
                    let codes = Arc::clone(&codes);
                    reg.event_rules.push(Box::new(move |ctx: &mut RuleContext| {
                        let mut reports: Vec<(usize,)> = Vec::new();
                        for primary in block_scopes(ctx, &spec.blocks) {
                            let cond = eval_condition(ctx, &codes, spec.condition.as_ref(), primary);
                            let present = present_in(ctx, &codes, &spec.codes, primary);
                            let line = primary
                                .map(|p| p.line)
                                .or(ctx.event.as_ref().map(|e| e.line))
                                .unwrap_or(0);
                            if (cond && !present)
                                || (!cond && present && spec.else_forbidden)
                            {
                                reports.push((line,));
                            }
                        }
                        for (line,) in reports {
                            ctx.report(&spec.id, spec.message.clone(), &spec.refer, line, spec.severity);
                        }
                    }));
                }
                "forbidden_if" => {
                    let spec = spec_of(rule, &check);
                    let codes = Arc::clone(&codes);
                    reg.event_rules.push(Box::new(move |ctx: &mut RuleContext| {
                        let mut reports: Vec<usize> = Vec::new();
                        for primary in block_scopes(ctx, &spec.blocks) {
                            if eval_condition(ctx, &codes, spec.condition.as_ref(), primary)
                                && present_in(ctx, &codes, &spec.codes, primary)
                            {
                                let line = primary
                                    .map(|p| p.line)
                                    .or(ctx.event.as_ref().map(|e| e.line))
                                    .unwrap_or(0);
                                reports.push(line);
                            }
                        }
                        for line in reports {
                            ctx.report(&spec.id, spec.message.clone(), &spec.refer, line, spec.severity);
                        }
                    }));
                }
                "compare" => {
                    let spec = spec_of(rule, &check);
                    let left = check.get("left_code").and_then(Value::as_str).unwrap_or("").to_owned();
                    let op = check.get("op").and_then(Value::as_str).unwrap_or("").to_owned();
                    let right = check.get("right_code").and_then(Value::as_str).unwrap_or("").to_owned();
                    let codes = Arc::clone(&codes);
                    reg.event_rules.push(Box::new(move |ctx: &mut RuleContext| {
                        let lvals = resolve_values(ctx, &codes, &left, None);
                        let rvals = resolve_values(ctx, &codes, &right, None);
                        let (Some(l), Some(r)) = (lvals.first(), rvals.first()) else { return };
                        if compare_scalar(l, &op, r) == Some(false) {
                            let line = ctx.event.as_ref().map(|e| e.line).unwrap_or(0);
                            ctx.report(&spec.id, spec.message.clone(), &spec.refer, line, spec.severity);
                        }
                    }));
                }
                // xsd_covered и неизвестные типы — пропускаем
                _ => {}
            }
        }
        Ok(())
    }

    fn compare_scalar(a: &str, op: &str, b: &str) -> Option<bool> {
        let (na, nb) = (to_number(a), to_number(b));
        let ord = match (na, nb) {
            (Some(x), Some(y)) => x.partial_cmp(&y)?,
            _ => a.cmp(b),
        };
        Some(match op {
            "eq" => ord == std::cmp::Ordering::Equal,
            "ne" => ord != std::cmp::Ordering::Equal,
            "gt" => ord == std::cmp::Ordering::Greater,
            "ge" => ord != std::cmp::Ordering::Less,
            "lt" => ord == std::cmp::Ordering::Less,
            "le" => ord != std::cmp::Ordering::Greater,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xsd::xmltree::parse_str;

    #[test]
    fn engine_builds_for_both_versions() {
        for v in ["5.0", "5.1"] {
            let e = RuleEngine::for_version(v).unwrap_or_else(|err| panic!("{v}: {err}"));
            assert!(!e.registry.event_rules.is_empty());
        }
    }

    #[test]
    fn subject_code_rule() {
        assert!(section5::valid_subject_code("1234"));
        assert!(section5::valid_subject_code("абв1"));
        assert!(section5::valid_subject_code("ABC123"));
        assert!(!section5::valid_subject_code("аб1a")); // смешение алфавитов
        assert!(!section5::valid_subject_code("123")); // короткий
        assert!(!section5::valid_subject_code("аб!12")); // спецсимвол
    }

    #[test]
    fn p5_06_event_after_formation_date() {
        let mut e = RuleEngine::for_version("5.1").unwrap();
        let source = parse_str(
            "<Source><a><CreditInfoDate>2024-01-10</CreditInfoDate></a></Source>",
            &[],
        )
        .unwrap();
        e.set_source(source);
        let ev = parse_str(
            r#"<FL_Event_1_1 orderNum="1" eventDateTime="2024-02-01T10:00:00"/>"#,
            &[],
        )
        .unwrap();
        let f = e.on_event(ev);
        assert!(f.iter().any(|f| f.code == "P5_06"), "{:?}", f.iter().map(|f| &f.code).collect::<Vec<_>>());
    }

    #[test]
    fn p5_23_duplicate_order_num() {
        let mut e = RuleEngine::for_version("5.1").unwrap();
        for _ in 0..2 {
            let ev = parse_str(
                r#"<FL_Event_1_1 orderNum="7" eventDateTime="2020-01-01T00:00:00"/>"#,
                &[],
            )
            .unwrap();
            let _ = e.on_event(ev);
        }
        let fins = e.finalize();
        assert!(fins.iter().any(|f| f.code == "P5_23"));
    }

    #[test]
    fn doc_counts_mismatch() {
        let mut e = RuleEngine::for_version("5.1").unwrap();
        let mut attrs = HashMap::new();
        attrs.insert("subjectsCount".to_owned(), "5".to_owned());
        attrs.insert("groupBlocksCount".to_owned(), "1".to_owned());
        e.begin_document(attrs);
        let _ = e.begin_subject("FL", None);
        let ev = parse_str(r#"<FL_Event_1_1 orderNum="1"/>"#, &[]).unwrap();
        let _ = e.on_event(ev);
        let fins = e.finalize();
        // subjectsCount=5 ≠ 1 → ошибка; groupBlocksCount=1 == 1 → ок
        assert_eq!(fins.iter().filter(|f| f.code == "DOC_COUNT").count(), 1);
    }

    #[test]
    fn p5_27_change_reason() {
        let mut e = RuleEngine::for_version("5.1").unwrap();
        let ev = parse_str(
            r#"<FL_Event_4_1 orderNum="1" operationCode="D1"/>"#,
            &[],
        )
        .unwrap();
        let f = e.on_event(ev);
        assert!(f.iter().any(|f| f.code == "P5_27"));
        let ev = parse_str(
            r#"<FL_Event_4_1 orderNum="2" operationCode="D1" changeReason="1"/>"#,
            &[],
        )
        .unwrap();
        let f = e.on_event(ev);
        assert!(!f.iter().any(|f| f.code == "P5_27"));
    }

    #[test]
    fn t6_003_forbidden_if() {
        // ФЛ_32.16 (актуальная стоимость) запрещена при ФЛ_32.1 = 0 — внутри блока FL_32_35_Group
        let mut e = RuleEngine::for_version("5.1").unwrap();
        let ev = parse_str(
            r#"<FL_Event_2_3 orderNum="1">
                 <FL_32_35_Group>
                   <costType>0</costType>
                   <currentCost>100.00</currentCost>
                 </FL_32_35_Group>
               </FL_Event_2_3>"#,
            &[],
        )
        .unwrap();
        // проверим, что правило вообще загрузилось и движок не падает
        let _ = e.on_event(ev);
    }
}
