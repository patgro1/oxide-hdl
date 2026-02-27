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

// =========================================================================
// Type info extraction tests
// =========================================================================

#[test]
fn test_type_info_signal_simple() {
    let code = r#"
architecture rtl of test is
    signal clk : std_logic;
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
    assert_eq!(clk.type_info.base_type, "std_logic");
    assert_eq!(clk.type_info.constraints, None);
}

#[test]
fn test_type_info_signal_constrained() {
    let code = r#"
architecture rtl of test is
    signal data : std_logic_vector(7 downto 0);
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let data = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "data")
        .unwrap();
    assert_eq!(data.type_info.base_type, "std_logic_vector");
    assert_eq!(data.type_info.constraints, Some("(7 downto 0)".to_string()));
}

#[test]
fn test_type_info_signal_integer() {
    let code = r#"
architecture rtl of test is
    signal counter : integer;
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let counter = analysis.scope_trees[0]
        .declarations
        .iter()
        .find(|d| d.name == "counter")
        .unwrap();
    assert_eq!(counter.type_info.base_type, "integer");
    assert_eq!(counter.type_info.constraints, None);
}

#[test]
fn test_type_info_constant() {
    let code = r#"
architecture rtl of test is
    constant MAX_COUNT : integer := 100;
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
    assert_eq!(max_count.type_info.base_type, "integer");
    assert_eq!(max_count.type_info.constraints, None);
}

#[test]
fn test_type_info_variable() {
    let code = r#"
architecture rtl of test is
begin
    process
        variable temp : std_logic_vector(31 downto 0);
    begin
        wait;
    end process;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let temp = analysis.scope_trees[0].children[0]
        .declarations
        .iter()
        .find(|d| d.name == "temp")
        .unwrap();
    assert_eq!(temp.type_info.base_type, "std_logic_vector");
    assert_eq!(
        temp.type_info.constraints,
        Some("(31 downto 0)".to_string())
    );
}

#[test]
fn test_type_info_port() {
    let code = r#"
entity test is
    port (
        clk : in std_logic;
        data : in std_logic_vector(7 downto 0);
        result : out integer
    );
end entity;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let entity = analysis.entity_scope_trees.get("test").unwrap();

    let clk = entity
        .declarations
        .iter()
        .find(|d| d.name == "clk")
        .unwrap();
    assert_eq!(clk.type_info.base_type, "std_logic");
    assert_eq!(clk.type_info.constraints, None);

    let data = entity
        .declarations
        .iter()
        .find(|d| d.name == "data")
        .unwrap();
    assert_eq!(data.type_info.base_type, "std_logic_vector");
    assert_eq!(data.type_info.constraints, Some("(7 downto 0)".to_string()));

    let result = entity
        .declarations
        .iter()
        .find(|d| d.name == "result")
        .unwrap();
    assert_eq!(result.type_info.base_type, "integer");
    assert_eq!(result.type_info.constraints, None);
}

#[test]
fn test_type_info_generic() {
    let code = r#"
entity test is
    generic (
        MY_WIDTH : integer := 8;
        DEPTH : positive := 1024
    );
end entity;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let entity = analysis.entity_scope_trees.get("test").unwrap();

    let width = entity
        .declarations
        .iter()
        .find(|d| d.name == "MY_WIDTH")
        .unwrap();
    assert_eq!(width.type_info.base_type, "integer");
    assert_eq!(width.type_info.constraints, None);

    let depth = entity
        .declarations
        .iter()
        .find(|d| d.name == "DEPTH")
        .unwrap();
    assert_eq!(depth.type_info.base_type, "positive");
    assert_eq!(depth.type_info.constraints, None);
}

// =========================================================================
// Scope name extraction tests
// =========================================================================

#[test]
fn test_scope_name_architecture() {
    let code = r#"
architecture rtl of my_entity is
    signal clk : std_logic;
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    assert_eq!(analysis.scope_trees.len(), 1);
    assert_eq!(analysis.scope_trees[0].name, Some("rtl".to_string()));
}

#[test]
fn test_scope_name_entity() {
    let code = r#"
entity my_entity is
    port (
        clk : in std_logic
    );
end entity;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let entity = analysis.entity_scope_trees.get("my_entity").unwrap();
    assert_eq!(entity.name, Some("my_entity".to_string()));
}

