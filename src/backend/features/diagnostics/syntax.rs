//! Syntax validation checks for VHDL code.
//!
//! This module contains individual checking functions for various VHDL constructs,
//! including signal declarations, port declarations, label validation, semicolon
//! placement, and parenthesis matching.

use crate::{
    backend::features::diagnostics::{DiagnosticCollectors, DiagnosticMessage, messages},
    utils::ast::find_child,
};
use tree_sitter::Node;

/// Recursively searches for a descendant node of a specific kind.
///
/// Performs a depth-first search through the node's subtree to find any
/// descendant (child, grandchild, etc.) matching the specified kind.
///
/// # Arguments
///
/// * `node` - The Tree-sitter node to search from
/// * `kind` - The node kind string to search for (e.g., "mode", "identifier")
///
/// # Returns
///
/// `true` if a matching descendant is found, `false` otherwise.
///
/// # Examples
///
/// ```ignore
/// // Check if an interface_declaration contains a mode child
/// if has_descendant_of_kind(port_node, "mode") {
///     // Port has direction specified
/// }
/// ```
pub fn has_descendant_of_kind(node: Node, kind: &str) -> bool {
    if node.kind() == kind {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_descendant_of_kind(child, kind) {
            return true;
        }
    }
    false
}

/// Reports a generic syntax error for an ERROR node.
///
/// Called when Tree-sitter encounters unparseable syntax and creates an ERROR node.
/// This represents syntax that couldn't be interpreted according to the VHDL grammar.
///
/// # Arguments
///
/// * `node` - The ERROR node from the Tree-sitter AST
/// * `collectors` - Mutable reference to diagnostic collectors
pub fn check_syntax_error(node: Node, collectors: &mut DiagnosticCollectors) {
    collectors.syntax.push(messages::syntax_error(
        node,
        DiagnosticMessage::SyntaxError,
        false,
        "",
    ));
}

/// Validates that a signal declaration has a type specification.
///
/// Checks for the presence of a `subtype_indication` child node that is non-empty.
/// Tree-sitter may create zero-width placeholder nodes when recovering from errors,
/// so we verify the node has actual content.
///
/// # Arguments
///
/// * `node` - The `signal_declaration` node to validate
/// * `collectors` - Mutable reference to diagnostic collectors
///
/// # Detected Errors
///
/// - `SignalMissingType` - Signal declared without type (e.g., `signal foo : ;`)
///
/// # Examples
///
/// ```vhdl
/// signal foo : ;              -- ERROR: missing type
/// signal bar : std_logic;     -- OK
/// ```
pub fn check_signal_declaration(node: Node, collectors: &mut DiagnosticCollectors) {
    let has_valid_type = find_child(node, "subtype_indication")
        .map(|type_node| type_node.end_position() != type_node.start_position())
        .unwrap_or(false);

    if !has_valid_type {
        collectors.syntax.push(messages::syntax_error(
            node,
            DiagnosticMessage::SignalMissingType,
            false,
            "",
        ));
    }
}

/// Validates that a port declaration has a direction specified.
///
/// Searches the port declaration's descendants for a `mode` node, which represents
/// the direction keyword (in, out, inout, buffer). Uses recursive search because
/// the mode is nested inside a `simple_mode_indication` child.
///
/// # Arguments
///
/// * `node` - The `interface_declaration` node to validate
/// * `collectors` - Mutable reference to diagnostic collectors
///
/// # Detected Errors
///
/// - `PortMissingDirection` - Port declared without direction
///
/// # Examples
///
/// ```vhdl
/// port (
///     clk : std_logic        -- ERROR: missing direction
/// );
/// port (
///     clk : in std_logic     -- OK
/// );
/// ```
pub fn check_port_declaration(node: Node, collectors: &mut DiagnosticCollectors) {
    // Try to find the simple_mode_indication that should have the direction (mode)
    // We need to figure out if we are in a port clause

    if is_in_port_clause(node) {
        let has_direction = has_descendant_of_kind(node, "mode");
        if !has_direction {
            collectors.syntax.push(messages::syntax_error(
                node,
                DiagnosticMessage::PortMissingDirection,
                false,
                "",
            ));
        }
    }
}

