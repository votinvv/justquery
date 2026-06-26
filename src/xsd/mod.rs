//! XSD-валидация: компиляция XSD-текста из XML-модели (`.jqmodel`) в [`Schema`] с кэшем на процесс.
//!
//! XSD приходит текстом из секции модели; компиляция кэшируется на процесс по SHA-256
//! нормализованного XSD (одинаковое содержимое → одна и та же скомпилированная схема).

pub mod loader;
pub mod model;
pub mod xmltree;

pub use model::Schema;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Кэш скомпилированных схем: SHA-256(XSD-текста) → Arc<Schema>. Первая компиляция ~сотни мс,
/// дальше — переиспользование (Arc — клон дёшев, Schema клонировать дорого). Блокировка короткая
/// (только поиск/вставка), компиляция — вне замка.
static SCHEMA_CACHE: Mutex<Option<HashMap<String, Arc<Schema>>>> = Mutex::new(None);

/// Скомпилировать XSD-текст модели в схему. Кэшируется по SHA-256 нормализованного текста, поэтому
/// повторные валидации одной модели не пересобирают схему. Вызывать можно из любого потока.
///
/// `label` — подпись модели (id/имя) для диагностических сообщений при ошибках компиляции.
///
/// Текст может содержать несколько `<xs:schema>…</xs:schema>` документов подряд (например, Main +
/// includes, склеенные в один XSD-блок модели). Они нарезаются по корневым элементам и подаются
/// `loader::compile` как отдельные «файлы».
pub fn compile(xsd_text: &str, label: &str) -> Result<Arc<Schema>, String> {
    let key = xsd_hash(xsd_text);
    // 1. быстро проверить кэш под замком
    {
        let guard = SCHEMA_CACHE.lock().ok();
        if let Some(map) = guard.as_ref().and_then(|opt| opt.as_ref()) {
            if let Some(s) = map.get(&key) {
                return Ok(Arc::clone(s));
            }
        }
    }
    // 2. компиляция вне замка (тяжёлая)
    let docs = split_schema_docs(xsd_text);
    let sources: Vec<&str> = docs.iter().map(String::as_str).collect();
    let schema = Arc::new(
        loader::compile(label, &sources)
            .map_err(|e| e.to_string())?,
    );
    // 3. положить в кэш
    if let Ok(mut guard) = SCHEMA_CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        map.entry(key).or_insert_with(|| Arc::clone(&schema));
    }
    Ok(schema)
}

/// Нарезать XSD-текст на отдельные schema-документы. Возвращает срезы от каждого `<…:schema>` до
/// парного `</…:schema>`. Если документ один — возвращается одноэлементный вектор с целым текстом.
///
/// Нужно для моделей, где Main + include-файлы склеены в один XSD-блок: `loader::compile` ожидает
/// каждый schema отдельным элементом массива (парсер берёт только первый корень).
///
/// Разбор ведёт `quick_xml` (надёжнее ручных регулярок на XML); глубина считается по локальному
/// имени `schema`, префикс пространства имён игнорируется.
fn split_schema_docs(text: &str) -> Vec<String> {
    use quick_xml::events::Event;
    let local_name_of = crate::xsd::xmltree::local_name_of;
    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    // диапазон [start..end] текущего корневого schema-элемента (байтовые позиции в исходном text)
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
        // ни одного schema — отдаём как есть, loader сообщит об ошибке
        vec![text.to_owned()]
    } else {
        out
    }
}

/// Префикс XML-декларации, вытащенный из начала `text` (если есть), чтобы каждый нарезанный
/// документ оставался самодостаточным валидным XML.
fn xml_decl_for(text: &str) -> &'static str {
    if text.trim_start().starts_with("<?xml") {
        "<?xml version=\"1.0\"?>\n"
    } else {
        ""
    }
}

/// SHA-256 от нормализованного XSD-текста (UTF-8 без BOM, `\n`-переносы) → hex-строка.
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

    /// Минимальный валидный XSD с одним комплексным корневым типом.
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
        // второй вызов с тем же текстом — должен попасть в кэш (Arc::ptr_eq = та же схема).
        let s2 = compile(tiny_xsd(), "test").expect("compile 2");
        assert!(Arc::ptr_eq(&s1, &s2), "повторная компиляция того же XSD должна брать кэш");
    }

    #[test]
    fn compile_rejects_non_schema() {
        let res = compile("<not-a-schema/>", "bad");
        assert!(res.is_err(), "ожидалась ошибка компиляции не-XSD");
    }
}

