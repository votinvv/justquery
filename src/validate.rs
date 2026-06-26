//! Streaming document validation: XSD (in-house engine) + declarative model rules — single pass.
//!
//! Works over a piece table snapshot. quick-xml reads through a line tracker (exact line numbers
//! without a full load), the XSD automaton walks the element stack, and the Source / Title /
//! `*_Event_*` event subtrees are materialized into a mini-DOM that feeds the rule engine — memory
//! is O(tree depth + size of a single event).

use crate::doc::piece_table::PieceSnapshot;
use crate::proc::{Finding, ProcMsg, Severity};
use crate::rules::RuleEngine;
use crate::xsd::model::TypeRef;
use crate::xsd::xmltree::{local_name_of, XNode};
use crate::xsd::Schema;
use quick_xml::events::Event;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Read;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

const FINDINGS_BATCH: usize = 100;
const MAX_SIMPLE_TEXT: usize = 4 * 1024 * 1024; // value buffer of a simple element

/// Resolve an entity reference from a `GeneralRef` event (quick-xml ≥0.38 delivers them
/// separately from text): numeric `&#NN;`/`&#xNN;` and the predefined ones become a character;
/// unknown ones revert to their original `&name;` form.
pub(crate) fn resolve_ref(r: &quick_xml::events::BytesRef) -> String {
    if let Ok(Some(ch)) = r.resolve_char_ref() {
        return ch.to_string();
    }
    let name = r.decode().unwrap_or_default();
    match name.as_ref() {
        "lt" => "<".to_owned(),
        "gt" => ">".to_owned(),
        "amp" => "&".to_owned(),
        "apos" => "'".to_owned(),
        "quot" => "\"".to_owned(),
        _ => format!("&{name};"),
    }
}

/// Start background validation of a document snapshot against an XML model.
///
/// `xsd` is the model's XSD text; `codes`/`rules` are its declarative sections; `label` is the
/// label for diagnostic messages (model id/name). The schema is compiled through the `xsd::compile`
/// cache.
pub fn spawn_validate(
    snap: PieceSnapshot,
    xsd: String,
    codes: String,
    rules: String,
    label: String,
    cancel: Arc<AtomicBool>,
    tx: Sender<ProcMsg>,
) {
    std::thread::spawn(move || {
        let schema = match crate::xsd::compile(&xsd, &label) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(ProcMsg::Failed(format!("schema failed to load: {e}")));
                return;
            }
        };
        let engine = match RuleEngine::for_model(&codes, &rules) {
            Ok(e) => e,
            Err(e) => {
                let _ = tx.send(ProcMsg::Failed(format!("rules failed to load: {e}")));
                return;
            }
        };
        let msg = run(&snap, schema, engine, &cancel, &tx);
        let _ = tx.send(msg);
    });
}

// ---------------------------------------------------------------------------
//  Line tracker: counts '\n' in consumed bytes; positions are queried monotonically
// ---------------------------------------------------------------------------

struct Tracker {
    chunks: VecDeque<(u64, Vec<u8>)>, // (start offset, bytes)
    consumed: u64,
    line: usize, // 1-based
}

impl Tracker {
    fn new() -> Self {
        Self { chunks: VecDeque::new(), consumed: 0, line: 1 }
    }

    fn push(&mut self, off: u64, data: &[u8]) {
        if !data.is_empty() {
            self.chunks.push_back((off, data.to_vec()));
        }
    }

    /// The (1-based) line that byte `pos` falls on. Positions must be non-decreasing.
    fn line_at(&mut self, pos: u64) -> usize {
        while self.consumed < pos {
            let Some((off, data)) = self.chunks.front() else { break };
            let start = (self.consumed - off) as usize;
            let want = (pos - self.consumed) as usize;
            let avail = data.len() - start;
            let take = want.min(avail);
            self.line += memchr::memchr_iter(b'\n', &data[start..start + take]).count();
            self.consumed += take as u64;
            if take == avail {
                self.chunks.pop_front();
            }
        }
        self.line
    }
}

/// Read wrapper: copies the bytes it reads into the tracker.
struct TrackingReader<'a> {
    inner: crate::doc::piece_table::SnapReader<'a>,
    tracker: Rc<RefCell<Tracker>>,
    offset: u64,
}

