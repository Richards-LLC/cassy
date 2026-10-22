#[cfg(test)]
mod cases {
    use std::path::Path;

    use crate::parser::LanguageParser;
    use crate::parser::rust::RustParser;
    use crate::types::SymbolKind;

    #[test]
    fn test_explicit_external_modules() {
        let mut parser = RustParser::new().unwrap();
        let source = r###"
// #[path = "comment_tests.rs"] mod tests;
const EXAMPLE: &str = r#"#[path = "example_tests.rs"] mod tests;"#;
#[cfg(test)]
#[path = "gear_tests.rs"]
pub(crate) mod tests;
#[path = r#"raw_tests.rs"#] mod raw;
#[path = "inline_tests.rs"] mod inline {}
mod ordinary;
#[path = "one.rs"] #[path = "two.rs"] mod ambiguous;
"###;
        assert_eq!(
            parser.explicit_external_modules(source).unwrap(),
            [
                ("tests".into(), "gear_tests.rs".into()),
                ("raw".into(), "raw_tests.rs".into()),
            ]
        );
        assert!(
            parser
                .explicit_external_modules("#[path = \"gear_tests.rs\"] mod")
                .is_err()
        );
    }

    #[test]
    fn test_parse_function() {
        let mut parser = RustParser::new().expect("parser init");
        let source = r#"
/// This is a doc comment
fn hello_world(name: &str) -> String {
    format!("Hello, {}!", name)
}
"#;
        let result = parser
            .parse_file(Path::new("test.rs"), source, "test-repo")
            .expect("parse file");

        assert_eq!(result.symbols.len(), 1);
        let sym = &result.symbols[0];
        assert_eq!(sym.name, "hello_world");
        assert_eq!(sym.kind, SymbolKind::Function);
        assert!(
            sym.signature
                .as_ref()
                .is_some_and(|s| s.contains("fn hello_world"))
        );
        assert!(sym.documentation.is_some());
    }

    #[test]
    fn test_parse_struct() {
        let mut parser = RustParser::new().expect("parser init");
        let source = r#"
/// A person struct
pub struct Person {
    name: String,
    age: u32,
}
"#;
        let result = parser
            .parse_file(Path::new("test.rs"), source, "test-repo")
            .expect("parse file");

        assert_eq!(result.symbols.len(), 1);
        let sym = &result.symbols[0];
        assert_eq!(sym.name, "Person");
        assert_eq!(sym.kind, SymbolKind::Struct);
    }

    #[test]
    fn test_parse_impl_with_methods() {
        let mut parser = RustParser::new().expect("parser init");
        let source = r#"
impl Person {
    fn new(name: String) -> Self {
        Self { name, age: 0 }
    }

    fn greet(&self) -> String {
        format!("Hello, {}", self.name)
    }
}
"#;
        let result = parser
            .parse_file(Path::new("test.rs"), source, "test-repo")
            .expect("parse file");

        assert_eq!(result.symbols.len(), 3);
        assert!(result.symbols.iter().any(|s| s.kind == SymbolKind::Impl));
        let methods: Vec<_> = result
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Method)
            .collect();
        assert_eq!(methods.len(), 2);
    }

    #[test]
    fn test_extract_imports() {
        let mut parser = RustParser::new().expect("parser init");
        let source = r#"
use std::collections::HashMap;
use crate::types::CodeSymbol;
pub use super::parser::RustParser;
"#;
        let imports = parser.extract_imports(source).expect("extract imports");

        assert_eq!(imports.len(), 3);
        assert!(imports[0].module_path.contains("std::collections"));
        assert!(imports[2].is_reexport);
    }
}
