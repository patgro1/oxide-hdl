use crate::analysis::{ScopeTree, UsageContext};
use crate::backend::features::diagnostics::{DiagnosticCollectors, DiagnosticContext, messages};
use crate::backend::features::lookup::lookup_symbol;
use std::collections::HashMap;
use tree_sitter::Node;

/// Checks all identifier usages in a scope tree for undeclared references.
///
/// Walks each usage in the scope's `local_usage` set, filters out identifiers
/// that appear in declaration contexts, dot-notation suffixes, or port map formals,
/// then attempts to resolve the remaining identifiers via [`lookup_symbol`].
/// Unresolved identifiers produce either an "undeclared identifier" or
/// "undefined type" diagnostic depending on the usage context.
///
/// Recurses into child scopes to cover the full hierarchy.
///
/// # Arguments
/// * `root` - The AST root node used to resolve usage positions back to tree-sitter nodes
/// * `ctx` - Diagnostic context containing all read-only validation parameters
/// * `scope_tree` - The scope tree node to check
/// * `collectors` - Diagnostic collectors to push errors into
/// * `lookup_cache` - Name-keyed cache to avoid redundant lookups
pub fn check_undeclared_identifiers(
    root: Node,
    ctx: &DiagnosticContext,
    scope_tree: &ScopeTree,
    collectors: &mut DiagnosticCollectors,
    lookup_cache: &mut HashMap<String, bool>,
) {
    for usage in &scope_tree.local_usage {
        // If the identifier matches an ignore pattern, skip it
        if ctx.ignored_patterns.iter().any(|r| r.is_match(&usage.name)) {
            continue;
        }

        // 1. Get the AST node using root
        let node_opt = root.descendant_for_point_range(
            tree_sitter::Point {
                row: usage.range.start.line as usize,
                column: usage.range.start.character as usize,
            },
            tree_sitter::Point {
                row: usage.range.end.line as usize,
                column: usage.range.end.character as usize,
            },
        );

        if let Some(node) = node_opt {
            if is_declaration_context(node) {
                continue;
            }
            if node.kind() == "attribute_identifier" {
                continue;
            }
            if is_loop_variable(node) {
                continue;
            }
            if is_dot_suffix(node) {
                continue;
            }
            if is_port_map_formal(node) {
                continue;
            }
            if is_aggregate_choice(node) {
                continue;
            }
        }

        let is_found = *lookup_cache.entry(usage.name.clone()).or_insert_with(|| {
            // Use ctx.current_uri and ctx.global_map
            let results = lookup_symbol(
                &usage.name,
                ctx.current_uri,
                ctx.global_map,
                &usage.range.start,
                false,
            );
            !results.is_empty()
        });

        if !is_found {
            match usage.context {
                UsageContext::TypeSpec => collectors
                    .undefined
                    .push(messages::undefined_type(&usage.name, &usage.range)),
                UsageContext::Behavioral => collectors
                    .undefined
                    .push(messages::undeclared_identifier(&usage.name, &usage.range)),
            }
        }
    }

    for child in &scope_tree.children {
        // Pass context down without exploding arguments
        check_undeclared_identifiers(root, ctx, child, collectors, lookup_cache);
    }
}

/// Returns `true` if the identifier is a for-loop variable declaration.
///
/// In `for i in 0 to 7 loop`, the `i` is an implicit constant declaration.
/// It's the direct `identifier` child of a `parameter_specification` node.
/// The range part (which may reference constants) should still be checked.
fn is_loop_variable(node: Node) -> bool {
    node.parent()
        .map(|p| p.kind() == "parameter_specification")
        .unwrap_or(false)
}

/// Returns `true` if the identifier is the suffix of a dot-notation access (e.g., `rec.field`).
///
/// Checks whether the node's parent is a `selection` node, which wraps the
/// right-hand side of a `selected_name`. These suffixes refer to record fields
/// or sub-elements and should not be resolved as standalone identifiers.
fn is_dot_suffix(node: Node) -> bool {
    node.parent()
        .map(|p| p.kind() == "selection")
        .unwrap_or(false)
}

/// Returns `true` if the identifier is a formal name in a port/generic map association.
///
/// In `port map (clk => clk_internal)`, the formal `clk` refers to a port of the
/// instantiated entity — not a local declaration. This checks whether the node is
/// inside an `association_element` (directly or one level up via a `name` wrapper).
fn is_port_map_formal(node: Node) -> bool {
    if let Some(parent) = node.parent() {
        if parent.kind() == "association_element" {
            return true;
        } else if let Some(parent) = parent.parent()
            && parent.kind() == "association_element"
        {
            return true;
        }
    }
    false
}

/// Returns `true` if the identifier is a field name (choice) in a record aggregate.
///
/// In `(field_a => 42, field_b => true)`, the field names `field_a` and `field_b`
/// are choices in `element_association` nodes. They refer to record fields, not
/// local declarations. Only the choice (first child) is skipped — the value
/// (second child) is still checked for undeclared references.
fn is_aggregate_choice(node: Node) -> bool {
    let mut current = node;
    for _ in 0..5 {
        if let Some(parent) = current.parent() {
            if parent.kind() == "element_association"
                && let Some(first) = parent.named_child(0)
            {
                return node.start_byte() >= first.start_byte()
                    && node.end_byte() <= first.end_byte();
            }
            current = parent;
        } else {
            break;
        }
    }
    false
}