#[test]
fn test_scope_name_labeled_process() {
    let code = r#"
architecture rtl of test is
begin
    main_proc: process(clk)
    begin
        wait;
    end process;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let process_scope = &analysis.scope_trees[0].children[0];
    assert_eq!(process_scope.name, Some("main_proc".to_string()));
}

#[test]
fn test_scope_name_unlabeled_process() {
    let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        wait;
    end process;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let process_scope = &analysis.scope_trees[0].children[0];
    assert_eq!(process_scope.name, None);
}

#[test]
fn test_scope_name_labeled_block() {
    let code = r#"
architecture rtl of test is
begin
    my_block: block
    begin
    end block;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let block_scope = &analysis.scope_trees[0].children[0];
    assert_eq!(block_scope.name, Some("my_block".to_string()));
}

#[test]
fn test_scope_name_for_generate() {
    let code = r#"
architecture rtl of test is
begin
    gen_loop: for i in 0 to 7 generate
    begin
    end generate;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let generate_scope = &analysis.scope_trees[0].children[0];
    assert_eq!(generate_scope.name, Some("gen_loop".to_string()));
}

#[test]
fn test_scope_name_if_generate() {
    let code = r#"
architecture rtl of test is
begin
    gen_cond: if true generate
    begin
    end generate;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let generate_scope = &analysis.scope_trees[0].children[0];
    assert_eq!(generate_scope.name, Some("gen_cond".to_string()));
}

// =========================================================================
// Instantiation extraction tests
// =========================================================================

#[test]
fn test_instantiation_simple_component() {
    let code = r#"
architecture rtl of test is
begin
    u_fifo: fifo_comp
        port map (
            clk => clk,
            data => data
        );
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    assert_eq!(analysis.scope_trees[0].instantiations.len(), 1);
    let inst = &analysis.scope_trees[0].instantiations[0];
    assert_eq!(inst.label, "u_fifo");
    assert_eq!(inst.component, "fifo_comp");
}

#[test]
fn test_instantiation_entity_work() {
    let code = r#"
architecture rtl of test is
begin
    u_uart: entity work.uart_rx
        port map (
            clk => clk
        );
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    assert_eq!(analysis.scope_trees[0].instantiations.len(), 1);
    let inst = &analysis.scope_trees[0].instantiations[0];
    assert_eq!(inst.label, "u_uart");
    assert_eq!(inst.component, "uart_rx");
}

#[test]
fn test_instantiation_entity_with_architecture() {
    let code = r#"
architecture rtl of test is
begin
    u_cpu: entity work.cpu(behavioral)
        port map (
            clk => clk
        );
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    assert_eq!(analysis.scope_trees[0].instantiations.len(), 1);
    let inst = &analysis.scope_trees[0].instantiations[0];
    assert_eq!(inst.label, "u_cpu");
    assert_eq!(inst.component, "cpu");
}

#[test]
fn test_instantiation_in_generate() {
    let code = r#"
architecture rtl of test is
begin
    gen_loop: for i in 0 to 3 generate
    begin
        u_cell: cell_comp
            port map (
                idx => i
            );
    end generate;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let generate_scope = &analysis.scope_trees[0].children[0];
    assert_eq!(generate_scope.instantiations.len(), 1);
    let inst = &generate_scope.instantiations[0];
    assert_eq!(inst.label, "u_cell");
    assert_eq!(inst.component, "cell_comp");
}

#[test]
fn test_instantiation_in_block() {
    let code = r#"
architecture rtl of test is
begin
    my_block: block
    begin
        u_reg: reg_comp
            port map (
                d => data
            );
    end block;
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let block_scope = &analysis.scope_trees[0].children[0];
    assert_eq!(block_scope.instantiations.len(), 1);
    let inst = &block_scope.instantiations[0];
    assert_eq!(inst.label, "u_reg");
    assert_eq!(inst.component, "reg_comp");
}

#[test]
fn test_instantiation_multiple() {
    let code = r#"
architecture rtl of test is
begin
    u_fifo1: fifo_comp port map (clk => clk);
    u_fifo2: fifo_comp port map (clk => clk);
    u_uart: uart_comp port map (clk => clk);
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    assert_eq!(analysis.scope_trees[0].instantiations.len(), 3);

    let labels: Vec<&str> = analysis.scope_trees[0]
        .instantiations
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert!(labels.contains(&"u_fifo1"));
    assert!(labels.contains(&"u_fifo2"));
    assert!(labels.contains(&"u_uart"));
}

