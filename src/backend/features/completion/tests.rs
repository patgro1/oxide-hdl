use super::*;
use crate::backend::test_utils::SHARED_PARSER_LOCK;
use tree_sitter::Parser;

// --- Test Helpers ---

/// Parses code with cursor marker and checks completion context.
fn check_context(code_with_cursor: &str, expected: CompletionContext) {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let (code, pos) = extract_cursor(code_with_cursor);

    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    let ctx = get_completion_context(&code, tree.root_node(), pos);
    assert_eq!(ctx, expected, "\nCode:\n{}\nContext mismatch!", code);
}

/// Extracts cursor position '|' and returns clean code and Position.
fn extract_cursor(text: &str) -> (String, Position) {
    let cursor_offset = text
        .find('|')
        .expect("Test case must have a '|' cursor marker");
    let clean_text = text.replace("|", "");

    let mut line = 0;
    let mut character = 0;
    for (i, c) in text.char_indices() {
        if i == cursor_offset {
            break;
        }
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }
    (clean_text, Position { line, character })
}

// --- Unit Tests for Helper Functions ---

#[test]
fn test_is_rhs_of_association() {
    // After arrow
    assert!(is_rhs_of_association("port map (clk => ", 17));
    assert!(is_rhs_of_association("(a => b, c => ", 14));

    // Before arrow
    assert!(!is_rhs_of_association("port map (clk ", 14));
    assert!(!is_rhs_of_association("(a => b, c ", 11));

    // After comma resets
    assert!(!is_rhs_of_association("(a => b, c", 10));

    // Multiple associations
    assert!(is_rhs_of_association("(a => 1, b => 2, c => ", 22));
    assert!(!is_rhs_of_association("(a => 1, b => 2, c ", 19));
}

#[test]
fn test_build_map_context() {
    assert_eq!(
        build_map_context("comp".to_string(), DetectedMapKind::Port, false),
        CompletionContext::PortMapLhs("comp".to_string())
    );
    assert_eq!(
        build_map_context("comp".to_string(), DetectedMapKind::Port, true),
        CompletionContext::PortMapRhs
    );
    assert_eq!(
        build_map_context("comp".to_string(), DetectedMapKind::Generic, false),
        CompletionContext::GenericMapLhs("comp".to_string())
    );
    assert_eq!(
        build_map_context("comp".to_string(), DetectedMapKind::Generic, true),
        CompletionContext::GenericMapRhs
    );
}

#[test]
fn test_extract_component_name_from_text() {
    assert_eq!(
        extract_component_name_from_text("entity work.my_comp(rtl)"),
        "my_comp"
    );
    assert_eq!(extract_component_name_from_text("work.my_comp"), "my_comp");
    assert_eq!(extract_component_name_from_text("my_comp(rtl)"), "my_comp");
    assert_eq!(extract_component_name_from_text("my_comp"), "my_comp");
    assert_eq!(
        extract_component_name_from_text("  entity  lib.pkg.comp  "),
        "comp"
    );
}

#[test]
fn test_get_tree_node() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();
    let code = "entity E is end E;";
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let binding = parser.parse(code, None).unwrap();
    let root = binding.root_node();
    drop(_guard);

    // Should find nodes at various positions
    let pos1 = Position {
        line: 0,
        character: 0,
    };
    assert!(get_tree_node(root, pos1).is_some());

    let pos2 = Position {
        line: 0,
        character: 7,
    };
    assert!(get_tree_node(root, pos2).is_some());

    let pos3 = Position {
        line: 0,
        character: 18,
    };
    // At end of file, may or may not find node, but shouldn't panic
    let _ = get_tree_node(root, pos3);
}

#[test]
fn test_find_component_declaration_text_fallback() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();
    let code = r#"
u1: entity work.my_broken_comp
port map (
    data_in => sig
"#;
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(code, None).unwrap();
    drop(_guard);

    let cursor_offset = code.find("sig").unwrap() + 3;
    let result = find_component_declaration(tree.root_node(), code, cursor_offset);

    assert!(result.is_some(), "Should find component via text fallback");
    let (name, kind) = result.unwrap();
    assert_eq!(name, "my_broken_comp");
    assert_eq!(kind, DetectedMapKind::Port);
}

// --- Basic Context Tests ---

