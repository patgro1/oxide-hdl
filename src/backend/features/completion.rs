use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind, Position, Range,
    Url,
};
use tree_sitter::{Node, Point};

use crate::analysis::{OxideSymbolKind, Symbol};
use crate::backend::AnalysisMap;

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

    // We are inside a port map before the =>. Payload: Component Name
    // Suggest: Ports from the component
    PortMapLhs(String),

    // We are inside a port map after the =>
    // Suggest: Signals from the current scope
    PortMapRhs,

    // TODO: Have a seperate context for left and right of the =>
    /// We are inside generic map
    /// Sugest: Generics of the compoenent
    GenericMap,
    GenericMapLhs(String),
    GenericMapRhs,

    /// Fallback for unknown or global scopes
    Unresolved,
}

/// Helper function to extract the instantiated unit name from a component instantiation node.
/// e.g. "u0: entity work.my_comp" -> returns "my_comp"
/// # Arguments
/// `node` the node of the component instantiation
/// `text` the text representing the parsed file
/// # Return
/// OK(String) with the valid instantiated unit name
/// None if the extraction failed
fn get_instantiated_name(node: Node, text: &str) -> Option<String> {
    // 1. Try to find the complex "instantiated_unit" (e.g., "entity work.comp")
    // Strategy: Look for field "unit" OR child kind "instantiated_unit"
    let unit_node = node.child_by_field_name("unit").or_else(|| {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| child.kind() == "instantiated_unit")
    });

    if let Some(unit_node) = unit_node {
        // --- CASE 1: Entity Instantiation ---
        let raw_text = unit_node.utf8_text(text.as_bytes()).ok()?;

        // Clean up "entity work.my_comp(rtl)"
        let trimmed = raw_text.trim();
        let remainder = if trimmed.to_lowercase().starts_with("entity") {
            trimmed[6..].trim()
        } else {
            trimmed
        };
        let text_no_arch = remainder.split('(').next().unwrap_or(remainder);
        let final_name = text_no_arch.split('.').next_back().unwrap_or(text_no_arch);

        let res = final_name.trim().to_string();
        return Some(res);
    }

    // 2. Fallback: Try to find a simple "name" node (e.g., "my_comp")
    // This happens in: "u1 : my_comp port map..."
    // Strategy: Look for child kind "name" (which holds the identifier)
    let mut cursor = node.walk();
    if let Some(name_node) = node.children(&mut cursor).find(|c| c.kind() == "name") {
        // --- CASE 2: Simple Component Instantiation ---
        let raw_text = name_node.utf8_text(text.as_bytes()).ok()?;

        // Even simple components might have an architecture suffix: "my_comp(rtl)"
        let text_no_arch = raw_text.split('(').next().unwrap_or(raw_text);

        let res = text_no_arch.trim().to_string();
        return Some(res);
    }

    // Print children to see what IS there (for future debugging)
    let mut cursor = node.walk();

    None
}