/// Check if the current node is in a port_clause or generic_clause
///
/// # Arguments
///
/// * `node` - The node to validate
///
/// # Return
/// true if in a port clause, false otherwise
fn is_in_port_clause(node: Node) -> bool {
    let mut current = node.parent();

    while let Some(parent) = current {
        match parent.kind() {
            "port_clause" => return true,
            "generic_clause" => return false,
            "interface_list" => {
                current = parent.parent();
            }
            _ => {
                current = parent.parent();
            }
        }
    }
    false
}

/// Validates that a label is attached to a valid construct.
///
/// In VHDL, labels can only appear on specific statements like processes, generates,
/// blocks, loops, and component instantiations. This check ensures labels aren't
/// orphaned or attached to invalid constructs.
///
/// Skips validation if the parent node has errors, as broken syntax may create
/// misleading label nodes.
///
/// # Arguments
///
/// * `node` - The `label_declaration` node to validate
/// * `collectors` - Mutable reference to diagnostic collectors
///
/// # Detected Errors
///
/// - `InvalidLabelStatement` - Label not attached to a valid construct
///
/// # Valid Parent Constructs
///
/// - `process_statement`
/// - `if_generate_statement`, `for_generate_statement`
/// - `block_statement`
/// - `loop_statement`
/// - `component_instantiation_statement`
///
/// # Examples
///
/// ```vhdl
/// my_label: process(clk)     -- OK
/// begin
/// end process;
///
/// bad_label: variable x;     -- ERROR: invalid label target
/// ```
pub fn check_label_has_valid_parent(node: Node, collectors: &mut DiagnosticCollectors) {
    if let Some(parent) = node.parent() {
        if parent.has_error() {
            return;
        }
        match parent.kind() {
            "process_statement"
            | "if_generate_statement"
            | "block_statement"
            | "loop_statement"
            | "component_instantiation_statement"
            | "for_generate_statement" => {}
            _ => collectors.syntax.push(messages::syntax_error(
                node,
                DiagnosticMessage::InvalidLabelStatement,
                false,
                "",
            )),
        }
    } else {
        collectors.syntax.push(messages::syntax_error(
            node,
            DiagnosticMessage::InvalidLabelStatement,
            false,
            "",
        ));
    }
}

/// Validates that a construct ends with a semicolon.
///
/// Checks the actual text content of the node (trimmed of trailing whitespace)
/// to see if it ends with `;`. Places the diagnostic at the end of the node's
/// content, where the semicolon should appear.
///
/// # Arguments
///
/// * `node` - The Tree-sitter node to validate
/// * `text` - The full source text (needed to extract node content)
/// * `collectors` - Mutable reference to diagnostic collectors
/// * `message` - The specific diagnostic message to use if semicolon is missing
///
/// # Detected Errors
///
/// - `MissingSemicolon` - Generic missing semicolon
/// - `MissingSemiColonAfterPort` - Missing semicolon after port clause
/// - `MissingSemicolonAfterInstance` - Missing semicolon after instantiation
///
/// # Examples
///
/// ```vhdl
/// port (clk : in std_logic)      -- ERROR: missing semicolon
/// port (clk : in std_logic);     -- OK
/// ```
pub fn check_end_with_semicolon(
    node: Node,
    text: &str,
    collectors: &mut DiagnosticCollectors,
    message: DiagnosticMessage,
) {
    let node_text = &text[node.byte_range()];
    if !node_text.trim_end().ends_with(';') {
        collectors
            .syntax
            .push(messages::syntax_error(node, message, true, text));
    }
}

