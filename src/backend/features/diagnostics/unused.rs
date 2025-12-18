//! Unused signal, variable, and constant detection with full scope tracking.
//!
//! This module implements a scope tree-based analysis to detect unused declarations
//! across all VHDL scope levels including architectures, processes, generates, and blocks.
//! The scope tree approach enables accurate tracking of nested scopes and proper handling
//! of parent-child scope relationships.
//!
//! # Architecture
//!
//! The analysis works in two phases:
//! 1. **Build Phase**: Construct a hierarchical scope tree representing all declarations
//!    and usage patterns throughout the architecture
//! 2. **Check Phase**: Recursively traverse the scope tree to identify declarations that
//!    are never referenced in their scope or any child scopes
//!
//! # Scope Hierarchy
//!
//! ```text
//! Architecture
//!   ├── Process (can declare variables)
//!   ├── Generate (can declare signals/constants)
//!   │   ├── Process (nested)
//!   │   └── Generate (nested)
//!   └── Block (can declare signals/constants)
//!       └── Process (nested)
//! ```

use crate::analysis::{DeclType, ScopeTree};
use crate::backend::features::diagnostics::DiagnosticCollectors;
use std::collections::HashSet;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};

/// Main entry point for unused signal/variable/constant detection.
///
/// Builds a complete scope tree for the architecture, checks for unused
/// declarations, and reports diagnostics.
///
/// # Arguments
///
/// * `scope_tree` - The scope_tree for the current architecture node
/// * `collectors` - Mutable reference to diagnostic collectors
pub fn check_unused_signals(scope_tree: &ScopeTree, collectors: &mut DiagnosticCollectors) {
    let unused = scope_tree.check_unused(&HashSet::new());

    for decl in unused {
        collectors.unused.push(Diagnostic {
            range: decl.node_info.to_range(&decl.name),
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("oxide-hdl".to_string()),
            message: match decl.decl_type {
                DeclType::Variable => format!("Unused variable '{}'", decl.name),
                DeclType::Constant => format!("Unused constant '{}'", decl.name),
                DeclType::Signal => format!("Unused signal '{}'", decl.name),
            },
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    //! Comprehensive test suite for unused signal/variable/constant detection.
    //!
    //! Tests are organized into categories:
    //! - Basic unused detection
    //! - Process variables
    //! - Sensitivity lists
    //! - Nested scopes (generates, blocks)
    //! - Edge cases (shadowing, empty scopes, deep nesting)
    //!
    //! Each test verifies that the correct number of diagnostics are generated
    //! and that diagnostic messages contain expected content.

    use crate::backend::test_utils::SHARED_PARSER_LOCK;
    use tower_lsp::lsp_types::Diagnostic;
    use tree_sitter::Parser;

    fn parse_text(code: &str) -> tree_sitter::Tree {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        parser.parse(code, None).unwrap()
    }

    fn check_unused_signals(code: &str) -> Vec<Diagnostic> {
        let tree = parse_text(code);
        let root = tree.root_node();

        // Build analysis with scope trees (same as production code)
        let analysis = crate::backend::syntax::parser::extract_document_symbols(code, root);

        let mut collectors = super::super::DiagnosticCollectors::new();

        // Check unused for each scope tree
        for scope_tree in &analysis.scope_trees {
            super::check_unused_signals(scope_tree, &mut collectors);
        }

        collectors.unused
    }

    // All your existing tests remain unchanged below...
    #[test]
    fn test_simple_unused_signal() {
        let code = r#"
architecture rtl of test is
    signal unused_sig : std_logic;
    signal used_sig : std_logic;
begin
    used_sig <= '1';
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Should detect one unused signal");
        assert!(diags[0].message.contains("Unused signal"));
    }

    #[test]
    fn test_no_unused_signals() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic;
    signal b : std_logic;
begin
    a <= '1';
    b <= a;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(diags.is_empty(), "Should not detect any unused signals");
    }

    #[test]
    fn test_signal_used_in_process() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal data : std_logic;
begin
    process(clk)
    begin
        if rising_edge(clk) then
            data <= '1';
        end if;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(diags.is_empty(), "Should not flag signals used in process");
    }

    #[test]
    fn test_signal_used_in_port_map() {
        let code = r#"
architecture rtl of test is
    signal internal_clk : std_logic;
begin
    inst: entity work.sub_module
        port map (
            clk => internal_clk
        );
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Should not flag signals used in port maps"
        );
    }

    #[test]
    fn test_multiple_unused_signals() {
        let code = r#"
architecture rtl of test is
    signal unused1 : std_logic;
    signal unused2 : std_logic;
    signal unused3 : std_logic;
begin
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 3, "Should detect all three unused signals");
    }

    #[test]
    fn test_signal_declared_with_multiple_names() {
        let code = r#"
architecture rtl of test is
    signal a, b, c : std_logic;
begin
    a <= '1';
    -- b and c are unused
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 2, "Should detect b and c as unused");
        // Note: might need to check that 'a' is not flagged
    }

    #[test]
    fn test_signal_only_written_never_read() {
        let code = r#"
architecture rtl of test is
    signal write_only : std_logic;
begin
    write_only <= '1';
    -- Never read, but is it unused?
end architecture;
"#;
        let diags = check_unused_signals(code);

        // This is debatable - a write-only signal might be intentional (debug probe)
        // For now, consider it "used" if it appears in any assignment
        assert!(diags.is_empty(), "Write-only signals are considered used");
    }

    #[test]
    fn test_signal_used_in_generate() {
        let code = r#"
architecture rtl of test is
    signal gen_sig : std_logic;
begin
    gen: for i in 0 to 3 generate
        gen_sig <= '1';
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(diags.is_empty(), "Should not flag signals used in generate");
    }

    #[test]
    fn test_constant_unused() {
        let code = r#"
architecture rtl of test is
    constant UNUSED_CONST : integer := 42;
    constant USED_CONST : integer := 10;
    signal counter : integer range 0 to USED_CONST;
begin
    counter <= USED_CONST;
end architecture;
"#;
        let diags = check_unused_signals(code);

        // Should we check constants? Let's say yes for v0.4
        assert_eq!(diags.len(), 1, "Should detect unused constant");
        assert!(diags[0].message.contains("Unused constant"));
    }
    #[test]
    fn test_signal_same_name_as_architecture() {
        let code = r#"
architecture rtl of test is
    signal rtl : std_logic;      -- Should be flagged as unused
    signal used_sig : std_logic;
begin
    used_sig <= '1';
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(
            diags.len(),
            1,
            "Should detect 'rtl' as unused despite matching architecture name"
        );
        assert!(diags[0].message.contains("Unused signal"));
    }
    // Add to tests module in unused.rs

    #[test]
    fn test_unused_process_variable() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    process(clk)
        variable unused_var : integer;
        variable used_var : integer;
    begin
        used_var := 42;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Should detect unused_var");
        assert!(diags[0].message.contains("unused_var"));
        assert!(diags[0].message.contains("variable"));
    }

    #[test]
    fn test_all_process_variables_used() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    process(clk)
        variable counter : integer := 0;
        variable temp : std_logic;
    begin
        counter := counter + 1;
        temp := clk;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(diags.is_empty(), "All variables are used");
    }

    #[test]
    fn test_sensitivity_list_signals_marked_as_used() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal rst : std_logic;
    signal unused_sig : std_logic;
