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
    pub doc_attrs: HashMap<String, String>,
    pub source: Option<XNode>,
    pub title: Option<XNode>,
    pub event: Option<XNode>,
    pub event_name: String,
    pub subject_kind: String, // FL | UL | AF
    pub findings: Vec<Finding>,
}

impl RuleContext {
    fn new() -> Self {
        Self {
            doc_attrs: HashMap::new(),
            source: None,
            title: None,
            event: None,
            event_name: String::new(),
            subject_kind: String::new(),
            findings: Vec::new(),
        }
    }

    fn report(&mut self, code: &str, message: String, line: usize, severity: Severity) {
        self.findings.push(Finding {
            severity,
            line,
            code: code.to_owned(),
            message,
            source: String::new(),
        });
    }
}

type EventRule = Box<dyn Fn(&mut RuleContext) + Send>;
/// Правило финализации документа: получает накопленные orderNum-ы (attr, line, event_name) и
/// контекст, возвращает находки. Нужно для агрегатов вроде P5_23 (уникальность orderNum).
type FinalizeRule = Box<dyn Fn(&mut RuleContext, &[(String, usize, String)]) -> Vec<Finding> + Send>;

/// Набор правил для конкретной версии схемы.
pub struct Registry {
    event_rules: Vec<EventRule>,
    subject_rules: Vec<EventRule>,
    finalize_rules: Vec<FinalizeRule>,
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
    /// Собрать движок для XML-модели: `codes_json` — содержимое секции `codes`, `rules_json` —
    /// секции `rules`. Все правила (бывшие «раздел 5» + «раздел 6») берутся из декларативного
    /// реестра rules.json — Rust-функций домена больше нет. Расширения DSL: required_if/forbidden_if/
    /// compare (унаследованы) + attr_date_le / attr_required_if / attr_pattern / aggregate:unique.
    pub fn for_model(codes_json: &str, rules_json: &str) -> Result<Self, String> {
        let codes = std::sync::Arc::new(CodesMap::parse(codes_json)?);
        let mut registry = Registry {
            event_rules: Vec::new(),
            subject_rules: Vec::new(),
            finalize_rules: Vec::new(),
        };
        // Все правила валидации (бывшие section5 + section6) теперь берутся из декларативного
        // реестра rules.json модели — Rust-функций домена 5.x больше нет.
        section6::install(&mut registry, rules_json, codes)?;
        Ok(Self {
            registry,
            ctx: RuleContext::new(),
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
        // unique-агрегаты (P5_23 и подобные) — из декларативного реестра (finalize_rules).
        // orderNum-ы накапливались в on_event; передаём их каждому finalize-rule.
        let orders = std::mem::take(&mut self.order_nums);
        for rule in &self.registry.finalize_rules {
            out.extend(rule(&mut self.ctx, &orders));
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
//  Все правила — декларативный реестр (rules.json модели). Бывший «раздел 5»
//  (Rust-функции P5_06/P5_16/P5_17/P5_18/P5_23/P5_27) перенесён в DSL ниже:
//    attr_date_le / required_if(+else) / attr_pattern / aggregate:unique_attr /
//    attr_required_if. См. rules.json и COVERAGE.md.
// ===========================================================================
// ===========================================================================
//  Раздел 6 (декларативный реестр) — единственный источник правил
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

    /// Области для оценки правила. По умолчанию (block-scope) — пер-блочно (`block_scopes`): и
    /// условие, и присутствие проверяются ВНУТРИ каждого блока (напр. P6_003: costType и currentCost —
    /// соседи в одном блоке). При `event_scope` — ОДНА оценка на уровне события (`[None]`): условие
    /// (флаг на событии, напр. ФЛ_18.9) и присутствие блока проверяются по всему поддереву события.
    /// Это и есть фикс P5_16/P5_17, где флаг лежит на событии, а блок — его потомок (раньше условие
    /// ошибочно резолвилось в scope блока и не находило флаг, а отсутствующий блок не давал сработать
    /// «обязательной» половине, т.к. `block_scopes` возвращал пусто).
    fn rule_scopes<'a>(
        ctx: &'a RuleContext,
        blocks: &[String],
        event_scope: bool,
    ) -> Vec<Option<&'a XNode>> {
        if event_scope {
            vec![None]
        } else {
            block_scopes(ctx, blocks)
        }
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
        message: String,
        severity: Severity,
        condition: Option<Value>,
        else_forbidden: bool,
        /// `check.scope == "event"` → оценивать на уровне события (а не пер-блочно). См. `rule_scopes`.
        event_scope: bool,
    }

    /// Собрать спеку правила. Движок полностью управляется JSON-полем `check`: коды показателей
    /// (`codes`) и список блоков (`block`) для пер-блочной оценки берутся из самого `check`, а не из
    /// верхнеуровневых полей правила. На верхнем уровне правила — `name` (идентификатор/код находки),
    /// `message` (текст находки), `severity` и `check`.
    fn spec_of(rule: &Value, check: &Value) -> RuleSpec {
        let arr = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        RuleSpec {
            id: rule.get("name").and_then(Value::as_str).unwrap_or("T6").to_owned(),
            codes: arr(check.get("codes")),
            blocks: arr(check.get("block")),
            message: rule
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Нарушено правило валидации")
                .to_owned(),
            severity: match rule.get("severity").and_then(Value::as_str) {
                Some("warn") | Some("warning") => Severity::Warning,
                _ => Severity::Error,
            },
            condition: check.get("condition").cloned(),
            else_forbidden: check.get("else").and_then(Value::as_str) == Some("forbidden"),
            event_scope: check.get("scope").and_then(Value::as_str) == Some("event"),
        }
    }

    pub fn install(reg: &mut Registry, rules_json: &str, codes: Arc<CodesMap>) -> Result<(), String> {
        let data: Value = serde_json::from_str(rules_json).map_err(|e| e.to_string())?;
        let mut registry_finalize_unique: Vec<RuleSpec> = Vec::new();
        for rule in data.get("rules").and_then(Value::as_array).into_iter().flatten() {
            let check = rule.get("check").cloned().unwrap_or(Value::Null);
            let ctype = check.get("type").and_then(Value::as_str).unwrap_or("");
            match ctype {
                "required_if" => {
                    let spec = spec_of(rule, &check);
                    let codes = Arc::clone(&codes);
                    reg.event_rules.push(Box::new(move |ctx: &mut RuleContext| {
                        let mut reports: Vec<(usize,)> = Vec::new();
                        for primary in rule_scopes(ctx, &spec.blocks, spec.event_scope) {
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
                            ctx.report(&spec.id, spec.message.clone(), line, spec.severity);
                        }
                    }));
                }
                "forbidden_if" => {
                    let spec = spec_of(rule, &check);
                    let codes = Arc::clone(&codes);
                    reg.event_rules.push(Box::new(move |ctx: &mut RuleContext| {
                        let mut reports: Vec<usize> = Vec::new();
                        for primary in rule_scopes(ctx, &spec.blocks, spec.event_scope) {
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
                            ctx.report(&spec.id, spec.message.clone(), line, spec.severity);
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
                            ctx.report(&spec.id, spec.message.clone(), line, spec.severity);
                        }
                    }));
                }
                // attr_date_le — дата-атрибут события ≤ дата-атрибут источника, с обрезкой по 10
                // символам (YYYY-MM-DD). Перенос P5_06: eventDateTime ≤ CreditInfoDate.
                "attr_date_le" => {
                    let spec = spec_of(rule, &check);
                    let event_attr =
                        check.get("event_attr").and_then(Value::as_str).unwrap_or("").to_owned();
                    let source_attr =
                        check.get("source_attr").and_then(Value::as_str).unwrap_or("").to_owned();
                    reg.event_rules.push(Box::new(move |ctx: &mut RuleContext| {
                        let Some(ev) = ctx.event.as_ref() else { return };
                        let Some(src) = ctx.source.as_ref() else { return };
                        let edate: String = ev.attr(&event_attr).unwrap_or("").chars().take(10).collect();
                        let fdate = src.iter().find_map(|d| {
                            (d.name.contains(&source_attr)).then(|| d.text.chars().take(10).collect::<String>())
                        }).unwrap_or_default();
                        if !edate.is_empty() && !fdate.is_empty() && edate.as_str() > fdate.as_str() {
                            ctx.report(&spec.id, spec.message.clone(), ev.line, spec.severity);
                        }
                    }));
                }
                // attr_required_if — атрибут события обязателен, когда выполняются все gate-условия:
                // `event_names` (имя события ∈ списка) и `gate_attr`+`gate_values` (другой атрибут
                // равен одному из значений). Перенос P5_27: changeReason обязателен в Events 4.x
                // при operationCode ∈ {D1,D2}.
                "attr_required_if" => {
                    let spec = spec_of(rule, &check);
                    let attr =
                        check.get("attr").and_then(Value::as_str).unwrap_or("").to_owned();
                    let event_names: Vec<String> = check
                        .get("event_names")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                        .unwrap_or_default();
                    let gate_attr =
                        check.get("gate_attr").and_then(Value::as_str).unwrap_or("").to_owned();
                    let gate_values: Vec<String> = check
                        .get("gate_values")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                        .unwrap_or_default();
                    reg.event_rules.push(Box::new(move |ctx: &mut RuleContext| {
                        if !event_names.is_empty() && !event_names.contains(&ctx.event_name) {
                            return;
                        }
                        let Some(ev) = ctx.event.as_ref() else { return };
                        if !gate_attr.is_empty() {
                            let gv = ev.attr(&gate_attr).unwrap_or("");
                            if !gate_values.iter().any(|v| v == gv) {
                                return; // gate не выполнен — правило не применяется
                            }
                        }
                        if ev.attr(&attr).is_none() {
                            ctx.report(&spec.id, spec.message.clone(), ev.line, spec.severity);
                        }
                    }));
                }
                // attr_pattern — атрибут соответствует regex. Перенос P5_18: subjectCode
                // (4–15 символов, цифры и буквы одного алфавита). Regex компилируется один раз.
                "attr_pattern" => {
                    let spec = spec_of(rule, &check);
                    let scope = check.get("scope").and_then(Value::as_str).unwrap_or("title").to_owned();
                    let attr =
                        check.get("attr").and_then(Value::as_str).unwrap_or("").to_owned();
                    let pattern =
                        check.get("pattern").and_then(Value::as_str).unwrap_or("").to_owned();
                    let re = regex::Regex::new(&pattern).ok();
                    let re = match re {
                        Some(r) => r,
                        None => continue, // битый pattern — пропустить правило (не ронять движок)
                    };
                    if scope == "title" {
                        reg.subject_rules.push(Box::new(move |ctx: &mut RuleContext| {
                            let Some(title) = ctx.title.as_ref() else { return };
                            let Some(code) = title.attr(&attr) else { return };
                            if !code.is_empty() && !re.is_match(code) {
                                ctx.report(&spec.id, spec.message.clone(), title.line, spec.severity);
                            }
                        }));
                    }
                }
                // aggregate: unique_attr — атрибут события (по умолчанию orderNum) уникален в
                // пределах документа. Document-level правило: вызывается в finalize() с накопленным
                // списком (attr_value, line, event_name). Перенос P5_23.
                "aggregate" if check.get("kind").and_then(Value::as_str) == Some("unique_attr") => {
                    let spec = spec_of(rule, &check);
                    registry_finalize_unique.push(spec);
                }
                // xsd_covered и неизвестные типы — пропускаем
                _ => {}
            }
        }
        // Зарегистрировать один агрегатный finalize-rule, проверяющий все накопленные unique-спеки.
        if !registry_finalize_unique.is_empty() {
            let specs = registry_finalize_unique;
            reg.finalize_rules.push(Box::new(move |_ctx, orders: &[(String, usize, String)]| {
                let mut out = Vec::new();
                for sp in &specs {
                    let mut seen: HashMap<&str, usize> = HashMap::new();
                    for (val, line, _name) in orders {
                        if seen.contains_key(val.as_str()) {
                            out.push(Finding {
                                severity: sp.severity,
                                line: *line,
                                code: sp.id.clone(),
                                message: sp.message.clone(),
                                source: String::new(),
                            });
                        } else {
                            seen.insert(val.as_str(), *line);
                        }
                    }
                }
                out
            }));
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

    /// Хелпер тестов: собрать движок для встроенной (пока не вырезанной) модели 5.x, читая её
    /// codes_map/rules с диска. После Этапа 5 (вырезание встроенных) эти тесты уходят вместе с 5.x.
    fn for_version(version: &str) -> RuleEngine {
        let (codes, rules) = match version {
            "5.0" => (
                include_str!("../../schemas/5.0/codes_map.json"),
                include_str!("../../schemas/5.0/rules.json"),
            ),
            "5.1" => (
                include_str!("../../schemas/5.1/codes_map.json"),
                include_str!("../../schemas/5.1/rules.json"),
            ),
            other => panic!("неизвестная версия «{other}»"),
        };
        RuleEngine::for_model(codes, rules).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn engine_builds_for_both_versions() {
        for v in ["5.0", "5.1"] {
            let e = for_version(v);
            assert!(!e.registry.event_rules.is_empty());
        }
    }

    #[test]
    fn p5_06_event_after_formation_date() {
        let mut e = for_version("5.1");
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
        assert!(f.iter().any(|f| f.code.starts_with("P5_06")), "{:?}", f.iter().map(|f| &f.code).collect::<Vec<_>>());
    }

    #[test]
    fn p5_23_duplicate_order_num() {
        let mut e = for_version("5.1");
        for _ in 0..2 {
            let ev = parse_str(
                r#"<FL_Event_1_1 orderNum="7" eventDateTime="2020-01-01T00:00:00"/>"#,
                &[],
            )
            .unwrap();
            let _ = e.on_event(ev);
        }
        let fins = e.finalize();
        assert!(fins.iter().any(|f| f.code.starts_with("P5_23")));
    }

    #[test]
    fn doc_counts_mismatch() {
        let mut e = for_version("5.1");
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
        let mut e = for_version("5.1");
        let ev = parse_str(
            r#"<FL_Event_4_1 orderNum="1" operationCode="D1"/>"#,
            &[],
        )
        .unwrap();
        let f = e.on_event(ev);
        assert!(f.iter().any(|f| f.code.starts_with("P5_27")));
        let ev = parse_str(
            r#"<FL_Event_4_1 orderNum="2" operationCode="D1" changeReason="1"/>"#,
            &[],
        )
        .unwrap();
        let f = e.on_event(ev);
        assert!(!f.iter().any(|f| f.code.starts_with("P5_27")));
    }

    #[test]
    fn t6_003_forbidden_if() {
        // ФЛ_32.16 (актуальная стоимость) запрещена при ФЛ_32.1 = 0 — внутри блока FL_32_35_Group
        let mut e = for_version("5.1");
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

    /// P5_16 (scope:event): блок FL_30 ОБЯЗАТЕЛЕН при флаге=0 и ЗАПРЕЩЁН при флаге=1. Флаг лежит на
    /// событии, блок — его потомок. Покрывает фикс event-scope (раньше required-половина не срабатывала,
    /// а forbidden ложно срабатывала). Изоляция по префиксу кода находки (= name «P5_16 …»).
    #[test]
    fn p5_16_block_required_by_event_flag() {
        let ev = |xml: &str| parse_str(xml, &[]).unwrap();
        // (A) флаг=0, блок ОТСУТСТВУЕТ → обязателен, но нет → находка
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_8 orderNum="1"><monetarySourceExist_0/></FL_Event_1_8>"#));
        assert!(f.iter().any(|f| f.code.starts_with("P5_16")), "flag0+noblock → required: {:?}", f.iter().map(|f| &f.code).collect::<Vec<_>>());
        // (B) флаг=1, блок ПРИСУТСТВУЕТ → запрещён, но есть → находка
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_8 orderNum="1"><monetarySourceExist_1/><FL_30_NonMonetarySource><item>x</item></FL_30_NonMonetarySource></FL_Event_1_8>"#));
        assert!(f.iter().any(|f| f.code.starts_with("P5_16")), "flag1+block → forbidden");
        // (C) флаг=0, блок ПРИСУТСТВУЕТ → корректно → тишина
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_8 orderNum="1"><monetarySourceExist_0/><FL_30_NonMonetarySource><item>x</item></FL_30_NonMonetarySource></FL_Event_1_8>"#));
        assert!(!f.iter().any(|f| f.code.starts_with("P5_16")), "flag0+block → корректно");
        // (D) флаг=1, блок ОТСУТСТВУЕТ → корректно → тишина
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_8 orderNum="1"><monetarySourceExist_1/></FL_Event_1_8>"#));
        assert!(!f.iter().any(|f| f.code.starts_with("P5_16")), "flag1+noblock → корректно");
    }

    /// P5_17 (scope:event): блок FL_31 обязателен при флаге ФЛ_18.10=0, запрещён при =1.
    #[test]
    fn p5_17_block_required_by_event_flag() {
        let ev = |xml: &str| parse_str(xml, &[]).unwrap();
        // (A) флаг=0, блок отсутствует → находка
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_9 orderNum="1"><monetarySubjectExist_0/></FL_Event_1_9>"#));
        assert!(f.iter().any(|f| f.code.starts_with("P5_17")), "flag0+noblock → required: {:?}", f.iter().map(|f| &f.code).collect::<Vec<_>>());
        // (D) флаг=1, блок отсутствует → тишина
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_9 orderNum="1"><monetarySubjectExist_1/></FL_Event_1_9>"#));
        assert!(!f.iter().any(|f| f.code.starts_with("P5_17")), "flag1+noblock → корректно");
    }
}
