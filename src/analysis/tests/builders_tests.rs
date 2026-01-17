// src/analysis/tests/builder_tests.rs

use super::parse_text;
use crate::analysis::*;
use crate::backend::syntax::parser::extract_document_symbols;

#[test]
fn test_doc_comment_extraction() {
    let code = r#"
architecture rtl of test is
    -- This is a clock signal
    signal clk : std_logic;
    
    -- Data bus
    -- Width is 8 bits
    signal data : std_logic_vector(7 downto 0);
    
    -- Random comment
    
    signal no_doc : std_logic;  -- Should have no doc comment
begin
end architecture;
"#;

    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    // Debug print all declarations with their doc comments
    for scope in &analysis.scope_trees {
        for decl in &scope.declarations {
            println!("Signal: {}", decl.name);
            println!("  Doc: {:?}", decl.doc_comment);
            println!();
        }
    }

    // Assertions
    let clk = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "clk")
        .unwrap();
    assert_eq!(clk.doc_comment, Some("This is a clock signal".to_string()));

    let data = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "data")
        .unwrap();
    assert_eq!(
        data.doc_comment,
        Some("Data bus\nWidth is 8 bits".to_string())
    );

    let no_doc = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "no_doc")
        .unwrap();
    assert_eq!(no_doc.doc_comment, None);
}

#[test]
fn test_default_value_signal() {
    let code = r#"
architecture rtl of test is
    signal clk : std_logic := '1';
    signal counter : integer := 0;
    signal data : std_logic_vector(7 downto 0) := (others => '0');
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let clk = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "clk")
        .unwrap();
    assert_eq!(clk.default_value, Some("'1'".to_string()));

    let counter = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "counter")
        .unwrap();
    assert_eq!(counter.default_value, Some("0".to_string()));

    let data = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "data")
        .unwrap();
    assert_eq!(data.default_value, Some("(others => '0')".to_string()));
}

#[test]
fn test_default_value_constant() {
    let code = r#"
architecture rtl of test is
    constant MAX_COUNT : integer := 100;
    constant ENABLE : std_logic := '1';
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let max_count = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "MAX_COUNT")
        .unwrap();
    assert_eq!(max_count.default_value, Some("100".to_string()));

    let enable = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "ENABLE")
        .unwrap();
    assert_eq!(enable.default_value, Some("'1'".to_string()));
}

#[test]
fn test_default_value_variable() {
    let code = r#"
architecture rtl of test is
begin
    process
        variable temp : integer := 42;
        variable flag : boolean := false;
    begin
        wait;
    end process;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    // Variables are in process scope (first child)
    let process_scope = &analysis.scope_trees[0].children[0];

    let temp = process_scope
        .declarations
        .iter()
        .find(|d| d.name == "temp")
        .unwrap();
    assert_eq!(temp.default_value, Some("42".to_string()));

    let flag = process_scope
        .declarations
        .iter()
        .find(|d| d.name == "flag")
        .unwrap();
    assert_eq!(flag.default_value, Some("false".to_string()));
}

#[test]
fn test_default_value_generic() {
    let code = r#"
entity test is
    generic (
        FIFO_WIDTH : integer := 8;
        DEPTH : positive := 1024
    );
end entity;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let width = analysis
        .entity_scope_trees
        .get("test")
        .unwrap()
        .declarations
        .iter()
        .find(|d| d.name == "FIFO_WIDTH")
        .unwrap();
    assert_eq!(width.default_value, Some("8".to_string()));

    let depth = analysis
        .entity_scope_trees
        .get("test")
        .unwrap()
        .declarations
        .iter()
        .find(|d| d.name == "DEPTH")
        .unwrap();
    assert_eq!(depth.default_value, Some("1024".to_string()));
}

#[test]
fn test_default_value_port() {
    let code = r#"
entity test is
    port (
        toto : in std_logic_vector := (others => '0');
        titi : in std_logic_vector 
    );
end entity;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let toto = analysis
        .entity_scope_trees
        .get("test")
        .unwrap()
        .declarations
        .iter()
        .find(|d| d.name == "toto")
        .unwrap();
    assert_eq!(toto.default_value, Some("(others => '0')".to_string()));

    let titi = analysis
        .entity_scope_trees
        .get("test")
        .unwrap()
        .declarations
        .iter()
        .find(|d| d.name == "titi")
        .unwrap();
    assert_eq!(titi.default_value, None);
}

#[test]
fn test_default_value_multiple_signals() {
    let code = r#"
architecture rtl of test is
    signal a, b, c : std_logic := '0';
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    // All three should have the same default value
    for name in &["a", "b", "c"] {
        let signal = analysis.scope_trees[0]
            .declarations
            .iter()
            .find(|d| d.name == *name)
            .unwrap();
        assert_eq!(
            signal.default_value,
            Some("'0'".to_string()),
            "Signal {} should have default value",
            name
        );
    }
}

#[test]
fn test_default_value_none() {
    let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal data : std_logic_vector(7 downto 0);
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let clk = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "clk")
        .unwrap();
    assert_eq!(clk.default_value, None);

    let data = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "data")
        .unwrap();
    assert_eq!(data.default_value, None);
}

#[test]
fn test_default_value_expression() {
    let code = r#"
architecture rtl of test is
    constant RESULT : integer := 10 + 5 * 2;
    signal index : integer := MAX_COUNT - 1;
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let result = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "RESULT")
        .unwrap();
    assert_eq!(result.default_value, Some("10 + 5 * 2".to_string()));

    let index = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "index")
        .unwrap();
    assert_eq!(index.default_value, Some("MAX_COUNT - 1".to_string()));
}

#[test]
fn test_default_value_string_literal() {
    let code = r#"
architecture rtl of test is
    constant MESSAGE : string := "Hello World";
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let message = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "MESSAGE")
        .unwrap();
    assert_eq!(message.default_value, Some("\"Hello World\"".to_string()));
}

#[test]
fn test_default_value_aggregate() {
    let code = r#"
architecture rtl of test is
    signal init_values : std_logic_vector(3 downto 0) := "1010";
    signal all_zeros : std_logic_vector(7 downto 0) := (others => '0');
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let init_values = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "init_values")
        .unwrap();
    assert_eq!(init_values.default_value, Some("\"1010\"".to_string()));

    let all_zeros = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "all_zeros")
        .unwrap();
    assert_eq!(all_zeros.default_value, Some("(others => '0')".to_string()));
}
