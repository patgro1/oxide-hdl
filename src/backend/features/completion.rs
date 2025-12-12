use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind, Position,
};
use tree_sitter::{Node, Point};

use crate::analysis::{Analysis, OxideSymbolKind, Symbol};

/// Representation of the context of a completion
#[derive(Debug, PartialEq, Eq)]
pub enum CompletionContext {
    /// Inside an architecture, outside process or blocks
    /// Suggest: Signals, Components, Instantiations
    Architecture,

    /// Inside a process
    /// Suggest: Variables, Signals, Constants
    Process,

    /// We are completing a record field or package name
    DotAccess,

    // TODO: Have a seperate context for left and right of the =>
    // We are inside a port map
    // Suggest: Ports of the component
    PortMap,

    // TODO: Have a seperate context for left and right of the =>
    /// We are inside generic map
    /// Sugest: Generics of the compoenent
    GenericMap,

    /// Fallback for unknown or global scopes
    Unresolved,
}

/// Determines the semantic context of the cursor position by analyzing the AST.
///
/// This function converts the LSP position into a Tree-sitter point, finds the
/// smallest syntax node at that location, and walks up the tree to identify
/// the enclosing scope (e.g., Architecture, Process).
///
/// # Logic
/// 1. **Cursor Mapping:** Converts 0-indexed LSP `Position` to Tree-sitter `Point`.
/// 2. **Node Traversal:** Finds the node covering the cursor.
/// 3. **Dot Detection:** Checks if the node (or its immediate predecessor) indicates
///    a dot-access (record field or package access).
/// 4. **Scope Walking:** Climbs the AST parents to find defining scopes like
///    `process_statement` or `architecture_body`.
///
/// # Arguments
///
/// * `text` - The full source code of the file (used for text extraction if needed).
/// * `root` - The root node of the Tree-sitter syntax tree.
/// * `pos` - The cursor position from the LSP request.
///
/// # Returns
///
/// A `CompletionContext` enum variant describing the detected scope.
pub fn get_completion_context(text: &str, root: Node, pos: Position) -> CompletionContext {
    let point = Point {
        row: pos.line as usize,
        column: pos.character as usize,
    };

    let node = match root.descendant_for_point_range(point, point) {
        Some(n) => n,
        None => return CompletionContext::Unresolved,
    };

    // Check for dot access.
    // If the cursor is right after the dot, the node might be the dot itself or the identifier
    // following it. Tree-sitter structure for `rec.field` is usually `selected_name`
    let walker = node;
    let kind = walker.kind();
    let parent_kind = walker.parent().map(|p| p.kind()).unwrap_or("");
    if kind == "." || parent_kind == "selected_name" {
        return CompletionContext::DotAccess;
    }

    // Selected name is the proper vhdl thing but when the tree is broken, it might be selection
    if parent_kind == "selected_name" || parent_kind == "selection" {
        return CompletionContext::DotAccess;
    }

    // Check if the character before was a dot. Use this as a fallback in case tree sitter
    // structure is broken or incoherent. Looks back for the . multiple characters
    if let Some(line) = text.lines().nth(pos.line as usize)
        && pos.character as usize <= line.len()
    {
        let prefix = &line[..pos.character as usize];
        let trimmed = prefix.trim_end_matches(|c: char| c.is_alphanumeric());
        if trimmed.ends_with('.') {
            return CompletionContext::DotAccess;
        }
    }

    // Walk the tree to find the scope
    let mut current = Some(node);
    while let Some(n) = current {
        println!("{:?} ( {} )", n, n.kind());
        match n.kind() {
            "process_statement" => return CompletionContext::Process,
            "subprogram_body" => return CompletionContext::Process,

            "architecture_definition" => return CompletionContext::Architecture,
            "architecture_body" => return CompletionContext::Architecture,
            "block_statement" => return CompletionContext::Architecture,
            "generate_statement" => return CompletionContext::Architecture,
            "concurrent_block" => return CompletionContext::Architecture,

            "component_instantiation_statement" => return CompletionContext::PortMap,
            _ => {}
        }
        current = n.parent();
    }

    CompletionContext::Unresolved
}

/// Generates a list of completions for the current file scope.
///
/// For V1, this is a "flat" completion. It grabs every Signal, Port, Constant,
/// and Component declared` in the file and offers them as suggestions.
///
/// # Arguments
/// * `analysis` - The deep analysis of the current file.
/// # Return
/// Vector of completion items
pub fn complete_local_scope(analysis: &Analysis) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Iterate over all top-level symbols
    for sym in analysis.symbols.values() {
        collect_symbols(sym, &mut items);
    }

    items
}

/// Recursive helper to flatten the symbol tree into suggestions.
/// # Arguments
/// * `symbol` - The top level symbol we want to collect symbols from
/// * `items` - Mutable reference to the a vector of completion items
fn collect_symbols(symbol: &Symbol, items: &mut Vec<CompletionItem>) {
    // Current symbol is a completion item
    if let Some(item) = symbol_to_completion(symbol) {
        items.push(item);
    }

    // Recurse into children
    for child in &symbol.children {
        collect_symbols(child, items);
    }
}

