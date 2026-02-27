use super::*;
use crate::backend::test_utils::{SHARED_PARSER_LOCK, parse_text};
use tower_lsp::lsp_types::Url;
use tree_sitter::Parser;

/// Helper to extract cursor position from code with | marker
fn extract_cursor(code_with_cursor: &str) -> (String, Position) {
    let lines: Vec<&str> = code_with_cursor.lines().collect();
    let mut line_num = 0;
    let mut char_pos = 0;
    let mut found = false;

    for (i, line) in lines.iter().enumerate() {
        if let Some(pos) = line.find('|') {
            line_num = i;
            char_pos = pos;
            found = true;
            break;
        }
    }

    assert!(found, "No cursor marker '|' found in test code");

    let code = code_with_cursor.replace('|', "");
    let position = Position {
        line: line_num as u32,
        character: char_pos as u32,
    };

    (code, position)
}

// =============================================================================
// Tests for prepare_rename()
// =============================================================================

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_prepare_rename_signal_declaration() {
    let code_with_cursor = r#"
architecture rtl of test is
    signal my_sig|nal : std_logic;
begin
    my_signal <= '1';
end architecture;
"#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);

    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        prepare_rename(&code, &pos, &analysis, &mut parser).await
    };

    assert!(
        result.is_some(),
        "Should be able to rename signal declaration"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_prepare_rename_signal_usage() {
    let code_with_cursor = r#"
architecture rtl of test is
    signal my_signal : std_logic;
begin
    my_sig|nal <= '1';
end architecture;
"#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);

    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        prepare_rename(&code, &pos, &analysis, &mut parser).await
    };

    assert!(result.is_some(), "Should be able to rename signal usage");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_prepare_rename_keyword() {
    let code_with_cursor = r#"
architecture rtl of test is
    sig|nal my_signal : std_logic;
begin
end architecture;
"#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);

    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        prepare_rename(&code, &pos, &analysis, &mut parser).await
    };

    assert!(result.is_none(), "Should NOT be able to rename keyword");
}

// =============================================================================
// Tests for rename_symbol()
// =============================================================================

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_rename_signal_in_architecture() {
    let code_with_cursor = r#"
architecture rtl of test is
    signal old_na|me : std_logic;
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

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);
    let uri = Url::parse("file:///test.vhd").unwrap();

    // Use full analysis instead of single scope tree
    // Handle poisoned lock (from other test panics)
    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        rename_symbol(&code, &pos, "new_name", &analysis, &uri, &mut parser).await
    };

    assert!(result.is_some(), "Rename should succeed");

    let workspace_edit = result.unwrap();
    let changes = workspace_edit.changes.unwrap();
    let edits = changes.get(&uri).unwrap();

    // Should have 3 edits: declaration + 2 usages
    assert_eq!(edits.len(), 3, "Should rename declaration and all usages");

    // All edits should replace with "new_name"
    for edit in edits {
        assert_eq!(edit.new_text, "new_name");
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_rename_with_invalid_identifier() {
    let code_with_cursor = r#"
architecture rtl of test is
    signal my_sig|nal : std_logic;
begin
end architecture;
"#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);
    let uri = Url::parse("file:///test.vhd").unwrap();

    // Use full analysis instead of single scope tree
    // Handle poisoned lock (from other test panics)
    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        rename_symbol(&code, &pos, "123invalid", &analysis, &uri, &mut parser).await
    };

    assert!(
        result.is_none(),
        "Should reject invalid identifier starting with digit"
    );
}

// =============================================================================
// Tests for is_valid_vhdl_identifier()
// =============================================================================

#[test]
fn test_valid_identifiers() {
    assert!(super::is_valid_vhdl_identifier("signal_name"));
    assert!(super::is_valid_vhdl_identifier("MySignal"));
    assert!(super::is_valid_vhdl_identifier("a"));
    assert!(super::is_valid_vhdl_identifier("signal123"));
    assert!(super::is_valid_vhdl_identifier("my_signal_name"));
    assert!(super::is_valid_vhdl_identifier("CLK"));
    assert!(super::is_valid_vhdl_identifier("data_in_0"));
}

#[test]
fn test_invalid_identifiers() {
    // Empty
    assert!(!super::is_valid_vhdl_identifier(""));

    // Starts with digit
    assert!(!super::is_valid_vhdl_identifier("123signal"));
    assert!(!super::is_valid_vhdl_identifier("0data"));

    // Starts with underscore
    assert!(!super::is_valid_vhdl_identifier("_signal"));

    // Ends with underscore
    assert!(!super::is_valid_vhdl_identifier("signal_"));
    assert!(!super::is_valid_vhdl_identifier("my_name_"));

    // Consecutive underscores
    assert!(!super::is_valid_vhdl_identifier("my__signal"));
    assert!(!super::is_valid_vhdl_identifier("data___out"));

    // Invalid characters
    assert!(!super::is_valid_vhdl_identifier("my-signal"));
    assert!(!super::is_valid_vhdl_identifier("signal.name"));
    assert!(!super::is_valid_vhdl_identifier("my signal"));
    assert!(!super::is_valid_vhdl_identifier("data@input"));

    // Reserved keywords (case-insensitive)
    assert!(!super::is_valid_vhdl_identifier("signal"));
    assert!(!super::is_valid_vhdl_identifier("SIGNAL"));
    assert!(!super::is_valid_vhdl_identifier("Signal"));
    assert!(!super::is_valid_vhdl_identifier("process"));
    assert!(!super::is_valid_vhdl_identifier("entity"));
    assert!(!super::is_valid_vhdl_identifier("architecture"));
    assert!(!super::is_valid_vhdl_identifier("begin"));
    assert!(!super::is_valid_vhdl_identifier("end"));
    assert!(!super::is_valid_vhdl_identifier("if"));
    assert!(!super::is_valid_vhdl_identifier("for"));
    assert!(!super::is_valid_vhdl_identifier("while"));
    assert!(!super::is_valid_vhdl_identifier("variable"));
    assert!(!super::is_valid_vhdl_identifier("constant"));
}

