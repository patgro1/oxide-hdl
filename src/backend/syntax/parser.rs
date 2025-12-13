// src/backend/syntax/parser.rs

use crate::analysis::{Analysis, OxideSymbolKind, Symbol};
use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::{Node, TreeCursor};

/// Converts a Tree-sitter `Node` into a standard LSP `Range`.
///
/// Tree-sitter uses row/column (0-indexed), which matches the LSP protocol.
///
/// # Arguments
/// * `node` - The Tree-sitter node to convert.
///
/// # Returns
/// An LSP `Range` struct covering the start and end positions of the node.
pub fn node_to_range(node: Node) -> Range {
    let start = node.start_position();
    let end = node.end_position();
    Range {
        start: Position {
            line: start.row as u32,
            character: start.column as u32,
        },
        end: Position {
            line: end.row as u32,
            character: end.column as u32,
        },
    }
}

/// Recursively extracts all VHDL symbols from a parsed syntax tree.
///
/// This is the main entry point for the "Deep Parse" phase. It walks the entire
/// AST, identifying symbols (Entities, Architectures, Signals, etc.) and building
/// a hierarchical `Analysis` struct.
///
/// # Logic
/// 1. Initializes a `TreeCursor` to walk the tree efficiently.
/// 2. Calls the recursive `visit_node` function to traverse the AST.
/// 3. Flattens the top-level symbols into a HashMap for O(1) lookup.
///
/// # Arguments
/// * `text` - The full source code of the document (used to extract names/types).
/// * `root_node` - The root node of the Tree-sitter tree.
///
/// # Returns
/// An `Analysis` struct containing a map of all top-level symbols found.
pub fn extract_document_symbols(text: &str, root_node: Node) -> Analysis {
    let mut analysis = Analysis::new();
    let mut cursor = root_node.walk();
    let symbols = visit_node(&mut cursor, text);

    for sym in symbols {
        analysis.symbols.insert(sym.name.to_lowercase(), sym);
    }

    analysis
}

/// Helper to determine the semantic kind of an `interface_declaration` node.
///
/// In VHDL, `interface_declaration` is generic and can represent Ports, Generics,
/// or Function Parameters depending on its context. This function walks up the
/// ancestry chain to find the defining clause (e.g., `port_clause` vs `generic_clause`).
///
/// # Arguments
/// * `node` - The `interface_declaration` node to inspect.
///
/// # Returns
/// * `Some(OxideSymbolKind)` - The specific kind (Port, Generic, Function Parameter).
/// * `None` - If the context could not be determined.
fn determine_interface_kind(node: Node) -> Option<OxideSymbolKind> {
    let mut current_node = node.parent();
    while let Some(n) = current_node {
        match n.kind() {
            "port_clause" => return Some(OxideSymbolKind::Port),
            "generic_clause" => return Some(OxideSymbolKind::Generic),
            "subprogram_header" | "function_specification" | "procedure_specification" => {
                return Some(OxideSymbolKind::Port);
            }

            "entity_declaration" | "architecture_body" | "package_declaration" => return None,

            _ => {
                current_node = n.parent();
            }
        }
    }
    None
}

