//! Declarative registry of XML validation rules, assembled from the model's `rules.json`.
//!
//! The model is accumulative, single-pass: events are fed as subtrees are materialized by the
//! streaming validator. The context carries the Document attributes, the Source block, the
//! current Title and the current event.
//!
//! - all domain rules come from `rules.json` (DSL); indicator codes are resolved via
//!   `codes_map.json`;
//! - aggregates — orderNum uniqueness, subjectsCount/groupBlocksCount counters — run at
//!   document finalization.

use crate::proc::{Finding, Severity};
use crate::xsd::xmltree::XNode;
use serde_json::Value;
use std::collections::HashMap;

/// Map of indicator codes: code → variants (text element or flag pair).
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
        let obj = v.as_object().ok_or("codes_map: expected an object")?;
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

/// Context for evaluating rules on the current event.
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
/// Document finalization rule: receives the accumulated orderNum values (attr, line, event_name)
/// and the context, returns findings. Needed for aggregates like P5_23 (orderNum uniqueness).
type FinalizeRule = Box<dyn Fn(&mut RuleContext, &[(String, usize, String)]) -> Vec<Finding> + Send>;

/// Set of rules for a specific model.
pub struct Registry {
    event_rules: Vec<EventRule>,
    subject_rules: Vec<EventRule>,
    finalize_rules: Vec<FinalizeRule>,
}

/// Engine: applies the registry's rules to the fed subtrees.
pub struct RuleEngine {
    registry: Registry,
    ctx: RuleContext,
    order_nums: Vec<(String, usize, String)>, // (orderNum, line, event_name)
    subject_count: usize,
    event_count: usize,
}

impl RuleEngine {
    /// Build the engine for an XML model: `codes_json` — contents of the `codes` section,
    /// `rules_json` — of the `rules` section. All rules (the former "section 5" + "section 6")
    /// come from the declarative rules.json registry — there are no more domain Rust functions.
    /// DSL extensions: required_if/forbidden_if/compare (inherited) + attr_date_le /
    /// attr_required_if / attr_pattern / aggregate:unique.
    pub fn for_model(codes_json: &str, rules_json: &str) -> Result<Self, String> {
        let codes = std::sync::Arc::new(CodesMap::parse(codes_json)?);
        let mut registry = Registry {
            event_rules: Vec::new(),
            subject_rules: Vec::new(),
            finalize_rules: Vec::new(),
        };
        // All validation rules (the former section5 + section6) now come from the model's
        // declarative rules.json registry — there are no more 5.x domain Rust functions.
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

    /// Subject start: kind ∈ {FL, UL, AF}; `title` — the materialized Title.
    /// Returns the findings from the subject rules.
    pub fn begin_subject(&mut self, kind: &str, title: Option<XNode>) -> Vec<Finding> {
        self.ctx.subject_kind = kind.to_owned();
        self.ctx.title = title;
        self.subject_count += 1;
        for rule in &self.registry.subject_rules {
            rule(&mut self.ctx);
        }
        std::mem::take(&mut self.ctx.findings)
    }

    /// The next `*_Event_*` event. Returns the findings from the event rules.
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

    /// End of document: aggregate checks.
    pub fn finalize(mut self) -> Vec<Finding> {
        let mut out = Vec::new();
        // unique aggregates (P5_23 and the like) — from the declarative registry (finalize_rules).
        // orderNum values were accumulated in on_event; we pass them to every finalize rule.
        let orders = std::mem::take(&mut self.order_nums);
        for rule in &self.registry.finalize_rules {
            out.extend(rule(&mut self.ctx, &orders));
        }
        // counter consistency
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
                        "Attribute subjectsCount=«{sc}» does not match the number of subjects in the document ({})." ,
                        self.subject_count
                    ),
                    source: "subjectsCount attribute".to_owned(),
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
                        "Attribute groupBlocksCount=«{gbc}» does not match the number of indicator block groups ({})." ,
                        self.event_count
                    ),
                    source: "groupBlocksCount attribute".to_owned(),
                });
            }
        }
        out
    }
}

// ===========================================================================
//  All rules — declarative registry (the model's rules.json). The former
//  "section 5" (Rust functions P5_06/P5_16/P5_17/P5_18/P5_23/P5_27) was moved
//  into the DSL below:
//    attr_date_le / required_if(+else) / attr_pattern / aggregate:unique_attr /
//    attr_required_if. See rules.json.
// ===========================================================================
// ===========================================================================
//  Section 6 (declarative registry) — the single source of rules
// ===========================================================================

mod section6 {
    use super::*;
    use std::sync::Arc;

    /// Values of indicator `code` in the nearest scope:
    /// primary (block) → event → title → source.
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

    /// Blocks for per-block evaluation, or [None] for event-level evaluation.
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