#[test]
fn test_context_architecture_body() {
    check_context(
        r#"
        architecture A of E is
            signal s : bit;
        begin
            s <= |
        end A;
        "#,
        CompletionContext::Architecture,
    );
}

#[test]
fn test_context_process_body() {
    check_context(
        r#"
        process(clk)
            variable v : integer;
        begin
            v := |
        end process;
        "#,
        CompletionContext::Process,
    );
}

#[test]
fn test_context_dot_access() {
    check_context(
        "architecture A of E is begin r.| end A;",
        CompletionContext::DotAccess,
    );
    check_context(
        "architecture A of E is begin r.fi| end A;",
        CompletionContext::DotAccess,
    );
}

// --- Port Map Tests ---

#[test]
fn test_port_map_lhs_simple() {
    check_context(
        r#"
        u1: my_comp port map (
            clk => clk,
            | => rst
        );
        "#,
        CompletionContext::PortMapLhs("my_comp".to_string()),
    );
}

#[test]
fn test_port_map_rhs() {
    check_context(
        r#"
        u1: my_comp port map (
            clk => |,
            rst => rst
        );
        "#,
        CompletionContext::PortMapRhs,
    );
}

#[test]
fn test_port_map_incomplete_cursor_inside() {
    check_context(
        r#"
        u1 : entity work.my_comp 
            port map (
                clk => |
        "#,
        CompletionContext::PortMapRhs,
    );
}

// --- Generic Map Tests ---

#[test]
fn test_generic_map_lhs() {
    check_context(
        r#"
        u0 : entity work.my_comp
            generic map (
                param_width => 8,
                | => 10
            );
        "#,
        CompletionContext::GenericMapLhs("my_comp".to_string()),
    );
}

#[test]
fn test_generic_map_rhs() {
    check_context(
        r#"
        u0 : entity work.my_comp
            generic map (
                param_width => |,
                param_depth => 16
            );
        "#,
        CompletionContext::GenericMapRhs,
    );
}

#[test]
fn test_misparsed_signal_assignment_enter_key() {
    check_context(
        r#"
        architecture A of B is
        begin
            inst_fifo2: avl_st_fifo
            generic map (
                |

            inst_fifo3: avl_st_fifo generic_map (); port map ();
        "#,
        CompletionContext::GenericMapLhs("avl_st_fifo".to_string()),
    );
}

#[test]
fn test_nested_complex_instantiation() {
    check_context(
        r#"
        u_complex: entity work.mylib.my_cpu(rtl)
            generic map (
                DATA_WIDTH => 32
            )
            port map (
                clk => |,
                rst => rst
            );
        "#,
        CompletionContext::PortMapRhs,
    );
}
#[test]
fn explore_instantiation_context() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
      architecture rtl of test is
      begin
          inst1: |
      end rtl;
  "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    let ctx = get_completion_context(&code, tree.root_node(), pos);
    println!("Context detected: {:?}", ctx);

    // Also print the tree structure
    let node = tree.root_node();
    println!("Tree: {}", node.to_sexp());
}
#[test]
fn explore_instantiation_with_partial_name() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
      architecture rtl of test is
      begin
          inst1: avl|
      end rtl;
  "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    let ctx = get_completion_context(&code, tree.root_node(), pos);
    println!("Context detected: {:?}", ctx);
    println!("Tree: {}", tree.root_node().to_sexp());
}
#[test]
fn explore_labeled_process() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
      architecture rtl of test is
      begin
          proc1: |
      end rtl;
  "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    println!("=== LABELED PROCESS ===");
    println!(
        "Context: {:?}",
        get_completion_context(&code, tree.root_node(), pos)
    );
    println!("Tree: {}", tree.root_node().to_sexp());
}

#[test]
fn explore_labeled_generate_for() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
  architecture rtl of test is
  begin
      proc1: process|
  end rtl;
  "#;
    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    println!("=== LABELED GENERATE ===");
    println!(
        "Context: {:?}",
        get_completion_context(&code, tree.root_node(), pos)
    );
    println!("Tree: {}", tree.root_node().to_sexp());
}
#[test]
fn explore_after_process_keyword() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
      architecture rtl of test is
      begin
          g_gen: process|
      end rtl;
  "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    println!("=== AFTER PROCESS KEYWORD ===");
    println!(
        "Context: {:?}",
        get_completion_context(&code, tree.root_node(), pos)
    );
    println!("Tree: {}", tree.root_node().to_sexp());
}
#[test]
fn explore_inside_process_statement() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
      architecture rtl of test is
      begin
          g_gen: process(clk)
          begin
              |
          end process;
      end rtl;
  "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    println!("=== INSIDE PROCESS BODY ===");
    println!(
        "Context: {:?}",
        get_completion_context(&code, tree.root_node(), pos)
    );
}

