//! Compilation of XSD files into a [`Schema`].
//!
//! Two passes: 1) collect named simpleType/complexType/element across all includes;
//! 2) compile the types (simple ones on demand with memoization; complex ones as a skeleton
//! keyed by name, then their bodies, and finally the NFA of the content models).

use super::model::*;
use super::xmltree::{parse_str, XNode};
use std::collections::HashMap;

pub struct LoadError(pub String);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type R<T> = Result<T, LoadError>;

fn err<T>(msg: impl Into<String>) -> R<T> {
    Err(LoadError(msg.into()))
}

/// Compile a schema from sources: `sources` is the content of the XSD files,
/// starting with Main.xsd; includes are already expanded by the caller (by file name).
pub fn compile(version: &str, sources: &[&str]) -> R<Schema> {
    // 1. parse all files, collect the named top-level nodes
    let mut named_simple: HashMap<String, XNode> = HashMap::new();
    let mut named_complex: HashMap<String, XNode> = HashMap::new();
    let mut top_elements: Vec<XNode> = Vec::new();
    for src in sources {
        let tree = parse_str(src, &["annotation"]).map_err(LoadError)?;
        if tree.name != "schema" {
            return err("ожидался корень xs:schema");
        }
        for child in tree.children {
            match child.name.as_str() {
                "simpleType" => {
                    let name = child.attr("name").unwrap_or_default().to_owned();
                    if name.is_empty() {
                        return err("безымянный топ-уровневый simpleType");
                    }
                    named_simple.insert(name, child);
                }
                "complexType" => {
                    let name = child.attr("name").unwrap_or_default().to_owned();
                    if name.is_empty() {
                        return err("безымянный топ-уровневый complexType");
                    }
                    named_complex.insert(name, child);
                }
                "element" => top_elements.push(child),
                "include" => {} // includes are expanded by the caller
                other => return err(format!("неподдерживаемая конструкция xs:{other}")),
            }
        }
    }

    let mut c = Compiler {
        named_simple,
        named_complex,
        simple_ids: HashMap::new(),
        complex_ids: HashMap::new(),
        simples: Vec::new(),
        complexes: Vec::new(),
    };

    // 2a. skeleton of the named complex types (for cyclic references)
    let complex_names: Vec<String> = c.named_complex.keys().cloned().collect();
    for name in &complex_names {
        let id = c.complexes.len();
        c.complexes.push(ComplexType {
            name: name.clone(),
            particle: None,
            nfa: None,
            attrs: Vec::new(),
        });
        c.complex_ids.insert(name.clone(), id);
    }
    // 2b. bodies of the named complex types
    for name in &complex_names {
        let node = c.named_complex.get(name).cloned().expect("собран выше");
        let id = c.complex_ids[name];
        let (particle, attrs) = c.compile_complex_body(&node)?;
        c.complexes[id].particle = particle;
        c.complexes[id].attrs = attrs;
    }

    // 3. root element: the first global <xs:element> (the name comes from the schema, we do NOT
    //    hardcode «Document») — so we can load arbitrary XSDs. For document schemas there is
    //    usually a single global element, and it is the root.
    let root = top_elements
        .first()
        .ok_or_else(|| LoadError("в схеме нет ни одного глобального xs:element".into()))?
        .clone();
    let root_name = root.attr("name").unwrap_or_default().to_owned();
    if root_name.is_empty() {
        return err("у корневого xs:element нет атрибута name");
    }
    let root_type = c.elem_type(&root)?;

    // 4. NFA of the content models
    for ct in &mut c.complexes {
        if let Some(p) = &ct.particle {
            ct.nfa = Some(Nfa::build(p));
        }
    }

    Ok(Schema {
        version: version.to_owned(),
        simples: c.simples,
        complexes: c.complexes,
        root_name,
        root_type,
    })
}

struct Compiler {
    named_simple: HashMap<String, XNode>,
    named_complex: HashMap<String, XNode>,
    simple_ids: HashMap<String, usize>,
    complex_ids: HashMap<String, usize>,
    simples: Vec<SimpleType>,
    complexes: Vec<ComplexType>,
}

impl Compiler {
    /// Type referenced by `type="..."`: builtin, named simple, or complex.
    fn type_by_name(&mut self, name: &str) -> R<TypeRef> {
        if let Some(b) = name.strip_prefix("xs:").and_then(Builtin::from_name) {
            return Ok(TypeRef::Simple(self.builtin_simple(b)));
        }
        if self.named_complex.contains_key(name) {
            return Ok(TypeRef::Complex(self.complex_ids[name]));
        }
        if self.named_simple.contains_key(name) {
            return Ok(TypeRef::Simple(self.simple_by_name(name)?));
        }
        err(format!("неизвестный тип «{name}»"))
    }

