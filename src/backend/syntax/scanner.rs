use crate::analysis::{OxideSymbolKind, Symbol};
use lazy_static::lazy_static;
use regex::Regex;
use tower_lsp::lsp_types::{Position, Range};

lazy_static! {
    // 1. Structural Regexes
    // (?im) -> Case Insensitive (i) AND Multi-line mode (m).
    // The 'm' flag is critical: it allows '^' to match the start of any line,
    // not just the absolute start of the file.

    // Matches "entity Name is" (Requires 'is' to avoid matching 'end entity Name')
    static ref RE_ENTITY: Regex = Regex::new(r"(?im)^\s*entity\s+(\w+)\s+is").unwrap();

    // Matches "package Name is"
    static ref RE_PACKAGE: Regex = Regex::new(r"(?im)^\s*package\s+(\w+)\s+is").unwrap();

    // Matches "architecture Name of Entity is"
    // Capture group 1: Arch Name, Capture group 2: Entity Name
    static ref RE_ARCH: Regex = Regex::new(r"(?im)^\s*architecture\s+(\w+)\s+of\s+(\w+)\s+is").unwrap();

    // Matches "component Name is" (Needed for instantiation lookups)
    static ref RE_COMPONENT: Regex = Regex::new(r"(?im)^\s*component\s+(\w+)").unwrap();

    // 2. Types & Constants (Critical for Global Linking)
    // Matches "type Name is" or "subtype Name is"
    static ref RE_TYPE: Regex = Regex::new(r"(?im)^\s*(?:type|subtype)\s+(\w+)").unwrap();

    // Matches "constant Name : Type"
    static ref RE_CONSTANT: Regex = Regex::new(r"(?im)^\s*constant\s+(\w+)").unwrap();

    // Matches "function Name" or "procedure Name" (for global package functions)
    static ref RE_SUBPROGRAM: Regex = Regex::new(r"(?im)^\s*(?:function|procedure)\s+(\w+)").unwrap();
}

/// Helper to convert a byte offset into a rough LSP `Range`.
///
/// # Limitations
/// This function counts newlines to find the line number, which is O(N) for the start of the file.
/// It creates a zero-width range at the start of the identifier line.
///
/// # Arguments
/// * `text` - The full source code string.
/// * `start_byte` - The byte index where the regex match starts.
fn byte_to_range(text: &str, start_byte: usize) -> Range {
    let line = text[..start_byte].bytes().filter(|&b| b == b'\n').count() as u32;
    Range {
        start: Position { line, character: 0 },
        end: Position { line, character: 0 },
    }
}

/// Scans a VHDL file using Regular Expressions to find top-level symbols.
///
/// This is the "Fast Lane" of the LSP. It ignores the VHDL grammar structure and simply
/// greps for keywords like `entity`, `package`, `type`.
///
/// # Why Regex?
/// * **Speed:** Can scan 3,000 files in ~100ms. Tree-sitter would take ~60s.
/// * **Concurrency:** Regex is pure Rust and thread-safe. Tree-sitter C-bindings require a Global Mutex.
///
/// # Trade-offs
/// * **Accuracy:** Can be fooled by comments (e.g. `-- entity Foo is`).
/// * **Depth:** Cannot find nested signals/variables. Returns shallow symbols with empty `children`.
///
/// # Returns
/// A vector of `Symbol` structs. These symbols are marked as "Shallow" (empty children)
/// and will be upgraded to "Deep" symbols by the JIT parser when the file is opened.
pub fn scan_fast(text: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let mut add_sym = |re: &Regex, kind: OxideSymbolKind, detail: &str| {
        for cap in re.captures_iter(text) {
            if let Some(m) = cap.get(1) {
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind,
                    detail: Some(detail.to_string()),
                    range: byte_to_range(text, m.start()),
                    children: Vec::new(),
                })
            }
        }
    };
    add_sym(&RE_ENTITY, OxideSymbolKind::Entity, "Entity");
    add_sym(&RE_PACKAGE, OxideSymbolKind::Package, "Package");
    add_sym(&RE_ARCH, OxideSymbolKind::Architecture, "Package");
    add_sym(&RE_TYPE, OxideSymbolKind::Struct, "Type");
    add_sym(&RE_CONSTANT, OxideSymbolKind::Constant, "Constant");
    add_sym(&RE_SUBPROGRAM, OxideSymbolKind::Function, "Subprogram");

    symbols
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::OxideSymbolKind;

    #[test]
    fn test_finds_entity_simple() {
        let src = "entity MyEntity is\nend MyEntity;";
        let syms = scan_fast(src);

        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "MyEntity");
        assert_eq!(syms[0].kind, OxideSymbolKind::Entity);
    }

    #[test]
    fn test_finds_entity_case_insensitive() {
        // VHDL is case insensitive. Regex must handle this.
        let src = "ENTITY UppercaseEnt IS\nEND UppercaseEnt;";
        let syms = scan_fast(src);

        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "UppercaseEnt");
    }

    #[test]
    fn test_finds_entity_with_messy_whitespace() {
        // This fails if you don't use the (?m) multiline flag in regex!
        let src = r#"
library IEEE;
use IEEE.std_logic_1164.all;

    entity   MessyEntity   is
        port (clk : in std_logic);
    end MessyEntity;
"#;
        let syms = scan_fast(src);

        // Debug print to see what we got
        for s in &syms {
            println!("Found: {:?} - {}", s.kind, s.name);
        }

        assert!(
            !syms.is_empty(),
            "Failed to find entity hidden behind newlines/comments"
        );
        assert_eq!(syms[0].name, "MessyEntity");
    }

    #[test]
    fn test_finds_packages_and_types() {
        let src = r#"
package MyPackage is
    type t_state is (IDLE, RUN);
    subtype t_data is std_logic_vector(7 downto 0);
    constant C_WIDTH : integer := 8;
end MyPackage;
"#;
        let syms = scan_fast(src);

        // We expect: 1 Package, 1 Type, 1 Subtype (mapped to Type), 1 Constant
        assert_eq!(syms.len(), 4);

        let names: Vec<String> = syms.iter().map(|s| s.name.clone()).collect();
        assert!(names.contains(&"MyPackage".to_string()));
        assert!(names.contains(&"t_state".to_string()));
        assert!(names.contains(&"t_data".to_string()));
        assert!(names.contains(&"C_WIDTH".to_string()));
    }
}