// =============================================================================
// Tests for VHDL Construct Snippets
// =============================================================================

#[test]
fn test_process_snippet_structure() {
    let snippet = create_process_snippet();

    assert_eq!(snippet.label, "process", "Label should be 'process'");
    assert_eq!(
        snippet.kind,
        Some(CompletionItemKind::SNIPPET),
        "Kind should be SNIPPET"
    );
    assert_eq!(
        snippet.insert_text_format,
        Some(InsertTextFormat::SNIPPET),
        "Format should be SNIPPET"
    );
    assert!(
        snippet.insert_text.is_some(),
        "insert_text should be populated"
    );

    let text = snippet.insert_text.unwrap();
    assert!(text.contains("process"), "Should contain 'process' keyword");
    assert!(text.contains("${1:"), "Should have placeholder 1");
    assert!(text.contains("$0"), "Should have final tab stop");
    assert!(
        !text.contains("rising_edge"),
        "Combinatorial process should not have rising_edge"
    );
}

#[test]
fn test_sync_process_snippet_structure() {
    let snippet = create_sync_process_snippet();

    assert_eq!(
        snippet.label, "process-sync",
        "Label should be 'process-sync'"
    );
    assert_eq!(snippet.kind, Some(CompletionItemKind::SNIPPET));
    assert_eq!(snippet.insert_text_format, Some(InsertTextFormat::SNIPPET));

    let text = snippet.insert_text.unwrap();
    assert!(text.contains("process"), "Should contain 'process' keyword");
    assert!(
        text.contains("rising_edge"),
        "Should have rising_edge for clocked process"
    );
    assert!(text.contains("${1:clk}"), "Should have clk placeholder");
    assert!(text.contains("$0"), "Should have final tab stop");
    assert!(
        !text.contains("rst"),
        "Should not have reset in sync-only process"
    );
}

#[test]
fn test_sync_rst_process_snippet_structure() {
    let snippet = create_sync_rst_process_snippet();

    assert_eq!(
        snippet.label, "process-sync-rst",
        "Label should be 'process-sync-rst'"
    );
    assert_eq!(snippet.kind, Some(CompletionItemKind::SNIPPET));
    assert_eq!(snippet.insert_text_format, Some(InsertTextFormat::SNIPPET));

    let text = snippet.insert_text.unwrap();
    assert!(text.contains("process"), "Should contain 'process' keyword");
    assert!(text.contains("rising_edge"), "Should have rising_edge");
    assert!(text.contains("${1:clk}"), "Should have clk placeholder");
    assert!(text.contains("${2:rst}"), "Should have rst placeholder");

    // Check that $0 comes BEFORE the reset check (user's preferred style)
    let main_pos = text.find("$0").expect("Should have $0");
    let rst_check_pos = text.find("if ${2:rst}").expect("Should have reset check");
    assert!(
        main_pos < rst_check_pos,
        "Main logic ($0) should come before reset check"
    );
}

#[test]
fn test_async_rst_process_snippet_structure() {
    let snippet = create_async_rst_process_snippet();

    assert_eq!(
        snippet.label, "process-async-rst",
        "Label should be 'process-async-rst'"
    );
    assert_eq!(snippet.kind, Some(CompletionItemKind::SNIPPET));
    assert_eq!(snippet.insert_text_format, Some(InsertTextFormat::SNIPPET));

    let text = snippet.insert_text.unwrap();
    assert!(text.contains("process"), "Should contain 'process' keyword");
    assert!(text.contains("${2:rst_n}"), "Should have rst_n placeholder");
    assert!(text.contains("elsif"), "Should have elsif for async reset");
    assert!(text.contains("rising_edge"), "Should have rising_edge");
    assert!(
        text.contains("= '0'"),
        "Should use active-low reset (best practice)"
    );

    // Check that reset check comes BEFORE rising_edge (async pattern)
    let rst_pos = text
        .find("if ${2:rst_n} = '0'")
        .expect("Should have reset check");
    let edge_pos = text
        .find("elsif rising_edge")
        .expect("Should have elsif rising_edge");
    assert!(
        rst_pos < edge_pos,
        "Reset check should come before elsif rising_edge in async pattern"
    );
}