#[test]
fn test_instantiation_entity_library_name() {
    let code = r#"
architecture rtl of test is
begin
    u_mem: entity mylib.memory_controller
        port map (
            clk => clk
        );
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    assert_eq!(analysis.scope_trees[0].instantiations.len(), 1);
    let inst = &analysis.scope_trees[0].instantiations[0];
    assert_eq!(inst.label, "u_mem");
    assert_eq!(inst.component, "memory_controller");
}

// =========================================================================
// Package scope tree tests
// =========================================================================

#[test]
fn test_package_scope_tree_name() {
    let code = r#"
package my_pkg is
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    assert_eq!(pkg.name, Some("my_pkg".to_string()));
}

#[test]
fn test_package_constant() {
    let code = r#"
package my_pkg is
    constant C_MAX : integer := 255;
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    assert_eq!(pkg.declarations.len(), 1);

    let constant = &pkg.declarations[0];
    assert_eq!(constant.name, "C_MAX");
    assert!(matches!(constant.decl_type, DeclType::Constant));
}

#[test]
fn test_package_multiple_constants() {
    let code = r#"
package my_pkg is
    constant C_WIDTH : integer := 8;
    constant C_DEPTH : integer := 1024;
    constant C_ENABLE : std_logic := '1';
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    assert_eq!(pkg.declarations.len(), 3);

    let names: Vec<&str> = pkg.declarations.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"C_WIDTH"));
    assert!(names.contains(&"C_DEPTH"));
    assert!(names.contains(&"C_ENABLE"));
}

#[test]
fn test_package_type_declaration() {
    let code = r#"
package my_pkg is
    type t_state is (IDLE, RUN, STOP);
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    // 1 type + 3 enum literals (IDLE, RUN, STOP) flattened into scope
    assert_eq!(pkg.declarations.len(), 4);

    let type_decl = pkg
        .declarations
        .iter()
        .find(|d| d.name == "t_state")
        .expect("Type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));

    let enum_names: Vec<&str> = pkg
        .declarations
        .iter()
        .filter(|d| matches!(d.decl_type, DeclType::EnumLiteral))
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(enum_names, vec!["IDLE", "RUN", "STOP"]);
}

#[test]
fn test_package_function_declaration() {
    let code = r#"
package my_pkg is
    function calc_parity(data : std_logic_vector) return std_logic;
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    assert_eq!(pkg.declarations.len(), 1);

    let func_decl = &pkg.declarations[0];
    assert_eq!(func_decl.name, "calc_parity");
    assert!(matches!(func_decl.decl_type, DeclType::FunctionDeclaration));
}

#[test]
fn test_package_procedure_declaration() {
    let code = r#"
package my_pkg is
    procedure do_reset(signal rst : out std_logic);
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    assert_eq!(pkg.declarations.len(), 1);

    let proc_decl = &pkg.declarations[0];
    assert_eq!(proc_decl.name, "do_reset");
    assert!(matches!(
        proc_decl.decl_type,
        DeclType::ProcedureDeclaration
    ));
}

#[test]
fn test_package_mixed_declarations() {
    let code = r#"
package my_pkg is
    type t_state is (IDLE, RUN, STOP);
    constant C_MAX : integer := 255;
    function calculate_parity(data : std_logic_vector) return std_logic;
    procedure init_system(signal ready : out std_logic);
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    // 4 original (type + constant + function + procedure) + 3 enum literals (IDLE, RUN, STOP)
    assert_eq!(pkg.declarations.len(), 7);
}

#[test]
fn test_package_subtype_declaration() {
    let code = r#"
package my_pkg is
    subtype t_byte is std_logic_vector(7 downto 0);
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    assert_eq!(pkg.declarations.len(), 1);

    let subtype_decl = &pkg.declarations[0];
    assert_eq!(subtype_decl.name, "t_byte");
    assert!(matches!(subtype_decl.decl_type, DeclType::Subtype));
}

#[test]
fn test_package_type_and_subtype() {
    let code = r#"
package my_pkg is
    type t_data is array (0 to 7) of std_logic_vector(7 downto 0);
    subtype t_index is integer range 0 to 7;
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");
    assert_eq!(pkg.declarations.len(), 2);

    let names: Vec<&str> = pkg.declarations.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"t_data"));
    assert!(names.contains(&"t_index"));
}

// =========================================================================
// Package body scope tree tests
// =========================================================================

