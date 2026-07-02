//! Mini-DOM: used to parse XSD schemas on load and to materialize event
//! subtrees in the rule engine. Not for gigabyte-sized documents — only for
//! schemas (~MB) and individual event blocks.

use quick_xml::events::Event;

#[derive(Debug, Clone)]
pub struct XNode {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XNode>,
    /// Concatenated direct text of the element (excluding children's text).
    pub text: String,
    /// 1-based source line (0 — unknown).
    pub line: usize,
}

impl XNode {
    pub fn new(name: String, line: usize) -> Self {
        Self { name, attrs: Vec::new(), children: Vec::new(), text: String::new(), line }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Depth-first walk: the node itself and all descendants.
    pub fn iter(&self) -> Iter<'_> {
        Iter { stack: vec![self] }
    }

    /// First child with the given name.
    pub fn child(&self, name: &str) -> Option<&XNode> {
        self.children.iter().find(|c| c.name == name)
    }
}

pub struct Iter<'a> {
    stack: Vec<&'a XNode>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a XNode;
    fn next(&mut self) -> Option<&'a XNode> {
        let n = self.stack.pop()?;
        // order doesn't matter for the rules; push in reverse for a forward walk
        for c in n.children.iter().rev() {
            self.stack.push(c);
        }
        Some(n)
    }
}

/// Parse an XML string into a tree. `skip` — local names of elements whose subtrees
/// are skipped entirely (xs:annotation in schemas — 4800 junk nodes).
pub fn parse_str(xml: &str, skip: &[&str]) -> Result<XNode, String> {
    let xml = xml.trim_start_matches('\u{feff}');
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<XNode> = Vec::new();
    let mut root: Option<XNode> = None;
    let mut skip_depth = 0usize;
    // an element node (name + attributes) from a Start/Empty event — the shared part of both arms
    let node_of = |e: &quick_xml::events::BytesStart, name: String| -> Result<XNode, String> {
        let mut node = XNode::new(name, 0);
        for a in e.attributes().flatten() {
            node.attrs.push((
                local_name_of(a.key.as_ref()),
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|e| e.to_string())?
                    .into_owned(),
            ));
        }
        Ok(node)
    };
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) => {
                let name = local_name_of(e.name().as_ref());
                if skip_depth > 0 || skip.contains(&name.as_str()) {
                    skip_depth += 1;
                    continue;
                }
                stack.push(node_of(&e, name)?);
            }
            Event::Empty(e) => {
                if skip_depth > 0 {
                    continue;
                }
                let name = local_name_of(e.name().as_ref());
                if skip.contains(&name.as_str()) {
                    continue;
                }
                let node = node_of(&e, name)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Event::End(_) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                let node = stack.pop().ok_or("unbalanced XML")?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Event::Text(t) => {
                if skip_depth == 0 {
                    if let Some(top) = stack.last_mut() {
                        top.text.push_str(&t.decode().map_err(|e| e.to_string())?);
                    }
                }
            }
            // quick-xml ≥0.38: entity references arrive separately from text
            Event::GeneralRef(r) => {
                if skip_depth == 0 {
                    if let Some(top) = stack.last_mut() {
                        top.text.push_str(&crate::validate::resolve_ref(&r));
                    }
                }
            }
            Event::CData(c) => {
                if skip_depth == 0 {
                    if let Some(top) = stack.last_mut() {
                        top.text.push_str(&String::from_utf8_lossy(c.as_ref()));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    root.ok_or_else(|| "empty document".to_owned())
}

/// Local name without the prefix (xs:element → element).
pub fn local_name_of(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    match s.rsplit_once(':') {
        Some((_, l)) => l.to_owned(),
        None => s.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_with_attrs() {
        let t = parse_str(r#"<a x="1"><b>text</b><c/></a>"#, &[]).unwrap();
        assert_eq!(t.name, "a");
        assert_eq!(t.attr("x"), Some("1"));
        assert_eq!(t.children.len(), 2);
        assert_eq!(t.child("b").unwrap().text, "text");
    }

    #[test]
    fn skips_subtrees() {
        let t = parse_str(
            "<xs:schema xmlns:xs=\"x\"><xs:annotation><xs:documentation>doc</xs:documentation></xs:annotation><xs:element name=\"e\"/></xs:schema>",
            &["annotation"],
        )
        .unwrap();
        assert_eq!(t.name, "schema");
        assert_eq!(t.children.len(), 1);
        assert_eq!(t.children[0].name, "element");
    }

    #[test]
    fn iter_walks_descendants() {
        let t = parse_str("<a><b><c/></b><d/></a>", &[]).unwrap();
        let names: Vec<&str> = t.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c", "d"]);
    }
}