/// Validates that a process sensitivity specification has matching parentheses.
///
/// Checks that the sensitivity specification starts with `(` and ends with `)`.
/// Tree-sitter may create zero-width `)` placeholder nodes when the closing
/// parenthesis is missing, which this check detects.
///
/// # Arguments
///
/// * `node` - The `sensitivity_specification` node to validate
/// * `text` - The full source text
/// * `collectors` - Mutable reference to diagnostic collectors
///
/// # Detected Errors
///
/// - `UnmatchedParentheses` - Missing closing parenthesis
///
/// # Examples
///
/// ```vhdl
/// process(clk              -- ERROR: missing )
/// begin
/// end process;
///
/// process(clk)             -- OK
/// begin
/// end process;
/// ```
pub fn check_sensitivity_parens(node: Node, text: &str, collectors: &mut DiagnosticCollectors) {
    let node_text = &text[node.byte_range()];

    // Should start with '(' and end with ')'
    let trimmed = node_text.trim();
    if !trimmed.starts_with('(') || !trimmed.ends_with(')') {
        collectors.syntax.push(messages::syntax_error(
            node,
            DiagnosticMessage::UnmatchedParentheses,
            true,
            text,
        ));
    }
}

/// Validates that an association list has matching parentheses.
///
/// Checks port map and generic map association lists to ensure they are properly
/// enclosed in parentheses. Similar to sensitivity list validation, detects
/// zero-width placeholder nodes created by Tree-sitter's error recovery.
///
/// # Arguments
///
/// * `node` - The `association_list` node to validate
/// * `text` - The full source text
/// * `collectors` - Mutable reference to diagnostic collectors
///
/// # Detected Errors
///
/// - `UnmatchedParentheses` - Missing opening or closing parenthesis
///
/// # Examples
///
/// ```vhdl
/// port map (
///     clk => clk,
///     data => data
/// ;                        -- ERROR: missing )
///
/// port map (
///     clk => clk
/// );                       -- OK
/// ```
pub fn check_association_list_parens(
    node: Node,
    text: &str,
    collectors: &mut DiagnosticCollectors,
) {
    let node_text = &text[node.byte_range()];

    // Should start with '(' and end with ')'
    let trimmed = node_text.trim();
    if !trimmed.starts_with('(') || !trimmed.ends_with(')') {
        collectors.syntax.push(messages::syntax_error(
            node,
            DiagnosticMessage::UnmatchedParentheses,
            true,
            text,
        ));
    }
}

#[cfg(test)]
mod tests {
    //! Comprehensive test suite for syntax validation.
    //!
    //! Tests cover various categories of syntax errors including:
    //! - Missing punctuation (semicolons, colons, parentheses)
    //! - Missing keywords (is, of, begin, then, loop)
    //! - Invalid declarations (missing types, missing directions)
    //! - Typos and invalid identifiers
    //! - Unmatched delimiters
    //!
    //! Each test verifies both that an error is detected and that the
    //! correct error message is generated.

    use super::*;
    use crate::backend::features::diagnostics::DiagnosticMessage;
    use crate::backend::test_utils::parse_text;
    use tower_lsp::lsp_types::Diagnostic;
    use tower_lsp::lsp_types::Url;