#[test]
fn test_for_generate_snippet_structure() {
    let snippet = create_for_generate_snippet();

    assert_eq!(snippet.label, "for-generate", "Label should be 'for'");
    assert_eq!(snippet.kind, Some(CompletionItemKind::SNIPPET));
    assert_eq!(snippet.insert_text_format, Some(InsertTextFormat::SNIPPET));

    let text = snippet.insert_text.unwrap();
    assert!(text.contains("for"), "Should contain 'for' keyword");
    assert!(
        text.contains("generate"),
        "Should contain 'generate' keyword"
    );
    assert!(text.contains("${1:"), "Should have placeholder 1");
    assert!(text.contains("$0"), "Should have final tab stop");
}

#[test]
fn test_if_generate_snippet_structure() {
    let snippet = create_if_generate_snippet();

    assert_eq!(snippet.label, "if-generate", "Label should be 'if'");
    assert_eq!(snippet.kind, Some(CompletionItemKind::SNIPPET));
    assert_eq!(snippet.insert_text_format, Some(InsertTextFormat::SNIPPET));

    let text = snippet.insert_text.unwrap();
    assert!(text.contains("if"), "Should contain 'if' keyword");
    assert!(
        text.contains("generate"),
        "Should contain 'generate' keyword"
    );
    assert!(text.contains("${1:"), "Should have condition placeholder");
    assert!(text.contains("$0"), "Should have final tab stop");
}

#[test]
fn test_block_snippet_structure() {
    let snippet = create_block_snippet();

    assert_eq!(snippet.label, "block", "Label should be 'block'");
    assert_eq!(snippet.kind, Some(CompletionItemKind::SNIPPET));
    assert_eq!(snippet.insert_text_format, Some(InsertTextFormat::SNIPPET));

    let text = snippet.insert_text.unwrap();
    assert!(text.contains("block"), "Should contain 'block' keyword");
    assert!(text.contains("begin"), "Should contain 'begin' keyword");
    assert!(text.contains("$0"), "Should have final tab stop");
}

// =============================================================================
// Tests for is_after_label()
// =============================================================================

#[test]
fn test_is_after_label_empty() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            inst1: |
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        is_after_label(&code, pos, tree.root_node()),
        "Should detect label pattern: 'inst1: |'"
    );
}

#[test]
fn test_is_after_label_partial_identifier() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            inst1: avl|
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        is_after_label(&code, pos, tree.root_node()),
        "Should detect label pattern: 'inst1: avl|'"
    );
}

#[test]
fn test_is_after_label_process_keyword() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            proc1: process|
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    // This is a tricky case - "process" without parens looks like an identifier
    // We should still return true here and let snippets be offered
    assert!(
        is_after_label(&code, pos, tree.root_node()),
        "Should detect label pattern even with 'process' keyword"
    );
}

#[test]
fn test_not_after_label_inside_process() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            proc1: process(clk)
            begin
                |
            end process;
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern inside process body"
    );
}

#[test]
fn test_not_after_label_random_architecture() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
            signal s : bit;
        begin
            s <= '1';
            |
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern in random architecture position"
    );
}

#[test]
fn test_not_after_label_inside_port_map() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            inst1: my_comp
                port map (
                    clk => |
                );
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern when inside port map"
    );
}

#[test]
fn test_not_after_label_no_label() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            my_signal <= |'1';
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern when no label exists"
    );
}

// =============================================================================
// Edge Case Tests for is_after_label()
// =============================================================================

#[test]
fn test_not_after_label_inside_generate() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            gen1: for i in 0 to 3 generate
                |
            end generate;
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern inside well-formed generate statement"
    );
}

#[test]
fn test_not_after_label_inside_block() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            blk1: block
            begin
                |
            end block;
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern inside well-formed block statement"
    );
}

#[test]
fn test_not_after_complete_instantiation() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            inst1: my_comp port map (clk => clk);
            |
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern after a completed instantiation"
    );
}

#[test]
fn test_not_after_label_if_generate_complete() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            gen_if: if condition generate
                |
            end generate;
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern inside well-formed if-generate"
    );
}

