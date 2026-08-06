use super::*;
use crate::backend::test_utils::SHARED_PARSER_LOCK;
use std::collections::HashSet;
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

// --- Unit Tests for collect_used_param_names ---

#[test]
fn test_collect_used_param_names_empty() {
    // Empty parens: no content between ( and cursor
    assert_eq!(collect_used_param_names("func(", 4, 5), HashSet::new());
}

#[test]
fn test_collect_used_param_names_whitespace_only() {
    assert_eq!(collect_used_param_names("func(  ", 4, 7), HashSet::new());
}

#[test]
fn test_collect_used_param_names_single() {
    // "func(a => x, " — only "a" should be collected
    let text = "func(a => x, ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_multiple() {
    let text = "func(a => x, b => y, ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a", "b"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_aggregate_rhs() {
    // "others" inside aggregate must NOT be collected
    let text = "func(a => (others => '0'), ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_array_aggregate_rhs() {
    // Array aggregate indexes like "0 =>" and "1 =>" must NOT be collected
    let text = "func(a => (0 => '1', 1 => '0'), ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_nested_call() {
    // "x" is inside inner_func call, depth 2 — must NOT be collected
    let text = "func(a => inner_func(x => y), ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_case_insensitive() {
    let text = "func(PARAM_A => x, ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["param_a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_mixed_aggregate_and_named() {
    let text = "func(a => x, b => (others => '0'), ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a", "b"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_positional_no_arrow() {
    // No "=>" at top level — positional args — nothing collected
    let text = "func(x, y, ";
    assert_eq!(collect_used_param_names(text, 4, text.len()), HashSet::new());
}

// --- Unit Tests for has_top_level_arrow ---

#[test]
fn test_has_top_level_arrow_simple() {
    assert!(has_top_level_arrow("a => x"));
    assert!(!has_top_level_arrow("a, b, c"));
    assert!(!has_top_level_arrow(""));
}

#[test]
fn test_has_top_level_arrow_nested_ignored() {
    // => inside parens is NOT at top level
    assert!(!has_top_level_arrow("(others => '0')"));
    // but one at top level + one nested
    assert!(has_top_level_arrow("a => (others => '0')"));
}

#[test]
fn test_has_top_level_arrow_nested_call() {
    assert!(!has_top_level_arrow("inner_func(x => y)"));
    assert!(has_top_level_arrow("a => inner_func(x => y)"));
}

// --- Unit Tests for classify_call_args ---

#[test]
fn test_classify_call_args_empty() {
    assert_eq!(
        classify_call_args("func".to_string(), "func(", 4, 5),
        CompletionContext::SubprogramCallBoth("func".to_string())
    );
}

#[test]
fn test_classify_call_args_whitespace_only() {
    let text = "func(   ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallBoth("func".to_string())
    );
}

#[test]
fn test_classify_call_args_named_lhs() {
    // After comma in named mode, cursor is on LHS
    let text = "func(a => x, ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallLhs("func".to_string())
    );
}

#[test]
fn test_classify_call_args_named_rhs() {
    // After => in named mode
    let text = "func(a => ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallRhs
    );
}

#[test]
fn test_classify_call_args_positional() {
    // Args present but no => at top level
    let text = "func(x, ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallRhs
    );
}

#[test]
fn test_classify_call_args_aggregate_does_not_trigger_named() {
    // (others => '0') is a positional arg — no top-level =>
    let text = "func((others => '0'), ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallRhs
    );
}

#[test]
fn test_classify_call_args_named_after_aggregate_rhs_lhs() {
    // Named arg whose value is an aggregate — cursor is after comma, on LHS
    let text = "func(a => (others => '0'), ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallLhs("func".to_string())
    );
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

// --- Context Detection Tests: Subprogram Call ---

// Each test below is a self-contained VHDL snippet with | as the cursor marker.

#[test]
fn test_context_subprogram_call_empty() {
    // Empty parens — offer both LHS and RHS
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(|);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallBoth("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_named_lhs_first_arg() {
    // Before => on the first arg
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(|a => 0, b => 1);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallLhs("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_named_rhs() {
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => |);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallRhs,
    );
}

#[test]
fn test_context_subprogram_call_named_lhs_after_comma() {
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, |);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallLhs("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_positional() {
    // Positional args — no => at top level → RHS only
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(0, |);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallRhs,
    );
}