impl Read for TrackingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.tracker.borrow_mut().push(self.offset, &buf[..n]);
        self.offset += n as u64;
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
//  XSD pass
// ---------------------------------------------------------------------------

/// Content of the current element for the XSD check.
enum Content {
    /// Complex type: content-model NFA (None = no children allowed).
    Complex { type_id: usize, states: Vec<usize>, text_reported: bool },
    /// Simple type: accumulate text.
    Simple { type_id: usize, text: String, overflow: bool },
    /// Unchecked (anyType or error recovery).
    Any,
}

struct Frame {
    name: String,
    content: Content,
    line: usize,
}

/// Materializes a subtree for the rule engine.
struct Materializer {
    stack: Vec<XNode>,
}

impl Materializer {
    fn start(&mut self, node: XNode) {
        self.stack.push(node);
    }
    fn text(&mut self, t: &str) {
        if let Some(top) = self.stack.last_mut() {
            top.text.push_str(t);
        }
    }
    /// Close the current element; return the root if the subtree is complete.
    fn end(&mut self) -> Option<XNode> {
        let node = self.stack.pop()?;
        match self.stack.last_mut() {
            Some(parent) => {
                parent.children.push(node);
                None
            }
            None => Some(node),
        }
    }
}

struct Run<'a> {
    schema: Arc<Schema>,
    engine: Option<RuleEngine>,
    tx: &'a Sender<ProcMsg>,
    batch: Vec<Finding>,
    last_flush: std::time::Instant,
    stack: Vec<Frame>,
    mat: Option<Materializer>,
    /// Materialization trigger: what to collect once the stack returns to depth `usize`.
    mat_kind: MatKind,
    subject_kind: String,
    tracker: Rc<RefCell<Tracker>>,
}

#[derive(Clone, Copy, PartialEq)]
enum MatKind {
    None,
    Source,
    Title,
    Event,
}

fn run(
    snap: &PieceSnapshot,
    schema: Arc<Schema>,
    engine: RuleEngine,
    cancel: &AtomicBool,
    tx: &Sender<ProcMsg>,
) -> ProcMsg {
    let total = snap.len().max(1) as u64;
    let tracker = Rc::new(RefCell::new(Tracker::new()));
    let treader = TrackingReader {
        inner: snap.reader(),
        tracker: Rc::clone(&tracker),
        offset: 0,
    };
    let mut reader = quick_xml::Reader::from_reader(std::io::BufReader::new(treader));
    reader.config_mut().check_end_names = true;

    let mut st = Run {
        schema,
        engine: Some(engine),
        tx,
        batch: Vec::new(),
        last_flush: std::time::Instant::now(),
        stack: Vec::new(),
        mat: None,
        mat_kind: MatKind::None,
        subject_kind: String::new(),
        tracker,
    };

    let mut buf: Vec<u8> = Vec::new();
    let mut prog = crate::proc::ProgressThrottle::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            st.flush();
            return ProcMsg::Cancelled;
        }
        let pos = reader.buffer_position();
        let ev = match reader.read_event_into(&mut buf) {
            Ok(ev) => ev,
            Err(e) => {
                // not well-formed XML — emit a single fatal finding and bail out
                let line = st.tracker.borrow_mut().line_at(reader.error_position());
                st.push(Finding {
                    severity: Severity::Error,
                    line,
                    code: "XML".to_owned(),
                    message: format!("Document is not well-formed XML: {e}"),
                    source: "XML".to_owned(),
                });
                st.flush();
                return ProcMsg::Done;
            }
        };
        match ev {
            Event::Start(e) => {
                let line = st.tracker.borrow_mut().line_at(pos);
                st.on_start(&e, line, false);
            }
            Event::Empty(e) => {
                let line = st.tracker.borrow_mut().line_at(pos);
                st.on_start(&e, line, true);
            }
            Event::End(_) => st.on_end(),
            Event::Text(t) => {
                let text = t.decode().unwrap_or_default();
                st.on_text(&text, pos);
            }
            // quick-xml ≥0.38: entity references arrive separately from text
            Event::GeneralRef(r) => st.on_text(&resolve_ref(&r), pos),
            Event::CData(c) => {
                let text = String::from_utf8_lossy(c.as_ref()).into_owned();
                st.on_text(&text, pos);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
        let p = (reader.buffer_position() as f32 / total as f32) * 99.0;
        prog.maybe_send(tx, p, 99.0);
        // findings are delivered as they appear: even a partial batch is flushed every ~150 ms
        if !st.batch.is_empty() && st.last_flush.elapsed().as_millis() >= 150 {
            st.flush();
        }
    }
    // elements left unclosed at end of file (quick-xml stays silent about them on Eof)
    if let Some(top) = st.stack.last() {
        let (name, line) = (top.name.clone(), top.line);
        st.push(Finding {
            severity: Severity::Error,
            line,
            code: "XML".to_owned(),
            message: format!("Element <{name}> is not closed before end of file"),
            source: "XML".to_owned(),
        });
    }
    // final aggregate rules
    if let Some(engine) = st.engine.take() {
        let fins = engine.finalize();
        for f in fins {
            st.push(f);
        }
    }
    st.flush();
    ProcMsg::Done
}

