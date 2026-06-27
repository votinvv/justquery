//! XSD validation: compiling the XSD text from an XML model (`.jqmodel`) into a [`Schema`] with a per-process cache.
//!
//! The XSD arrives as text from the model's section; compilation is cached per process by the SHA-256
//! of the normalized XSD (identical content → the same compiled schema).

pub mod loader;
pub mod model;
pub mod xmltree;

pub use model::Schema;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Cache of compiled schemas: SHA-256(XSD text) → Arc<Schema>. The first compilation takes ~hundreds of ms,
/// afterwards it is reused (cloning an Arc is cheap, cloning a Schema is expensive). The lock is short
/// (only lookup/insert), compilation happens outside the lock.
static SCHEMA_CACHE: Mutex<Option<HashMap<String, Arc<Schema>>>> = Mutex::new(None);

/// Hard cap on cached compiled schemas. Only a handful of models are ever active at once; the cap
/// just keeps a long session that compiles many distinct XSDs (e.g. repeatedly edited model XSDs,
/// or many models scanned on file open) from retaining every compiled `Schema` for the whole
/// process — each is an expensive `Vec<SimpleType>` + `Vec<ComplexType>`. On overflow the whole map
/// is dropped; the live models simply recompile lazily on next use (see also [`clear_schema_cache`],
/// invoked when the model registry is reloaded so schemas for deleted models don't linger).
const SCHEMA_CACHE_CAP: usize = 24;

/// Drop every cached compiled schema. Called from the model-registry reload so schemas for models
/// that were edited or deleted don't stay resident for the process lifetime.
pub fn clear_schema_cache() {
    if let Ok(mut guard) = SCHEMA_CACHE.lock() {
        *guard = None;
    }
}

/// Compile the model's XSD text into a schema. Cached by the SHA-256 of the normalized text, so
/// repeated validations of one model do not rebuild the schema. Can be called from any thread.
///
/// `label` is the model's caption (id/name) for diagnostic messages on compilation errors.
///
/// The text may contain several `<xs:schema>…</xs:schema>` documents in a row (for example, Main +
/// includes glued into a single XSD block of the model). They are split by their root elements and fed
/// to `loader::compile` as separate "files".
pub fn compile(xsd_text: &str, label: &str) -> Result<Arc<Schema>, String> {
    let key = xsd_hash(xsd_text);
    // 1. quickly check the cache under the lock
    {
        let guard = SCHEMA_CACHE.lock().ok();
        if let Some(map) = guard.as_ref().and_then(|opt| opt.as_ref()) {
            if let Some(s) = map.get(&key) {
                return Ok(Arc::clone(s));
            }
        }
    }
    // 2. compilation outside the lock (heavy)
    let docs = split_schema_docs(xsd_text);
    let sources: Vec<&str> = docs.iter().map(String::as_str).collect();
    let schema = Arc::new(
        loader::compile(label, &sources)
            .map_err(|e| e.to_string())?,
    );
    // 3. put into the cache (bounded — drop everything on overflow rather than grow forever)
    if let Ok(mut guard) = SCHEMA_CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() >= SCHEMA_CACHE_CAP && !map.contains_key(&key) {
            map.clear();
        }
        map.entry(key).or_insert_with(|| Arc::clone(&schema));
    }
    Ok(schema)
}

/// Split the XSD text into separate schema documents. Returns slices from each `<…:schema>` to the
/// matching `</…:schema>`. If there is only one document, a single-element vector with the whole text is returned.
///
/// Needed for models where Main + include files are glued into a single XSD block: `loader::compile` expects
/// each schema as a separate array element (the parser only takes the first root).
///
/// Parsing is done by `quick_xml` (more reliable than hand-written regexes on XML); depth is counted by the local
/// name `schema`, the namespace prefix is ignored.
fn split_schema_docs(text: &str) -> Vec<String> {
    use quick_xml::events::Event;
    let local_name_of = crate::xsd::xmltree::local_name_of;
    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    // range [start..end] of the current root schema element (byte positions in the source text)
    let mut start: Option<usize> = None;
    let mut depth: i32 = 0;
    loop {
        let pos = reader.buffer_position() as usize;
        let ev = match reader.read_event_into(&mut buf) {
            Ok(ev) => ev,
            Err(_) => break,
        };
        match ev {
            Event::Start(e) => {
                let local = local_name_of(e.name().as_ref());
                if local == "schema" {
                    if depth == 0 {
                        start = Some(pos);
                    }
                    depth += 1;
                }
            }
            Event::End(e) => {
                let local = local_name_of(e.name().as_ref());
                if local == "schema" {
                    depth -= 1;
                    if depth == 0 {
                        let end = reader.buffer_position() as usize;
                        if let Some(s) = start.take() {
                            let mut frag = text[s..end].to_owned();
                            frag.insert_str(0, xml_decl_for(text));
                            out.push(frag);
                        }
                    }
                }
            }
            Event::Empty(e) => {
                let local = local_name_of(e.name().as_ref());
                if local == "schema" && depth == 0 {
                    let end = reader.buffer_position() as usize;
                    let mut frag = text[pos..end].to_owned();
                    frag.insert_str(0, xml_decl_for(text));
                    out.push(frag);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if out.is_empty() {
        // no schema at all — return as is, the loader will report the error
        vec![text.to_owned()]
    } else {
        out
    }
}

/// The XML declaration prefix pulled from the start of `text` (if present), so that each split
/// document stays a self-contained valid XML.
fn xml_decl_for(text: &str) -> &'static str {
    if text.trim_start().starts_with("<?xml") {
        "<?xml version=\"1.0\"?>\n"
    } else {
        ""
    }
}

/// SHA-256 of the normalized XSD text (UTF-8 without BOM, `\n` line breaks) → hex string.
fn xsd_hash(xsd: &str) -> String {
    use sha2::{Digest, Sha256};
    let norm = xsd.strip_prefix('\u{feff}').unwrap_or(xsd).replace("\r\n", "\n").replace('\r', "\n");
    let mut h = Sha256::new();
    h.update(norm.as_bytes());
    let d = h.finalize();
    crate::crypt::to_hex(&d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid XSD with a single complex root type.
    fn tiny_xsd() -> &'static str {
        r#"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="Document" type="Doc"/>
  <xs:complexType name="Doc">
    <xs:sequence>
      <xs:element name="Source" type="xs:string"/>
    </xs:sequence>
  </xs:complexType>
</xs:schema>"#
    }

    #[test]
    fn compile_caches_by_content() {
        let s1 = compile(tiny_xsd(), "test").expect("compile 1");
        assert_eq!(s1.root_name, "Document");
        // a second call with the same text — should hit the cache (Arc::ptr_eq = the same schema).
        let s2 = compile(tiny_xsd(), "test").expect("compile 2");
        assert!(Arc::ptr_eq(&s1, &s2), "recompiling the same XSD should hit the cache");
    }

    #[test]
    fn compile_rejects_non_schema() {
        let res = compile("<not-a-schema/>", "bad");
        assert!(res.is_err(), "expected a compilation error for non-XSD");
    }
}