/// Recursively walks the AST to find and extract symbols.
///
/// This function implements a Depth-First Search (DFS) using the Tree-sitter cursor.
/// It identifies nodes that represent symbols, determines if they are containers
/// (like Architectures) or leaves (like Signals), and recurses accordingly.
///
/// # Recursion Logic
/// * If a node is a **Container** (e.g., Entity), it calls `visit_node` on a *new*
///   cursor starting at that node to collect its children.
/// * If a node is **Structural** (e.g., `block_statement`), it recurses but does not
///   create a symbol itself, bubbling up any symbols found inside.
///
/// # Arguments
/// * `cursor` - A mutable reference to the `TreeCursor` used for traversal.
/// * `text` - The source code string.
///
/// # Returns
/// A vector of `Symbol` structs found at this level of the tree.
fn visit_node(cursor: &mut TreeCursor, text: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    if cursor.goto_first_child() {
        loop {
            let node = cursor.node();
            let kind = node.kind();
            let (symbol_kind, is_container) = match kind {
                // Scope
                "package_declaration" => (Some(OxideSymbolKind::Package), true),
                "component_declaration" => (Some(OxideSymbolKind::Component), true),
                "entity_declaration" => (Some(OxideSymbolKind::Entity), true),
                "subprogram_definition" => (Some(OxideSymbolKind::Function), true),
                "subprogram_declaration" => (Some(OxideSymbolKind::Function), true),
                // "function_specification" | "procedure_specification" => {
                //     (Some(OxideSymbolKind::Function), true)
                // }
                "package_definition" => (Some(OxideSymbolKind::Package), true),
                "architecture_definition" => (Some(OxideSymbolKind::Architecture), true),
                "block_statement" => (Some(OxideSymbolKind::Block), true),
                "if_generate_statement" | "for_generate_statement" => {
                    (Some(OxideSymbolKind::Generate), true)
                }
                "process_statement" => (Some(OxideSymbolKind::Process), true),

                // Leaf
                "type_declaration" => (Some(OxideSymbolKind::Struct), false),
                "subtype_declaration" => (Some(OxideSymbolKind::Struct), false),
                "signal_declaration" => (Some(OxideSymbolKind::Signal), false),
                "variable_declaration" => (Some(OxideSymbolKind::Variable), false),
                "constant_declaration" => (Some(OxideSymbolKind::Constant), false),
                "interface_declaration" => {
                    let kind = determine_interface_kind(node);
                    (kind, false)
                }
                "component_instantiation_statement" => {
                    (Some(OxideSymbolKind::ComponentInstantiation), false)
                }
                _ => (None, true),
            };

            let inner_symbols = if is_container {
                visit_node(&mut node.walk(), text)
            } else {
                Vec::new()
            };

            if let Some(k) = symbol_kind {
                let extracted = extract_details(node, k, text, inner_symbols);
                symbols.extend(extracted);
            } else {
                symbols.extend(inner_symbols);
            }

            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
    symbols
}

/// Extracts detailed information (name, type, children) from a specific AST node.
///
/// This function handles the complex logic of extracting symbol names from various
/// VHDL constructs (which might use `label`, `name`, `identifier_list`, etc.).
/// It also handles type extraction (`std_logic_vector`) and attaching nested children.
///
/// # Structural Guard
/// Crucially, this function enforces a "One Symbol Out" rule for containers like
/// Architectures and Entities. It consumes the `children` vector and ensures that
/// leaked siblings (like signals found during the fallback scan) do not pollute
/// the parent list.
///
/// # Arguments
/// * `node` - The Tree-sitter node to extract from.
/// * `kind` - The identified `OxideSymbolKind` of the symbol.
/// * `text` - The source code string.
/// * `children` - A list of symbols found recursively inside this node.
///
/// # Returns
/// A vector of `Symbol` structs. (Returns a vector because a single declaration like
/// `signal x, y : bit` produces multiple symbols).
fn extract_details(
    node: Node,
    kind: OxideSymbolKind,
    text: &str,
    children: Vec<Symbol>,
) -> Vec<Symbol> {
    let mut result = Vec::new();

    let mut name_nodes = Vec::new();

    let get_text = |n: Node| n.utf8_text(text.as_bytes()).unwrap_or("").to_string();

    // Special case for architecture name extraction
    if let Some(arch) = node.child_by_field_name("architecture") {
        name_nodes.push(arch)
    }
    // We start by handling the name of the symbol
    else if let Some(label) = node.child_by_field_name("label") {
        if let Some(label) = label.child_by_field_name("label") {
            name_nodes.push(label);
        }
    } else if let Some(name) = node.child_by_field_name("name") {
        name_nodes.push(name);
    } else {
        // Here we will try to find the first label/identifier underneath
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" => name_nodes.push(child),
                "identifier_list" | "label_declaration" => {
                    let mut list_cursor = child.walk();
                    for list_item in child.children(&mut list_cursor) {
                        if list_item.kind() == "identifier" || list_item.kind() == "label" {
                            name_nodes.push(list_item);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Handle subprogram
    if name_nodes.is_empty() && kind == OxideSymbolKind::Function {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind().contains("specification") {
                let mut spec_cursor = child.walk();
                for grand_child in child.children(&mut spec_cursor) {
                    if grand_child.kind() == "identifier" || grand_child.kind() == "designator" {
                        name_nodes.push(grand_child);
                        break;
                    }
                }
            }
        }
    }

    let mut detail_text = None;
    // Handle instantiation
    // We need to make sure we capture also the entity name
    if kind == OxideSymbolKind::ComponentInstantiation {
        if let Some(comp_node) = node.child_by_field_name("component") {
            detail_text = Some(get_text(comp_node));
        } else if let Some(ent_node) = node.child_by_field_name("entity") {
            let raw = get_text(ent_node);
            // Remove any library from the compoenent name
            let clean = raw.split('.').next_back().unwrap_or(&raw).to_string();
            detail_text = Some(clean);
        } else {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "instantiated_unit" {
                    let mut inner_cursor = child.walk();
                    for inner in child.named_children(&mut inner_cursor) {
                        if let Some(name_node) = inner.child_by_field_name("name") {
                            let raw = get_text(name_node);
                            let clean = raw.split('.').next_back().unwrap_or(&raw).to_string();
                            detail_text = Some(clean);
                        }
                    }
                }
            }
        }
    }

    if kind == OxideSymbolKind::Function {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind().contains("specification") {
                if let Some(return_type) = child.child_by_field_name("type") {
                    detail_text = Some(get_text(return_type));
                }
                break;
            }
        }
    }

    // Now we can try to extract the details
    if detail_text.is_none() {
        if let Some(type_node) = node.child_by_field_name("type") {
            detail_text = Some(get_text(type_node));
        } else {
            // Fallback to subtype indication
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                let k = child.kind();
                // Direct hit on the type
                if k.contains("subtype") || k.contains("type") {
                    detail_text = Some(get_text(child));
                    break;
                }
                // Port and generics might be deeper
                if k == "simple_mode_indication" {
                    let mut inner_cursor = child.walk();
                    for inner in child.named_children(&mut inner_cursor) {
                        if inner.kind() == "subtype_indication" {
                            detail_text = Some(get_text(inner));
                            break;
                        }
                    }
                }
            }
        }
    }

    // Handle special cases
    if name_nodes.is_empty() {
        if kind == OxideSymbolKind::Process {
            // NOTE: We need to make sure we do not crash on unamed process...
            result.push(Symbol {
                name: "process".to_string(),
                kind,
                range: node_to_range(node),
                detail: detail_text,
                children,
            });
        }
    } else {
        for name_node in name_nodes {
            result.push(Symbol {
                name: name_node
                    .utf8_text(text.as_bytes())
                    .unwrap_or("?")
                    .to_string(),
                kind,
                range: node_to_range(node),
                detail: detail_text.clone(),
                children: children.clone(),
            });
        }
    }
    if kind == OxideSymbolKind::Architecture || kind == OxideSymbolKind::Entity {
        if result.is_empty() {
            return Vec::new();
        }
        return result.into_iter().take(1).collect();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    use crate::backend::test_utils::SHARED_PARSER_LOCK;

    // Helper to parse string to tree
    fn parse_text(code: &str) -> (tree_sitter::Tree, Parser) {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(code, None).unwrap();
        (tree, parser)
    }

    #[test]
    fn test_01_entity_structure() {
        let src = r#"
entity MyEnt is
    port (
        clk : in std_logic;
        rst : in std_logic
    );
end MyEnt;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        // 1. Check Entity
        let ent = analysis.symbols.get("myent").expect("Entity not found");
        assert_eq!(ent.kind, OxideSymbolKind::Entity);

        // 2. Check Children (Ports)
        // We expect clk and rst to be children of the Entity
        assert_eq!(ent.children.len(), 2, "Entity should have 2 children");

        let clk = &ent.children[0];
        assert_eq!(clk.name, "clk");
        assert_eq!(clk.kind, OxideSymbolKind::Port);

        let rst = &ent.children[1];
        assert_eq!(rst.name, "rst");
    }

    #[test]
    fn test_02_multiple_signals_one_line() {
        let src = r#"
     architecture Behave of MyEnt is
    signal x, y, z : std_logic_vector(7 downto 0);
     begin
     end Behave;
     "#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        // Find Architecture
        let arch = analysis
            .symbols
            .get("behave")
            .expect("Architecture not found");

        // Should have 3 children (x, y, z)
        assert_eq!(arch.children.len(), 3);
        assert_eq!(arch.children[0].name, "x");
        assert_eq!(arch.children[1].name, "y");
        assert_eq!(arch.children[2].name, "z");

        // Check types
        assert!(
            arch.children[0]
                .detail
                .as_ref()
                .unwrap()
                .contains("std_logic_vector")
        );
    }

    #[test]
    fn test_03_nested_process() {
        let src = r#"
     architecture A of B is
     begin
    process(clk)
        variable counter : integer;
    begin
    end process;
     end A;
     "#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        let arch = analysis.symbols.get("a").unwrap();
        let proc = &arch.children[0];

        assert_eq!(proc.kind, OxideSymbolKind::Process);

        // Check variable inside process
        assert_eq!(proc.children.len(), 1);
        assert_eq!(proc.children[0].name, "counter");
        assert_eq!(proc.children[0].kind, OxideSymbolKind::Variable); // or Variable if you distinguish
    }
    #[test]
    fn test_04_generics_and_ports() {
        let src = r#"
entity Configurable is
    generic (
        G_WIDTH : integer := 32
    );
    port (
        clk : in std_logic
    );
end Configurable;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        let ent = analysis
            .symbols
            .get("configurable")
            .expect("Entity not found");

        // Should have 2 children
        assert_eq!(ent.children.len(), 2);

        let generic = &ent.children[0];
        assert_eq!(generic.name, "G_WIDTH");
        assert_eq!(generic.kind, OxideSymbolKind::Generic); // or Constant, depending on your mapping

        let port = &ent.children[1];
        assert_eq!(port.name, "clk");
        assert_eq!(port.kind, OxideSymbolKind::Port);
    }
    #[test]
    fn test_05_component_instantiation() {
        let src = r#"
architecture Structure of Top is
begin
    u_uart : entity work.uart_tx
        port map (
            clk => sys_clk
        );
end Structure;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        let arch = analysis.symbols.get("structure").unwrap();

        assert_eq!(arch.children.len(), 1);
        let inst = &arch.children[0];

        assert_eq!(inst.name, "u_uart"); // The label
        assert_eq!(inst.kind, OxideSymbolKind::ComponentInstantiation);

        // Bonus: Check if we can verify the target entity?
        // Your current extractor might not put "uart_tx" in the name,
        // but checking the label is the first step.
    }
    #[test]
    fn test_06_package_definitions() {
        let src = r#"
package MyTypes is
    type t_state is (IDLE, RUN, STOP);
    constant C_MAX : integer := 255;
    
    function calculate_parity(data : std_logic_vector) return std_logic;
end MyTypes;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        let pkg = analysis.symbols.get("mytypes").expect("Package not found");

        println!("pkg={:?}", pkg);
        // Expecting: t_state (Type), C_MAX (Constant), calculate_parity (Function)
        assert_eq!(pkg.children.len(), 3);

        let type_sym = &pkg.children[0];
        assert_eq!(type_sym.name, "t_state");
        assert_eq!(type_sym.kind, OxideSymbolKind::Struct); // or Type/Enum

        let const_sym = &pkg.children[1];
        assert_eq!(const_sym.name, "C_MAX");
        assert_eq!(const_sym.kind, OxideSymbolKind::Constant);

        let func_sym = &pkg.children[2];
        assert_eq!(func_sym.name, "calculate_parity");
        assert_eq!(func_sym.kind, OxideSymbolKind::Function);
    }
    #[test]
    fn test_07_generate_blocks() {
        let src = r#"
     architecture Gen of Test is
     begin
    gen_loop: for i in 0 to 3 generate
        signal local_sig : bit;
    begin
    end generate;
     end Gen;
     "#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        let arch = analysis.symbols.get("gen").unwrap();

        // Arch -> Generate
        assert_eq!(arch.children.len(), 1);
        let generate = &arch.children[0];
        assert_eq!(generate.name, "gen_loop");
        assert_eq!(generate.kind, OxideSymbolKind::Generate); // or Namespace

        // Generate -> Signal
        assert_eq!(generate.children.len(), 1);
        let sig = &generate.children[0];
        assert_eq!(sig.name, "local_sig");
    }
    #[test]
    fn test_08_deeply_nested_scope() {
        let src = r#"
architecture Complex of Chip is
begin
    -- Level 1: Generate
    gen_x: for i in 0 to 1 generate
        -- Level 2: Block
        b_scope: block
            -- Level 3: Signal in Block
            signal inner_sig : bit;
        begin
            -- Level 3: Process in Block
            p_deep: process(inner_sig)
                -- Level 4: Variable in Process
                variable deep_var : integer;
            begin
            end process;
        end block;
    end generate;
end Complex;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        // 1. Arch
        let arch = analysis.symbols.get("complex").unwrap();

        // 2. Generate
        let generate = &arch.children[0];
        assert_eq!(generate.name, "gen_x");
        assert_eq!(generate.kind, OxideSymbolKind::Generate);

        // 3. Block
        let block = &generate.children[0];
        assert_eq!(block.name, "b_scope");
        assert_eq!(block.kind, OxideSymbolKind::Block);

        // 4. Signal inside Block (Check siblings)
        let inner_sig = &block.children[0];
        assert_eq!(inner_sig.name, "inner_sig");

        // 5. Process inside Block
        let proc = &block.children[1];
        assert_eq!(proc.name, "p_deep");

        // 6. Variable inside Process (The deepest level)
        let deep_var = &proc.children[0];
        assert_eq!(deep_var.name, "deep_var");
        assert_eq!(deep_var.kind, OxideSymbolKind::Variable); // Or Variable if mapped
    }
    #[test]
    fn test_10_subprogram_declarations() {
        let src = r#"
package body MathOps is
    function add_bits(a, b : std_logic) return std_logic is
        variable temp : std_logic;
        constant delay : time := 1 ns;
    begin
        return a xor b;
    end function;
end MathOps;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        let pkg = analysis.symbols.get("mathops").unwrap();

        // Function Name
        let func = &pkg.children[0];
        assert_eq!(func.name, "add_bits");
        assert_eq!(func.kind, OxideSymbolKind::Function);
        println!("func: {:?}", func);

        // Function Parameters (a, b) -> From Interface List
        // Note: Your extractor puts them first
        let param_a = &func.children[0];
        assert_eq!(param_a.name, "a");

        let param_b = &func.children[1];
        assert_eq!(param_b.name, "b");

        // Function Variables (temp)
        let var = &func.children[2];
        assert_eq!(var.name, "temp");

        // Function Constants (delay)
        let cons = &func.children[3];
        assert_eq!(cons.name, "delay");
    }
    #[test]
    fn test_11_resiliency_on_broken_code() {
        let src = r#"
entity Broken is
    port (
        clk : in std_logic;
        -- SYNTAX ERROR: Missing semicolon below
        rst : in std_logic
        data : in std_logic
    );
end Broken;

architecture incomplete of Broken is
    signal valid_sig : bit;
    -- SYNTAX ERROR: Half-finished signal declaration
    signal broken_sig : 
begin
    process(clk)
    begin
        -- Valid statement inside
        valid_sig <= '1';
        -- SYNTAX ERROR: Garbage text
        what is this even
    end process;
end incomplete;
"#;
        // 1. Parse (Tree-sitter produces a tree with ERROR nodes)
        let (tree, _) = parse_text(src);

        // 2. Extract (MUST NOT PANIC)
        let analysis = extract_document_symbols(src, tree.root_node());

        // 3. Verify Partial Success
        // We expect to find the Entity, even though the ports are malformed
        let ent = analysis
            .symbols
            .get("broken")
            .expect("Should find Entity despite errors");

        // It definitely finds 'clk' (before error)
        assert!(ent.children.iter().any(|s| s.name == "clk"));

        // Tree-sitter is magical: it usually recovers and finds 'rst' and 'data' too!
        // Let's verify how robust it is:
        if ent.children.iter().any(|s| s.name == "rst") {
            println!("Wow, Tree-sitter recovered the missing semicolon port!");
        }

        // Check Architecture
        let arch = analysis
            .symbols
            .get("incomplete")
            .expect("Should find Arch");

        // Should find 'valid_sig' (before error)
        assert!(arch.children.iter().any(|s| s.name == "valid_sig"));

        // The process should exist
        let _proc = arch
            .children
            .iter()
            .find(|s| s.kind == OxideSymbolKind::Process)
            .expect("Should find Process");

        // Ideally, it shouldn't have crashed on "what is this even"
        println!("Successfully parsed broken code without panicking.");
    }
    #[test]
    fn test_12_single_signal_extraction() {
        let src = r#"
architecture A of B is
    signal basic_test_sig : std_logic;
begin
end A;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        let arch = analysis.symbols.get("a").unwrap();

        // Assert that the architecture has one child (the signal)
        assert_eq!(
            arch.children.len(),
            1,
            "Architecture should have 1 signal child"
        );

        let sig = &arch.children[0];
        assert_eq!(sig.name, "basic_test_sig");
        assert_eq!(sig.kind, OxideSymbolKind::Signal);
    }
    #[test]
    fn test_13_structural_leak_and_nesting() {
        let src = r#"
entity SimpleEnt is
end SimpleEnt;

architecture LeakTest of SimpleEnt is
    constant C_VAL : integer := 10;
    signal S_TEST : std_logic;
begin
end LeakTest;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        // --- ASSERTION 1: Find the Leak (This should FAIL on the buggy code) ---
        // We expect only 2 top-level symbols: the Entity and the Architecture.
        // If the code is buggy, it will show 4 symbols (the 2 containers + C_VAL + S_TEST).
        assert_eq!(
            analysis.symbols.len(),
            2,
            "FAIL: The map is polluted. Expected only 2 top-level containers (Entity/Architecture)."
        );

        // --- ASSERTION 2: Verify Correct Nesting (If Assertion 1 passed, this verifies the nesting) ---
        let arch = analysis.symbols.get("leaktest").unwrap();

        // The architecture symbol must have the constant and signal as children.
        assert_eq!(
            arch.children.len(),
            2,
            "Architecture should consume its 2 declarations as children."
        );

        let constant = arch
            .children
            .iter()
            .find(|s| s.name == "C_VAL")
            .expect("Should find Constant nested.");
        let signal = arch
            .children
            .iter()
            .find(|s| s.name == "S_TEST")
            .expect("Should find Signal nested.");

        assert_eq!(constant.kind, OxideSymbolKind::Constant);
        assert_eq!(signal.kind, OxideSymbolKind::Signal);
    }
    #[test]
    fn test_14_complex_signal_extraction() {
        let src = r#"
package MyMath is
    function log2_and_ceil(x: integer) return integer;
end MyMath;

architecture A of B is
    -- Signal with a complex, nested type expression
    signal r_session_udp_id : std_logic_vector(int_to_natural(log2_and_ceil(100)-1) downto 0);
begin
end A;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        // 1. Verify the Architecture exists and the leak is contained (using the expected 2 top-level symbols)
        assert_eq!(
            analysis.symbols.len(),
            2,
            "FAIL: Map should contain only Package (MyMath) and Architecture (A)."
        );

        let arch = analysis.symbols.get("a").expect("Architecture A not found");

        // 2. Verify the complex signal is correctly nested and extracted
        assert_eq!(
            arch.children.len(),
            1,
            "Architecture should contain exactly one signal child."
        );

        let sig = &arch.children[0];

        assert_eq!(
            sig.name, "r_session_udp_id",
            "Signal name was not extracted correctly."
        );
        assert_eq!(
            sig.kind,
            OxideSymbolKind::Signal,
            "Symbol kind is incorrect."
        );

        // 3. Verify the type detail is extracted (this is the most complex part)
        let detail = sig
            .detail
            .as_ref()
            .expect("Type detail should be extracted.");
        assert!(
            detail.contains("std_logic_vector") && detail.contains("log2_and_ceil"),
            "Type detail is missing or incorrect for the complex expression."
        );
    }
    #[test]
    fn test_15_debug_complex_signal() {
        // Valid VHDL wrapper so Tree-sitter is happy
        let src = r#"
architecture DebugArch of Chip is
    signal r_session_udp_id : std_logic_vector(int_to_natural(log2_and_ceil(UDP_RX_SESSION_COUNT)-1) downto 0);
begin
end DebugArch;
"#;

        let (tree, _) = parse_text(src);

        // 1. Run the Extractor (Visitor)
        // This will walk down: design_unit -> architecture_definition -> architecture_head -> signal_declaration
        let analysis = extract_document_symbols(src, tree.root_node());

        // 2. Check the Result
        let arch = analysis
            .symbols
            .get("debugarch")
            .expect("Architecture 'DebugArch' not found in map");

        // 3. Find the Signal
        // This confirms if the recursion worked and the name was extracted
        let sig = arch.children.iter().find(|s| s.name == "r_session_udp_id");

        if let Some(s) = sig {
            println!("✅ Found Signal: {} ({:?})", s.name, s.kind);
            assert_eq!(s.kind, OxideSymbolKind::Signal);

            // Check if the detail captured that massive type string
            if let Some(d) = &s.detail {
                println!("   Detail: {}", d);
                // We expect it to capture the whole type definition
                assert!(d.contains("int_to_natural"));
            }
        } else {
            // Failure Dump
            println!("❌ Signal NOT found in Architecture children.");
            println!(
                "   Children found: {:?}",
                arch.children.iter().map(|s| &s.name).collect::<Vec<_>>()
            );
            panic!("Parser failed to extract the complex signal.");
        }
    }
    #[test]
    fn test_16_debug_package_children() {
        let src = r#"
package MyMath is
    function log2_and_ceil(x: integer) return integer;
    constant PI : real := 3.14;
end MyMath;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        // 1. Find Package
        let pkg = analysis.symbols.get("mymath").expect("Package not found");

        // 2. Check Children count
        println!("Package '{}' has {} children", pkg.name, pkg.children.len());
        for c in &pkg.children {
            println!(" - Child: {} ({:?})", c.name, c.kind);
        }

        // If this asserts 0, we found the bug.
        assert!(pkg.children.len() > 0, "Package children were dropped!");
        assert_eq!(pkg.children.len(), 2);
    }
    #[test]
    fn test_17_variable_extraction() {
        let src = r#"
architecture Behavior of Chip is
    signal s_data : std_logic;
begin
    process(clk)
        variable v_count : integer;
    begin
        v_count := v_count + 1;
    end process;
end Behavior;
"#;
        let (tree, _) = parse_text(src);
        let analysis = extract_document_symbols(src, tree.root_node());

        let arch = analysis
            .symbols
            .get("behavior")
            .expect("Architecture not found");

        // 1. Check Signal in Architecture
        let signal = arch
            .children
            .iter()
            .find(|s| s.name == "s_data")
            .expect("Signal s_data not found");
        assert_eq!(
            signal.kind,
            OxideSymbolKind::Signal,
            "Signals should be parsed as Signal"
        );

        // 2. Check Variable in Process
        let process = arch
            .children
            .iter()
            .find(|s| s.kind == OxideSymbolKind::Process)
            .expect("Process not found");
        let variable = process
            .children
            .iter()
            .find(|s| s.name == "v_count")
            .expect("Variable v_count not found");

        assert_eq!(
            variable.kind,
            OxideSymbolKind::Variable,
            "Variables should be parsed as Variable"
        );
    }
}