fn symbol_to_completion(symbol: &Symbol) -> Option<CompletionItem> {
    let kind = match symbol.kind {
        OxideSymbolKind::Entity => CompletionItemKind::INTERFACE,
        OxideSymbolKind::Architecture => return None, // Don't suggest Arch names in code usually
        OxideSymbolKind::Port => CompletionItemKind::FIELD,
        OxideSymbolKind::Signal => CompletionItemKind::VARIABLE,
        OxideSymbolKind::Constant => CompletionItemKind::CONSTANT,
        OxideSymbolKind::Struct => CompletionItemKind::STRUCT,
        OxideSymbolKind::Function => CompletionItemKind::FUNCTION,
        OxideSymbolKind::Process => return None, // Don't suggest Process labels usually
        OxideSymbolKind::Component => CompletionItemKind::CLASS,
        _ => CompletionItemKind::TEXT,
    };
    Some(CompletionItem {
        label: symbol.name.clone(),
        kind: Some(kind),
        detail: symbol.detail.clone(),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("**{}** ({})", symbol.name, symbol.kind),
        })),
        ..CompletionItem::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    use crate::backend::test_utils::SHARED_PARSER_LOCK;

    fn check_context(code_with_cursor: &str, expected: CompletionContext) {
        // 2. ACQUIRE LOCK
        // The lock is held until `_guard` goes out of scope at the end of the function.
        // This blocks other threads from entering this function until we are done.
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();

        let (code, pos) = extract_cursor(code_with_cursor);

        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(&code, None).unwrap();

        // Debug print (Safe to uncomment now, output won't interleave!)
        println!("Cursor is at: [{},{}]", pos.line, pos.character);
        debug_print_tree(tree.root_node(), 0);

        let ctx = get_completion_context(&code, tree.root_node(), pos);

        assert_eq!(ctx, expected, "\nCode:\n{}\nContext mismatch!", code);
    }

    fn debug_print_tree(node: Node, depth: usize) {
        let indent = "  ".repeat(depth);
        let start = node.start_position();
        let end = node.end_position();

        println!(
            "{}{} [{}:{}] - [{}:{}]",
            indent,
            node.kind(),
            start.row,
            start.column,
            end.row,
            end.column
        );

        // Recursive walk
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            debug_print_tree(child, depth + 1);
        }
    }

    fn extract_cursor(text: &str) -> (String, Position) {
        // Find the cursor
        let cursor_offset = text
            .find('|')
            .expect("Test case must have a '|' cursor marker");

        // Remove the cursor marker to get the "Clean" code
        let clean_text = text.replace("|", "");

        // Calculate the Position (Line/Col) based on the ORIGINAL text up to the cursor
        // We iterate chars to be Unicode-safe and handle newlines correctly.
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

    // --- STANDARD TESTS ---

    #[test]
    fn test_context_architecture_body() {
        let src = r#"
entity E is end E;
architecture A of E is
    signal s : bit;
begin
    s <= | 
end A;
"#;
        check_context(src, CompletionContext::Architecture);
    }

    #[test]
    fn test_context_process_body() {
        let src = r#"
architecture A of E is
begin
    process(clk)
        variable v : integer;
    begin
        v := |
    end process;
end A;
"#;
        check_context(src, CompletionContext::Process);
    }

    #[test]
    fn test_context_dot_access() {
        check_context(
            "architecture A of E is begin r.| end A;",
            CompletionContext::DotAccess,
        );
    }

    #[test]
    fn test_context_dot_access_mid_type() {
        check_context(
            "architecture A of E is begin r.fi| end A;",
            CompletionContext::DotAccess,
        );
    }

    // --- DEEP NESTING TESTS ---

    #[test]
    fn test_nested_block_architecture() {
        // Blocks are concurrent regions -> Architecture Context
        let src = r#"
architecture A of B is
begin
    my_block: block
    begin
        |
    end block;
end A;
"#;
        check_context(src, CompletionContext::Architecture);
    }

    #[test]
    fn test_nested_process_in_block() {
        // Arch -> Block -> Process -> Process Context
        let src = r#"
architecture A of B is
begin
    block_name: block
    begin
        process(clk)
        begin
            if rising_edge(clk) then
                |
            end if;
        end process;
    end block;
end A;
"#;
        check_context(src, CompletionContext::Process);
    }

    #[test]
    fn test_generate_statement() {
        // Generate loops are concurrent regions
        let src = r#"
architecture A of B is
begin
    gen_loop: for i in 0 to 10 generate
        |
    end generate;
end A;
"#;
        check_context(src, CompletionContext::Architecture);
    }

    // --- BROKEN SYNTAX TESTS ---

    #[test]
    fn test_broken_process_decl() {
        // User is typing a variable declaration but hasn't finished.
        // Tree-sitter often produces ERROR nodes here.
        let src = r#"
process(clk)
    variable x : |
begin
end process;
"#;
        check_context(src, CompletionContext::Process);
    }

    #[test]
    fn test_broken_architecture_assignment() {
        // Missing semicolon, messy code.
        let src = r#"
architecture A of B is
    signal s : std_logic;
begin
    s <= '1' when |
end A;
"#;
        check_context(src, CompletionContext::Architecture);
    }

    #[test]
    fn test_port_map_association() {
        // Inside the parens of a port map.
        let src = r#"
architecture A of B is
begin
    u_inst : entity work.comp
        port map (
            clk => |
        );
end A;
"#;
        // Note: You might need to add "association_list" to your matcher if this fails!
        check_context(src, CompletionContext::PortMap);
    }
}