#[test]
fn test_is_after_label_multiple_on_same_line() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        architecture rtl of test is
        begin
            inst1: comp1; inst2: |
        end rtl;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    // Second label on same line should still trigger
    assert!(
        is_after_label(&code, pos, tree.root_node()),
        "Should detect label pattern for second label on same line"
    );
}

#[test]
fn test_not_after_label_in_entity() {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();

    let code_with_cursor = r#"
        entity test is
            generic (WIDTH : integer := 8);
            port (clk : in std_logic);
            |
        end entity;
    "#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(&code, None).unwrap();
    drop(_guard);

    assert!(
        !is_after_label(&code, pos, tree.root_node()),
        "Should NOT detect label pattern in entity context"
    );
}

// =============================================================================
// Tests for generate_instantiation_snippet()
// =============================================================================

use crate::analysis::{Declaration, PortDirection, ScopeTree, TypeInfo};
use tower_lsp::lsp_types::Range;

/// Helper to create a test Declaration
fn make_decl(name: &str, decl_type: DeclType) -> Declaration {
    Declaration {
        name: name.to_string(),
        decl_type,
        range: Range::default(),
        selection_range: Range::default(),
        type_info: TypeInfo::new(),
        default_value: None,
        doc_comment: None,
        parameters: None,
    }
}

/// Helper to create a test ScopeTree with generics and ports
fn make_scope_tree(generics: Vec<Declaration>, ports: Vec<Declaration>) -> ScopeTree {
    use crate::analysis::ScopeKind;
    use std::collections::{HashMap, HashSet};

    let mut declarations = Vec::new();
    declarations.extend(generics);
    declarations.extend(ports);

    ScopeTree {
        kind: ScopeKind::Entity,
        name: None,
        entity: None,
        package: None,
        range: Range::default(),
        declarations,
        local_usage: HashSet::new(),
        children: Vec::new(),
        decl_index: HashMap::new(),
        instantiations: Vec::new(),
        use_clauses: Vec::new(),
        attr_specs: HashMap::new(),
    }
}

#[test]
fn test_instantiation_snippet_with_generics_and_ports() {
    let generics = vec![
        make_decl("BAUD_RATE", DeclType::Generic),
        make_decl("DATA_BITS", DeclType::Generic),
    ];
    let ports = vec![
        make_decl("clk", DeclType::Port(PortDirection::In)),
        make_decl("reset", DeclType::Port(PortDirection::In)),
        make_decl("tx_data", DeclType::Port(PortDirection::In)),
    ];
    let scope_tree = make_scope_tree(generics, ports);

    let snippet = generate_instantiation_snippet("uart_tx", &scope_tree);

    // Should contain entity name
    assert!(snippet.contains("uart_tx"), "Should contain entity name");

    // Should contain generic map section
    assert!(snippet.contains("generic map"), "Should have generic map");
    assert!(
        snippet.contains("${1:BAUD_RATE}"),
        "Should have BAUD_RATE generic with tab stop 1"
    );
    assert!(
        snippet.contains("${2:DATA_BITS}"),
        "Should have DATA_BITS generic with tab stop 2"
    );

    // Should contain port map section
    assert!(snippet.contains("port map"), "Should have port map");
    assert!(
        snippet.contains("${3:clk}"),
        "Should have clk port with tab stop 3"
    );
    assert!(
        snippet.contains("${4:reset}"),
        "Should have reset port with tab stop 4"
    );
    assert!(
        snippet.contains("${5:tx_data}"),
        "Should have tx_data port with tab stop 5"
    );

    // Should end with semicolon
    assert!(snippet.trim().ends_with(");"), "Should end with );");
}

#[test]
fn test_instantiation_snippet_ports_only_no_generics() {
    let ports = vec![
        make_decl("clk", DeclType::Port(PortDirection::In)),
        make_decl("data_out", DeclType::Port(PortDirection::Out)),
    ];
    let scope_tree = make_scope_tree(vec![], ports);

    let snippet = generate_instantiation_snippet("simple_comp", &scope_tree);

    // Should NOT contain generic map
    assert!(
        !snippet.contains("generic map"),
        "Should NOT have generic map"
    );

    // Should contain port map section
    assert!(snippet.contains("port map"), "Should have port map");
    assert!(snippet.contains("${1:clk}"), "Tab stops should start at 1");
    assert!(
        snippet.contains("${2:data_out}"),
        "Should have data_out port"
    );
}

