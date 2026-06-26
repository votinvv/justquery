//! The `.jqmodel` format and the XML-model type.
//!
//! An XML model = XSD schema + code map + declarative validation rules + a match predicate.
//! It is stored as a single text file so it diffs cleanly in git as-is. Sections are separated by
//! marker delimiters on their own lines; XSD and JSON are stored unescaped, so the diff stays
//! readable.
//!
//! File format (see the canonical reference in `CLAUDE.md` → "XML models"):
//! ```text
//! ---model---   YAML-like manifest: id/name/description/priority/match
//! ---xsd---     XSD as-is
//! ---codes---   codes_map.json
//! ---rules---   rules.json
//! ---checksum--- SHA-256 of the normalized contents minus this field
//! ```
//!
//! The checksum is the SHA-256 over the concatenation of the normalized sections
//! (model/xsd/codes/rules), written as `"<hex>"`. Normalization: UTF-8 without BOM, `\n` line
//! endings, no trailing whitespace in the manifest (XSD/JSON are taken as-is, BOM stripped). On a
//! mismatch the model is flagged as corrupted and loaded read-only.

use std::path::{Path, PathBuf};

/// A section delimiter marker. The `name` field has no leading dashes (model/xsd/codes/rules/checksum).
struct SectionMarker(&'static str);

const MODEL: SectionMarker = SectionMarker("model");
const XSD: SectionMarker = SectionMarker("xsd");
const CODES: SectionMarker = SectionMarker("codes");
const RULES: SectionMarker = SectionMarker("rules");
const CHECKSUM: SectionMarker = SectionMarker("checksum");

/// Turn `SectionMarker("xsd")` into the string `---xsd---`.
fn marker_line(m: &SectionMarker) -> String {
    format!("---{}---", m.0)
}

/// The list of markers in canonical serialization order.
const ORDER: &[&SectionMarker] = &[&MODEL, &XSD, &CODES, &RULES, &CHECKSUM];

// ---------------------------------------------------------------------------
//  Manifest (model section)
// ---------------------------------------------------------------------------

/// A model identification rule: "the root element attribute is present (or equals one of the
/// values)". The set of rules per model = a list of `MatchRule`; a document matches if AT LEAST
/// ONE rule holds (OR). The root element name is checked separately, at the registry level
/// (`Registry::match_doc`): it must match the root element of the model's XSD schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchRule {
    /// The name of the root element attribute (for example `schemaVersion`).
    pub attr: String,
    /// Allowed values. An empty vector = the attribute must be present, value arbitrary.
    pub values: Vec<String>,
}

/// The set of model identification rules (model section → `match`). A document matches the set if
/// AT LEAST ONE rule holds (OR). The check runs against the root element attributes from the XML
/// head (without a full parse). An empty list = anything matches (then only the root element vs.
/// XSD match decides, checked in `Registry::match_doc`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchPred {
    /// Identification rules (OR). An empty list = any document (with the right root).
    pub rules: Vec<MatchRule>,
}

impl MatchPred {
    /// Check whether a document matches the set of identification rules based on the root element
    /// attributes (from the XML head). The semantics are OR: a single satisfied rule is enough. An
    /// empty list → `true` (only the root match decides, see `Registry::match_doc`). The root
    /// element name is NOT verified here — `match_doc` does that against the model's XSD schema.
    pub fn matches(&self, attrs: &[(String, String)]) -> bool {
        if self.rules.is_empty() {
            return true;
        }
        self.rules.iter().any(|rule| {
            let val = attrs.iter().find(|(n, _)| n == &rule.attr).map(|(_, v)| v.as_str());
            match (val, rule.values.is_empty()) {
                (None, _) => false,                  // attribute absent — rule not satisfied
                (Some(_), true) => true,             // presence is sufficient
                (Some(v), false) => rule.values.iter().any(|a| a == v),
            }
        })
    }
}