impl Run<'_> {
    fn push(&mut self, f: Finding) {
        self.batch.push(f);
        if self.batch.len() >= FINDINGS_BATCH {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.batch.is_empty() {
            let _ = self.tx.send(ProcMsg::Findings(std::mem::take(&mut self.batch)));
        }
        self.last_flush = std::time::Instant::now();
    }

    fn on_start(&mut self, e: &quick_xml::events::BytesStart, line: usize, empty: bool) {
        let name = local_name_of(e.name().as_ref());
        // --- attributes (needed by both XSD and rules) ----------------------
        let mut attrs: Vec<(String, String)> = Vec::new();
        for a in e.attributes().flatten() {
            attrs.push((
                local_name_of(a.key.as_ref()),
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map(|v| v.into_owned())
                    .unwrap_or_default(),
            ));
        }

        // --- XSD: determine the type of the new element ---------------------
        let ty: TypeRef = if self.stack.is_empty() {
            if name != self.schema.root_name {
                self.push(Finding {
                    severity: Severity::Error,
                    line,
                    code: "XSD".to_owned(),
                    message: format!(
                        "Root element <{name}> is not allowed: expected <{}>",
                        self.schema.root_name
                    ),
                    source: "XSD".to_owned(),
                });
                TypeRef::Any
            } else {
                self.schema.root_type
            }
        } else {
            // advance the parent's content model
            let parent = self.stack.last_mut().expect("не пуст");
            match &mut parent.content {
                Content::Any => TypeRef::Any,
                Content::Simple { .. } => {
                    let pname = parent.name.clone();
                    self.push(Finding {
                        severity: Severity::Error,
                        line,
                        code: "XSD".to_owned(),
                        message: format!(
                            "Element <{name}> is not allowed inside <{pname}> (simple content)"
                        ),
                        source: "XSD".to_owned(),
                    });
                    TypeRef::Any
                }
                Content::Complex { type_id, states, .. } => {
                    let ct = &self.schema.complexes[*type_id];
                    match &ct.nfa {
                        None => {
                            let pname = parent.name.clone();
                            self.push(Finding {
                                severity: Severity::Error,
                                line,
                                code: "XSD".to_owned(),
                                message: format!(
                                    "Element <{name}> is not allowed inside <{pname}> (empty content)"
                                ),
                                source: "XSD".to_owned(),
                            });
                            TypeRef::Any
                        }
                        Some(nfa) => match nfa.advance(states, &name) {
                            Ok((next, ty)) => {
                                *states = next;
                                ty
                            }
                            Err(expected) => {
                                let pname = parent.name.clone();
                                let exp = expected_str(&expected);
                                self.push(Finding {
                                    severity: Severity::Error,
                                    line,
                                    code: "XSD".to_owned(),
                                    message: format!(
                                        "Unexpected element <{name}> inside <{pname}>{exp}"
                                    ),
                                    source: "XSD".to_owned(),
                                });
                                TypeRef::Any
                            }
                        },
                    }
                }
            }
        };

        // --- attributes by type -----------------------------------------------
        let content = match ty {
            TypeRef::Any => Content::Any,
            TypeRef::Simple(id) => {
                // attributes are not allowed on a simple type
                for (an, _) in &attrs {
                    self.push(Finding {
                        severity: Severity::Error,
                        line,
                        code: "XSD".to_owned(),
                        message: format!("Attribute \"{an}\" is not allowed on element <{name}>"),
                        source: "XSD".to_owned(),
                    });
                }
                Content::Simple { type_id: id, text: String::new(), overflow: false }
            }
            TypeRef::Complex(id) => {
                self.check_attrs(id, &name, &attrs, line);
                let states = self.schema.complexes[id]
                    .nfa
                    .as_ref()
                    .map(|n| n.start_set())
                    .unwrap_or_default();
                Content::Complex { type_id: id, states, text_reported: false }
            }
        };

        // --- rules: document / materialization triggers -----------------------
        let depth = self.stack.len();
        if depth == 0 && name == self.schema.root_name {
            if let Some(engine) = self.engine.as_mut() {
                engine.begin_document(attrs.iter().cloned().collect());
            }
        }
        if self.mat.is_none() {
            let kind = self.mat_trigger(&name, depth);
            if kind != MatKind::None {
                self.mat_kind = kind;
                self.mat = Some(Materializer { stack: Vec::new() });
            }
        }
        if let Some(m) = self.mat.as_mut() {
            let mut node = XNode::new(name.clone(), line);
            node.attrs = attrs;
            m.start(node);
            if empty {
                self.mat_end();
            }
        }

        if empty {
            // empty element: close it immediately in the XSD pass
            self.stack.push(Frame { name, content, line });
            self.close_top();
        } else {
            self.stack.push(Frame { name, content, line });
        }
    }

    /// Materialization trigger for the rule engine.
    fn mat_trigger(&self, name: &str, depth: usize) -> MatKind {
        let path = |i: usize| self.stack.get(i).map(|f| f.name.as_str()).unwrap_or("");
        // the root is taken from the XSD (root_name) rather than hardcoded to "Document" — otherwise
        // Source materialization would not fire for a non-Document schema. The inner
        // Source/Data/Title/Events are names from the model's rule structure, not tied to the root.
        if depth == 1 && name == "Source" && path(0) == self.schema.root_name {
            return MatKind::Source;
        }
        if depth == 3
            && name == "Title"
            && (path(1) == "Data" || path(1) == "Data_AF")
        {
            return MatKind::Title;
        }
        if depth == 4 && path(3) == "Events" && (path(1) == "Data" || path(1) == "Data_AF") {
            return MatKind::Event;
        }
        MatKind::None
    }

    fn on_text(&mut self, text: &str, _pos: u64) {
        if let Some(m) = self.mat.as_mut() {
            m.text(text);
        }
        let Some(top) = self.stack.last_mut() else { return };
        match &mut top.content {
            Content::Simple { text: buf, overflow, .. } => {
                if buf.len() + text.len() <= MAX_SIMPLE_TEXT {
                    buf.push_str(text);
                } else {
                    *overflow = true;
                }
            }
            Content::Complex { text_reported, .. } => {
                if !*text_reported && !text.trim().is_empty() {
                    *text_reported = true;
                    let (name, line) = (top.name.clone(), top.line);
                    self.push(Finding {
                        severity: Severity::Error,
                        line,
                        code: "XSD".to_owned(),
                        message: format!(
                            "Text inside element <{name}> is not allowed (child elements only)"
                        ),
                        source: "XSD".to_owned(),
                    });
                }
            }
            Content::Any => {}
        }
    }

    fn on_end(&mut self) {
        if self.mat.is_some() {
            self.mat_end();
        }
        self.close_top();
    }

    /// Close the materializer's current element; if the subtree is ready, hand it to the rules.
    fn mat_end(&mut self) {
        let Some(m) = self.mat.as_mut() else { return };
        let Some(root) = m.end() else { return };
        self.mat = None;
        let kind = std::mem::replace(&mut self.mat_kind, MatKind::None);
        let Some(engine) = self.engine.as_mut() else { return };
        let fins = match kind {
            MatKind::Source => {
                engine.set_source(root);
                Vec::new()
            }
            MatKind::Title => {
                // subject kind from the path: Data_AF → AF, otherwise by the Subject_* name
                let data = self.stack.get(1).map(|f| f.name.as_str()).unwrap_or("");
                let subj = self.stack.get(2).map(|f| f.name.as_str()).unwrap_or("");
                let kind = if data == "Data_AF" {
                    "AF"
                } else if subj == "Subject_FL" {
                    "FL"
                } else {
                    "UL"
                };
                self.subject_kind = kind.to_owned();
                engine.begin_subject(kind, Some(root))
            }
            MatKind::Event => engine.on_event(root),
            MatKind::None => Vec::new(),
        };
        for f in fins {
            self.push(f);
        }
    }

    /// Finalize the top element of the XSD pass: check the value / completeness of children.
    fn close_top(&mut self) {
        let Some(frame) = self.stack.pop() else { return };
        match frame.content {
            Content::Any => {}
            Content::Simple { type_id, text, overflow } => {
                if overflow {
                    return; // an overly long value is left unchecked (not our data)
                }
                if let Err(msg) = self.schema.check_simple(type_id, text.trim()) {
                    self.push(Finding {
                        severity: Severity::Error,
                        line: frame.line,
                        code: "XSD".to_owned(),
                        message: format!("Element <{}>: {msg}", frame.name),
                        source: "XSD".to_owned(),
                    });
                }
            }
            Content::Complex { type_id, states, .. } => {
                if let Some(nfa) = &self.schema.complexes[type_id].nfa {
                    if !nfa.accepting(&states) {
                        let exp = expected_str(&nfa.expected(&states));
                        self.push(Finding {
                            severity: Severity::Error,
                            line: frame.line,
                            code: "XSD".to_owned(),
                            message: format!(
                                "Element <{}> is incomplete: missing required child elements{exp}",
                                frame.name
                            ),
                            source: "XSD".to_owned(),
                        });
                    }
                }
            }
        }
    }

    /// Check the attributes of a complex-type element.
    fn check_attrs(
        &mut self,
        type_id: usize,
        elem: &str,
        attrs: &[(String, String)],
        line: usize,
    ) {
        let decls = &self.schema.complexes[type_id].attrs;
        // declared: required-ness and value
        let mut findings: Vec<Finding> = Vec::new();
        for d in decls {
            match attrs.iter().find(|(n, _)| n == &d.name) {
                None if d.required => findings.push(Finding {
                    severity: Severity::Error,
                    line,
                    code: "XSD".to_owned(),
                    message: format!(
                        "Element <{elem}> is missing required attribute \"{}\"",
                        d.name
                    ),
                    source: "XSD".to_owned(),
                }),
                Some((_, v)) => {
                    if let Some(st) = d.ty {
                        if let Err(msg) = self.schema.check_simple(st, v) {
                            findings.push(Finding {
                                severity: Severity::Error,
                                line,
                                code: "XSD".to_owned(),
                                message: format!(
                                    "Attribute \"{}\" of element <{elem}>: {msg}",
                                    d.name
                                ),
                                source: "XSD".to_owned(),
                            });
                        }
                    }
                }
                None => {}
            }
        }
        // undeclared (skip the service xmlns*/xsi* ones)
        for (n, _) in attrs {
            if !decls.iter().any(|d| &d.name == n) && !n.starts_with("xmlns") && !n.starts_with("xsi") {
                findings.push(Finding {
                    severity: Severity::Error,
                    line,
                    code: "XSD".to_owned(),
                    message: format!("Unknown attribute \"{n}\" on element <{elem}>"),
                    source: "XSD".to_owned(),
                });
            }
        }
        for f in findings {
            self.push(f);
        }
    }
}