    /// Simple wrapper type over a builtin (without facets), cached by name.
    fn builtin_simple(&mut self, b: Builtin) -> usize {
        let key = format!("xs__{b:?}");
        if let Some(&id) = self.simple_ids.get(&key) {
            return id;
        }
        let id = self.simples.len();
        self.simples.push(SimpleType {
            name: key.clone(),
            variety: Variety::Atomic { base: b, steps: Vec::new() },
        });
        self.simple_ids.insert(key, id);
        id
    }

    /// Named simple type (memoized).
    fn simple_by_name(&mut self, name: &str) -> R<usize> {
        if let Some(&id) = self.simple_ids.get(name) {
            return Ok(id);
        }
        let node = self
            .named_simple
            .get(name)
            .cloned()
            .ok_or_else(|| LoadError(format!("неизвестный простой тип «{name}»")))?;
        let id = self.compile_simple(&node, name)?;
        self.simple_ids.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Compile an xs:simpleType node (restriction chain or union).
    fn compile_simple(&mut self, node: &XNode, name: &str) -> R<usize> {
        if let Some(u) = node.child("union") {
            let mt = u.attr("memberTypes").unwrap_or_default();
            let mut members = Vec::new();
            for m in mt.split_whitespace() {
                let id = match self.type_by_name(m)? {
                    TypeRef::Simple(id) => id,
                    _ => return err(format!("member union «{m}» не простой тип")),
                };
                members.push(id);
            }
            // inline union members (<xs:simpleType> inside) — do not occur in our schemas
            if members.is_empty() {
                return err(format!("пустой union в типе «{name}»"));
            }
            let id = self.simples.len();
            self.simples.push(SimpleType {
                name: name.to_owned(),
                variety: Variety::Union { members },
            });
            return Ok(id);
        }
        let Some(r) = node.child("restriction") else {
            return err(format!("simpleType «{name}» без restriction/union"));
        };
        let base_name = r.attr("base").unwrap_or_default().to_owned();
        // base: builtin or named simple — assemble the chain of steps
        let (base, mut steps) = if let Some(b) =
            base_name.strip_prefix("xs:").and_then(Builtin::from_name)
        {
            (b, Vec::new())
        } else {
            let base_id = self.simple_by_name(&base_name)?;
            match &self.simples[base_id].variety {
                Variety::Atomic { base, steps } => {
                    (*base, steps.to_vec())
                }
                Variety::Union { .. } => {
                    return err(format!("restriction от union («{base_name}») не поддержан"))
                }
            }
        };
        let f = parse_facets(r)?;
        if !f.is_empty() {
            steps.push(f);
        }
        let id = self.simples.len();
        self.simples.push(SimpleType {
            name: name.to_owned(),
            variety: Variety::Atomic { base, steps },
        });
        Ok(id)
    }

    /// Element type: type attribute / inline simpleType / inline complexType / none (anyType).
    fn elem_type(&mut self, el: &XNode) -> R<TypeRef> {
        if let Some(t) = el.attr("type") {
            return self.type_by_name(t);
        }
        if let Some(st) = el.child("simpleType") {
            let id = self.compile_simple(st, el.attr("name").unwrap_or(""))?;
            return Ok(TypeRef::Simple(id));
        }
        if let Some(ct) = el.child("complexType") {
            let (particle, attrs) = self.compile_complex_body(ct)?;
            let id = self.complexes.len();
            self.complexes.push(ComplexType {
                name: String::new(),
                particle,
                nfa: None,
                attrs,
            });
            return Ok(TypeRef::Complex(id));
        }
        Ok(TypeRef::Any)
    }

    /// Body of a complexType: (content particle, attributes).
    /// Supported: sequence|choice + attribute*, complexContent/extension.
    fn compile_complex_body(&mut self, ct: &XNode) -> R<(Option<Particle>, Vec<AttrDecl>)> {
        if let Some(cc) = ct.child("complexContent") {
            let Some(ext) = cc.child("extension") else {
                return err("complexContent без extension не поддержан");
            };
            let base_name = ext.attr("base").unwrap_or_default().to_owned();
            let Some(&base_id) = self.complex_ids.get(&base_name) else {
                return err(format!("база расширения «{base_name}» не комплексный тип"));
            };
            // the base may not be compiled yet (phase 2b walks the list in order) — compile it now
            if self.complexes[base_id].particle.is_none()
                && self.complexes[base_id].attrs.is_empty()
            {
                let node = self
                    .named_complex
                    .get(&base_name)
                    .cloned()
                    .expect("каркас есть — узел есть");
                let (p, a) = self.compile_complex_body(&node)?;
                self.complexes[base_id].particle = p;
                self.complexes[base_id].attrs = a;
            }
            let base_particle = self.complexes[base_id].particle.clone();
            let base_attrs: Vec<AttrDecl> = self.complexes[base_id].attrs.to_vec();
            let (own_particle, own_attrs) = self.compile_particles_and_attrs(ext)?;
            let particle = match (base_particle, own_particle) {
                (Some(b), Some(o)) => {
                    Some(Particle::Seq { items: vec![b, o], q: Quant::One })
                }
                (Some(b), None) => Some(b),
                (None, o) => o,
            };
            let mut attrs = base_attrs;
            attrs.extend(own_attrs);
            return Ok((particle, attrs));
        }
        self.compile_particles_and_attrs(ct)
    }

    /// Direct children of a container (complexType or extension): sequence|choice + attribute*.
    fn compile_particles_and_attrs(
        &mut self,
        node: &XNode,
    ) -> R<(Option<Particle>, Vec<AttrDecl>)> {
        let mut particle = None;
        let mut attrs = Vec::new();
        for child in &node.children {
            match child.name.as_str() {
                "sequence" | "choice" => {
                    if particle.is_some() {
                        return err("двойная контентная модель в complexType");
                    }
                    particle = Some(self.compile_particle(child)?);
                }
                "attribute" => attrs.push(self.compile_attr(child)?),
                "complexContent" => return err("вложенный complexContent"),
                other => return err(format!("неподдержанный узел в complexType: xs:{other}")),
            }
        }
        Ok((particle, attrs))
    }

    fn compile_particle(&mut self, node: &XNode) -> R<Particle> {
        let q = quant(node)?;
        match node.name.as_str() {
            "sequence" => {
                let mut items = Vec::new();
                for ch in &node.children {
                    items.push(self.compile_particle(ch)?);
                }
                Ok(Particle::Seq { items, q })
            }
            "choice" => {
                let mut items = Vec::new();
                for ch in &node.children {
                    items.push(self.compile_particle(ch)?);
                }
                Ok(Particle::Choice { items, q })
            }
            "element" => {
                let name = node
                    .attr("name")
                    .ok_or_else(|| LoadError("локальный элемент без имени (ref?)".into()))?
                    .to_owned();
                let ty = self.elem_type(node)?;
                Ok(Particle::Elem { name, ty, q })
            }
            other => err(format!("неподдержанная частица xs:{other}")),
        }
    }

    fn compile_attr(&mut self, node: &XNode) -> R<AttrDecl> {
        let name = node
            .attr("name")
            .ok_or_else(|| LoadError("атрибут без имени".into()))?
            .to_owned();
        let required = node.attr("use") == Some("required");
        let ty = if let Some(t) = node.attr("type") {
            match self.type_by_name(t)? {
                TypeRef::Simple(id) => Some(id),
                _ => return err(format!("тип атрибута «{name}» не простой")),
            }
        } else if let Some(st) = node.child("simpleType") {
            Some(self.compile_simple(st, &format!("@{name}"))?)
        } else {
            None
        };
        Ok(AttrDecl { name, required, ty })
    }
}

fn quant(node: &XNode) -> R<Quant> {
    let min = node.attr("minOccurs").unwrap_or("1");
    let max = node.attr("maxOccurs").unwrap_or("1");
    Ok(match (min, max) {
        ("1", "1") => Quant::One,
        ("0", "1") => Quant::Opt,
        ("0", "unbounded") => Quant::Star,
        ("1", "unbounded") => Quant::Plus,
        _ => {
            return err(format!(
                "неподдержанные occurs: minOccurs={min} maxOccurs={max}"
            ))
        }
    })
}

fn parse_facets(r: &XNode) -> R<Facets> {
    let mut f = Facets::default();
    for ch in &r.children {
        let val = ch.attr("value").unwrap_or_default();
        match ch.name.as_str() {
            "enumeration" => f.enums.get_or_insert_with(Vec::new).push(val.to_owned()),
            "pattern" => {
                let rx = regex::Regex::new(&format!(r"\A(?:{val})\z"))
                    .map_err(|e| LoadError(format!("не скомпилировался шаблон «{val}»: {e}")))?;
                f.patterns.push(rx);
            }
            "length" => f.length = val.parse().ok(),
            "minLength" => f.min_length = val.parse().ok(),
            "maxLength" => f.max_length = val.parse().ok(),
            "minInclusive" => f.min_inclusive = val.parse().ok(),
            "maxInclusive" => f.max_inclusive = val.parse().ok(),
            "totalDigits" => f.total_digits = val.parse().ok(),
            "whiteSpace" | "fractionDigits" => {} // does not affect our checks
            other => return err(format!("неподдержанный фасет xs:{other}")),
        }
    }
    Ok(f)
}