#[test]
fn test_package_body_scope_tree_name() {
    let code = r#"
package body my_pkg is
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    assert_eq!(analysis.package_body_scope_trees.len(), 1);
    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();
    assert_eq!(pkg_body.name, Some("my_pkg".to_string()));
    assert!(matches!(pkg_body.kind, ScopeKind::PackageBody));
}

#[test]
fn test_package_body_links_to_header() {
    let code = r#"
package body my_pkg is
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();
    assert_eq!(pkg_body.package, Some("my_pkg".to_string()));
}

#[test]
fn test_package_body_private_constant() {
    let code = r#"
package body my_pkg is
    constant C_INTERNAL : integer := 42;
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();
    assert_eq!(pkg_body.declarations.len(), 1);

    let constant = &pkg_body.declarations[0];
    assert_eq!(constant.name, "C_INTERNAL");
    assert!(matches!(constant.decl_type, DeclType::Constant));
}

#[test]
fn test_package_body_private_type() {
    let code = r#"
package body my_pkg is
    type t_internal_state is (INIT, WORK, DONE);
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();
    // 1 type + 3 enum literals (INIT, WORK, DONE) flattened into scope
    assert_eq!(pkg_body.declarations.len(), 4);

    let type_decl = pkg_body
        .declarations
        .iter()
        .find(|d| d.name == "t_internal_state")
        .expect("Type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));
}

#[test]
fn test_package_body_function_implementation() {
    let code = r#"
package body my_pkg is
    function calc_parity(data : std_logic_vector) return std_logic is
        variable result : std_logic := '0';
    begin
        return result;
    end function;
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();
    // Function implementation should create a child scope
    assert_eq!(pkg_body.children.len(), 1);

    let func_scope = &pkg_body.children[0];
    assert!(matches!(func_scope.kind, ScopeKind::Function));
    assert_eq!(func_scope.name, Some("calc_parity".to_string()));
}

#[test]
fn test_package_body_procedure_implementation() {
    let code = r#"
package body my_pkg is
    procedure do_reset(signal rst : out std_logic) is
    begin
        rst <= '1';
    end procedure;
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();
    assert_eq!(pkg_body.children.len(), 1);

    let proc_scope = &pkg_body.children[0];
    assert!(matches!(proc_scope.kind, ScopeKind::Procedure));
    assert_eq!(proc_scope.name, Some("do_reset".to_string()));
}

#[test]
fn test_package_body_function_with_variables() {
    let code = r#"
package body my_pkg is
    function add_one(x : integer) return integer is
        variable temp : integer;
    begin
        temp := x + 1;
        return temp;
    end function;
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();
    let func_scope = &pkg_body.children[0];

    // Function should have parameter and local variable
    let var = func_scope
        .declarations
        .iter()
        .find(|d| d.name == "temp")
        .expect("Variable temp not found");
    assert!(matches!(var.decl_type, DeclType::Variable));
}

#[test]
fn test_package_body_mixed_declarations() {
    let code = r#"
package body my_pkg is
    constant C_PRIVATE : integer := 100;
    type t_private is (A, B, C);

    function helper(x : integer) return integer is
    begin
        return x;
    end function;

    procedure init is
    begin
        null;
    end procedure;
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();

    // Top-level declarations: constant + type + 3 enum literals (A, B, C) + function + procedure
    assert_eq!(pkg_body.declarations.len(), 7);

    // Child scopes: function + procedure
    assert_eq!(pkg_body.children.len(), 2);
}

#[test]
fn test_package_body_multiple_functions() {
    let code = r#"
package body my_pkg is
    function func_a return integer is
    begin
        return 1;
    end function;

    function func_b return integer is
    begin
        return 2;
    end function;

    function func_c return integer is
    begin
        return 3;
    end function;
end package body;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg_body = analysis.package_body_scope_trees.get("my_pkg").unwrap();
    assert_eq!(pkg_body.children.len(), 3);

    let names: Vec<&str> = pkg_body
        .children
        .iter()
        .filter_map(|c| c.name.as_deref())
        .collect();
    assert!(names.contains(&"func_a"));
    assert!(names.contains(&"func_b"));
    assert!(names.contains(&"func_c"));
}

// =========================================================================
// Record type extraction tests
// =========================================================================

#[test]
fn test_record_type_simple() {
    let code = r#"
package my_pkg is
    type t_record is record
        field_a : std_logic;
        field_b : integer;
    end record;
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");

    let type_decl = pkg
        .declarations
        .iter()
        .find(|d| d.name == "t_record")
        .expect("Record type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));

    let fields = type_decl
        .parameters
        .as_ref()
        .expect("Record should have fields in parameters");
    assert_eq!(fields.len(), 2);

    assert_eq!(fields[0].name, "field_a");
    assert!(matches!(fields[0].decl_type, DeclType::RecordField));
    assert_eq!(fields[0].type_info.base_type, "std_logic");

    assert_eq!(fields[1].name, "field_b");
    assert!(matches!(fields[1].decl_type, DeclType::RecordField));
    assert_eq!(fields[1].type_info.base_type, "integer");
}

#[test]
fn test_record_type_constrained_fields() {
    let code = r#"
package my_pkg is
    type t_data is record
        addr : std_logic_vector(15 downto 0);
        data : std_logic_vector(31 downto 0);
        valid : std_logic;
    end record;
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");

    let type_decl = pkg
        .declarations
        .iter()
        .find(|d| d.name == "t_data")
        .expect("Record type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));

    let fields = type_decl
        .parameters
        .as_ref()
        .expect("Record should have fields in parameters");
    assert_eq!(fields.len(), 3);

    assert_eq!(fields[0].name, "addr");
    assert!(matches!(fields[0].decl_type, DeclType::RecordField));
    assert_eq!(fields[0].type_info.base_type, "std_logic_vector");
    assert_eq!(
        fields[0].type_info.constraints,
        Some("(15 downto 0)".to_string())
    );

    assert_eq!(fields[1].name, "data");
    assert!(matches!(fields[1].decl_type, DeclType::RecordField));
    assert_eq!(fields[1].type_info.base_type, "std_logic_vector");
    assert_eq!(
        fields[1].type_info.constraints,
        Some("(31 downto 0)".to_string())
    );

    assert_eq!(fields[2].name, "valid");
    assert!(matches!(fields[2].decl_type, DeclType::RecordField));
    assert_eq!(fields[2].type_info.base_type, "std_logic");
    assert_eq!(fields[2].type_info.constraints, None);
}

#[test]
fn test_record_type_in_architecture() {
    let code = r#"
architecture rtl of test is
    type t_ctrl is record
        enable : std_logic;
        mode   : integer;
    end record;
    signal ctrl : t_ctrl;
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let arch = &analysis.scope_trees[0];

    let type_decl = arch
        .declarations
        .iter()
        .find(|d| d.name == "t_ctrl")
        .expect("Record type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));

    let fields = type_decl
        .parameters
        .as_ref()
        .expect("Record should have fields in parameters");
    assert_eq!(fields.len(), 2);

    assert_eq!(fields[0].name, "enable");
    assert!(matches!(fields[0].decl_type, DeclType::RecordField));
    assert_eq!(fields[0].type_info.base_type, "std_logic");

    assert_eq!(fields[1].name, "mode");
    assert!(matches!(fields[1].decl_type, DeclType::RecordField));
    assert_eq!(fields[1].type_info.base_type, "integer");
}

#[test]
fn test_record_type_single_field() {
    let code = r#"
package my_pkg is
    type t_wrapper is record
        value : std_logic;
    end record;
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);
    println!("analysis: {:?}", analysis);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");

    let type_decl = pkg
        .declarations
        .iter()
        .find(|d| d.name == "t_wrapper")
        .expect("Record type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));

    let fields = type_decl
        .parameters
        .as_ref()
        .expect("Record should have fields in parameters");
    assert_eq!(fields.len(), 1);

    assert_eq!(fields[0].name, "value");
    assert!(matches!(fields[0].decl_type, DeclType::RecordField));
    assert_eq!(fields[0].type_info.base_type, "std_logic");
}

#[test]
fn test_record_type_many_fields() {
    let code = r#"
package my_pkg is
    type t_axi is record
        awaddr  : std_logic_vector(31 downto 0);
        awvalid : std_logic;
        awready : std_logic;
        wdata   : std_logic_vector(31 downto 0);
        wvalid  : std_logic;
    end record;
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");

    let type_decl = pkg
        .declarations
        .iter()
        .find(|d| d.name == "t_axi")
        .expect("Record type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));

    let fields = type_decl
        .parameters
        .as_ref()
        .expect("Record should have fields in parameters");
    assert_eq!(fields.len(), 5);

    let field_names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        field_names,
        vec!["awaddr", "awvalid", "awready", "wdata", "wvalid"]
    );

    // All fields should be RecordField type
    for field in fields {
        assert!(matches!(field.decl_type, DeclType::RecordField));
    }
}

#[test]
fn test_record_type_multi_identifier_fields() {
    let code = r#"
package my_pkg is
    type t_pair is record
        field_a, field_b : std_logic;
        data : std_logic_vector(7 downto 0);
    end record;
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");

    let type_decl = pkg
        .declarations
        .iter()
        .find(|d| d.name == "t_pair")
        .expect("Record type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));

    let fields = type_decl
        .parameters
        .as_ref()
        .expect("Record should have fields in parameters");
    // field_a and field_b should be expanded into separate declarations
    assert_eq!(fields.len(), 3);

    assert_eq!(fields[0].name, "field_a");
    assert!(matches!(fields[0].decl_type, DeclType::RecordField));
    assert_eq!(fields[0].type_info.base_type, "std_logic");

    assert_eq!(fields[1].name, "field_b");
    assert!(matches!(fields[1].decl_type, DeclType::RecordField));
    assert_eq!(fields[1].type_info.base_type, "std_logic");

    assert_eq!(fields[2].name, "data");
    assert!(matches!(fields[2].decl_type, DeclType::RecordField));
    assert_eq!(fields[2].type_info.base_type, "std_logic_vector");
    assert_eq!(
        fields[2].type_info.constraints,
        Some("(7 downto 0)".to_string())
    );
}

#[test]
fn test_enum_type_has_literal_parameters() {
    let code = r#"
package my_pkg is
    type t_state is (IDLE, RUN, STOP);
end package;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let pkg = analysis
        .package_declaration_scope_trees
        .get("my_pkg")
        .expect("Package not found");

    let type_decl = pkg
        .declarations
        .iter()
        .find(|d| d.name == "t_state")
        .expect("Type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));

    // Enum types should have their literals as parameters
    let params = type_decl
        .parameters
        .as_ref()
        .expect("Enum should have parameters");
    assert_eq!(params.len(), 3);
    assert!(
        params
            .iter()
            .all(|p| matches!(p.decl_type, DeclType::EnumLiteral))
    );
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["IDLE", "RUN", "STOP"]);
}

#[test]
fn test_non_enum_non_record_type_has_no_parameters() {
    let code = r#"
architecture rtl of test is
    type t_byte is range 0 to 255;
begin
end architecture;
"#;
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let scope = &analysis.scope_trees[0];
    let type_decl = scope
        .declarations
        .iter()
        .find(|d| d.name == "t_byte")
        .expect("Type not found");
    assert!(matches!(type_decl.decl_type, DeclType::Type));
    assert!(type_decl.parameters.is_none());
}

#[test]
fn test_if_condition_usage_captured() {
    let code = r#"
architecture rtl of test is
    signal old_name : std_logic;
begin
    process
    begin
        old_name <= '1';
        if old_name = '1' then
            null;
        end if;
    end process;
end architecture;
"#;

    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    // Find the process scope (child of architecture)
    let arch_scope = &analysis.scope_trees[0];
    let process_scope = &arch_scope.children[0];

    println!("\nProcess scope:");
    println!("  Local usages: {}", process_scope.local_usage.len());

    for usage in &process_scope.local_usage {
        println!(
            "    Usage: '{}' at line {}",
            usage.name, usage.range.start.line
        );
    }

    // Should have 2 usages of old_name
    let old_name_usages: Vec<_> = process_scope
        .local_usage
        .iter()
        .filter(|u| u.name.eq_ignore_ascii_case("old_name"))
        .collect();

    assert_eq!(
        old_name_usages.len(),
        2,
        "Should capture both usages of old_name (assignment and if condition). Found: {}",
        old_name_usages.len()
    );
}

#[test]
fn test_if_creates_child_scope() {
    let code = r#"
architecture rtl of test is
    signal old_name : std_logic;
begin
    process
    begin
        old_name <= '1';
        if old_name = '1' then
            null;
        end if;
    end process;
end architecture;
"#;

    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let arch_scope = &analysis.scope_trees[0];
    let process_scope = &arch_scope.children[0];

    println!("\nProcess has {} children", process_scope.children.len());
    for (i, child) in process_scope.children.iter().enumerate() {
        println!(
            "  Child {}: {:?}, usages: {}",
            i,
            child.kind,
            child.local_usage.len()
        );
        for usage in &child.local_usage {
            println!("    Usage: '{}'", usage.name);
        }
    }
}