fn expected_str(expected: &[String]) -> String {
    if expected.is_empty() {
        String::new()
    } else {
        let mut list = expected.join(", ");
        if list.chars().count() > 200 {
            list = list.chars().take(197).collect::<String>() + "…";
        }
        format!(" (expected: {list})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;
    use std::sync::mpsc::channel;

    /// XSD text of the 785-P 5.x fixture model — all 6 files concatenated into one text (test helper).
    /// `xsd::compile` ignores `xs:include` (the includes are already expanded), so concatenation works.
    fn xsd_text(version: &str) -> String {
        let files = match version {
            "5.0" => &[
                "schemas/5.0/xsd/Main.xsd",
                "schemas/5.0/xsd/BKIApiCommonTypes.xsd",
                "schemas/5.0/xsd/ReferencesTypes.xsd",
                "schemas/5.0/xsd/Blocks.xsd",
                "schemas/5.0/xsd/BlocksCur.xsd",
                "schemas/5.0/xsd/Events.xsd",
            ][..],
            "5.1" => &[
                "schemas/5.1/xsd/Main.xsd",
                "schemas/5.1/xsd/BKIApiCommonTypes.xsd",
                "schemas/5.1/xsd/ReferencesTypes.xsd",
                "schemas/5.1/xsd/Blocks.xsd",
                "schemas/5.1/xsd/BlocksCur.xsd",
                "schemas/5.1/xsd/Events.xsd",
            ][..],
            _ => unreachable!(),
        };
        let mut out = String::new();
        for f in files {
            out.push_str(&include_str_relative(f));
        }
        out
    }

    /// `include_str!` with a path computed at compile time is impossible, so we read from disk
    /// at test time (tests run from the manifest root, the path is relative).
    fn include_str_relative(rel: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn codes_text(version: &str) -> String {
        include_str_relative(&format!("schemas/{version}/codes_map.json"))
    }
    fn rules_text(version: &str) -> String {
        include_str_relative(&format!("schemas/{version}/rules.json"))
    }

    /// Run validation synchronously; return the findings.
    fn validate(text: &str, version: &str) -> Vec<Finding> {
        let mut d = Document::new_empty();
        d.replace_range((0, 0), (0, 0), text);
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_validate(
            d.snapshot(),
            xsd_text(version),
            codes_text(version),
            rules_text(version),
            version.to_owned(),
            cancel,
            tx,
        );
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(ProcMsg::Findings(b)) => out.extend(b),
                Ok(ProcMsg::Done) | Ok(ProcMsg::Cancelled) => break,
                Ok(ProcMsg::Failed(e)) => panic!("failed: {e}"),
                Ok(_) => {}
                Err(_) if std::time::Instant::now() > deadline => panic!("validate hung"),
                Err(_) => {}
            }
        }
        out
    }

    #[test]
    fn wrong_root_reported() {
        let f = validate("<NotDocument/>", "5.1");
        assert!(f.iter().any(|f| f.message.contains("Root element")), "{:?}",
            f.iter().map(|f| &f.message).collect::<Vec<_>>());
    }

    #[test]
    fn malformed_reported_with_line() {
        let f = validate("<Document>\n<broken attr=>\n", "5.1");
        let xml: Vec<_> = f.iter().filter(|f| f.code == "XML").collect();
        assert!(!xml.is_empty(), "{:?}", f.iter().map(|f| &f.message).collect::<Vec<_>>());
        assert!(xml[0].line >= 1, "line={}", xml[0].line);
    }

    #[test]
    fn unclosed_element_reported() {
        let f = validate("<Document>\n", "5.1");
        assert!(
            f.iter().any(|f| f.code == "XML" && f.message.contains("not closed")),
            "{:?}",
            f.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn incomplete_document_reported() {
        // Document without Source/Data — incomplete
        let f = validate("<Document/>", "5.1");
        assert!(
            f.iter().any(|f| f.message.contains("incomplete")),
            "{:?}",
            f.iter().map(|f| &f.message).collect::<Vec<_>>()
        );
    }

    /// Reference valid document: there should be no findings.
    #[test]
    fn fixture_valid_5_1_passes() {
        let text = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/valid_5_1.xml"),
        )
        .unwrap();
        let f = validate(&text, "5.1");
        assert!(
            f.is_empty(),
            "ожидался чистый документ, получено {} находок:\n{}",
            f.len(),
            f.iter()
                .map(|f| format!("  стр {} [{}] {}", f.line, f.code, f.message))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// Reference invalid document: errors must be found.
    #[test]
    fn fixture_invalid_5_1_reports() {
        let text = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/invalid_5_1.xml"),
        )
        .unwrap();
        let f = validate(&text, "5.1");
        assert!(
            !f.is_empty(),
            "невалидный документ прошёл без находок"
        );
        for fi in &f {
            println!("стр {} [{}] {} — {}", fi.line, fi.code, fi.message, fi.source);
        }
    }

    #[test]
    fn tracker_lines() {
        let mut t = Tracker::new();
        t.push(0, b"ab\ncd\nef");
        assert_eq!(t.line_at(0), 1);
        assert_eq!(t.line_at(2), 1); // the '\n' position is still line 1
        assert_eq!(t.line_at(3), 2);
        assert_eq!(t.line_at(7), 3);
    }
}