/// Walks up the tree to find the component name.
/// Falls back to text search if the tree is incomplete (ERROR nodes).
fn find_component_name(mut node: Node, text: &str) -> Option<String> {
    // 1. Try Tree Traversal
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "component_instantiation_statement" {
            if let Some(name) = get_instantiated_name(n, text) {
                return Some(name);
            }
            break;
        }
        current = n.parent();
    }

    // 2. Fallback: Text Search
    // CRITICAL FIX: Use end_byte(), not start_byte().
    // If we are inside a broken ERROR node, the text "entity ... port map" is INSIDE the node,
    // so we need to search the text up to the end of the node, not just what comes before it.
    let search_limit = node.end_byte().min(text.len());
    let code_range = &text[..search_limit];

    if let Some(port_map_idx) = code_range.rfind("port map") {
        let slice = &code_range[..port_map_idx];

        if let Some(colon_idx) = slice.rfind(':') {
            let raw_decl = &slice[colon_idx + 1..];
            let raw_text = raw_decl.trim();

            let mut parts = raw_text.split_whitespace();
            let first_word = parts.next().unwrap_or("");

            let name_part = if first_word.eq_ignore_ascii_case("entity") {
                if let Some(entity_idx) = raw_text.to_lowercase().find("entity") {
                    let after = &raw_text[entity_idx + 6..];
                    after.trim()
                } else {
                    parts.next().unwrap_or("")
                }
            } else {
                raw_text
            };

            let text_no_arch = name_part.split('(').next().unwrap_or(name_part);
            let final_name = text_no_arch.split('.').next_back().unwrap_or(text_no_arch);

            let final_clean = final_name.trim();
            if !final_clean.is_empty()
                && final_clean.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                return Some(final_clean.to_string());
            }
        }
    }

    None
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
        match n.kind() {
            "process_statement" | "subprogram_body" => return CompletionContext::Process,

            "architecture_definition"
            | "architecture_body"
            | "block_statement"
            | "generate_statement"
            | "concurrent_block" => return CompletionContext::Architecture,

            // "component_instantiation_statement" => return CompletionContext::PortMap,
            "association_element" => {
                let cursor_col = pos.character as usize;
                let mut is_rhs = false;
                let mut cursor = n.walk();
                for child in n.children(&mut cursor) {
                    if child.kind() == "=>" {
                        // If cursor is passed the arrow, we are RHS
                        let arrow_end = child.end_position().column;
                        if cursor_col >= arrow_end {
                            is_rhs = true;
                        }
                    }
                }

                if is_rhs {
                    return CompletionContext::PortMapRhs;
                } else {
                    if let Some(name) = find_component_name(n, text) {
                        return CompletionContext::PortMapLhs(name); // Fallback
                    }
                    return CompletionContext::PortMapRhs;
                }
            }
            // This is hit when cursor is between elements or immediatly after a broken element
            "association_list" => {
                let mut is_rhs = false; // Default: Expecting new port (LHS)
                let mut cursor_chk = n.walk();

                for child in n.children(&mut cursor_chk) {
                    let start = child.start_position();
                    let end = child.end_position();

                    // --- STOP CONDITION FIX ---
                    // Stop if child starts strictly after cursor.
                    // CRITICAL: If child starts AT cursor and is a comma, stop.
                    // This handles "clk => |," -> we are completing the value before the comma.
                    let starts_at_cursor = start.row == point.row && start.column == point.column;
                    let starts_after_cursor = start.row > point.row
                        || (start.row == point.row && start.column > point.column);

                    if starts_after_cursor || (starts_at_cursor && child.kind() == ",") {
                        // Check for broken " | =>" case (Error starts after cursor)
                        if starts_after_cursor && child.kind() == "ERROR" {
                            let mut sub = child.walk();
                            for sub_child in child.children(&mut sub) {
                                if sub_child.kind() == "=>" {
                                    // Writing LHS
                                }
                            }
                        }
                        break;
                    }

                    if child.kind() == "," {
                        is_rhs = false;
                        continue;
                    }

                    if child.kind() == "association_element" {
                        let mut has_arrow = false;
                        let mut has_rhs = false;
                        let mut sub = child.walk();
                        for sub_child in child.children(&mut sub) {
                            if sub_child.kind() == "=>" {
                                has_arrow = true;
                            } else if has_arrow {
                                has_rhs = true;
                            }
                        }

                        // Strict check: Are we physically INSIDE this element?
                        let is_inside = if point.row < start.row || point.row > end.row {
                            false
                        } else if point.row == start.row && point.column < start.column {
                            false
                        } else if point.row == end.row && point.column > end.column {
                            false
                        } else {
                            true
                        };

                        if is_inside {
                            if has_arrow {
                                let mut arrow_end = 0;
                                let mut s2 = child.walk();
                                for sc in child.children(&mut s2) {
                                    if sc.kind() == "=>" {
                                        arrow_end = sc.end_position().column;
                                    }
                                }
                                is_rhs = point.column >= arrow_end;
                            } else {
                                is_rhs = false;
                            }
                            break;
                        }

                        // We are PAST this element.
                        if has_arrow && !has_rhs {
                            is_rhs = true;
                        } else {
                            is_rhs = false;
                        }
                    }

                    if child.kind() == "ERROR" {
                        let mut sub = child.walk();
                        for sub_child in child.children(&mut sub) {
                            if sub_child.kind() == "=>" {
                                if point.column >= sub_child.end_position().column {
                                    is_rhs = true;
                                }
                            }
                        }
                    }
                }

                if is_rhs {
                    return CompletionContext::PortMapRhs;
                } else {
                    if let Some(name) = find_component_name(n, text) {
                        return CompletionContext::PortMapLhs(name);
                    }
                    return CompletionContext::PortMapRhs;
                }
            }
            "component_instantiation_statement" | "port_map_aspect" => {
                if let Some(name) = find_component_name(n, text) {
                    return CompletionContext::PortMapLhs(name);
                }
            }

            _ => {}
        }
        current = n.parent();
    }

    CompletionContext::Unresolved
}

