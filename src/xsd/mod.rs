//! XSD-валидация: схемы (5.0 / 5.1) встроены в бинарник и компилируются
//! при первом обращении (кэш на процесс).

pub mod loader;
pub mod model;
pub mod xmltree;

pub use model::Schema;
#[cfg(test)]
pub use model::TypeRef;

use std::sync::OnceLock;

/// Исходники схемы 5.0: Main.xsd + транзитивные включения (Events → BlocksCur).
const SRC_50: &[&str] = &[
    include_str!("../../schemas/5.0/xsd/Main.xsd"),
    include_str!("../../schemas/5.0/xsd/BKIApiCommonTypes.xsd"),
    include_str!("../../schemas/5.0/xsd/ReferencesTypes.xsd"),
    include_str!("../../schemas/5.0/xsd/Blocks.xsd"),
    include_str!("../../schemas/5.0/xsd/BlocksCur.xsd"),
    include_str!("../../schemas/5.0/xsd/Events.xsd"),
];

/// Исходники схемы 5.1: Main.xsd + транзитивные включения (Events → BlocksCur).
const SRC_51: &[&str] = &[
    include_str!("../../schemas/5.1/xsd/Main.xsd"),
    include_str!("../../schemas/5.1/xsd/BKIApiCommonTypes.xsd"),
    include_str!("../../schemas/5.1/xsd/ReferencesTypes.xsd"),
    include_str!("../../schemas/5.1/xsd/Blocks.xsd"),
    include_str!("../../schemas/5.1/xsd/BlocksCur.xsd"),
    include_str!("../../schemas/5.1/xsd/Events.xsd"),
];

static SCHEMA_50: OnceLock<Result<Schema, String>> = OnceLock::new();
static SCHEMA_51: OnceLock<Result<Schema, String>> = OnceLock::new();

/// Скомпилированная схема по версии («5.0» / «5.1»). Первый вызов компилирует (~сотни мс),
/// дальше — кэш. Вызывать из фонового потока валидации.
pub fn schema(version: &str) -> Result<&'static Schema, String> {
    let (slot, sources) = match version {
        "5.0" => (&SCHEMA_50, SRC_50),
        "5.1" => (&SCHEMA_51, SRC_51),
        other => return Err(format!("неизвестная версия схемы «{other}»")),
    };
    slot.get_or_init(|| {
        loader::compile(version, sources).map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| e.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_compile() {
        for v in ["5.0", "5.1"] {
            let s = schema(v).unwrap_or_else(|e| panic!("schema {v}: {e}"));
            assert_eq!(s.root_name, "Document");
            assert!(matches!(s.root_type, TypeRef::Complex(_)));
            assert!(s.complexes.len() > 500, "{v}: complexes={}", s.complexes.len());
            assert!(s.simples.len() > 300, "{v}: simples={}", s.simples.len());
        }
    }

    #[test]
    fn document_content_model_starts_with_source() {
        let s = schema("5.1").unwrap();
        let TypeRef::Complex(id) = s.root_type else { panic!() };
        let nfa = s.complexes[id].nfa.as_ref().unwrap();
        let s0 = nfa.start_set();
        let exp = nfa.expected(&s0);
        assert_eq!(exp, vec!["Source".to_owned()]);
    }
}