    /// Scopes for evaluating a rule. By default (block-scope) — per block (`block_scopes`): both
    /// the condition and the presence are checked WITHIN each block (e.g. P6_003: costType and
    /// currentCost — siblings in one block). With `event_scope` — a SINGLE evaluation at the event
    /// level (`[None]`): the condition (a flag on the event, e.g. FL_18.9) and block presence are
    /// checked across the entire event subtree. This is the P5_16/P5_17 fix, where the flag sits on
    /// the event and the block is its descendant (previously the condition was wrongly resolved in
    /// the block scope and did not find the flag, and a missing block prevented the "required" half
    /// from firing, since `block_scopes` returned empty).
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
        /// `check.scope == "event"` → evaluate at the event level (not per block). See `rule_scopes`.
        event_scope: bool,
    }

    /// Build a rule spec. The engine is driven entirely by the JSON `check` field: the indicator
    /// codes (`codes`) and the list of blocks (`block`) for per-block evaluation are taken from
    /// `check` itself, not from the rule's top-level fields. At the rule's top level are `name`
    /// (finding identifier/code), `message` (finding text), `severity` and `check`.
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
                .unwrap_or("Validation rule violated")
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
                        let mut reports: Vec<usize> = Vec::new();
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
                                reports.push(line);
                            }
                        }
                        for line in reports {
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
                // attr_date_le — the event's date attribute ≤ the source's date attribute, trimmed
                // to 10 characters (YYYY-MM-DD). Port of P5_06: eventDateTime ≤ CreditInfoDate.
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
                // attr_required_if — the event attribute is required when all gate conditions hold:
                // `event_names` (event name ∈ the list) and `gate_attr`+`gate_values` (another
                // attribute equals one of the values). Port of P5_27: changeReason is required in
                // Events 4.x when operationCode ∈ {D1,D2}.
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
                                return; // gate not satisfied — the rule does not apply
                            }
                        }
                        if ev.attr(&attr).is_none() {
                            ctx.report(&spec.id, spec.message.clone(), ev.line, spec.severity);
                        }
                    }));
                }
                // attr_pattern — the attribute matches a regex. Port of P5_18: subjectCode
                // (4–15 characters, digits and letters of a single alphabet). The regex is
                // compiled once.
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
                        None => continue, // broken pattern — skip the rule (don't crash the engine)
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
                // aggregate: unique_attr — the event attribute (orderNum by default) is unique
                // within the document. A document-level rule: invoked in finalize() with the
                // accumulated list (attr_value, line, event_name). Port of P5_23.
                "aggregate" if check.get("kind").and_then(Value::as_str) == Some("unique_attr") => {
                    let spec = spec_of(rule, &check);
                    registry_finalize_unique.push(spec);
                }
                // xsd_covered and unknown types — skip
                _ => {}
            }
        }
        // Register a single aggregate finalize rule that checks all accumulated unique specs.
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

    /// Test helper: build the engine from the 785-P rule fixtures in `schemas/5.x`, reading
    /// codes_map/rules from disk.
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
        // subjectsCount=5 ≠ 1 → error; groupBlocksCount=1 == 1 → ok
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
        // FL_32.16 (current cost) is forbidden when FL_32.1 = 0 — within the FL_32_35_Group block
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
        // verify that the rule loaded at all and the engine does not crash
        let _ = e.on_event(ev);
    }

    /// P5_16 (scope:event): the FL_30 block is REQUIRED when flag=0 and FORBIDDEN when flag=1. The
    /// flag sits on the event, the block is its descendant. Covers the event-scope fix (previously
    /// the required half did not fire and forbidden fired falsely). Isolation by finding code prefix
    /// (= name «P5_16 …»).
    #[test]
    fn p5_16_block_required_by_event_flag() {
        let ev = |xml: &str| parse_str(xml, &[]).unwrap();
        // (A) flag=0, block ABSENT → required, but missing → finding
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_8 orderNum="1"><monetarySourceExist_0/></FL_Event_1_8>"#));
        assert!(f.iter().any(|f| f.code.starts_with("P5_16")), "flag0+noblock → required: {:?}", f.iter().map(|f| &f.code).collect::<Vec<_>>());
        // (B) flag=1, block PRESENT → forbidden, but present → finding
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_8 orderNum="1"><monetarySourceExist_1/><FL_30_NonMonetarySource><item>x</item></FL_30_NonMonetarySource></FL_Event_1_8>"#));
        assert!(f.iter().any(|f| f.code.starts_with("P5_16")), "flag1+block → forbidden");
        // (C) flag=0, block PRESENT → correct → silence
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_8 orderNum="1"><monetarySourceExist_0/><FL_30_NonMonetarySource><item>x</item></FL_30_NonMonetarySource></FL_Event_1_8>"#));
        assert!(!f.iter().any(|f| f.code.starts_with("P5_16")), "flag0+block → корректно");
        // (D) flag=1, block ABSENT → correct → silence
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_8 orderNum="1"><monetarySourceExist_1/></FL_Event_1_8>"#));
        assert!(!f.iter().any(|f| f.code.starts_with("P5_16")), "flag1+noblock → корректно");
    }

    /// P5_17 (scope:event): the FL_31 block is required when flag FL_18.10=0, forbidden when =1.
    #[test]
    fn p5_17_block_required_by_event_flag() {
        let ev = |xml: &str| parse_str(xml, &[]).unwrap();
        // (A) flag=0, block absent → finding
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_9 orderNum="1"><monetarySubjectExist_0/></FL_Event_1_9>"#));
        assert!(f.iter().any(|f| f.code.starts_with("P5_17")), "flag0+noblock → required: {:?}", f.iter().map(|f| &f.code).collect::<Vec<_>>());
        // (D) flag=1, block absent → silence
        let mut e = for_version("5.1");
        let f = e.on_event(ev(r#"<FL_Event_1_9 orderNum="1"><monetarySubjectExist_1/></FL_Event_1_9>"#));
        assert!(!f.iter().any(|f| f.code.starts_with("P5_17")), "flag1+noblock → корректно");
    }
}