#[test]
fn test_context_subprogram_partial_name_stays_both() {
    // User typed "co" after the open paren — no arrow, no comma yet.
    // Must stay SubprogramCallBoth so param names survive editor re-trigger.
    check_context(
        r#"
architecture rtl of e is
    function my_func(condition : boolean; value : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(co|);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallBoth("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_aggregate_in_named_rhs_then_lhs() {
    // Aggregate value for first arg — cursor is on LHS of second arg
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => (0 + 1), |);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallLhs("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_nested_inner_wins() {
    // Cursor inside inner call — context should be for inner call, not outer
    check_context(
        r#"
architecture rtl of e is
    function outer(x : integer) return integer is begin return x; end function;
    function inner(p : integer) return integer is begin return p; end function;
begin
    process is variable v : integer; begin
        v := outer(inner(|));
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallBoth("inner".to_string()),
    );
}

#[test]
fn test_context_procedure_call_named_lhs() {
    check_context(
        r#"
architecture rtl of e is
    procedure my_proc(signal clk : in bit; constant n : in integer) is
    begin null; end procedure;
    signal sys_clk : bit;
begin
    process is begin
        my_proc(|clk => sys_clk, n => 8);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallLhs("my_proc".to_string()),
    );
}

// =============================================================================
// Subprogram Call Completion (Resolution) Tests
// =============================================================================

/// Helper for subprogram call completion tests.
/// Returns completion labels at the cursor position in the given arch code.
fn complete_subprogram_call(arch_code: &str) -> Vec<String> {
    use crate::backend::AnalysisMap;
    use crate::backend::test_utils::parse_text;
    use tower_lsp::lsp_types::Url;

    let arch_uri = Url::parse("file:///arch.vhd").unwrap();
    let (code, pos) = extract_cursor(arch_code);

    // parse_text acquires SHARED_PARSER_LOCK internally — do NOT hold it here.
    let arch_tree = parse_text(&code);
    let arch_root = arch_tree.root_node();
    let arch_analysis =
        crate::backend::syntax::parser::extract_document_symbols(&code, arch_root);

    let mut analysis_map = AnalysisMap::new();
    analysis_map.insert(arch_uri.clone(), arch_analysis);

    let ctx = get_completion_context(&code, arch_root, pos);
    let items = complete_scope(&analysis_map, &arch_uri, &ctx, pos, &code, arch_root);
    labels(&items).iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_subprogram_lhs_offers_all_params_when_empty() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer; c : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(|);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(names.contains(&"a".to_string()), "param 'a' should appear. Got: {:?}", names);
    assert!(names.contains(&"b".to_string()), "param 'b' should appear. Got: {:?}", names);
    assert!(names.contains(&"c".to_string()), "param 'c' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_filters_already_supplied_param() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer; c : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' should be filtered out. Got: {:?}", names);
    assert!(names.contains(&"b".to_string()), "param 'b' should appear. Got: {:?}", names);
    assert!(names.contains(&"c".to_string()), "param 'c' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_filters_multiple_supplied_params() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer; c : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, b => 1, |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' should be filtered. Got: {:?}", names);
    assert!(!names.contains(&"b".to_string()), "'b' should be filtered. Got: {:?}", names);
    assert!(names.contains(&"c".to_string()), "param 'c' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_all_params_filtered_when_all_supplied() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, b => 1, |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' should be filtered. Got: {:?}", names);
    assert!(!names.contains(&"b".to_string()), "'b' should be filtered. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_aggregate_rhs_does_not_filter_wrong_param() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => (0 + 1), |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' should be filtered. Got: {:?}", names);
    assert!(names.contains(&"b".to_string()), "'b' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_positional_offers_no_param_names() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return 0; end function;
    signal s : integer;
begin
    process is variable v : integer; begin
        v := my_func(0, |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    // In positional mode, param names should not appear as completions
    assert!(!names.contains(&"a".to_string()), "'a' should not appear in positional mode. Got: {:?}", names);
    assert!(!names.contains(&"b".to_string()), "'b' should not appear in positional mode. Got: {:?}", names);
    // But in-scope signals/variables should appear
    assert!(names.contains(&"s".to_string()) || names.contains(&"v".to_string()),
        "in-scope items should appear in positional mode. Got: {:?}", names);
}

// --- Phase 2: Instantiation already-used param filtering ---

#[test]
fn test_port_map_lhs_filters_already_connected_port() {
    // clk is already connected — should NOT appear in LHS suggestions
    let arch = r#"
architecture rtl of e is
    component dut is
        port (clk : in bit; data : in bit; q : out bit);
    end component;
    signal sys_clk : bit;
    signal d : bit;
    signal out_q : bit;
begin
    u1: dut port map (clk => sys_clk, |);
end architecture;"#;

    let (_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch.replace("|", ""), pos);
    let names = labels(&items);
    assert!(!names.contains(&"clk"), "'clk' should be filtered (already connected). Got: {:?}", names);
    assert!(names.contains(&"data"), "'data' should appear. Got: {:?}", names);
    assert!(names.contains(&"q"), "'q' should appear. Got: {:?}", names);
}

#[test]
fn test_port_map_lhs_filters_multiple_connected_ports() {
    let arch = r#"
architecture rtl of e is
    component dut is
        port (clk : in bit; data : in bit; q : out bit);
    end component;
    signal sys_clk : bit;
    signal d : bit;
begin
    u1: dut port map (clk => sys_clk, data => d, |);
end architecture;"#;

    let (_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch.replace("|", ""), pos);
    let names = labels(&items);
    assert!(!names.contains(&"clk"), "'clk' should be filtered. Got: {:?}", names);
    assert!(!names.contains(&"data"), "'data' should be filtered. Got: {:?}", names);
    assert!(names.contains(&"q"), "'q' should still appear. Got: {:?}", names);
}

#[test]
fn test_port_map_lhs_aggregate_value_does_not_confuse_filter() {
    // port map (data => (0 or 0), | ) — the word inside aggregate must not be filtered as a port
    let arch = r#"
architecture rtl of e is
    component dut is
        port (data : in bit; q : out bit);
    end component;
begin
    u1: dut port map (data => (0 or 0), |);
end architecture;"#;

    let (_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch.replace("|", ""), pos);
    let names = labels(&items);
    // The key assertion: "others" or any word from inside the expression must not incorrectly
    // be treated as a port name that's filtered
    assert!(!names.contains(&"others"), "'others' must not appear as a filtered port name. Got: {:?}", names);
}

#[test]
fn test_generic_map_lhs_filters_already_set_generic() {
    let arch = r#"
architecture rtl of e is
    component dut is
        generic (WIDTH : integer; DEPTH : integer);
        port (clk : in bit);
    end component;
begin
    u1: dut
        generic map (WIDTH => 8, |)
        port map (clk => '0');
end architecture;"#;

    let (_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch.replace("|", ""), pos);
    let names = labels(&items);
    assert!(!names.contains(&"WIDTH"), "'WIDTH' should be filtered. Got: {:?}", names);
    assert!(names.contains(&"DEPTH") || names.contains(&"depth"), "'DEPTH' should appear. Got: {:?}", names);
}

// --- Bug-fix regression: cursor-in-the-middle filtering (Bug A) ---

#[test]
fn test_port_map_lhs_filters_port_after_cursor() {
    // Cursor is BETWEEN two already-connected ports: both should be filtered
    // even though `data` appears after the cursor position.
    let arch = r#"
architecture rtl of e is
    component dut is port (clk : in bit; data : in bit; rst : in bit); end component;
begin
    u1: dut port map (clk => '0', |, data => '1');
end architecture;"#;

    let (_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch.replace("|", ""), pos);
    let names = labels(&items);
    assert!(!names.contains(&"clk"), "'clk' (before cursor) should be filtered. Got: {:?}", names);
    assert!(!names.contains(&"data"), "'data' (after cursor) should be filtered. Got: {:?}", names);
    assert!(names.contains(&"rst") || names.contains(&"RST"), "'rst' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_filters_param_after_cursor() {
    // Cursor is between two already-supplied params; param after cursor must also be filtered.
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer; c : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, |, c => 2);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' (before cursor) should be filtered. Got: {:?}", names);
    assert!(!names.contains(&"c".to_string()), "'c' (after cursor) should be filtered. Got: {:?}", names);
    assert!(names.contains(&"b".to_string()), "'b' should appear. Got: {:?}", names);
}

// --- Bug-fix regression: package-imported subprogram resolution (Bug B) ---

#[test]
fn test_subprogram_lhs_resolves_from_imported_package() {
    // Function is declared in a package, not locally — params must still be offered.
    let pkg = r#"
package my_pkg is
    function pkg_func(x : integer; y : integer) return integer;
end package;"#;

    let arch = r#"
use work.my_pkg.all;
architecture rtl of e is begin
    process is variable v : integer; begin
        v := pkg_func(|);
    end process;
end architecture;"#;

    let (arch_code, pos) = extract_cursor(arch);
    let items = complete_in_arch(pkg, &arch_code, pos);
    let names = labels(&items);
    assert!(names.contains(&"x"), "param 'x' from package should appear. Got: {:?}", names);
    assert!(names.contains(&"y"), "param 'y' from package should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_filters_used_param_from_imported_package() {
    // Same as above, but one param already supplied — it should be filtered.
    let pkg = r#"
package my_pkg is
    function pkg_func(x : integer; y : integer) return integer;
end package;"#;

    let arch = r#"
use work.my_pkg.all;
architecture rtl of e is begin
    process is variable v : integer; begin
        v := pkg_func(x => 0, |);
    end process;
end architecture;"#;

    let (arch_code, pos) = extract_cursor(arch);
    let items = complete_in_arch(pkg, &arch_code, pos);
    let names = labels(&items);
    assert!(!names.contains(&"x"), "'x' already supplied should be filtered. Got: {:?}", names);
    assert!(names.contains(&"y"), "param 'y' should appear. Got: {:?}", names);
}

// --- SubprogramCallBoth sortText ordering ---

#[test]
fn test_subprogram_both_params_sort_before_scope_items() {
    // In the empty-args case, param items must have sortText "0_<label>"
    // and scope items must have sortText "1_<label>" so editors float params first,
    // regardless of alphabetical order (zzz_param vs aaa_signal is the stress case).
    let arch = r#"
architecture rtl of e is
    function my_func(zzz_param : integer) return integer is
    begin return 0; end function;
    signal aaa_signal : integer;
begin
    process is variable v : integer; begin
        v := my_func(|);
    end process;
end architecture;"#;

    let (arch_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch_code, pos);

    let param_item = items.iter().find(|i| i.label == "zzz_param");
    let scope_item = items.iter().find(|i| i.label == "aaa_signal");

    assert!(param_item.is_some(), "zzz_param should appear. Got: {:?}", labels(&items));
    assert!(scope_item.is_some(), "aaa_signal should appear. Got: {:?}", labels(&items));

    let param_sort = param_item.unwrap().sort_text.as_deref().unwrap_or("");
    let scope_sort = scope_item.unwrap().sort_text.as_deref().unwrap_or("");

    assert!(
        param_sort < scope_sort,
        "param sortText ({param_sort:?}) should sort before scope item sortText ({scope_sort:?})"
    );
}

// --- Regression: text-based fallback for incomplete code (no closing paren yet) ---

#[test]
fn test_subprogram_lhs_incomplete_code_no_close_paren() {
    // Simulates real typing: cursor is after the first named param, no closing ')' yet.
    // The AST has ERROR nodes because the expression is unfinished. The text-based
    // fallback must still detect SubprogramCallLhs and return remaining params.
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer; c : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, |
    end process;
end architecture;"#;

    // Note: do NOT strip the | before parsing — we intentionally parse broken code
    // to confirm the text-based fallback works when the AST has ERROR nodes.
    use crate::backend::AnalysisMap;
    use crate::backend::test_utils::parse_text;
    use tower_lsp::lsp_types::Url;

    let (code, pos) = extract_cursor(arch);
    let arch_uri = Url::parse("file:///arch.vhd").unwrap();
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);
    let mut analysis_map = AnalysisMap::new();
    analysis_map.insert(arch_uri.clone(), analysis);

    let ctx = get_completion_context(&code, root, pos);
    let items = complete_scope(&analysis_map, &arch_uri, &ctx, pos, &code, root);
    let names: Vec<&str> = labels(&items);

    assert!(!names.contains(&"a"), "'a' already supplied should be filtered. Got: {:?}", names);
    assert!(names.contains(&"b"), "param 'b' should appear via text fallback. Got: {:?}", names);
    assert!(names.contains(&"c"), "param 'c' should appear via text fallback. Got: {:?}", names);
}

// =========================================================================
// Instantiation unit completion: `entity <lib>.<name>`
// =========================================================================

use crate::analysis::{OxideSymbolKind as OSK, ParseLevel, Symbol as Sym};

/// Builds a shallow Analysis in `library` declaring `entities`.
fn shallow_lib(library: &str, entities: &[&str]) -> crate::analysis::Analysis {
    let mut a = crate::analysis::Analysis::new();
    a.library = library.to_string();
    a.parse_level = ParseLevel::Shallow;
    for e in entities {
        a.symbols.insert(
            e.to_lowercase(),
            Sym {
                name: e.to_string(),
                kind: OSK::Entity,
                detail: Some("Entity".to_string()),
                range: tower_lsp::lsp_types::Range::default(),
                children: Vec::new(),
            },
        );
    }
    a
}

#[test]
fn test_detect_context_after_library_dot() {
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.my_\n";
    let pos = Position {
        line: 2,
        character: 26,
    };
    assert_eq!(
        super::detect_instantiation_unit_context(text, pos),
        Some(CompletionContext::LibraryUnits("rtl_lib".to_string()))
    );
}

#[test]
fn test_detect_context_after_library_dot_empty_prefix() {
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.\n";
    let pos = Position {
        line: 2,
        character: 24,
    };
    assert_eq!(
        super::detect_instantiation_unit_context(text, pos),
        Some(CompletionContext::LibraryUnits("rtl_lib".to_string()))
    );
}

#[test]
fn test_detect_context_after_entity_keyword() {
    let text = "architecture rtl of top is\nbegin\n  u0: entity \n";
    let pos = Position {
        line: 2,
        character: 13,
    };
    assert_eq!(
        super::detect_instantiation_unit_context(text, pos),
        Some(CompletionContext::InstantiationLibrary)
    );
}

#[test]
fn test_detect_context_is_case_insensitive() {
    let text = "architecture rtl of top is\nbegin\n  U0: ENTITY RTL_LIB.MY_\n";
    let pos = Position {
        line: 2,
        character: 26,
    };
    assert_eq!(
        super::detect_instantiation_unit_context(text, pos),
        Some(CompletionContext::LibraryUnits("rtl_lib".to_string()))
    );
}

#[test]
fn test_detect_context_ignores_unrelated_dotted_names() {
    // A record field access must NOT be mistaken for a library prefix.
    let text = "architecture rtl of top is\nbegin\n  x <= my_rec.fie\n";
    let pos = Position {
        line: 2,
        character: 18,
    };
    assert_eq!(super::detect_instantiation_unit_context(text, pos), None);
}

#[test]
fn test_detect_context_ignores_use_clause() {
    // `use ieee.std_logic_1164` is not an instantiation.
    let text = "use ieee.std_\n";
    let pos = Position {
        line: 0,
        character: 13,
    };
    assert_eq!(super::detect_instantiation_unit_context(text, pos), None);
}

#[test]
fn test_library_units_completion_lists_entities_of_that_library() {
    use crate::backend::AnalysisMap;
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.\n";
    let pos = Position {
        line: 2,
        character: 24,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        shallow_lib("rtl_lib", &["uart_tx", "cpu"]),
    );
    map.insert(
        Url::parse("file:///b.vhd").unwrap(),
        shallow_lib("other_lib", &["excluded"]),
    );
    map.insert(top_uri.clone(), shallow_lib("rtl_lib", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();

    let ctx = get_completion_context(text, root, pos);
    assert_eq!(ctx, CompletionContext::LibraryUnits("rtl_lib".to_string()));

    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);
    let names = labels(&items);
    assert!(names.contains(&"cpu"), "expected cpu, got {names:?}");
    assert!(names.contains(&"uart_tx"), "expected uart_tx, got {names:?}");
    assert!(
        !names.contains(&"excluded"),
        "entity from another library leaked: {names:?}"
    );
}

#[test]
fn test_library_units_shallow_item_carries_resolve_data() {
    use crate::backend::AnalysisMap;
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.\n";
    let pos = Position {
        line: 2,
        character: 24,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();
    let entity_uri = Url::parse("file:///a.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(entity_uri.clone(), shallow_lib("rtl_lib", &["uart_tx"]));
    map.insert(top_uri.clone(), shallow_lib("rtl_lib", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();
    let ctx = get_completion_context(text, root, pos);
    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);

    let item = items
        .iter()
        .find(|i| i.label == "uart_tx")
        .expect("expected uart_tx in the list");
    assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
    let data = item
        .data
        .clone()
        .expect("shallow item should carry resolve data");
    assert_eq!(data["uri"], entity_uri.to_string());
    assert_eq!(data["name"], "uart_tx");
}

#[test]
fn test_library_units_deep_item_carries_no_resolve_data() {
    use crate::backend::AnalysisMap;
    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.\n";
    let pos = Position {
        line: 2,
        character: 24,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    map.insert(top_uri.clone(), shallow_lib("rtl_lib", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();
    let ctx = get_completion_context(text, root, pos);
    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);

    let item = items
        .iter()
        .find(|i| i.label == "uart_tx")
        .expect("expected uart_tx in the list");
    assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    assert!(item.data.is_none(), "deep item needs no resolve data");
}

#[test]
fn test_decode_entity_snippet_data_roundtrip() {
    let uri = Url::parse("file:///a.vhd").unwrap();
    let item = CompletionItem {
        data: Some(serde_json::json!({"uri": uri.to_string(), "name": "uart_tx"})),
        ..Default::default()
    };
    let (decoded_uri, decoded_name) =
        decode_entity_snippet_data(&item).expect("expected valid data to decode");
    assert_eq!(decoded_uri, uri);
    assert_eq!(decoded_name, "uart_tx");
}

#[test]
fn test_decode_entity_snippet_data_missing_returns_none() {
    let item = CompletionItem::default();
    assert!(decode_entity_snippet_data(&item).is_none());
}

#[test]
fn test_apply_entity_snippet_fills_in_snippet_once_deep() {
    use crate::backend::AnalysisMap;
    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let entity_uri = Url::parse("file:///a.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        entity_uri.clone(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );

    let item = CompletionItem {
        label: "uart_tx".to_string(),
        insert_text: Some("uart_tx".to_string()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        ..Default::default()
    };

    let resolved = apply_entity_snippet(item, &entity_uri, "uart_tx", &map);
    assert_eq!(resolved.insert_text_format, Some(InsertTextFormat::SNIPPET));
    assert!(
        resolved.insert_text.unwrap().contains("port map"),
        "expected the real port-map snippet"
    );
}

#[test]
fn test_apply_entity_snippet_still_shallow_returns_unchanged() {
    use crate::backend::AnalysisMap;
    let entity_uri = Url::parse("file:///a.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(entity_uri.clone(), shallow_lib("rtl_lib", &["uart_tx"]));

    let item = CompletionItem {
        label: "uart_tx".to_string(),
        insert_text: Some("uart_tx".to_string()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        data: Some(serde_json::json!({"uri": entity_uri.to_string(), "name": "uart_tx"})),
        ..Default::default()
    };

    let resolved = apply_entity_snippet(item, &entity_uri, "uart_tx", &map);
    assert_eq!(
        resolved.insert_text_format,
        Some(InsertTextFormat::PLAIN_TEXT)
    );
}

#[test]
fn test_work_prefix_lists_current_files_library() {
    use crate::backend::AnalysisMap;
    let text = "architecture rtl of top is\nbegin\n  u0: entity work.\n";
    let pos = Position {
        line: 2,
        character: 19,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        shallow_lib("rtl_lib", &["uart_tx"]),
    );
    map.insert(
        Url::parse("file:///b.vhd").unwrap(),
        shallow_lib("other_lib", &["excluded"]),
    );
    // The file being edited lives in rtl_lib, so `work.` means rtl_lib.
    map.insert(top_uri.clone(), shallow_lib("rtl_lib", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();

    let ctx = get_completion_context(text, root, pos);
    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);
    let names = labels(&items);
    assert!(names.contains(&"uart_tx"), "expected uart_tx, got {names:?}");
    assert!(
        !names.contains(&"excluded"),
        "work must not reach other_lib: {names:?}"
    );
}

#[test]
fn test_instantiation_library_completion_lists_libraries() {
    use crate::backend::AnalysisMap;
    let text = "architecture rtl of top is\nbegin\n  u0: entity \n";
    let pos = Position {
        line: 2,
        character: 13,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        shallow_lib("rtl_lib", &["uart_tx"]),
    );
    map.insert(top_uri.clone(), shallow_lib("work", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();

    let ctx = get_completion_context(text, root, pos);
    assert_eq!(ctx, CompletionContext::InstantiationLibrary);

    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);
    let names = labels(&items);
    assert!(names.contains(&"work"), "expected work, got {names:?}");
    assert!(
        names.contains(&"rtl_lib"),
        "expected rtl_lib, got {names:?}"
    );
}

// =========================================================================
// Workspace-wide instantiation snippets, direct form
// =========================================================================

/// Builds a deep-parsed Analysis in `library` with one entity that has ports.
fn deep_entity_analysis(library: &str, entity: &str, src: &str) -> crate::analysis::Analysis {
    let tree = crate::backend::test_utils::parse_text(src);
    let mut a = crate::backend::syntax::parser::extract_document_symbols(src, tree.root_node());
    a.library = library.to_string();
    assert!(
        a.entity_scope_trees.contains_key(entity),
        "fixture did not produce an entity scope tree for {entity}"
    );
    a
}

#[test]
fn test_instantiation_snippet_offered_for_entity_in_another_file() {
    use crate::backend::AnalysisMap;

    let sub_src = "entity uart_tx is\n  port (clk : in bit; tx : out bit);\nend entity;\n";
    let top_src = "architecture rtl of top is\nbegin\nend architecture;\n";

    let sub_uri = Url::parse("file:///sub.vhd").unwrap();
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(sub_uri, deep_entity_analysis("rtl_lib", "uart_tx", sub_src));
    let top_analysis = {
        let tree = crate::backend::test_utils::parse_text(top_src);
        let mut a =
            crate::backend::syntax::parser::extract_document_symbols(top_src, tree.root_node());
        a.library = "rtl_lib".to_string();
        a
    };
    map.insert(top_uri, top_analysis.clone());

    let items = generate_entity_completions(&map, &top_analysis);
    let names = labels(&items);
    assert!(
        names.contains(&"uart_tx"),
        "cross-file entity must be offered, got {names:?}"
    );
}

#[test]
fn test_same_library_entity_uses_work_prefix() {
    use crate::backend::AnalysisMap;

    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let top_src = "architecture rtl of top is\nbegin\nend architecture;\n";

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///sub.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    let top_analysis = {
        let tree = crate::backend::test_utils::parse_text(top_src);
        let mut a =
            crate::backend::syntax::parser::extract_document_symbols(top_src, tree.root_node());
        a.library = "rtl_lib".to_string();
        a
    };
    map.insert(Url::parse("file:///top.vhd").unwrap(), top_analysis.clone());

    let items = generate_entity_completions(&map, &top_analysis);
    let item = items
        .iter()
        .find(|i| i.label == "uart_tx")
        .expect("uart_tx must be offered");
    let text = item.insert_text.as_ref().expect("snippet must have text");
    assert!(
        text.starts_with("entity work.uart_tx"),
        "same-library entity should use the work prefix, got: {text}"
    );
}

#[test]
fn test_cross_library_entity_uses_explicit_library_prefix() {
    use crate::backend::AnalysisMap;

    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let top_src = "architecture rtl of top is\nbegin\nend architecture;\n";

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///sub.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    let top_analysis = {
        let tree = crate::backend::test_utils::parse_text(top_src);
        let mut a =
            crate::backend::syntax::parser::extract_document_symbols(top_src, tree.root_node());
        a.library = "top_lib".to_string();
        a
    };
    map.insert(Url::parse("file:///top.vhd").unwrap(), top_analysis.clone());

    let items = generate_entity_completions(&map, &top_analysis);
    let item = items
        .iter()
        .find(|i| i.label == "uart_tx")
        .expect("uart_tx must be offered");
    let text = item.insert_text.as_ref().expect("snippet must have text");
    assert!(
        text.starts_with("entity rtl_lib.uart_tx"),
        "cross-library entity needs an explicit prefix, got: {text}"
    );
}

#[test]
fn test_entity_snippet_is_deduplicated_across_files() {
    use crate::backend::AnalysisMap;

    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let top_src = "architecture rtl of top is\nbegin\nend architecture;\n";

    let mut map = AnalysisMap::new();
    // Same entity name declared twice in the same library.
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    map.insert(
        Url::parse("file:///b.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    let top_analysis = {
        let tree = crate::backend::test_utils::parse_text(top_src);
        let mut a =
            crate::backend::syntax::parser::extract_document_symbols(top_src, tree.root_node());
        a.library = "rtl_lib".to_string();
        a
    };
    map.insert(Url::parse("file:///top.vhd").unwrap(), top_analysis.clone());

    let items = generate_entity_completions(&map, &top_analysis);
    let count = items.iter().filter(|i| i.label == "uart_tx").count();
    assert_eq!(count, 1, "duplicate entity must be offered once");
}