#[test]
fn test_instantiation_snippet_alignment() {
    let generics = vec![
        make_decl("A", DeclType::Generic),
        make_decl("VERY_LONG_NAME", DeclType::Generic),
    ];
    let ports = vec![
        make_decl("x", DeclType::Port(PortDirection::In)),
        make_decl("long_port", DeclType::Port(PortDirection::Out)),
    ];
    let scope_tree = make_scope_tree(generics, ports);

    let snippet = generate_instantiation_snippet("test_entity", &scope_tree);

    // Extract generic map section
    let generic_section = snippet
        .lines()
        .skip_while(|l| !l.contains("generic map"))
        .take_while(|l| !l.contains("port map"))
        .collect::<Vec<_>>()
        .join("\n");

    // Extract port map section
    let port_section = snippet
        .lines()
        .skip_while(|l| !l.contains("port map"))
        .collect::<Vec<_>>()
        .join("\n");

    // Check that => are aligned in generic section
    // Both lines should have => at the same column position
    let generic_lines: Vec<&str> = generic_section
        .lines()
        .filter(|l| l.contains("=>"))
        .collect();
    if generic_lines.len() >= 2 {
        let arrow_pos_1 = generic_lines[0].find("=>").unwrap();
        let arrow_pos_2 = generic_lines[1].find("=>").unwrap();
        assert_eq!(arrow_pos_1, arrow_pos_2, "Generic => should be aligned");
    }

    // Check that => are aligned in port section
    let port_lines: Vec<&str> = port_section.lines().filter(|l| l.contains("=>")).collect();
    if port_lines.len() >= 2 {
        let arrow_pos_1 = port_lines[0].find("=>").unwrap();
        let arrow_pos_2 = port_lines[1].find("=>").unwrap();
        assert_eq!(arrow_pos_1, arrow_pos_2, "Port => should be aligned");
    }
}

#[test]
fn test_instantiation_snippet_empty_scope() {
    let scope_tree = make_scope_tree(vec![], vec![]);

    let snippet = generate_instantiation_snippet("empty_entity", &scope_tree);

    // Should have entity name
    assert!(
        snippet.contains("empty_entity"),
        "Should contain entity name"
    );

    // With no ports or generics, should still have port map (might be empty)
    // This is a design decision - adjust based on desired behavior
}

#[test]
fn test_instantiation_snippet_tab_stop_numbering() {
    let generics = vec![
        make_decl("G1", DeclType::Generic),
        make_decl("G2", DeclType::Generic),
    ];
    let ports = vec![
        make_decl("P1", DeclType::Port(PortDirection::In)),
        make_decl("P2", DeclType::Port(PortDirection::Out)),
        make_decl("P3", DeclType::Port(PortDirection::Out)),
    ];
    let scope_tree = make_scope_tree(generics, ports);

    let snippet = generate_instantiation_snippet("test", &scope_tree);

    // Verify sequential tab stop numbering
    assert!(snippet.contains("${1:G1}"), "First generic should be tab 1");
    assert!(
        snippet.contains("${2:G2}"),
        "Second generic should be tab 2"
    );
    assert!(snippet.contains("${3:P1}"), "First port should be tab 3");
    assert!(snippet.contains("${4:P2}"), "Second port should be tab 4");
    assert!(snippet.contains("${5:P3}"), "Third port should be tab 5");
}

// =============================================================================
// Package symbol completion tests
// =============================================================================

/// Helper: build a two-file analysis map (package + architecture), parse the
/// architecture text and return the completion items at the given position.
fn complete_in_arch(pkg_code: &str, arch_code: &str, pos: Position) -> Vec<CompletionItem> {
    use crate::backend::AnalysisMap;
    use crate::backend::test_utils::parse_text;
    use tower_lsp::lsp_types::Url;

    let pkg_uri = Url::parse("file:///pkg.vhd").unwrap();
    let arch_uri = Url::parse("file:///arch.vhd").unwrap();

    let pkg_tree = parse_text(pkg_code);
    let pkg_analysis =
        crate::backend::syntax::parser::extract_document_symbols(pkg_code, pkg_tree.root_node());

    let arch_tree = parse_text(arch_code);
    let arch_root = arch_tree.root_node();
    let arch_analysis =
        crate::backend::syntax::parser::extract_document_symbols(arch_code, arch_root);

    let mut analysis_map = AnalysisMap::new();
    analysis_map.insert(pkg_uri, pkg_analysis);
    analysis_map.insert(arch_uri.clone(), arch_analysis);

    let ctx = get_completion_context(arch_code, arch_root, pos);
    complete_scope(&analysis_map, &arch_uri, &ctx, pos, arch_code, arch_root)
}