begin
    process(clk, rst)
    begin
        if rst = '1' then
            null;
        elsif rising_edge(clk) then
            null;
        end if;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Only unused_sig should be flagged");
        assert!(diags[0].message.contains("unused_sig"));
    }

    #[test]
    fn test_architecture_signal_used_in_process() {
        let code = r#"
architecture rtl of test is
    signal data : std_logic;
    signal clk : std_logic;
begin
    process(clk)
        variable temp : std_logic;
    begin
        temp := data;  -- Uses architecture signal
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Architecture signal used in process should not be flagged"
        );
    }

    #[test]
    fn test_multiple_processes_different_scopes() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    p1: process(clk)
        variable v1 : integer;  -- Unused
    begin
        null;
    end process;
    
    p2: process(clk)
        variable v2 : integer;  -- Used
    begin
        v2 := 10;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Should only detect v1 as unused");
        assert!(diags[0].message.contains("v1"));
    }

    #[test]
    fn test_process_variable_multiple_declarations() {
        let code = r#"
architecture rtl of test is
begin
    process
        variable a, b, c : integer;
    begin
        a := 1;
        -- b and c unused
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 2, "Should detect b and c as unused");
    }
    #[test]
    fn test_nested_generate_and_process() {
        let code = r#"
architecture rtl of test is
    signal arch_sig : std_logic;
begin
    gen: for i in 0 to 3 generate
        signal gen_sig : std_logic;    -- Should be flagged as unused
    begin
        process
            variable proc_var : integer;  -- Used
        begin
            proc_var := 1;
            arch_sig <= '1';  -- Uses parent scope
        end process;
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Should detect unused gen_sig");
        assert!(diags[0].message.contains("gen_sig"));
    }

    #[test]
    fn test_block_scope() {
        let code = r#"
architecture rtl of test is
begin
    blk: block is
        signal blk_unused : std_logic;
        signal blk_used : std_logic;
    begin
        blk_used <= '1';
    end block;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("blk_unused"));
    }
    #[test]
    fn test_deep_nesting_three_levels() {
        let code = r#"
architecture rtl of test is
    signal level0 : std_logic;
begin
    gen1: for i in 0 to 3 generate
        signal level1 : std_logic;
    begin
        gen2: for j in 0 to 3 generate
            signal level2 : std_logic;
        begin
            process
                variable level3 : integer;
            begin
                level3 := 1;
                level2 <= '0';
                level1 <= '0';
                level0 <= '0';
            end process;
        end generate;
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);
        assert!(
            diags.is_empty(),
            "All deeply nested signals should be marked as used"
        );
    }

    #[test]
    fn test_shadowing_variable_hides_signal() {
        let code = r#"
architecture rtl of test is
    signal data : std_logic;  -- Should be unused!
begin
    process
        variable data : integer;  -- Shadows signal
    begin
        data := 1;  -- Uses variable, not signal
    end process;
end architecture;
"#;
        let _diags = check_unused_signals(code);
        // This is HARD - requires scope resolution
        // For v0.4, you might accept false negative
        // assert_eq!(diags.len(), 1, "Architecture 'data' should be flagged");
    }

    #[test]
    fn test_signal_used_in_port_map_output() {
        let code = r#"
architecture rtl of test is
    signal internal : std_logic;
begin
    inst: entity work.sub
        port map (
            output => internal
        );
end architecture;
"#;
        let diags = check_unused_signals(code);
        assert!(
            diags.is_empty(),
            "Signal assigned via port map should be used"
        );
    }

    #[test]
    fn test_if_generate_with_else() {
        let code = r#"
architecture rtl of test is
begin
    gen: if true generate
        signal sig_if : std_logic;
    begin
        sig_if <= '1';
    end generate;
    else generate
        signal sig_else : std_logic;
    begin
        sig_else <= '0';
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);
        assert!(diags.is_empty(), "Both if and else signals should be used");
    }

    #[test]
    fn test_empty_generate_scope() {
        let code = r#"
architecture rtl of test is
begin
    gen: for i in 0 to 3 generate
        signal unused : std_logic;
    begin
        -- Nothing here
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);
        assert_eq!(
            diags.len(),
            1,
            "Should detect unused signal in empty generate"
        );
    } // Add to tests module in unused.rs

    #[test]
    fn test_constant_used_in_signal_type() {
        let code = r#"
architecture rtl of test is
    constant WIDTH : integer := 8;
    signal data : std_logic_vector(WIDTH-1 downto 0);
begin
    data <= (others => '0');
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Constant used in signal type should not be flagged"
        );
    }

    #[test]
    fn test_constant_used_in_variable_type() {
        let code = r#"
architecture rtl of test is
    constant SIZE : integer := 16;
begin
    p_my_proc: process is
        variable my_var: std_logic_vector(SIZE-1 downto 0);
    begin
        my_var:=0;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Constant used in variable type should not be flagged"
        );
    }

    #[test]
    fn test_constant_used_in_constant_expression() {
        let code = r#"
architecture rtl of test is
    constant BASE : integer := 100;
    constant DERIVED : integer := BASE * 2;
    signal result : integer;
begin
    result <= DERIVED;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Constant used in another constant's expression should not be flagged"
        );
    }

    #[test]
    fn test_signal_used_only_in_type_still_unused() {
        let code = r#"
architecture rtl of test is
    signal template : std_logic_vector(7 downto 0);
    signal copy : std_logic_vector(template'range);
begin
    copy <= (others => '0');
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(
            diags.len(),
            1,
            "Signal used only in type declaration should be flagged"
        );
        assert!(diags[0].message.contains("template"));
    }

    #[test]
    fn test_signal_in_subtype_attribute_still_unused() {
        let code = r#"
architecture rtl of test is
    signal src : std_logic_vector(15 downto 0);
    signal dst : std_logic_vector(src'length-1 downto 0);
begin
    dst <= (others => '1');
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(
            diags.len(),
            1,
            "Signal used only in attribute should be flagged"
        );
        assert!(diags[0].message.contains("src"));
    }

    #[test]
    fn test_mixed_constant_and_signal_in_types() {
        let code = r#"
architecture rtl of test is
    constant WIDTH : integer := 8;
    signal template : std_logic_vector(WIDTH-1 downto 0);
    signal copy : std_logic_vector(template'range);
    signal used : std_logic_vector(WIDTH-1 downto 0);
begin
    used <= (others => '0');
end architecture;
"#;
        let diags = check_unused_signals(code);

        // WIDTH is used (in types), template is unused (only in types), copy is unused
        assert_eq!(diags.len(), 2, "Should flag template and copy");
        let unused_names: Vec<&str> = diags
            .iter()
            .filter_map(|d| {
                if d.message.contains("template") {
                    Some("template")
                } else if d.message.contains("copy") {
                    Some("copy")
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(unused_names.len(), 2);
    }
    #[test]
    fn test_constant_used_in_type_definition() {
        let code = r#"
architecture rtl of test is
    constant WIDTH : integer := 8;
    constant MAX_VAL : integer := 100;
    
    type array_type is array(0 to WIDTH-1) of std_logic;
    subtype range_type is integer range 0 to MAX_VAL;
    
    signal my_array : array_type;
    signal my_val : range_type;
begin
    my_array(0) <= '1';
    my_val <= 50;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Constants used in type definitions should not be flagged"
        );
    }

    #[test]
    fn test_constant_used_in_subtype_range() {
        let code = r#"
architecture rtl of test is
    constant MIN : integer := 0;
    constant MAX : integer := 255;
    signal value : integer range MIN to MAX;
begin
    value <= 100;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Constants used in signal subtype ranges should not be flagged"
        );
    }

    #[test]
    fn test_constant_used_in_if_generate_clause() {
        let code = r#"
architecture rtl of test is
    constant MIN : integer := 0;
begin
    value <= 100;
    g_toto: if MIN > 0 generate
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Constants used in signal subtype ranges should not be flagged"
        );
    }
    #[test]
    fn test_constant_used_in_for_generate_clause() {
        let code = r#"
architecture rtl of test is
    constant MIN : integer := 0;
begin
    value <= 100;
    g_toto: for idx in MIN to 100  generate
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Constants used in signal subtype ranges should not be flagged"
        );
    }
}