/// The model manifest (model section) — metadata + match predicate.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub id: String,
    pub description: String,
    /// Lower = earlier in the match queue; tie-break by `id`.
    pub priority: i64,
    pub r#match: MatchPred,
}

/// A compiled model — what lives in the registry and is fed into the validation engines.
#[derive(Clone, Debug)]
pub struct Model {
    pub manifest: Manifest,
    /// The XSD text as-is (needed for `xsd::loader::compile`).
    pub xsd: String,
    /// `codes_map.json` as raw text (parsed by `rules::CodesMap`).
    pub codes: String,
    /// `rules.json` as raw text (parsed by the engine into `RuleSpec`-like closures).
    pub rules: String,
    /// The checksum read from the file (empty = was absent). Import reads it to distinguish "no
    /// checksum" from "checksum mismatch" in the error text; integrity is via `intact`.
    pub checksum: String,
    /// Whether the hash in the file matches the actual contents. `false` → model read-only + banner.
    pub intact: bool,
    /// Path to the model file (for the registry/export); `None` — just created, not saved.
    pub path: Option<PathBuf>,
}

impl Model {
    /// `true` if the model has an XSD (a non-empty section). This is the "no XSD → no rules" gate.
    pub fn has_xsd(&self) -> bool {
        !self.xsd.trim().is_empty()
    }
}

// ---------------------------------------------------------------------------
//  Parsing
// ---------------------------------------------------------------------------

/// The result of parsing a file: either a valid model or a readable error.
pub fn parse(text: &str) -> Result<Model, String> {
    let sections = split_sections(text)?;
    let model_body = sections.get(MODEL.0).ok_or("no ---model--- section")?;
    let xsd = sections.get(XSD.0).cloned().unwrap_or_default();
    let codes = sections.get(CODES.0).cloned().unwrap_or_default();
    let rules = sections.get(RULES.0).cloned().unwrap_or_default();
    let file_checksum = sections.get(CHECKSUM.0).map(|s| s.trim().to_owned()).unwrap_or_default();

    let manifest = parse_manifest(model_body)?;
    if manifest.id.is_empty() {
        return Err("manifest: id not set".to_owned());
    }

    // The checksum — over the four normalized sections.
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

/// Split the text by markers. Returns section name (without dashes) → body (without the trailing
/// newline). Out-of-order markers and duplicates are allowed — the last occurrence is taken.
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
        } else if current.is_some() {
            buf.push_str(trimmed);
            buf.push('\n');
        }
    }
    if let Some(cur) = current {
        map.insert(cur, trim_trailing_newline(&buf));
    }

    // Section name validity is already checked in parse_marker, but verify there are no unknown ones.
    Ok(map)
}

/// Recognize a marker line. Returns the section name if the line has the form `---name---`.
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

/// Parse the model section (`key: value` / `key:` lines + nested `  - value`).
///
/// Supports exactly what the manifest needs:
/// ```text
/// id: 785p_5_1
/// description: |
///   multi-line
///   description
/// priority: 10
/// match:
///   - attr: schemaVersion
///     values:
///       - "5.1"
/// ```
///
/// The `name` field (from an early version of the format) is silently ignored — the display name = id.
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
        let (key, rest) = split_kv(line).ok_or_else(|| format!("manifest: invalid line «{line}»"))?;
        match key {
            "id" => id = rest.trim().to_owned(),
            "name" => {
                // legacy: name is no longer stored (the display name = id). We discard the value,
                // but don't complain — old .jqmodel files must still load.
            }
            "description" => {
                // `|` → a block value up to a line with smaller indentation / an empty section.
                description = if rest.trim() == "|" {
                    take_block(&mut lines)
                } else {
                    rest.trim().to_owned()
                };
            }
            "priority" => {
                priority = rest.trim().parse().map_err(|_| format!("priority is not a number: «{rest}»"))?;
            }
            "match" => {
                // The list of identification rules. Two formats are supported:
                //   (1) new — a YAML list:
                //         match:
                //           - attr: schemaVersion
                //             values:
                //               - "5.1"
                //           - attr: presenceOnlyAttr
                //   (2) legacy — `attr NAME:` + nested values (old .jqmodel files still load).
                while let Some(next) = lines.peek() {
                    if !next.starts_with("  ") {
                        break;
                    }
                    let nested = lines.next().unwrap();
                    let trimmed = nested.trim_start();
                    if let Some(rest) = trimmed.strip_prefix("- attr:") {
                        // new format: start of a rule, followed by `    values:` with a list
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
                                // the values block — the following lines with 6+ spaces
                            }
                        }
                        match_rules.push(MatchRule { attr: attr_name, values });
                    } else if let Some(rest) = trimmed.strip_prefix("- ") {
                        // `- attr: NAME` on a single line (values may follow below as `values:`)
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
                        // legacy: `attr schemaVersion:` + nested values
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
                        // legacy root: ignored (the root element name is no longer checked)
                    } else {
                        return Err(format!("match: unknown line «{nested}»"));
                    }
                }
            }
            other => return Err(format!("manifest: unknown field «{other}»")),
        }
    }

    Ok(Manifest {
        id,
        description,
        priority,
        r#match: MatchPred { rules: match_rules },
    })
}