/// Generates completion items based on the global context.
///
/// # Arguments
/// * `analysis` - The deep analysis of the current file.
/// # Return
/// Vector of completion items
pub fn complete_scope(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    context: &CompletionContext,
    position: Position,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    if let Some(current_analysis) = analysis_map.get(current_uri) {
        // If we are on the LHS of a compnent instantiation,
        // we need to fetch the component so we iterate all files
        // to figure out where it is
        if let CompletionContext::PortMapLhs(target_name) = context {
            eprintln!("DEBUG: Completion looking for component '{}'", target_name);

            // Check if it exists in the map RIGHT NOW
            let found = analysis_map
                .values()
                .any(|a| a.symbols.values().any(|s| s.name == *target_name));

            if found {
                eprintln!("DEBUG: Symbol FOUND in analysis map.");
            } else {
                eprintln!("DEBUG: Symbol NOT FOUND. (This explains why completion failed)");
            }
            for analysis in analysis_map.values() {
                if let Some(target_sym) = analysis
                    .symbols
                    .values()
                    .find(|s| s.name.eq_ignore_ascii_case(target_name))
                {
                    collect_ports(target_sym, &mut items);
                }
            }
            return items;
        }
        // Iterate over all top-level symbols
        for sym in current_analysis.symbols.values() {
            collect_symbols(sym, context, position, &mut items);
        }
    }

    // Clean up duplicates
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);

    items
}

/// Recursive helper to extract ONLY the ports and generics from a symbol (for LHS completion)
/// # Arguments
/// * `symbol` - The top level symbol we want to collect symbols from
/// * `items` - Mutable reference to the a vector of completion items
fn collect_ports(symbol: &Symbol, items: &mut Vec<CompletionItem>) {
    if (symbol.kind == OxideSymbolKind::Port || symbol.kind == OxideSymbolKind::Generic)
        && let Some(item) = symbol_to_completion(symbol)
    {
        items.push(item);
    }
    for child in &symbol.children {
        collect_ports(child, items);
    }
}

/// Recursive helper to flatten the symbol tree into suggestions.
/// # Arguments
/// * `symbol` - The top level symbol we want to collect symbols from
/// * `items` - Mutable reference to the a vector of completion items
fn collect_symbols(
    symbol: &Symbol,
    context: &CompletionContext,
    position: Position,
    items: &mut Vec<CompletionItem>,
) {
    let is_strict_scope = matches!(
        symbol.kind,
        OxideSymbolKind::Architecture
            | OxideSymbolKind::Block
            | OxideSymbolKind::Generate
            | OxideSymbolKind::Process
    );
    // Check if the symbol is outside the container and stop to
    // prevent poluting the suggestions with out-of-scope values.
    if is_strict_scope && !range_contains(symbol.range, position) {
        return;
    }
    // Current symbol is a completion item
    // We usually do not suggest the container itself but we check visibility
    if is_visible(symbol, context)
        && !is_strict_scope // Dont suggest the arcvhitecture name itself...
        && let Some(item) = symbol_to_completion(symbol)
    {
        items.push(item);
    }

    // Recurse into children
    for child in &symbol.children {
        collect_symbols(child, context, position, items);
    }
}

/// Helper function that gives the completion visibility of a symbol
/// depending on the context of the completion.
/// # Arguments
/// * `symbol` - The symbol on which we are requesting visibility
/// * `context` - The current completion context
/// # Return
/// True if the symbol should be visible in the completion
fn is_visible(sym: &Symbol, context: &CompletionContext) -> bool {
    match context {
        CompletionContext::Architecture => match sym.kind {
            OxideSymbolKind::Component => true,
            OxideSymbolKind::Entity => true,
            OxideSymbolKind::Package => true,
            OxideSymbolKind::Constant => true,
            OxideSymbolKind::Port => true,
            OxideSymbolKind::Signal => true,
            OxideSymbolKind::Variable => false,
            _ => false,
        },
        CompletionContext::Process => match sym.kind {
            OxideSymbolKind::Signal => true,
            OxideSymbolKind::Variable => true,
            OxideSymbolKind::Constant => true,
            OxideSymbolKind::Port => true,
            OxideSymbolKind::Component => false,
            _ => false,
        },
        CompletionContext::PortMapRhs => {
            matches!(
                sym.kind,
                OxideSymbolKind::Signal | OxideSymbolKind::Constant | OxideSymbolKind::Port
            )
        }
        _ => true,
    }
}