    /// Recursively prints the AST structure for debugging.
    ///
    /// Useful for understanding how Tree-sitter parses broken VHDL syntax.
    #[allow(dead_code)]
    fn print_ast(node: Node, text: &str, indent: usize) {
        let indent_str = "  ".repeat(indent);
        let node_text = &text[node.byte_range()];
        let preview = if node_text.len() > 50 {
            format!("{}...", &node_text[..50])
        } else {
            node_text.to_string()
        };

        eprintln!(
            "{}{} [{}, {}] - [{}, {}] {:?}",
            indent_str,
            node.kind(),
            node.start_position().row,
            node.start_position().column,
            node.end_position().row,
            node.end_position().column,
            preview.replace('\n', "\\n")
        );

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            print_ast(child, text, indent + 1);
        }
    }

    fn check_syntax_errors(code: &str) -> Vec<Diagnostic> {
        let tree = parse_text(code);
        let root = tree.root_node();
        let dummy_uri = Url::parse("file:///test.vhd").unwrap();

        let analysis = crate::backend::syntax::parser::extract_document_symbols(code, root);
        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(dummy_uri.clone(), analysis.clone());

        let all_diags =
            super::super::collect_all_diagnostics(root, &analysis, code, &analysis_map, &dummy_uri);

        // DEBUG: Print what we got
        eprintln!("\n=== Diagnostics returned: {} ===", all_diags.len());
        for (i, d) in all_diags.iter().enumerate() {
            eprintln!("  [{}] {} at line {}", i, d.message, d.range.start.line);
        }
        eprintln!("=== End Diagnostics ===\n");

        all_diags
    }

    #[test]
    fn test_triple_colon_detected() {
        let code = r#"
entity test1 is
    port (
        clk ::: out std_logic
    );
end entity;
"#;
        let diags = check_syntax_errors(code);

        assert!(!diags.is_empty(), "Should detect triple colon syntax error");
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    // ... rest of tests unchanged ...

    #[test]
    fn test_missing_semicolon_after_port() {
        let code = r#"
entity test2 is
    port (
        clk : in std_logic
    )
end entity;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect missing semicolon after port clause"
        );
        assert_eq!(
            diags[0].message,
            DiagnosticMessage::MissingSemiColonAfterPort.to_string()
        );
    }

    #[test]
    fn test_missing_colon_in_signal() {
        let code = r#"
architecture rtl of test is
    signal foo bar;
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect missing colon in signal declaration"
        );
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_closing_paren() {
        let code = r#"
architecture rtl of test is
begin
    process(clk
    begin
        foo <= '1';
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);

        assert!(
            !diags.is_empty(),
            "Should detect missing closing parenthesis"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::UnmatchedParentheses.to_string()),
            "Should contain unmatched parentheses diagnostic"
        );
    }

    #[test]
    fn test_wrong_keyword_signal() {
        let code = r#"
architecture rtl of test is
    signl toto: std_logic;
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect signal typo");
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_wrong_keyword_generate() {
        let code = r#"
architecture rtl of test is
    signal toto: std_logic;
begin
    g: geneate
    begin
    end generate;
end architecture;
"#;
        let diags = check_syntax_errors(code);

        assert!(!diags.is_empty(), "Should detect generate typo");
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_missing_semicolon_after_instance() {
        let code = r#"
architecture rtl of test is
begin
    inst_toto: toto
    port map ()
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect missing semicolon after instance"
        );
        // NOTE: The tree is messed with with the missing semi colon and we do
        // not properly detect the port map and instance resulting in a bad
        // label detection
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_end_architecture() {
        let code = r#"
architecture rtl of test is
begin
    inst_toto: toto()
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing end architecture");
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_end_if() {
        let code = r#"
architecture rtl of test is
begin
    p_process: process(clk)
        if (toto = '1') then
            a <= 1;
        else a <= 2
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing end if");
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_comas_in_function_call() {
        let code = r#"
architecture rtl of test is
begin
    p_process: process(clk)
        if (toto = '1') then
            a <= 1;
        else 
            a <= bool_to_int(tata toto);
        end if;
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect missing comma in function call"
        );
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_assignment_operator() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        foo '1';
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect missing assignment operator"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_wrong_comparison_operator() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        if foo == '1' then
            bar <= '0';
        end if;
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect wrong comparison operator");
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_missing_is_after_entity() {
        let code = r#"
entity test
    port (clk : in std_logic);
end entity;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing 'is' after entity");
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_of_in_architecture() {
        let code = r#"
architecture rtl test is
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing 'of'");
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_begin_in_process() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
        foo <= '1';
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing 'begin'");
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_type_in_signal() {
        let code = r#"
architecture rtl of test is
    signal foo : ;
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing type");
        assert_eq!(
            diags[0].message,
            DiagnosticMessage::SignalMissingType.to_string()
        );
    }

    #[test]
    fn test_missing_type_in_signal_no_colon() {
        let code = r#"
architecture rtl of test is
    signal foo  ;
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing type");
        // This creates ERROR node, so generic syntax error
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_missing_direction_in_port() {
        let code = r#"
entity test is
    port (
        clk : std_logic
    );
end entity;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing port direction");
        assert_eq!(
            diags[0].message,
            DiagnosticMessage::PortMissingDirection.to_string()
        );
    }

    #[test]
    fn test_good_port_declaration() {
        let code = r#"
entity test is
    port (
        clk : in std_logic  
    );
end entity;
"#;
        let diags = check_syntax_errors(code);
        assert!(diags.is_empty(), "Should not detect any port error");
    }

    #[test]
    fn test_unclosed_string() {
        let code = r#"
architecture rtl of test is
    constant msg : string := "hello world;
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect unclosed string");
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_unclosed_character() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        foo <= '1;
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect unclosed character literal"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_missing_then_after_if() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        if foo = '1'
            bar <= '0';
        end if;
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing 'then' after if");
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_missing_loop_after_for() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        for i in 0 to 10
            foo(i) <= '0';
        end loop;
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(!diags.is_empty(), "Should detect missing 'loop' after for");
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_missing_loop_after_while() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        while foo = '1'
            bar <= '0';
        end loop;
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect missing 'loop' after while"
        );
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }

    #[test]
    fn test_unmatched_opening_paren_in_expression() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        result <= (a + b * c;
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect unmatched opening parenthesis"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_unmatched_closing_paren_in_expression() {
        let code = r#"
architecture rtl of test is
begin
    process(clk)
    begin
        result <= a + b) * c;
    end process;
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect unmatched closing parenthesis"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_unmatched_parens_in_port_map() {
        let code = r#"
architecture rtl of test is
begin
    inst: entity work.foo
        port map (
            clk => clk,
            data => data
        ;  -- Missing opening paren
end architecture;
"#;
        let diags = check_syntax_errors(code);

        assert!(
            !diags.is_empty(),
            "Should detect unmatched parentheses in port map"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::UnmatchedParentheses.to_string()),
            "Should contain unmatched parentheses diagnostic"
        );
    }

    #[test]
    fn test_unmatched_parens_in_generic_map() {
        let code = r#"
architecture rtl of test is
begin
    inst: entity work.foo
        generic map (
            DATA_WIDTH => 64
          -- Missing closing paren
        port map (
            clk => clk,
            data => data
        );

end architecture;
"#;
        let diags = check_syntax_errors(code);

        assert!(
            !diags.is_empty(),
            "Should detect unmatched parentheses in generic map"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::UnmatchedParentheses.to_string()),
            "Should contain unmatched parentheses diagnostic"
        );
    }

    #[test]
    fn test_identifier_starts_with_number() {
        let code = r#"
architecture rtl of test is
    signal 1foo : std_logic;
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect identifier starting with number"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_identifier_with_special_chars() {
        let code = r#"
architecture rtl of test is
    signal foo@bar : std_logic;
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect identifier with invalid characters"
        );
        assert!(
            diags.iter().any(|d| d.message == DiagnosticMessage::SyntaxError.to_string()),
            "Should contain syntax error diagnostic"
        );
    }

    #[test]
    fn test_identifier_reserved_keyword() {
        let code = r#"
architecture rtl of test is
    signal signal : std_logic;
begin
end architecture;
"#;
        let diags = check_syntax_errors(code);
        assert!(
            !diags.is_empty(),
            "Should detect reserved keyword as identifier"
        );
        assert_eq!(diags[0].message, DiagnosticMessage::SyntaxError.to_string());
    }
}