/// Helper: three-file analysis map (package + entity + architecture), where entity
/// and architecture live in separate files. Returns completion items at `pos` in the arch file.
fn complete_in_arch_cross_file(
    pkg_code: &str,
    entity_code: &str,
    arch_code: &str,
    pos: Position,
) -> Vec<CompletionItem> {
    use crate::backend::AnalysisMap;
    use crate::backend::test_utils::parse_text;
    use tower_lsp::lsp_types::Url;

    let pkg_uri = Url::parse("file:///pkg.vhd").unwrap();
    let entity_uri = Url::parse("file:///entity.vhd").unwrap();
    let arch_uri = Url::parse("file:///arch.vhd").unwrap();

    let pkg_tree = parse_text(pkg_code);
    let pkg_analysis =
        crate::backend::syntax::parser::extract_document_symbols(pkg_code, pkg_tree.root_node());

    let entity_tree = parse_text(entity_code);
    let entity_analysis = crate::backend::syntax::parser::extract_document_symbols(
        entity_code,
        entity_tree.root_node(),
    );

    let arch_tree = parse_text(arch_code);
    let arch_root = arch_tree.root_node();
    let arch_analysis =
        crate::backend::syntax::parser::extract_document_symbols(arch_code, arch_root);

    let mut analysis_map = AnalysisMap::new();
    analysis_map.insert(pkg_uri, pkg_analysis);
    analysis_map.insert(entity_uri, entity_analysis);
    analysis_map.insert(arch_uri.clone(), arch_analysis);

    let ctx = get_completion_context(arch_code, arch_root, pos);
    complete_scope(&analysis_map, &arch_uri, &ctx, pos, arch_code, arch_root)
}