// =============================================================================
// Tests for rename_symbol() with rename_port
// =============================================================================

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_rename_port() {
    let code_with_cursor = r#"
entity test is
    port (
        cl|k : in std_logic;
        data : out std_logic
    );
end entity;

architecture rtl of test is
begin
    process(clk)
    begin
        if rising_edge(clk) then
            data <= '1';
        end if;
    end process;
end architecture;
"#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);
    let uri = Url::parse("file:///test.vhd").unwrap();

    // Rename function will search both entity_scope_trees and scope_trees
    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        rename_symbol(&code, &pos, "clock", &analysis, &uri, &mut parser).await
    };

    assert!(result.is_some(), "Should be able to rename port");

    let workspace_edit = result.unwrap();
    let changes = workspace_edit.changes.unwrap();
    let edits = changes.get(&uri).unwrap();

    // Should rename declaration + usages in process
    assert!(
        edits.len() >= 2,
        "Should rename port declaration and usages"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_rename_generic() {
    let code_with_cursor = r#"
entity test is
    generic (
        DATA_W|IDTH : integer := 8
    );
    port (
        data_in : in std_logic_vector(DATA_WIDTH-1 downto 0)
    );
end entity;

architecture rtl of test is
    signal temp : std_logic_vector(DATA_WIDTH-1 downto 0);
begin
    gen_width: if DATA_WIDTH > 4 generate
        temp <= data_in;
    end generate;
end architecture;
"#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);
    let uri = Url::parse("file:///test.vhd").unwrap();

    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        rename_symbol(&code, &pos, "BUS_WIDTH", &analysis, &uri, &mut parser).await
    };

    assert!(result.is_some(), "Should be able to rename generic");

    let workspace_edit = result.unwrap();
    let changes = workspace_edit.changes.unwrap();
    let edits = changes.get(&uri).unwrap();

    // Should rename:
    // 1. Generic declaration (WIDTH)
    // 2. Usage in port (WIDTH-1)
    // 3. Usage in signal type (WIDTH-1)
    // 4. Usage in generate condition (WIDTH > 4)
    assert!(
        edits.len() >= 4,
        "Should rename generic declaration and all usages (found {})",
        edits.len()
    );

    // All edits should replace with "BUS_WIDTH"
    for edit in edits {
        assert_eq!(edit.new_text, "BUS_WIDTH");
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_rename_signal_should_not_rename_port_names() {
    let code_with_cursor = r#"
entity some_module is
    port (
        old_name : in std_logic;
        output : out std_logic
    );
end entity;

architecture rtl of test is
    signal old_n|ame : std_logic;
    signal data : std_logic;
begin
    u_inst : entity work.some_module
        port map (
            old_name => data,
            output => open
        );
    old_name <= '1';
end architecture;
"#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);
    let uri = Url::parse("file:///test.vhd").unwrap();

    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        rename_symbol(&code, &pos, "new_signal", &analysis, &uri, &mut parser).await
    };

    assert!(result.is_some(), "Should be able to rename signal");

    let workspace_edit = result.unwrap();
    let changes = workspace_edit.changes.unwrap();
    let edits = changes.get(&uri).unwrap();

    // Should rename:
    // 1. Signal declaration (old_name : std_logic)
    // 2. Signal usage (old_name <= '1')
    // Should NOT rename:
    // 3. Port name in port map (old_name => data) - this refers to the entity's port
    assert_eq!(
        edits.len(),
        2,
        "Should only rename signal declaration and usage, not port names in port map. Got {} edits",
        edits.len()
    );

    // All edits should replace with "new_signal"
    for edit in edits {
        assert_eq!(edit.new_text, "new_signal");
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_rename_generic_used_in_signal_type() {
    let code_with_cursor = r#"
entity test is
    generic (
        BUS_WI|DTH : integer := 8
    );
end entity;

architecture rtl of test is
    signal data : std_logic_vector(BUS_WIDTH-1 downto 0);
begin
    data <= (others => '0');
end architecture;
"#;

    let (code, pos) = extract_cursor(code_with_cursor);
    let tree = parse_text(&code);
    let root = tree.root_node();
    let analysis = crate::backend::syntax::parser::extract_document_symbols(&code, root);
    let uri = Url::parse("file:///test.vhd").unwrap();

    let result = {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        rename_symbol(&code, &pos, "SIZE", &analysis, &uri, &mut parser).await
    };

    assert!(result.is_some(), "Should be able to rename generic");

    let workspace_edit = result.unwrap();
    let changes = workspace_edit.changes.unwrap();
    let edits = changes.get(&uri).unwrap();

    // Should rename:
    // 1. Generic declaration (BUS_WIDTH : integer)
    // 2. Usage in signal type (BUS_WIDTH-1 downto 0)
    assert_eq!(
        edits.len(),
        2,
        "Should rename generic declaration and usage in signal type. Got {} edits",
        edits.len()
    );

    // All edits should replace with "SIZE"
    for edit in edits {
        assert_eq!(edit.new_text, "SIZE");
    }
}