/// Returns `true` if the identifier is a name being *declared* (not a type reference).
///
/// Walks up to 5 parent nodes checking whether any ancestor is a node that
/// exclusively contains declared names (e.g., `identifier_list` for signals,
/// `record_type_definition` for field names, `enumeration_type_definition` for
/// enum literals). This intentionally does NOT match whole declaration nodes
/// like `signal_declaration`, because those also contain type references
/// that should be checked for validity.
fn is_declaration_context(node: Node) -> bool {
    let mut curr = Some(node);
    for _ in 0..5 {
        if let Some(n) = curr {
            match n.kind() {
                // Name-defining wrappers — identifiers here are being declared,
                // not referenced. We intentionally exclude broad declaration
                // nodes (entity_declaration, signal_declaration, etc.) because
                // they also contain type references that should be validated.
                "identifier_list"
                | "record_type_definition"
                | "enumeration_type_definition"
                | "subprogram_specification"
                | "subprogram_header"
                | "alias_declaration"
                | "attribute_declaration"
                | "label_declaration" => return true,
                _ => {}
            }
            curr = n.parent();
        } else {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::{backend::test_utils::parse_text, config::OxideConfig};
    use std::collections::HashMap;
    use tower_lsp::lsp_types::{Diagnostic, Url};

    /// Helper: parses VHDL code, builds analysis, runs undeclared-identifier checks,
    /// and returns the resulting diagnostics from `collectors.undefined`.
    fn check_undeclared(code: &str) -> Vec<Diagnostic> {
        let tree = parse_text(code);
        let root = tree.root_node();
        let dummy_uri = Url::parse("file:///test.vhd").unwrap();

        let analysis = crate::backend::syntax::parser::extract_document_symbols(code, root);

        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(dummy_uri.clone(), analysis.clone());

        let mut collectors = crate::backend::features::diagnostics::DiagnosticCollectors::new();
        let mut lookup_cache = HashMap::new();

        let ctx = super::super::DiagnosticContext {
            text: code,
            scope_tree: None,
            analysis: &analysis,
            global_map: &analysis_map,
            current_uri: &dummy_uri,
            ignored_patterns: &[],
        };

        for scope_tree in &analysis.scope_trees {
            super::check_undeclared_identifiers(
                root,
                &ctx,
                scope_tree,
                &mut collectors,
                &mut lookup_cache,
            );
        }

        for scope_tree in analysis.entity_scope_trees.values() {
            super::check_undeclared_identifiers(
                root,
                &ctx,
                scope_tree,
                &mut collectors,
                &mut lookup_cache,
            );
        }

        collectors.undefined
    }

    /// Convenience: collect all diagnostic messages as lowercase strings for assertion.
    fn diag_messages(diags: &[Diagnostic]) -> Vec<String> {
        diags.iter().map(|d| d.message.to_lowercase()).collect()
    }

    // =========================================================================
    // Happy path: all identifiers declared — no diagnostics
    // =========================================================================

    #[test]
    fn test_no_undeclared_all_signals_declared() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic;
    signal b : std_logic;
begin
    a <= b;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "All signals are declared. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_no_undeclared_signal_used_in_process() {
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
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Signals used in process are declared. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_no_undeclared_constants_and_variables() {
        let code = r#"
architecture rtl of test is
    constant MAX : integer := 100;
    signal counter : integer;
begin
    process
        variable temp : integer;
    begin
        temp := MAX;
        counter <= temp;
    end process;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Constants and variables are declared. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_empty_architecture_no_diagnostics() {
        let code = r#"
architecture rtl of test is
begin
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Empty architecture should have no diagnostics"
        );
    }

    // =========================================================================
    // Basic undeclared identifiers
    // =========================================================================

    #[test]
    fn test_undeclared_signal_on_rhs() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic;
begin
    a <= unknown_signal;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert_eq!(
            diags.len(),
            1,
            "Should detect one undeclared identifier. Got: {:?}",
            diag_messages(&diags)
        );
        assert!(diags[0].message.to_lowercase().contains("unknown_signal"));
    }

    #[test]
    fn test_undeclared_signal_on_lhs() {
        let code = r#"
architecture rtl of test is
    signal b : std_logic;
begin
    undeclared_target <= b;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert_eq!(
            diags.len(),
            1,
            "Should detect undeclared LHS signal. Got: {:?}",
            diag_messages(&diags)
        );
        assert!(
            diags[0]
                .message
                .to_lowercase()
                .contains("undeclared_target")
        );
    }

    #[test]
    fn test_multiple_undeclared_signals() {
        let code = r#"
architecture rtl of test is
begin
    foo <= bar;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.len() >= 2,
            "Should detect at least foo and bar as undeclared. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_undeclared_variable_in_process() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    process(clk)
    begin
        unknown_var := 1;
    end process;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            !diags.is_empty(),
            "Should detect undeclared variable in process. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Entity ports and generics should be visible
    // =========================================================================

    #[test]
    fn test_entity_ports_not_flagged() {
        let code = r#"
entity test is
    port (
        clk : in std_logic;
        data_out : out std_logic
    );
end entity;
architecture rtl of test is
begin
    data_out <= clk;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Entity ports should be visible in architecture. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_entity_generics_not_flagged() {
        let code = r#"
entity test is
    generic (
        WIDTH : integer := 8
    );
end entity;
architecture rtl of test is
    signal internal : integer;
begin
    internal <= WIDTH;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Entity generics should be visible in architecture. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Constants — various usage contexts
    // =========================================================================

    #[test]
    fn test_constant_used_in_assignment_not_flagged() {
        let code = r#"
architecture rtl of test is
    constant MAX : integer := 255;
    signal result : integer;
begin
    result <= MAX;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Constant used in assignment should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_constant_used_in_type_spec_not_flagged() {
        let code = r#"
architecture rtl of test is
    constant WIDTH : integer := 8;
    signal data : std_logic_vector(WIDTH-1 downto 0);
begin
    data <= (others => '0');
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Constant used in type specification should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_constant_used_in_process_not_flagged() {
        let code = r#"
architecture rtl of test is
    constant LIMIT : integer := 100;
    signal counter : integer;
begin
    process
    begin
        if counter < LIMIT then
            counter <= counter + 1;
        end if;
    end process;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Constant used in process should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_constant_used_in_generate_range_not_flagged() {
        let code = r#"
architecture rtl of test is
    constant N : integer := 4;
    signal sigs : std_logic;
begin
    gen: for i in 0 to N-1 generate
    begin
        sigs <= '0';
    end generate;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Constant used in generate range should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_constant_derived_from_other_constant_not_flagged() {
        let code = r#"
architecture rtl of test is
    constant BASE : integer := 10;
    constant DERIVED : integer := BASE * 2;
    signal result : integer;
begin
    result <= DERIVED;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Constants referencing other constants should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Nested scopes: generate, block
    // =========================================================================

    #[test]
    fn test_signal_in_generate_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal arch_sig : std_logic;
begin
    gen: for i in 0 to 3 generate
        signal gen_sig : std_logic;
    begin
        gen_sig <= arch_sig;
    end generate;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Both arch_sig and gen_sig should be visible inside generate. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_undeclared_in_generate() {
        let code = r#"
architecture rtl of test is
begin
    gen: for i in 0 to 3 generate
    begin
        missing_sig <= '1';
    end generate;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            !diags.is_empty(),
            "Should detect undeclared signal inside generate. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_signal_in_block_not_flagged() {
        let code = r#"
architecture rtl of test is
begin
    blk: block is
        signal blk_sig : std_logic;
    begin
        blk_sig <= '1';
    end block;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Block-local signal should be visible inside block. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_deep_nesting_parent_signal_visible() {
        let code = r#"
architecture rtl of test is
    signal top_sig : std_logic;
begin
    gen1: for i in 0 to 3 generate
    begin
        gen2: for j in 0 to 3 generate
        begin
            process
                variable v : std_logic;
            begin
                v := top_sig;
            end process;
        end generate;
    end generate;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Parent architecture signal should be visible in deeply nested scope. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_variable_not_visible_outside_process() {
        let code = r#"
architecture rtl of test is
    signal result : integer;
begin
    process
        variable local_var : integer;
    begin
        local_var := 42;
    end process;
    result <= local_var;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags
                .iter()
                .any(|d| d.message.to_lowercase().contains("local_var")),
            "local_var should not be visible outside its process. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Port map / instantiation — formals should NOT be flagged
    // =========================================================================

    #[test]
    fn test_port_map_signals_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal clk_internal : std_logic;
    signal data_out : std_logic;
begin
    inst: entity work.sub_module
        port map (
            clk => clk_internal,
            output => data_out
        );
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Signals in port maps should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_port_map_formal_names_not_flagged() {
        // The formal names (clk, output) are ports of the instantiated entity,
        // not local declarations. They should NOT be flagged as undeclared.
        let code = r#"
architecture rtl of test is
    signal my_clk : std_logic;
    signal my_data : std_logic;
begin
    inst: entity work.some_entity
        port map (
            clk    => my_clk,
            output => my_data
        );
end architecture;
"#;
        let diags = check_undeclared(code);
        let false_positives: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.message.to_lowercase().contains("clk")
                    || d.message.to_lowercase().contains("output")
            })
            .collect();
        assert!(
            false_positives.is_empty(),
            "Formal port names in port map should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_generic_map_formal_names_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal my_out : std_logic;
begin
    inst: entity work.some_entity
        generic map (
            WIDTH => 8,
            DEPTH => 16
        )
        port map (
            output => my_out
        );
end architecture;
"#;
        let diags = check_undeclared(code);
        let false_positives: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.message.to_lowercase().contains("width")
                    || d.message.to_lowercase().contains("depth")
            })
            .collect();
        assert!(
            false_positives.is_empty(),
            "Formal generic names in generic map should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_component_instantiation_port_map_not_flagged() {
        let code = r#"
architecture rtl of test is
    component my_comp is
        port (
            clk : in std_logic;
            data : out std_logic
        );
    end component;
    signal clk_sig : std_logic;
    signal data_sig : std_logic;
begin
    inst: my_comp
        port map (
            clk  => clk_sig,
            data => data_sig
        );
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Component instantiation port map formals should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Record field access — dot notation should NOT flag the field part
    // =========================================================================

    #[test]
    fn test_record_field_access_not_flagged() {
        // rec.field_a — "field_a" is a suffix in the selected_name and should be skipped
        let code = r#"
architecture rtl of test is
    type t_rec is record
        field_a : std_logic;
        field_b : integer;
    end record;
    signal rec : t_rec;
    signal out_sig : std_logic;
begin
    out_sig <= rec.field_a;
end architecture;
"#;
        let diags = check_undeclared(code);
        let false_positives: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains("field_a"))
            .collect();
        assert!(
            false_positives.is_empty(),
            "Record field in dot notation should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_record_field_write_not_flagged() {
        let code = r#"
architecture rtl of test is
    type t_rec is record
        field_a : std_logic;
        field_b : integer;
    end record;
    signal rec : t_rec;
begin
    rec.field_a <= '1';
    rec.field_b <= 42;
end architecture;
"#;
        let diags = check_undeclared(code);
        let field_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.message.to_lowercase().contains("field_a")
                    || d.message.to_lowercase().contains("field_b")
            })
            .collect();
        assert!(
            field_diags.is_empty(),
            "Record fields in write access should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_record_base_signal_still_resolved() {
        // The base "rec" must be declared — only the suffix after the dot should be skipped
        let code = r#"
architecture rtl of test is
    type t_rec is record
        field_a : std_logic;
    end record;
    signal out_sig : std_logic;
begin
    out_sig <= undeclared_rec.field_a;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags
                .iter()
                .any(|d| d.message.to_lowercase().contains("undeclared_rec")),
            "The base of a record access should still be checked. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_record_field_in_process_not_flagged() {
        let code = r#"
architecture rtl of test is
    type t_data is record
        valid : std_logic;
        value : integer;
    end record;
    signal data : t_data;
    signal result : integer;
begin
    process(data)
    begin
        if data.valid = '1' then
            result <= data.value;
        end if;
    end process;
end architecture;
"#;
        let diags = check_undeclared(code);
        let field_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.message.to_lowercase().contains("valid")
                    || d.message.to_lowercase().contains("value")
            })
            .collect();
        assert!(
            field_diags.is_empty(),
            "Record fields in process should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_nested_record_field_not_flagged() {
        // rec.inner.field — both "inner" and "field" are suffixes
        let code = r#"
architecture rtl of test is
    type t_inner is record
        field : std_logic;
    end record;
    type t_outer is record
        inner : t_inner;
    end record;
    signal rec : t_outer;
    signal out_sig : std_logic;
begin
    out_sig <= rec.inner.field;
end architecture;
"#;
        let diags = check_undeclared(code);
        let field_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.message.to_lowercase().contains("inner")
                    || d.message.to_lowercase().contains("field")
            })
            .collect();
        assert!(
            field_diags.is_empty(),
            "Nested record field suffixes should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Type declarations — type names and enum literals
    // =========================================================================

    #[test]
    fn test_type_name_and_enum_literals_not_flagged() {
        let code = r#"
architecture rtl of test is
    type t_state is (IDLE, RUN, STOP);
    signal state : t_state;
begin
    state <= IDLE;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Type names and enum literals should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Allowlisted built-in types — std_logic etc.
    // =========================================================================

    #[test]
    fn test_builtin_types_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic;
    signal b : std_logic_vector(7 downto 0);
    signal c : integer;
    signal d : boolean;
    signal e : natural;
    signal f : positive;
begin
    a <= '0';
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Built-in types should be allowlisted. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Attribute usage — should not flag attribute identifiers
    // =========================================================================

    #[test]
    fn test_attribute_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal data : std_logic_vector(7 downto 0);
    signal len : integer;
begin
    len <= data'length;
end architecture;
"#;
        let diags = check_undeclared(code);
        let attr_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains("length"))
            .collect();
        assert!(
            attr_diags.is_empty(),
            "Attribute identifiers should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Library prefixes — ieee, std, work
    // =========================================================================

    #[test]
    fn test_library_prefixes_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic;
begin
    a <= '0';
end architecture;
"#;
        let diags = check_undeclared(code);
        let lib_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                let msg = d.message.to_lowercase();
                msg.contains("ieee") || msg.contains("std") || msg.contains("work")
            })
            .collect();
        assert!(
            lib_diags.is_empty(),
            "Library prefixes should be allowlisted. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Mixed scenarios
    // =========================================================================

    #[test]
    fn test_mix_of_declared_and_undeclared() {
        let code = r#"
architecture rtl of test is
    signal declared_sig : std_logic;
begin
    declared_sig <= ghost_signal;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert_eq!(
            diags.len(),
            1,
            "Should flag only ghost_signal. Got: {:?}",
            diag_messages(&diags)
        );
        assert!(diags[0].message.to_lowercase().contains("ghost_signal"));
    }

    #[test]
    fn test_constant_in_block_visible_in_nested_function() {
        let code = r#"
architecture rtl of test is
begin
    blk: block is
        constant MY_CONST : natural := 42;
        signal result : natural;
        function use_const return natural is
        begin
            return MY_CONST;
        end function;
    begin
        result <= use_const;
    end block;
end architecture;
"#;
        let diags = check_undeclared(code);
        let const_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains("my_const"))
            .collect();
        assert!(
            const_diags.is_empty(),
            "Constant in block should be visible in nested function. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Type references in declarations — typos in types should be flagged
    // =========================================================================

    #[test]
    fn test_typo_in_type_name_flagged() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic_vectr;
begin
    a <= (others => '0');
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags
                .iter()
                .any(|d| d.message.to_lowercase().contains("std_logic_vectr")),
            "Typo in type name should be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_valid_type_in_declaration_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic;
    signal b : integer;
begin
    a <= '0';
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Valid types should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_typo_in_constant_type_flagged() {
        let code = r#"
architecture rtl of test is
    constant MAX : integr := 100;
    signal a : std_logic;
begin
    a <= '0';
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags
                .iter()
                .any(|d| d.message.to_lowercase().contains("integr")),
            "Typo in constant type should be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Entity port types — typos in entity port types should be flagged
    // =========================================================================

    #[test]
    fn test_typo_in_entity_port_type_flagged() {
        let code = r#"
entity test is
    port (
        clk : in std_logic;
        data : out std_logic_vectr
    );
end entity;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags
                .iter()
                .any(|d| d.message.to_lowercase().contains("std_logic_vectr")),
            "Typo in entity port type should be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_valid_entity_port_types_not_flagged() {
        let code = r#"
entity test is
    port (
        clk : in std_logic;
        data : out std_logic_vector(7 downto 0);
        count : inout integer
    );
end entity;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Valid entity port types should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_typo_in_entity_generic_type_flagged() {
        let code = r#"
entity test is
    generic (
        WIDTH : integr := 8
    );
end entity;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags
                .iter()
                .any(|d| d.message.to_lowercase().contains("integr")),
            "Typo in entity generic type should be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_multiple_signals_same_declaration_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal a, b, c : std_logic;
begin
    a <= b;
    c <= a;
end architecture;
"#;
        let diags = check_undeclared(code);
        assert!(
            diags.is_empty(),
            "Signals from multi-identifier declaration should all be visible. Got: {:?}",
            diag_messages(&diags)
        );
    }

    // =========================================================================
    // Process for-loop variable should not be flagged
    // =========================================================================

    #[test]
    fn test_protected_type_in_arch_not_flagged() {
        let code = r#"
architecture rtl of test is
    type t_mutex is protected
        procedure lock;
        procedure unlock;
    end protected t_mutex;

    type t_mutex is protected body
        variable locked : boolean := false;
        procedure lock is
        begin
            locked := true;
        end procedure;
        procedure unlock is
        begin
            locked := false;
        end procedure;
    end protected body t_mutex;

    shared variable mtx : t_mutex;
begin
end architecture;
"#;
        let diags = check_undeclared(code);
        let type_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains("t_mutex"))
            .collect();
        assert!(
            type_diags.is_empty(),
            "Protected type should not be flagged as undefined. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_protected_type_from_package_not_flagged() {
        // Protected type declared in a package, used in architecture via shared variable
        let code = r#"
package my_pkg is
    type t_counter is protected
        procedure increment;
        function get_value return integer;
    end protected t_counter;
end package;

package body my_pkg is
    type t_counter is protected body
        variable count : integer := 0;
        procedure increment is
        begin
            count := count + 1;
        end procedure;
        function get_value return integer is
        begin
            return count;
        end function;
    end protected body t_counter;
end package body;
"#;
        let diags = check_undeclared(code);
        let type_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains("t_counter"))
            .collect();
        assert!(
            type_diags.is_empty(),
            "Protected type from package should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_protected_type_cross_file_not_flagged() {
        // Simulates two files: package in one, architecture in another
        let pkg_code = r#"
package my_pkg is
    type t_counter is protected
        procedure increment;
        function get_value return integer;
    end protected t_counter;
end package;
"#;
        let arch_code = r#"
library ieee;
use work.my_pkg.all;

architecture rtl of test is
    shared variable counter : t_counter;
begin
end architecture;
"#;
        let pkg_uri = Url::parse("file:///pkg.vhd").unwrap();
        let arch_uri = Url::parse("file:///arch.vhd").unwrap();

        let pkg_tree = parse_text(pkg_code);
        let pkg_analysis = crate::backend::syntax::parser::extract_document_symbols(
            pkg_code,
            pkg_tree.root_node(),
        );

        let arch_tree = parse_text(arch_code);
        let arch_root = arch_tree.root_node();
        let arch_analysis =
            crate::backend::syntax::parser::extract_document_symbols(arch_code, arch_root);

        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(pkg_uri.clone(), pkg_analysis);
        analysis_map.insert(arch_uri.clone(), arch_analysis.clone());

        let mut collectors = crate::backend::features::diagnostics::DiagnosticCollectors::new();
        let mut lookup_cache = HashMap::new();

        let ctx = super::super::DiagnosticContext {
            text: arch_code,
            scope_tree: None,
            analysis: &arch_analysis,
            global_map: &analysis_map,
            current_uri: &arch_uri,
            ignored_patterns: &[],
        };

        for scope_tree in &arch_analysis.scope_trees {
            super::check_undeclared_identifiers(
                arch_root,
                &ctx,
                scope_tree,
                &mut collectors,
                &mut lookup_cache,
            );
        }

        let diags = collectors.undefined;
        let type_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains("t_counter"))
            .collect();
        assert!(
            type_diags.is_empty(),
            "Protected type imported from another file should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_shared_variable_not_flagged() {
        let code = r#"
architecture rtl of test is
    shared variable sv : integer := 0;
begin
    process
    begin
        sv := sv + 1;
    end process;
end architecture;
"#;
        let diags = check_undeclared(code);
        let sv_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains(" sv"))
            .collect();
        assert!(
            sv_diags.is_empty(),
            "Shared variable should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    /// Test that uses the full production diagnostic pipeline (collect_all_diagnostics)
    /// for cross-file protected type resolution, matching the real LSP path.
    #[test]
    fn test_protected_type_cross_file_production_pipeline() {
        let pkg_code = r#"
package my_pkg is
    type t_counter is protected
        procedure increment;
        function get_value return integer;
    end protected t_counter;
end package;
"#;
        let arch_code = r#"
use work.my_pkg.all;

entity test is
end entity;

architecture rtl of test is
    shared variable counter : t_counter;
begin
end architecture;
"#;
        let pkg_uri = Url::parse("file:///pkg.vhd").unwrap();
        let arch_uri = Url::parse("file:///arch.vhd").unwrap();

        let pkg_tree = parse_text(pkg_code);
        let pkg_analysis = crate::backend::syntax::parser::extract_document_symbols(
            pkg_code,
            pkg_tree.root_node(),
        );

        let arch_tree = parse_text(arch_code);
        let arch_root = arch_tree.root_node();
        let arch_analysis =
            crate::backend::syntax::parser::extract_document_symbols(arch_code, arch_root);

        // Verify the package scope tree has the protected type
        assert!(
            pkg_analysis.package_declaration_scope_trees.contains_key("my_pkg"),
            "Package scope tree should exist. Keys: {:?}",
            pkg_analysis.package_declaration_scope_trees.keys().collect::<Vec<_>>()
        );
        let pkg_scope = pkg_analysis.package_declaration_scope_trees.get("my_pkg").unwrap();
        let has_t_counter = pkg_scope
            .declarations
            .iter()
            .any(|d| d.name.eq_ignore_ascii_case("t_counter"));
        assert!(
            has_t_counter,
            "Package scope should contain t_counter. Declarations: {:?}",
            pkg_scope
                .declarations
                .iter()
                .map(|d| &d.name)
                .collect::<Vec<_>>()
        );

        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(pkg_uri.clone(), pkg_analysis);
        analysis_map.insert(arch_uri.clone(), arch_analysis.clone());

        // Use the FULL production pipeline
        let diags = crate::backend::features::diagnostics::collect_all_diagnostics(
            arch_root,
            &arch_analysis,
            arch_code,
            &analysis_map,
            &arch_uri,
            &OxideConfig::default(),
        );

        let type_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains("t_counter"))
            .collect();
        assert!(
            type_diags.is_empty(),
            "Protected type from package should not be flagged via production pipeline. Got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    /// Test with the EXACT production package content (forward declarations,
    /// access types, records, protected type with many methods, standalone procedure).
    #[test]
    fn test_complex_protected_type_cross_file_not_flagged() {
        let pkg_code = r#"
package sim_fifo_pkg is

    type sim_fifo_pkt;
    type sim_fifo_pkt_ptr is access sim_fifo_pkt;

    type sim_fifo_pkt is
    record
        data: integer;
        error : boolean;
        nxt : sim_fifo_pkt_ptr;
    end record;

    type sim_fifo_desc is
    record
        fifo_cnt      : natural;
        fifo_ptr      : sim_fifo_pkt_ptr;
        fifo_byte     : natural;
    end record;

    type sim_fifo_desc_array is array(natural range<>) of sim_fifo_desc;
    type sim_fifo_desc_array_ptr is access sim_fifo_desc_array;

    type sim_fifo is protected
        procedure realloc(len : natural);
        procedure flush(verbose: boolean := false);
        procedure push_id(id: natural; data : integer ; error : boolean := false);
        procedure push_id(id: natural; file_path : string);
        impure function get_pop_len_id (id: natural) return natural;
        impure function pop_id (id: natural) return integer;
        impure function peek_id (id: natural) return integer;
        impure function peek_error_state_id (id: natural) return boolean;
        impure function fifo_depth_id (id: natural) return natural;
        impure function fifo_amount_byte_id (id: natural) return natural;
        procedure push(data : integer; error : boolean := false);
        procedure push(file_path : string);
        procedure push(file_path : string; file_id_path : string);
        impure function get_pop_len return natural;
        impure function pop return integer;
        impure function peek return integer;
        impure function fifo_depth return natural;
        impure function fifo_length return natural;
        procedure print_id(id: natural ; print_error : boolean := false);
        procedure print(print_error : boolean := false);
        impure function merge_id (id: natural) return integer;
        impure function merge_error_state_id (id: natural) return boolean;
        impure function merge return integer;
    end protected;

    procedure duplicate_sim_fifo(variable sim_fifo_source    : inout sim_fifo;
                                 variable sim_fifo_dest      : inout sim_fifo);

end package;
"#;
        // NOTE: Uses the exact typo from broken.vhd: "end architecutre;"
        let arch_code = r#"
use work.sim_fifo_pkg.all;

entity test is
    generic (
        WIDTH : integer := 8;
        DEPTH : positive := 1024
    );
end entity;

architecture rtl of test is
    shared variable sim_fifo_a: sim_fifo;
begin
end architecutre;
"#;
        let pkg_uri = Url::parse("file:///my_pkg.vhd").unwrap();
        let arch_uri = Url::parse("file:///broken.vhd").unwrap();

        let pkg_tree = parse_text(pkg_code);
        let pkg_analysis = crate::backend::syntax::parser::extract_document_symbols(
            pkg_code,
            pkg_tree.root_node(),
        );

        let arch_tree = parse_text(arch_code);
        let arch_root = arch_tree.root_node();
        let arch_analysis =
            crate::backend::syntax::parser::extract_document_symbols(arch_code, arch_root);

        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(pkg_uri.clone(), pkg_analysis);
        analysis_map.insert(arch_uri.clone(), arch_analysis.clone());

        let diags = crate::backend::features::diagnostics::collect_all_diagnostics(
            arch_root,
            &arch_analysis,
            arch_code,
            &analysis_map,
            &arch_uri,
            &OxideConfig::default(),
        );

        let type_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.message
                    .to_lowercase()
                    .contains("undefined type 'sim_fifo'")
            })
            .collect();
        assert!(
            type_diags.is_empty(),
            "Protected type sim_fifo from package should not be flagged. Got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    /// Test: what happens when the package is only SHALLOW-indexed (not deep-parsed)?
    /// This simulates the case where workspace indexing ran but JIT parsing didn't happen.
    #[test]
    fn test_shallow_indexed_package_resolution() {
        let pkg_code = r#"
package sim_fifo_pkg is
    type sim_fifo is protected
        procedure increment;
    end protected;
end package;
"#;
        let arch_code = r#"
use work.sim_fifo_pkg.all;

entity test is
end entity;

architecture rtl of test is
    shared variable my_var: sim_fifo;
begin
end architecture;
"#;
        let pkg_uri = Url::parse("file:///my_pkg.vhd").unwrap();
        let arch_uri = Url::parse("file:///broken.vhd").unwrap();

        // Shallow-index the package (mimicking scan_fast + index_workspace)
        let shallow_symbols = crate::backend::syntax::scanner::scan_fast(pkg_code);
        let mut shallow_analysis = crate::analysis::Analysis::new();
        shallow_analysis.parse_level = crate::analysis::ParseLevel::Shallow;
        for s in shallow_symbols {
            shallow_analysis
                .symbols
                .insert(s.name.clone().to_lowercase(), s);
        }

        // Deep-parse the current file
        let arch_tree = parse_text(arch_code);
        let arch_root = arch_tree.root_node();
        let arch_analysis =
            crate::backend::syntax::parser::extract_document_symbols(arch_code, arch_root);

        // Put both in the map (package is shallow, current file is deep)
        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(pkg_uri.clone(), shallow_analysis);
        analysis_map.insert(arch_uri.clone(), arch_analysis.clone());

        let diags = crate::backend::features::diagnostics::collect_all_diagnostics(
            arch_root,
            &arch_analysis,
            arch_code,
            &analysis_map,
            &arch_uri,
            &OxideConfig::default(),
        );

        // Check: sim_fifo should ideally NOT be flagged.
        // If it IS flagged, the shallow fallback path is broken.
        let type_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains("undefined type"))
            .collect();

        assert!(
            type_diags.is_empty(),
            "Types from shallow-indexed packages should resolve via fallback. Got: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_record_aggregate_constant_fields_not_flagged() {
        let code = r#"
architecture rtl of test is
    type my_rec is record
        field_a : integer;
        field_b : boolean;
    end record;
    constant MY_CONST : my_rec := (field_a => 42, field_b => true);
    signal s : integer;
begin
    s <= MY_CONST.field_a;
end architecture;
"#;
        let diags = check_undeclared(code);
        let field_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.message.to_lowercase().contains("field_a")
                    || d.message.to_lowercase().contains("field_b")
            })
            .collect();
        assert!(
            field_diags.is_empty(),
            "Record aggregate field names should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_for_loop_variable_in_process_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal data : std_logic_vector(7 downto 0);
begin
    process
    begin
        for i in 0 to 7 loop
            data(i) <= '0';
        end loop;
    end process;
end architecture;
"#;
        let diags = check_undeclared(code);
        let loop_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains(" i"))
            .collect();
        assert!(
            loop_diags.is_empty(),
            "For-loop variable 'i' should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }
    #[test]
    fn test_for_loop_variable_rhs_in_process_not_flagged() {
        let code = r#"
architecture rtl of toto is
    signal some_data: std_logic_vector(8-1 downto 0);
    signal signal_b: std_logic;
begin
    p_proc : process(some_data,signal_b)
    begin
        if (signal_b = '1') then
            for i in 8/8-1 downto 0 loop
                if (some_data((i+1)*8-1 downto 8*i)/=X"55") then
                end if;
            end loop;
        end if;
    end process;
end architecture;

"#;
        let diags = check_undeclared(code);
        let loop_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.to_lowercase().contains(" i"))
            .collect();
        assert!(
            loop_diags.is_empty(),
            "For-loop variable 'i' should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_package_header_and_body_in_same_file_visibility() {
        let pkg_code = r#"
package my_pkg is
    constant C_PKG_CONST : integer := 42;
    function calc(x : integer) return integer;
end package;

package body my_pkg is
    function calc(x : integer) return integer is
    begin
        return x + C_PKG_CONST;
    end function;
end package body;
"#;
        let arch_code = r#"
use work.my_pkg.all;
architecture rtl of top is
    signal s : integer := calc(10);
begin
end architecture;
"#;
        let pkg_uri = Url::parse("file:///pkg.vhd").unwrap();
        let arch_uri = Url::parse("file:///top.vhd").unwrap();

        let pkg_tree = parse_text(pkg_code);
        let pkg_analysis = crate::backend::syntax::parser::extract_document_symbols(
            pkg_code,
            pkg_tree.root_node(),
        );

        let arch_tree = parse_text(arch_code);
        let arch_root = arch_tree.root_node();
        let arch_analysis =
            crate::backend::syntax::parser::extract_document_symbols(arch_code, arch_root);

        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(pkg_uri.clone(), pkg_analysis);
        analysis_map.insert(arch_uri.clone(), arch_analysis.clone());

        let mut collectors = crate::backend::features::diagnostics::DiagnosticCollectors::new();
        let mut lookup_cache = HashMap::new();

        let ctx = super::super::DiagnosticContext {
            text: arch_code,
            scope_tree: None,
            analysis: &arch_analysis,
            global_map: &analysis_map,
            current_uri: &arch_uri,
            ignored_patterns: &[],
        };

        for scope_tree in &arch_analysis.scope_trees {
            super::check_undeclared_identifiers(
                arch_root,
                &ctx,
                scope_tree,
                &mut collectors,
                &mut lookup_cache,
            );
        }

        assert!(
            collectors.undefined.is_empty(),
            "C_PKG_CONST should be visible from the package header. Got: {:?}",
            diag_messages(&collectors.undefined)
        );
    }

    #[test]
    fn test_loop_variable_visibility_in_nested_case_if() {
        let code = r#"
architecture rtl of top is
    type t_state is (S0, S1);
    signal r_state : t_state;
    signal w_val : integer;
    signal w_data : std_logic_vector(7 downto 0);
begin
    process(r_state, w_val)
    begin
        case r_state is
            when S0 =>
                if w_val > 0 then
                    for i in 0 to 7 loop
                        if w_val = i then
                            w_data(i) <= '0';
                        end if;
                    end loop;
                end if;
            when others =>
                null;
        end case;
    end process;
end architecture;
"#;
        let diags = check_undeclared(code);
        let i_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains(" i"))
            .collect();
        assert!(
            i_diags.is_empty(),
            "Loop variable 'i' should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }

    #[test]
    fn test_byte_remainder_regression() {
        let code = r#"
architecture rtl of top is
    constant MAC2TCP_DATA_WIDTH : integer := 64;
    signal r_padd_to_0 : integer;
    signal w_tcp_ip_hdr_and_payload_out_data : std_logic_vector(63 downto 0);
    type t_state is (S_IDLE, S_BLANK);
    signal r_blank_state : t_state;
begin
    process(r_blank_state, r_padd_to_0)
    begin
        case r_blank_state is
            when S_IDLE =>
                for byte_remainder in 0 to MAC2TCP_DATA_WIDTH/8 - 1 loop
                    if r_padd_to_0 = byte_remainder then
                        w_tcp_ip_hdr_and_payload_out_data(byte_remainder*8 -1 downto 0) <= (others=>'0');
                    end if;
                end loop;
            when others =>
                null;
        end case;
    end process;
end architecture;
"#;
        let diags = check_undeclared(code);
        let rem_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("byte_remainder"))
            .collect();
        assert!(
            rem_diags.is_empty(),
            "Loop variable 'byte_remainder' should not be flagged. Got: {:?}",
            diag_messages(&diags)
        );
    }
}