/// `key: value` → (`key`, `value`). `value` may be empty (for nested sections/blocks).
fn split_kv(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let colon = line.find(':')?;
    Some((&line[..colon], line[colon + 1..].trim_start()))
}

/// Read a block value (after `key: |`) — lines with indentation ≥ 2, up to an indentation < 2 or EOF.
/// Lines are joined with `\n` WITHOUT trim — to preserve trailing spaces and line breaks (mirroring
/// `serialize_manifest`, which writes each line via `split('\n')`). The round-trip is idempotent:
/// `serialize → parse → serialize` is stable.
fn take_block<'a, I: Iterator<Item = &'a str>>(lines: &mut std::iter::Peekable<I>) -> String {
    let mut parts: Vec<String> = Vec::new();
    while let Some(next) = lines.peek() {
        if !next.starts_with("  ") {
            break;
        }
        let line = lines.next().unwrap();
        // strip one indentation level (2 spaces), keep the rest as-is (trailing spaces are significant)
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
//  Serialization
// ---------------------------------------------------------------------------

/// Serialize a model into `.jqmodel` text. The checksum is recomputed over the normalized body
/// (model/xsd/codes/rules), so a freshly serialized file is always `intact`.
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
    // description — always block YAML form (|), even for a single line: this way the content isn't
    // escaped (no quotes/backslashes), the diff is readable, and the parser strips one indentation
    // level. split('\n') (not .lines()) — to PRESERVE trailing line breaks and empty lines
    // (take_block restores them by joining without trim). Trailing spaces in a line are significant too.
    out.push_str("description: |\n");
    for line in m.description.split('\n') {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!("priority: {}\n", m.priority));
    // match — the list of identification rules (only if something is set).
    if !m.r#match.rules.is_empty() {
        out.push_str("match:\n");
        for rule in &m.r#match.rules {
            out.push_str(&format!("  - attr: {}\n", escape_yaml_str(&rule.attr)));
            if rule.values.is_empty() {
                // attribute presence — no values (we don't write values)
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
    // If there are YAML special characters — quote with `"` escaping.
    if s.is_empty() || s.contains([':', '#', '\n', '"', '\'']) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_owned()
    }
}

// ---------------------------------------------------------------------------
//  Checksum
// ---------------------------------------------------------------------------

/// The normalized body for hashing: four sections separated by `\n---<name>---\n`, UTF-8 without
/// BOM, `\n` line endings. The checksum field is not part of it (by definition it can't be — it's
/// computed from this).
fn canonical_body(model: &str, xsd: &str, codes: &str, rules: &str) -> Vec<u8> {
    let mut out = String::new();
    // Each section: `---<name>---\n` + normalized body + a guaranteed trailing `\n`.
    // The trailing-newline check runs over the ALREADY normalized body (not the raw one) — otherwise
    // a lone trailing `\r` would make the normalized content and the extra-`\n` decision disagree.
    let mut section = |name: &str, raw: &str| {
        let body = raw.strip_prefix('\u{feff}').unwrap_or(raw).replace("\r\n", "\n").replace('\r', "\n");
        out.push_str("---");
        out.push_str(name);
        out.push_str("---\n");
        out.push_str(&body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    };
    section("model", model);
    section("xsd", xsd);
    section("codes", codes);
    section("rules", rules);
    out.into_bytes()
}

/// SHA-256 of the canonical body, as a quoted hex string (see `checksum`).
fn checksum_of(model: &str, xsd: &str, codes: &str, rules: &str) -> String {
    use sha2::{Digest, Sha256};
    let body = canonical_body(model, xsd, codes, rules);
    let mut hasher = Sha256::new();
    hasher.update(&body);
    let digest = hasher.finalize();
    format!("\"{}\"", crate::crypt::to_hex(&digest))
}

// ---------------------------------------------------------------------------
//  I/O
// ---------------------------------------------------------------------------

/// Read a model from a file. `path` is stored in `Model.path`. The checksum is verified.
pub fn load_file(path: &Path) -> Result<Model, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut model = parse(&text)?;
    model.path = Some(path.to_path_buf());
    Ok(model)
}

/// Write a model to a file (recomputing the checksum). Used by the manager/editor on save and import.
pub fn save_file(model: &Model, path: &Path) -> Result<(), String> {
    let text = serialize(model);
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
//  Root element detector (without a full parse)
// ---------------------------------------------------------------------------

/// Scan the XML "head" and extract the local name of the root element + its attributes.
/// Skips the XML declaration (`<?xml …?>`) and skips comments/whitespace up to the first tag.
/// Returns `None` if the root element is not found within the first N bytes (or the XML is broken).
///
/// Local names/attributes — without the namespace prefix (as in the XSD engine and validator).
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
//  Model registry
// ---------------------------------------------------------------------------

/// The application's model registry. Loads all `.jqmodel` files from the app-data folder, keeps
/// them sorted by `priority` (tie-break by `id`), and matches documents against this ordered queue.
pub struct Registry {
    /// Models in match order (higher priority → earlier).
    models: Vec<Model>,
    /// Load errors for individual files (file name → message) — shown in the manager, so that a
    /// broken file doesn't silently drop out of the list.
    load_errors: Vec<(PathBuf, String)>,
}

impl Registry {
    /// Read all `.jqmodel` files from a directory. Broken files don't crash but land in `load_errors`.
    /// If the directory doesn't exist — an empty registry is returned (no models).
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
        // order: higher priority (smaller number) earlier, ties broken by id lexicographically.
        models.sort_by(|a, b| {
            a.manifest
                .priority
                .cmp(&b.manifest.priority)
                .then_with(|| a.manifest.id.cmp(&b.manifest.id))
        });
        Self { models, load_errors }
    }

    /// A snapshot of the registry (in match order). A clone — models are small, the registry is
    /// re-read rarely.
    pub fn models(&self) -> &[Model] {
        &self.models
    }

    /// Load errors for individual files.
    pub fn load_errors(&self) -> &[(PathBuf, String)] {
        &self.load_errors
    }

    /// Find the first (by priority) model that matches a document. Criteria: (1) the local name of
    /// the document's root element matches the root element of the model's XSD schema AND (2) at
    /// least one identification rule holds (or there are no rules). The XSD is compiled via a cache
    /// (`xsd::compile`, key — SHA-256 of the contents), so repeated opens are cheap; the scan goes
    /// over the pre-sorted queue and stops at the first matching model. A model without an XSD or
    /// with an unusable schema does not match (there's nothing to derive the root from).
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

    /// `true` if the registry has at least one model.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Build an empty registry (for the manager/tests: fill via `push`, then `sort`).
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self { models: Vec::new(), load_errors: Vec::new() }
    }

    /// Add a model to the registry. The match order is recomputed by `sort`.
    #[allow(dead_code)]
    pub fn push(&mut self, model: Model) {
        self.models.push(model);
    }

    /// Recompute the match order (priority → id). Call after a series of `push`/removals.
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
            "", // will be recomputed
        ]
        .join("\n")
    }

    #[test]
    fn roundtrip_parse_and_serialize() {
        // serialize → parse → serialize — stable (idempotent), checksum matches.
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
        // idempotent: re-serialization matches
        assert_eq!(serialize(&reparsed), serialized);
    }

    #[test]
    fn serialize_intact_checksum() {
        // A freshly serialized model has a correct checksum.
        let model = parse(&sample_text()).unwrap();
        let text = serialize(&model);
        let reparsed = parse(&text).unwrap();
        assert!(reparsed.intact);
        assert!(!reparsed.checksum.is_empty());
    }

    #[test]
    fn tampered_xsd_detected() {
        // The user edited the XSD by hand — the checksum doesn't match, intact=false.
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
        // An empty values list → only an attribute-presence check.
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
        // Multiple rules — satisfying AT LEAST ONE is enough (OR).
        let pred = MatchPred {
            rules: vec![
                MatchRule { attr: "schemaVersion".to_owned(), values: vec!["5.1".to_owned()] },
                MatchRule { attr: "kind".to_owned(), values: Vec::new() },
            ],
        };
        // both satisfied → match
        let both = vec![
            ("schemaVersion".to_owned(), "5.1".to_owned()),
            ("kind".to_owned(), "x".to_owned()),
        ];
        assert!(pred.matches(&both));
        // only the first rule satisfied → still a match (OR)
        assert!(pred.matches(&[("schemaVersion".to_owned(), "5.1".to_owned())]));
        // only the second rule satisfied (presence of kind) → match (OR)
        assert!(pred.matches(&[("kind".to_owned(), "y".to_owned())]));
        // no rule satisfied → no match
        assert!(!pred.matches(&[("schemaVersion".to_owned(), "9.9".to_owned())]));
    }

    #[test]
    fn missing_model_section_errors() {
        let text = "---xsd---\n<xs:schema/>\n";
        assert!(parse(text).is_err());
    }

    #[test]
    fn empty_id_errors() {
        let text = "---model---\nid:\ndescription: x\n---xsd---\n\n";
        assert!(parse(text).is_err());
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
        // The namespace prefix is stripped — local name "Document".
        let head = "<bk:Document xmlns:bk=\"urn:x\" schemaVersion=\"5.0\"/>";
        let (root, _) = detect_root(head).unwrap();
        assert_eq!(root, "Document");
    }

    #[test]
    fn detect_root_none_on_text() {
        // No tag in the head → None.
        assert!(detect_root("just text, no xml").is_none());
    }

    /// Build a registry from models with given priorities (in memory, without files).
    fn registry_of(specs: &[(i64, &str, &str)]) -> Registry {
        // (priority, id, attr-value) → a model with a match on schemaVersion=value.
        let mut models = Vec::new();
        for &(prio, id, val) in specs {
            let mut m = parse(&sample_text()).unwrap();
            m.manifest.id = id.to_owned();
            m.manifest.priority = prio;
            // match_doc verifies the document root against the XSD schema root → the test model needs
            // a minimal compilable schema with a Document root element.
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
        // The registry is built via the private constructor in tests: we'd use load_dir with a
        // tempdir — but for speed we assemble it via an empty registry + patch. Here we just check
        // the match logic with a minimal Registry.
        let mut reg = Registry::load_dir(std::path::Path::new("__nonexistent_for_test__"));
        reg.models = models;
        reg
    }

    #[test]
    fn registry_matches_by_priority() {
        // Two models match (schemaVersion=5.1); the one with the lower priority wins.
        let reg = registry_of(&[(20, "low", "5.1"), (10, "high", "5.1")]);
        let m = reg.match_doc("<Document schemaVersion=\"5.1\"/>").expect("match");
        assert_eq!(m.manifest.id, "high", "должна выиграть модель с меньшим priority");
    }

    #[test]
    fn registry_tiebreak_by_id() {
        // Equal priority → tie-break by id (lexicographically).
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

    /// End-to-end pipeline over real `.jqmodel` files from `schemas/built/`: load the registry, match
    /// a 5.1 document, compile the XSD and build the rule engine — all of it should pass without
    /// errors. Skipped if the files haven't been generated yet (see `gen_builtin_models`).
    #[test]
    fn built_models_load_and_match() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas").join("built");
        let reg = Registry::load_dir(&dir);
        if reg.is_empty() {
            eprintln!("schemas/built пуст — пропускаю (запусти gen_builtin_models для генерации)");
            return;
        }
        assert_eq!(reg.models().len(), 2, "ожидалось 2 встроенных модели в built/");
        // both models are intact (the checksum matches)
        assert!(reg.models().iter().all(|m| m.intact), "контрольная сумма не бьётся");
        // match 5.1
        let m = reg
            .match_doc("<?xml version=\"1.0\"?>\n<Document schemaVersion=\"5.1\"/>")
            .expect("5.1 должна сматчиться");
        assert_eq!(m.manifest.id, "785p_5_1");
        // the XSD compiles via the cache
        let schema = crate::xsd::compile(&m.xsd, &m.manifest.id).expect("xsd compile");
        assert_eq!(schema.root_name, "Document");
        // the rule engine builds
        let _engine = crate::rules::RuleEngine::for_model(&m.codes, &m.rules).expect("rules engine");
    }

    /// Generate `.jqmodel` files for the built-in (former) 5.0/5.1 models from `schemas/` and write
    /// them to `schemas/built/`. This is the "export built-ins locally": after the move to models,
    /// the product no longer carries schemas in the binary — the user imports these files via the
    /// manager. Marked `#[ignore]` so it doesn't run in a normal `cargo test`; run with:
    /// `cargo test --release gen_builtin_models -- --ignored`.
    #[test]
    #[ignore]
    fn gen_builtin_models() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let out_dir = root.join("schemas").join("built");
        std::fs::create_dir_all(&out_dir).expect("create built dir");
        for (v, mid) in [("5.0", "785p_5_0"), ("5.1", "785p_5_1")] {
            // XSD section: Main + transitive includes, concatenated into one block (as xsd::compile
            // does in prod — slicing by <xs:schema> happens when the model is loaded).
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
                        "Модель 785-П (ред. {v}): XSD + карта показателей + декларативные правила."
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
                checksum: String::new(), // save_file will recompute it
                intact: true,
                path: None,
            };
            let dest = out_dir.join(format!("{mid}.jqmodel"));
            save_file(&model, &dest).unwrap();
            // the checksum must match immediately after writing
            let reloaded = load_file(&dest).unwrap();
            assert!(reloaded.intact, "{mid}: контрольная сумма не бьётся после генерации");
            println!("written: {}", dest.display());
        }
    }

    /// Rewrite the `rules` section to the new `{name, message, severity, check}` format: `codes`/`block`
    /// move INSIDE `check`; `name` = `id` [+ " " + `title`]; the human-readable text → `message`
    /// (from `message` or `description` of past formats); `ref`/`title`/`id`/`description` are removed.
    /// Idempotent. Returns the new section JSON.
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
            // name = existing name, otherwise id [+ " " + title] (old formats).
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
            // message = existing message, otherwise description (the previous round's format).
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

    /// Migrate `.jqmodel` files to the new rules format IN PLACE: the user's models folder
    /// (`%APPDATA%\JustQuery\models`) + the repository root (stale export artifacts). Each file is
    /// loaded, the rules section is rewritten, and the checksum is recomputed on write. Ignored in a
    /// normal run; run with:
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