/// Checks if a position is contained within a range
/// # Arguments
/// * `range` The range the position needs to be checked against
/// * 'position` The position we want to check
/// # Return
/// True of the position is within the range
fn range_contains(range: Range, position: Position) -> bool {
    if position.line < range.start.line || position.line > range.end.line {
        return false;
    }

    // edge case on start line
    if position.line == range.start.line && position.character < range.start.character {
        return false;
    }

    // edge case on end line
    if position.character == range.end.line && position.character > range.end.character {
        return false;
    }
    true
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
        drop(_guard);

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
        check_context(src, CompletionContext::PortMapRhs);
    }
    #[test]
    fn test_port_map_lhs_detection() {
        // Cursor is on the LEFT of the arrow.
        // Should detect we are mapping the component "my_comp"
        let src = r#"
     architecture A of B is
     begin
         u0 : entity work.my_comp
             port map (
                 clk_in => clk,
                 | => rst
             );
     end A;
     "#;
        // We expect it to capture the component name "my_comp"
        // Note: The parser logic usually extracts just the name, stripping "entity work."
        check_context(src, CompletionContext::PortMapLhs("my_comp".to_string()));
    }

    #[test]
    fn test_port_map_rhs_detection() {
        // Cursor is on the RIGHT of the arrow.
        // Should detect we are looking for a local signal.
        let src = r#"
     architecture A of B is
     begin
         u0 : entity work.my_comp
             port map (
                 clk_in => |,
                 rst_in => rst
             );
     end A;
     "#;
        check_context(src, CompletionContext::PortMapRhs);
    }
    #[test]
    fn test_port_map_empty_line_should_be_lhs() {
        // Cursor is on the RIGHT of the arrow.
        // Should detect we are looking for a local signal.
        let src = r#"
     architecture A of B is
     begin
         u0 : entity work.my_comp
             port map (
                 clk_in => clk,
                 rst_in => rst
                 |
             );
     end A;
     "#;
        check_context(src, CompletionContext::PortMapLhs("my_comp".to_string()));
    }
    fn check_name(code: &str, expected_name: Option<&str>) {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(code, None).unwrap();
        drop(_guard);

        let root = tree.root_node();

        println!("--- DEBUG TREE START ---");
        debug_print_tree(tree.root_node(), 0);
        println!("--- DEBUG TREE END ---");

        // Find the component instantiation node
        let mut target_node = None;
        let mut cursor = root.walk();
        // Simple BFS/DFS to find the first component_instantiation_statement
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if n.kind() == "component_instantiation_statement" {
                target_node = Some(n);
                break;
            }
            // Add children to stack
            let mut c_cursor = n.walk();
            for child in n.children(&mut c_cursor) {
                stack.push(child);
            }
        }

        let node = target_node.expect("Test code must contain a component instantiation!");
        let result = get_instantiated_name(node, code);

        assert_eq!(result.as_deref(), expected_name, "\nCode: {}", code);
    }

    #[test]
    fn test_simple_component() {
        check_name("u1: my_comp port map (clk => clk);", Some("my_comp"));
    }

    #[test]
    fn test_entity_instantiation_work() {
        check_name(
            "u1: entity work.my_comp port map (clk => clk);",
            Some("my_comp"),
        );
    }

    #[test]
    fn test_entity_instantiation_lib() {
        check_name(
            "u1: entity some_lib.my_comp port map (clk => clk);",
            Some("my_comp"),
        );
    }

    #[test]
    fn test_entity_with_architecture() {
        check_name(
            "u1: entity work.my_comp(rtl) port map (clk => clk);",
            Some("my_comp"),
        );
    }

    #[test]
    fn test_entity_no_library() {
        check_name("u1: entity my_comp port map (clk => clk);", Some("my_comp"));
    }

    #[test]
    fn test_messy_whitespace() {
        check_name(
            "u1 :    entity    work . my_comp ( rtl ) port map (a=>b);",
            Some("my_comp"),
        );
    }

    #[test]
    fn test_incomplete_port_map_cursor_inside() {
        let code = r#"
            u1 : entity work.my_comp 
                port map (
                    clk => 
        "#;

        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(code, None).unwrap();
        drop(_guard);

        let root = tree.root_node();

        // 1. Locate the ERROR node (since we know the tree is broken)
        // or just pick the leaf node where the "cursor" would be (the end of the file)
        let end_byte = code.len();
        let end_point = tree.root_node().end_position();

        // Find the node at the very end (where user is typing)
        let target_node = root
            .descendant_for_point_range(end_point, end_point)
            .unwrap_or(root);

        // 2. Call the ROBUST find_component_name (which includes text fallback)
        // We use the 'find_component_name' (the wrapper), NOT 'get_instantiated_name'
        // because 'get_instantiated_name' expects a valid node, whereas 'find...' walks up.
        let result = find_component_name(target_node, code);

        assert_eq!(
            result.as_deref(),
            Some("my_comp"),
            "Failed to extract name from broken tree via text fallback"
        );
    }
}