/// Convenience: collect item labels from completions.
fn labels(items: &[CompletionItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

#[test]
fn test_package_constant_appears_in_arch_completion() {
    let pkg_code = r#"
package my_pkg is
    constant C_WIDTH : integer := 8;
    constant C_DEPTH : integer := 1024;
end package;
"#;
    let arch_code = r#"
use work.my_pkg.all;

architecture rtl of test is
    signal s : integer;
begin
    s <= C_WIDTH;
end architecture;
"#;
    // Cursor inside the architecture declarative region (after `signal s : integer;`)
    let pos = Position {
        line: 4,
        character: 4,
    };
    let items = complete_in_arch(pkg_code, arch_code, pos);
    let names = labels(&items);

    assert!(
        names.contains(&"C_WIDTH"),
        "Package constant C_WIDTH should appear in completions. Got: {:?}",
        names
    );
    assert!(
        names.contains(&"C_DEPTH"),
        "Package constant C_DEPTH should appear in completions. Got: {:?}",
        names
    );
}

#[test]
fn test_package_function_appears_in_completion() {
    let pkg_code = r#"
package my_pkg is
    function to_slv(x : integer; width : integer) return std_logic_vector;
end package;
"#;
    let arch_code = r#"
use work.my_pkg.all;

architecture rtl of test is
    signal result : std_logic_vector(7 downto 0);
begin
    result <= to_slv(42, 8);
end architecture;
"#;
    let pos = Position {
        line: 4,
        character: 4,
    };
    let items = complete_in_arch(pkg_code, arch_code, pos);
    let names = labels(&items);

    assert!(
        names.contains(&"to_slv"),
        "Package function to_slv should appear in completions. Got: {:?}",
        names
    );
}

#[test]
fn test_package_procedure_appears_in_completion() {
    let pkg_code = r#"
package my_pkg is
    procedure log_msg(msg : string);
end package;
"#;
    let arch_code = r#"
use work.my_pkg.all;

architecture rtl of test is
begin
    process
    begin
        log_msg("hello");
    end process;
end architecture;
"#;
    // Cursor inside process body
    let pos = Position {
        line: 7,
        character: 8,
    };
    let items = complete_in_arch(pkg_code, arch_code, pos);
    let names = labels(&items);

    assert!(
        names.contains(&"log_msg"),
        "Package procedure log_msg should appear in completions. Got: {:?}",
        names
    );
}

#[test]
fn test_package_type_appears_in_completion() {
    let pkg_code = r#"
package my_pkg is
    type t_state is (IDLE, RUN, STOP);
end package;
"#;
    let arch_code = r#"
use work.my_pkg.all;

architecture rtl of test is
    signal state : t_state;
begin
end architecture;
"#;
    let pos = Position {
        line: 4,
        character: 4,
    };
    let items = complete_in_arch(pkg_code, arch_code, pos);
    let names = labels(&items);

    assert!(
        names.contains(&"t_state"),
        "Package type t_state should appear in completions. Got: {:?}",
        names
    );
}

#[test]
fn test_entity_context_clause_visible_in_same_file_architecture() {
    // Regression: context clauses (use statements at top of file) that precede an entity
    // must remain visible in architectures in the same file. Tests the common case.
    let pkg_code = r#"
package my_pkg is
    constant C_MAGIC : integer := 42;
end package;
"#;
    // use clause is the context clause for the entity, at the top of the file.
    // The architecture in the same file must see C_MAGIC without its own use clause.
    let arch_code = r#"
use work.my_pkg.all;

entity my_ent is
    port (clk : in std_logic);
end entity;

architecture rtl of my_ent is
    signal s : integer;
begin
end architecture;
"#;
    // line 8 = "    signal s : integer;" — inside arch declarative region
    let pos = Position {
        line: 8,
        character: 4,
    };
    let items = complete_in_arch(pkg_code, arch_code, pos);
    let names = labels(&items);

    assert!(
        names.contains(&"C_MAGIC"),
        "Symbols from entity file context clause should be visible in same-file architecture. Got: {:?}",
        names
    );
}

#[test]
fn test_entity_context_clause_visible_in_cross_file_architecture() {
    // The entity's file has a context clause (use work.my_pkg.all).
    // The architecture is in a separate file with no use clause of its own.
    // The arch must still see my_pkg symbols — the entity's context clause is
    // inherited by all implementing architectures.
    let pkg_code = r#"
package my_pkg is
    constant C_CROSS : integer := 99;
end package;
"#;
    // entity_code has the use clause at the top — context clause for this design unit.
    let entity_code = r#"
use work.my_pkg.all;

entity my_ent is
    port (clk : in std_logic);
end entity;
"#;
    // arch_code has no use clause — relies on inheriting the entity's context.
    let arch_code = r#"
architecture rtl of my_ent is
    signal s : integer;
begin
end architecture;
"#;
    // line 2 = "    signal s : integer;" — inside arch declarative region
    let pos = Position {
        line: 2,
        character: 4,
    };
    let items = complete_in_arch_cross_file(pkg_code, entity_code, arch_code, pos);
    let names = labels(&items);

    assert!(
        names.contains(&"C_CROSS"),
        "Symbols from entity file context clause should be visible in arch in a separate file. Got: {:?}",
        names
    );
}

#[test]
fn test_entity_ports_and_generics_visible_in_arch_cross_file() {
    // Validates that ports and generics declared in an entity are visible inside
    // an architecture that lives in a separate file.
    let entity_code = r#"
entity my_ent is
    generic (
        G_WIDTH : integer := 8
    );
    port (
        clk  : in std_logic;
        data : out std_logic
    );
end entity;
"#;
    // Architecture in its own file — no redeclaration of ports/generics.
    let arch_code = r#"
architecture rtl of my_ent is
begin
    process(clk) is
    begin
    end process;
end architecture;
"#;
    // Cursor inside the process body where ports/generics should be in scope.
    // line 4 = "    begin"  (inside process)
    let pos = Position {
        line: 4,
        character: 4,
    };
    let items = complete_in_arch_cross_file("", entity_code, arch_code, pos);
    let names = labels(&items);

    assert!(
        names.contains(&"clk"),
        "Entity port 'clk' should be visible inside arch body when entity is in a separate file. Got: {:?}",
        names
    );
    assert!(
        names.contains(&"G_WIDTH"),
        "Entity generic 'G_WIDTH' should be visible inside arch body when entity is in a separate file. Got: {:?}",
        names
    );
}
