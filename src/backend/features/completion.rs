use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind, Position, Url,
};
use tree_sitter::{Node, Point};

use crate::analysis::{DeclType, Declaration, OxideSymbolKind, Symbol};
use crate::backend::AnalysisMap;
use crate::backend::features::hover;

// =============================================================================
// Node Kind Constants
// =============================================================================
// Centralized constants for tree-sitter node kinds to avoid stringly-typed
// comparisons scattered throughout the code.

mod node_kinds {
    pub const DESIGN_FILE: &str = "design_file";
    pub const ARCHITECTURE_BODY: &str = "architecture_body";
    pub const ARCHITECTURE_DEFINITION: &str = "architecture_definition";
    pub const BLOCK_STATEMENT: &str = "block_statement";
    pub const GENERATE_STATEMENT: &str = "generate_statement";
    pub const PROCESS_STATEMENT: &str = "process_statement";
    pub const SUBPROGRAM_BODY: &str = "subprogram_body";
    pub const CONCURRENT_BLOCK: &str = "concurrent_block";
    pub const COMPONENT_INSTANTIATION: &str = "component_instantiation_statement";
    pub const SIGNAL_ASSIGNMENT: &str = "concurrent_simple_signal_assignment";
    pub const ASSOCIATION_LIST: &str = "association_list";
    pub const ASSOCIATION_ELEMENT: &str = "association_element";
    pub const GENERIC_MAP_ASPECT: &str = "generic_map_aspect";
    pub const PORT_MAP_ASPECT: &str = "port_map_aspect";
    pub const INSTANTIATED_UNIT: &str = "instantiated_unit";
    pub const SELECTED_NAME: &str = "selected_name";
    pub const SELECTION: &str = "selection";
    pub const ERROR: &str = "ERROR";
    pub const NAME: &str = "name";
    pub const IDENTIFIER: &str = "identifier";
    pub const DOT: &str = ".";
    pub const ARROW: &str = "=>";
    pub const COMMA: &str = ",";
}

use node_kinds::*;

// =============================================================================
// Completion Context Types
// =============================================================================

/// Representation of the context of a completion, guiding the language server
/// on what symbols and keywords to suggest.
#[derive(Debug, PartialEq, Eq)]
pub enum CompletionContext {
    /// Inside an architecture, outside process or blocks.
    /// Suggests: Signals, Components, Instantiations, Concurrent Statements.
    Architecture,

    /// Inside a sequential process statement or a subprogram body.
    /// Suggests: Variables, Signals, Constants.
    Process,

    /// We are completing a record field or package name, usually after a `.`.
    DotAccess,

    /// We are inside a port map before the `=>`. Payload: Component Name.
    /// Suggests: Ports from the specified component/entity.
    PortMapLhs(String),

    /// We are inside a port map after the `=>`.
    /// Suggests: Signals from the current scope.
    PortMapRhs,

    /// We are inside a generic map before the `=>`. Payload: Component Name.
    /// Suggests: Generics from the specified component/entity.
    GenericMapLhs(String),

    /// We are inside a generic map after the `=>`.
    /// Suggests: Constants or expressions from the current scope.
    GenericMapRhs,

    /// Fallback for unknown or global scopes (e.g., top of file).
    Unresolved,
}

/// Helper enum to detect whether we are processing a generic map or a port map.
#[derive(Debug, PartialEq, Clone, Copy)]
enum DetectedMapKind {
    Generic,
    Port,
}

// =============================================================================
// Helper Functions - LHS/RHS Detection
// =============================================================================

/// Determines if the cursor is on the right-hand side of an association (after `=>`).
///
/// This function looks backward from the cursor position to find the start of the
/// current association element (after the last `,` or `(`), then checks if there's
/// an `=>` arrow in that segment.
///
/// # Arguments
/// * `text` - The full source code.
/// * `cursor_offset` - The byte offset of the cursor position.
///
/// # Returns
/// `true` if the cursor is after an `=>` in the current association element.
///
/// # Example
/// ```text
/// port map (clk => sig, data => |)
///                               ^ cursor here, returns true
/// port map (clk => sig, | => rst)
///                       ^ cursor here, returns false
/// ```
fn is_rhs_of_association(text: &str, cursor_offset: usize) -> bool {
    let limit = cursor_offset.min(text.len());
    let prefix = &text[..limit];

    // Find the start of the current association element
    let last_comma = prefix.rfind(',');
    let last_paren = prefix.rfind('(');
    let start_idx = match (last_comma, last_paren) {
        (Some(c), Some(p)) => c.max(p),
        (Some(c), None) => c,
        (None, Some(p)) => p,
        (None, None) => 0,
    };

    let current_segment = &prefix[start_idx..];
    current_segment.contains("=>")
}

/// Builds the appropriate `CompletionContext` for a map association.
///
/// This is a helper to avoid repeating the same if/else logic for building
/// Generic/Port and Lhs/Rhs combinations throughout the codebase.
///
/// # Arguments
/// * `component_name` - The name of the component/entity being instantiated.
/// * `map_kind` - Whether this is a generic map or port map.
/// * `is_rhs` - Whether the cursor is on the right-hand side of the association.
///
/// # Returns
/// The appropriate `CompletionContext` variant.
fn build_map_context(
    component_name: String,
    map_kind: DetectedMapKind,
    is_rhs: bool,
) -> CompletionContext {
    match (map_kind, is_rhs) {
        (DetectedMapKind::Generic, true) => CompletionContext::GenericMapRhs,
        (DetectedMapKind::Generic, false) => CompletionContext::GenericMapLhs(component_name),
        (DetectedMapKind::Port, true) => CompletionContext::PortMapRhs,
        (DetectedMapKind::Port, false) => CompletionContext::PortMapLhs(component_name),
    }
}

// =============================================================================
// Helper Functions - Tree Navigation
// =============================================================================

/// Converts an LSP position (line/character) into a byte offset within the source text.
///
/// # Arguments
/// * `text` - The full source code.
/// * `pos` - The LSP `Position`.
///
/// # Returns
/// The byte offset (`usize`).
fn position_to_offset(text: &str, pos: Position) -> usize {
    let mut offset = 0;
    for (i, line) in text.lines().enumerate() {
        if i == pos.line as usize {
            let line_pre: String = line.chars().take(pos.character as usize).collect();
            return offset + line_pre.len();
        }
        offset += line.len() + 1; // +1 for newline
    }
    text.len()
}

/// Gets the starting AST node for the cursor position, applying a fallback.
///
/// If no node is found exactly at the point (common when typing at boundaries),
/// it checks the position immediately preceding it.
///
/// # Arguments
/// * `root` - The root node of the Tree-sitter syntax tree.
/// * `pos` - The cursor position from the LSP request.
///
/// # Returns
/// `Some(Node)` at the cursor, or the node right before it, or `None`.
fn get_tree_node(root: Node, pos: Position) -> Option<Node> {
    let point = Point {
        row: pos.line as usize,
        column: pos.character as usize,
    };

    match root.descendant_for_point_range(point, point) {
        Some(n) => Some(n),
        None => {
            if point.column > 0 {
                let prev = Point {
                    row: point.row,
                    column: point.column - 1,
                };
                root.descendant_for_point_range(prev, prev)
            } else {
                None
            }
        }
    }
}

/// Checks if a tree-sitter Point is contained within a Node's range.
///
/// # Arguments
/// * `node` - The node to check against.
/// * `point` - The point to check.
///
/// # Returns
/// `true` if the point is within the node's start and end positions.
fn node_contains_point(node: &Node, point: Point) -> bool {
    let start = node.start_position();
    let end = node.end_position();

    let after_start =
        point.row > start.row || (point.row == start.row && point.column >= start.column);
    let before_end = point.row < end.row || (point.row == end.row && point.column <= end.column);

    after_start && before_end
}

/// Helper to check if a node has a descendant of a specific kind.
///
/// Checks direct children and one level deep inside ERROR nodes.
///
/// # Arguments
/// * `node` - The node to traverse.
/// * `kind` - The kind to search for.
///
/// # Returns
/// `true` if a matching descendant is found.
fn has_descendant_kind(node: Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return true;
        }
        // Check 1 level deep inside ERROR nodes
        if child.kind() == ERROR {
            let mut sub = child.walk();
            for sub_child in child.children(&mut sub) {
                if sub_child.kind() == kind {
                    return true;
                }
            }
        }
    }
    false
}

/// Checks if a node kind represents a scope container (architecture, block, etc.).
///
/// # Arguments
/// * `kind` - The node kind string.
///
/// # Returns
/// `true` if the kind represents a scope container.
fn is_scope_container(kind: &str) -> bool {
    matches!(
        kind,
        ARCHITECTURE_BODY
            | ARCHITECTURE_DEFINITION
            | BLOCK_STATEMENT
            | GENERATE_STATEMENT
            | CONCURRENT_BLOCK
    )
}

/// Checks if a node kind represents a sequential scope (process, subprogram).
///
/// # Arguments
/// * `kind` - The node kind string.
///
/// # Returns
/// `true` if the kind represents a sequential scope.
fn is_sequential_scope(kind: &str) -> bool {
    matches!(kind, PROCESS_STATEMENT | SUBPROGRAM_BODY)
}

// =============================================================================
// Component Name Extraction
// =============================================================================

/// Extracts the instantiated unit name from a component instantiation node.
///
/// Handles various VHDL instantiation forms:
/// - `u0: my_comp` (simple component)
/// - `u0: entity work.my_comp` (direct entity instantiation)
/// - `u0: entity work.my_comp(rtl)` (with architecture)
///
/// # Arguments
/// * `node` - The component instantiation node.
/// * `text` - The source code text.
///
/// # Returns
/// `Some(String)` with the component/entity name, or `None` if extraction failed.
fn get_instantiated_name(node: Node, text: &str) -> Option<String> {
    // 1. Try to find the complex "instantiated_unit" (e.g., "entity work.comp")
    let unit_node = node.child_by_field_name("unit").or_else(|| {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| child.kind() == INSTANTIATED_UNIT)
    });

    if let Some(unit_node) = unit_node {
        let raw_text = unit_node.utf8_text(text.as_bytes()).ok()?;
        return Some(extract_component_name_from_text(raw_text));
    }

    // 2. Fallback: Try to find a simple "name" node (e.g., "my_comp")
    let mut cursor = node.walk();
    if let Some(name_node) = node.children(&mut cursor).find(|c| c.kind() == NAME) {
        let raw_text = name_node.utf8_text(text.as_bytes()).ok()?;
        let text_no_arch = raw_text.split('(').next().unwrap_or(raw_text);
        return Some(text_no_arch.trim().to_string());
    }

    None
}

/// Extracts a clean component name from raw instantiation text.
///
/// Handles text like:
/// - `"entity work.my_comp(rtl)"` → `"my_comp"`
/// - `"work.my_comp"` → `"my_comp"`
/// - `"my_comp(rtl)"` → `"my_comp"`
///
/// # Arguments
/// * `raw_text` - The raw text from the instantiated unit node.
///
/// # Returns
/// The cleaned component name.
fn extract_component_name_from_text(raw_text: &str) -> String {
    let trimmed = raw_text.trim();

    // Remove "entity" keyword if present
    let remainder = if trimmed.to_lowercase().starts_with("entity") {
        trimmed[6..].trim()
    } else {
        trimmed
    };

    // Remove architecture specification (e.g., "(rtl)")
    let text_no_arch = remainder.split('(').next().unwrap_or(remainder);

    // Get the last part after any dots (e.g., "work.my_comp" → "my_comp")
    let final_name = text_no_arch.split('.').next_back().unwrap_or(text_no_arch);

    final_name.trim().to_string()
}

// =============================================================================
// Component Declaration Finding
// =============================================================================

/// Result of component declaration search, containing the component name and map kind.
type ComponentDeclaration = (String, DetectedMapKind);

/// Walks up the tree (or uses text heuristic) to find the component name and map kind.
///
/// This function encapsulates the complexity of handling broken ASTs and extracting
/// the component name. It tries tree traversal first, then falls back to text search.
///
/// # Arguments
/// * `node` - The starting AST node.
/// * `text` - The full source code.
/// * `cursor_offset` - The byte offset of the cursor.
///
/// # Returns
/// `Some((component_name, map_kind))` if found, or `None`.
fn find_component_declaration(
    node: Node,
    text: &str,
    cursor_offset: usize,
) -> Option<ComponentDeclaration> {
    // 1. Try Tree Traversal
    if let Some(result) = find_component_via_tree(node, text) {
        return Some(result);
    }

    // 2. Fallback: Text Search
    find_component_via_text(text, cursor_offset)
}

/// Attempts to find component declaration by walking up the AST.
///
/// # Arguments
/// * `node` - The starting node.
/// * `text` - The source code.
///
/// # Returns
/// `Some((component_name, map_kind))` if found via tree traversal.
fn find_component_via_tree(node: Node, text: &str) -> Option<ComponentDeclaration> {
    let mut map_kind: Option<DetectedMapKind> = None;
    let mut current = Some(node);

    while let Some(n) = current {
        let kind = n.kind();

        // Track which map type we're in
        if kind == GENERIC_MAP_ASPECT {
            map_kind = Some(DetectedMapKind::Generic);
        }
        if kind == PORT_MAP_ASPECT {
            map_kind = Some(DetectedMapKind::Port);
        }

        // CASE A: Normal Instantiation
        if kind == COMPONENT_INSTANTIATION {
            if let Some(name) = get_instantiated_name(n, text) {
                return Some((name, map_kind.unwrap_or(DetectedMapKind::Port)));
            }
            break;
        }

        // CASE B: Mis-parsed as Signal Assignment
        if kind == SIGNAL_ASSIGNMENT
            && let Some(result) = try_extract_from_misparsed_assignment(n, text, map_kind)
        {
            return Some(result);
        }

        current = n.parent();
    }

    None
}

/// Tries to extract component info from a mis-parsed signal assignment.
///
/// When tree-sitter can't fully parse an instantiation, it sometimes wraps
/// it in a `concurrent_simple_signal_assignment` node.
///
/// # Arguments
/// * `node` - The signal assignment node.
/// * `text` - The source code.
/// * `current_map_kind` - The map kind detected so far (if any).
///
/// # Returns
/// `Some((component_name, map_kind))` if extraction succeeded.
fn try_extract_from_misparsed_assignment(
    node: Node,
    text: &str,
    current_map_kind: Option<DetectedMapKind>,
) -> Option<ComponentDeclaration> {
    let has_generic = has_descendant_kind(node, GENERIC_MAP_ASPECT);
    let has_port = has_descendant_kind(node, PORT_MAP_ASPECT);

    if !has_generic && !has_port {
        return None;
    }

    let map_kind = current_map_kind.unwrap_or({
        if has_generic && !has_port {
            DetectedMapKind::Generic
        } else {
            DetectedMapKind::Port
        }
    });

    let mut start_search = node.start_byte();
    if let Some(label) = node.child_by_field_name("label") {
        start_search = label.end_byte();
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() >= start_search {
            if child.kind() == ERROR {
                let mut err_cursor = child.walk();
                for err_child in child.children(&mut err_cursor) {
                    if err_child.kind() == NAME || err_child.kind() == IDENTIFIER {
                        let name = err_child.utf8_text(text.as_bytes()).ok()?.to_string();
                        return Some((name, map_kind));
                    }
                }
            }
            if child.kind() == NAME || child.kind() == IDENTIFIER {
                let name = child.utf8_text(text.as_bytes()).ok()?.to_string();
                return Some((name, map_kind));
            }
        }
    }

    None
}

/// Attempts to find component declaration using text-based search.
///
/// This is the fallback when tree traversal fails due to broken syntax.
/// It searches backward from the cursor for patterns like `label: entity work.comp`.
///
/// # Arguments
/// * `text` - The full source code.
/// * `cursor_offset` - The byte offset of the cursor.
///
/// # Returns
/// `Some((component_name, map_kind))` if found via text search.
fn find_component_via_text(text: &str, cursor_offset: usize) -> Option<ComponentDeclaration> {
    let search_limit = cursor_offset.min(text.len());
    let text_before = &text[..search_limit];

    let colon_idx = text_before.rfind(':')?;
    let after_colon = &text_before[colon_idx + 1..];
    let lower = after_colon.to_lowercase();

    let gmap_idx = lower.rfind("generic map");
    let pmap_idx = lower.rfind("port map");

    let (cut_idx, map_kind) = match (gmap_idx, pmap_idx) {
        (Some(g), Some(p)) => {
            if g > p {
                (g, DetectedMapKind::Generic)
            } else {
                (p, DetectedMapKind::Port)
            }
        }
        (Some(g), None) => (g, DetectedMapKind::Generic),
        (None, Some(p)) => (p, DetectedMapKind::Port),
        (None, None) => return None,
    };

    let raw_decl = &after_colon[..cut_idx];
    let name = extract_component_name_from_text(raw_decl);

    if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        Some((name, map_kind))
    } else {
        None
    }
}

// =============================================================================
// Context Detection - Deep Scan
// =============================================================================

/// Recursively traverses descendants to find map context in broken trees.
///
/// This function is called when the cursor node is at `design_file` level,
/// typically indicating a broken parse tree. It scans downward looking for
/// instantiation patterns.
///
/// # Arguments
/// * `start_node` - The node to start scanning from.
/// * `text` - The full source code.
/// * `point` - The Tree-sitter `Point` of the cursor.
/// * `cursor_offset` - The byte offset of the cursor.
///
/// # Returns
/// `Some(CompletionContext)` if a map context is detected, or `None`.
fn deep_scan_for_map_context(
    start_node: Node,
    text: &str,
    point: Point,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let mut cursor = start_node.walk();

    for child in start_node.children(&mut cursor) {
        let kind = child.kind();

        // For ERROR nodes, ALWAYS try to extract map context regardless of point containment.
        // ERROR node boundaries are unreliable in broken syntax.
        if kind == ERROR {
            if let Some(ctx) = try_build_context_from_node(child, text, cursor_offset) {
                return Some(ctx);
            }
            // Also recurse into ERROR node
            if let Some(ctx) = deep_scan_for_map_context(child, text, point, cursor_offset) {
                return Some(ctx);
            }
        }

        let is_contained = node_contains_point(&child, point);

        if !is_contained {
            // If cursor not in this node, recurse into scope containers
            if (is_scope_container(kind) || kind == PROCESS_STATEMENT || kind == ERROR)
                && let Some(ctx) = deep_scan_for_map_context(child, text, point, cursor_offset)
            {
                return Some(ctx);
            }
            continue;
        }

        // Point is contained: check if it's a map context
        if (kind == COMPONENT_INSTANTIATION || kind == SIGNAL_ASSIGNMENT)
            && let Some(ctx) = try_build_context_from_node(child, text, cursor_offset)
        {
            return Some(ctx);
        }

        // Recurse into scope containers
        if (is_scope_container(kind) || kind == PROCESS_STATEMENT)
            && let Some(ctx) = deep_scan_for_map_context(child, text, point, cursor_offset)
        {
            return Some(ctx);
        }
    }
    None
}

/// Attempts to build a CompletionContext from a node that might be an instantiation.
///
/// # Arguments
/// * `node` - The node to analyze.
/// * `text` - The source code.
/// * `cursor_offset` - The byte offset of the cursor.
///
/// # Returns
/// `Some(CompletionContext)` if the node contains a valid map context.
fn try_build_context_from_node(
    node: Node,
    text: &str,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let (name, map_kind) = find_component_declaration(node, text, cursor_offset)?;
    let is_rhs = is_rhs_of_association(text, cursor_offset);

    Some(build_map_context(name, map_kind, is_rhs))
}

// =============================================================================
// Context Detection - Upward Traversal
// =============================================================================

/// Handles upward traversal of the AST for context detection.
///
/// This is the main logic for determining completion context when we have
/// a valid (or partially valid) parse tree. It walks up from the cursor
/// position to find the enclosing scope.
///
/// # Arguments
/// * `node` - The starting node (must not be `design_file`).
/// * `text` - The full source code.
/// * `point` - The Tree-sitter `Point` of the cursor.
/// * `cursor_offset` - The byte offset of the cursor.
///
/// # Returns
/// The determined `CompletionContext`.
fn handle_upward_traversal(
    node: Node,
    text: &str,
    point: Point,
    cursor_offset: usize,
) -> CompletionContext {
    let mut current = Some(node);

    while let Some(n) = current {
        let kind = n.kind();

        // Check for sequential scope (process, subprogram)
        if is_sequential_scope(kind) {
            return CompletionContext::Process;
        }

        // Check for architecture/concurrent scope
        if is_scope_container(kind) {
            return CompletionContext::Architecture;
        }

        // Handle various map-related nodes
        if let Some(ctx) = handle_map_node(n, text, point, cursor_offset) {
            return ctx;
        }

        current = n.parent();
    }

    CompletionContext::Unresolved
}

/// Handles map-related nodes during upward traversal.
///
/// # Arguments
/// * `node` - The current node being examined.
/// * `text` - The source code.
/// * `point` - The cursor point.
/// * `cursor_offset` - The cursor byte offset.
///
/// # Returns
/// `Some(CompletionContext)` if the node is a map context, `None` to continue traversal.
fn handle_map_node(
    node: Node,
    text: &str,
    point: Point,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let kind = node.kind();

    match kind {
        ERROR | SIGNAL_ASSIGNMENT => handle_error_or_assignment_node(node, text, cursor_offset),
        ASSOCIATION_ELEMENT => handle_association_element(node, text, point, cursor_offset),
        ASSOCIATION_LIST => handle_association_list(node, text, point, cursor_offset),
        COMPONENT_INSTANTIATION | PORT_MAP_ASPECT | GENERIC_MAP_ASPECT => {
            if let Some((name, map_kind)) = find_component_declaration(node, text, cursor_offset) {
                let is_rhs = is_rhs_of_association(text, cursor_offset);
                Some(build_map_context(name, map_kind, is_rhs))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Handles ERROR or mis-parsed signal assignment nodes.
fn handle_error_or_assignment_node(
    node: Node,
    text: &str,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let (name, map_kind) = find_component_declaration(node, text, cursor_offset)?;
    let is_rhs = is_rhs_of_association(text, cursor_offset);

    Some(build_map_context(name, map_kind, is_rhs))
}

/// Handles association_element nodes to determine LHS/RHS position.
fn handle_association_element(
    node: Node,
    text: &str,
    point: Point,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let cursor_col = point.column;
    let mut is_rhs = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == ARROW {
            let arrow_end = child.end_position().column;
            if cursor_col >= arrow_end {
                is_rhs = true;
            }
        }
    }

    if let Some((name, map_kind)) = find_component_declaration(node, text, cursor_offset) {
        return Some(build_map_context(name, map_kind, is_rhs));
    }

    // Fallback: determine map kind from parent
    let parent_kind = node
        .parent()
        .and_then(|p| p.parent())
        .map(|pp| pp.kind())
        .unwrap_or("");

    let map_kind = if parent_kind == GENERIC_MAP_ASPECT {
        DetectedMapKind::Generic
    } else {
        DetectedMapKind::Port
    };

    Some(build_map_context(String::new(), map_kind, is_rhs))
}

/// Handles association_list nodes with complex LHS/RHS detection.
fn handle_association_list(
    node: Node,
    text: &str,
    point: Point,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let is_rhs = determine_rhs_in_association_list(node, point);

    if let Some((name, map_kind)) = find_component_declaration(node, text, cursor_offset) {
        return Some(build_map_context(name, map_kind, is_rhs));
    }

    // Fallback: determine map kind from parent
    let parent_kind = node.parent().map(|p| p.kind()).unwrap_or("");
    let map_kind = if parent_kind == GENERIC_MAP_ASPECT {
        DetectedMapKind::Generic
    } else {
        DetectedMapKind::Port
    };

    Some(build_map_context(String::new(), map_kind, is_rhs))
}

/// Determines if cursor is on RHS within an association list by analyzing children.
fn determine_rhs_in_association_list(node: Node, point: Point) -> bool {
    let mut is_rhs = false;
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        let start = child.start_position();

        // Check if we've passed the cursor position
        let strictly_after =
            start.row > point.row || (start.row == point.row && start.column > point.column);
        let at_start = start.row == point.row && start.column == point.column;

        if strictly_after || (at_start && child.kind() == COMMA) {
            break;
        }

        if child.kind() == COMMA {
            is_rhs = false;
            continue;
        }

        if child.kind() == ASSOCIATION_ELEMENT {
            is_rhs = check_rhs_in_association_element(child, point);
        }

        if child.kind() == ERROR {
            is_rhs = check_rhs_in_error_node(child, point);
        }
    }

    is_rhs
}

/// Checks if cursor is on RHS within a single association element.
fn check_rhs_in_association_element(element: Node, point: Point) -> bool {
    let mut cursor = element.walk();
    for child in element.children(&mut cursor) {
        if child.kind() == ARROW && child.end_position().column <= point.column {
            return true;
        }
    }
    false
}

/// Checks if cursor is on RHS within an ERROR node containing an arrow.
fn check_rhs_in_error_node(error_node: Node, point: Point) -> bool {
    let mut cursor = error_node.walk();
    for child in error_node.children(&mut cursor) {
        if child.kind() == ARROW && point.column >= child.end_position().column {
            return true;
        }
    }
    false
}

// =============================================================================
// Main Context Detection
// =============================================================================

/// Determines the semantic context of the cursor position by analyzing the AST.
///
/// This is the main entry point for completion context detection. It converts
/// the LSP position into a Tree-sitter point, finds the smallest syntax node
/// at that location, and dispatches to the appropriate handler.
///
/// # Arguments
/// * `text` - The full source code of the file.
/// * `root` - The root node of the Tree-sitter syntax tree.
/// * `pos` - The cursor position from the LSP request.
///
/// # Returns
/// A `CompletionContext` enum variant describing the detected scope.
pub fn get_completion_context(text: &str, root: Node, pos: Position) -> CompletionContext {
    let point = Point {
        row: pos.line as usize,
        column: pos.character as usize,
    };
    let cursor_offset = position_to_offset(text, pos);

    let node = match get_tree_node(root, pos) {
        Some(n) => n,
        None => return CompletionContext::Unresolved,
    };

    // 1. Handle Dot Access (fast path)
    if is_dot_access_context(&node, text, pos) {
        return CompletionContext::DotAccess;
    }

    // 2. Handle Broken Tree (deep scan from root)
    if node.kind() == DESIGN_FILE
        && let Some(context) = deep_scan_for_map_context(node, text, point, cursor_offset)
    {
        return context;
    }

    // 3. Walk the tree upwards (default/valid tree logic)
    handle_upward_traversal(node, text, point, cursor_offset)
}

/// Checks if the cursor is in a dot-access context (e.g., `record.field`).
///
/// # Arguments
/// * `node` - The node at cursor position.
/// * `text` - The source code.
/// * `pos` - The cursor position.
///
/// # Returns
/// `true` if this is a dot-access context.
fn is_dot_access_context(node: &Node, text: &str, pos: Position) -> bool {
    let kind = node.kind();
    let parent_kind = node.parent().map(|p| p.kind()).unwrap_or("");

    // Direct dot or selected name
    if kind == DOT || parent_kind == SELECTED_NAME || parent_kind == SELECTION {
        return true;
    }

    // Text-based fallback for typing `r.f|`
    if let Some(line) = text.lines().nth(pos.line as usize)
        && pos.character as usize <= line.len()
    {
        let prefix = &line[..pos.character as usize];
        let trimmed = prefix.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
        if trimmed.ends_with('.') {
            return true;
        }
    }

    false
}

// =============================================================================
// Completion Item Generation
// =============================================================================

/// Generates completion items based on the detected context.
///
/// # Arguments
/// * `analysis_map` - The global map of all file analyses.
/// * `current_uri` - The URI of the file being completed.
/// * `context` - The determined `CompletionContext`.
/// * `position` - The cursor position.
///
/// # Returns
/// Vector of completion items appropriate for the context.
pub fn complete_scope(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    context: &CompletionContext,
    position: Position,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    if let Some(current_analysis) = analysis_map.get(current_uri) {
        // Handle Map LHS Lookups (requires searching all files)
        match context {
            CompletionContext::PortMapLhs(target_name)
            | CompletionContext::GenericMapLhs(target_name) => {
                let is_generic = matches!(context, CompletionContext::GenericMapLhs(_));
                for analysis in analysis_map.values() {
                    if let Some(target_sym) = analysis
                        .symbols
                        .values()
                        .find(|s| s.name.eq_ignore_ascii_case(target_name))
                    {
                        if is_generic {
                            collect_generics(target_sym, &mut items);
                        } else {
                            collect_ports(target_sym, &mut items);
                        }
                    }
                }
                return items;
            }
            _ => {}
        }

        // // Handle general scope lookups (only searches current file)
        // for sym in current_analysis.symbols.values() {
        //     collect_symbols(sym, context, position, &mut items);
        // }
        if let Some(scope_tree) = current_analysis.find_scope_tree_at(&position) {
            let innermost_scope = scope_tree.find_innermost_scope(&position);
            let header = scope_tree
                .entity
                .as_ref()
                .and_then(|name| current_analysis.entity_scope_trees.get(name))
                .or_else(|| {
                    scope_tree
                        .package
                        .as_ref()
                        .and_then(|name| current_analysis.package_scope_trees.get(name))
                });
            let declarations =
                scope_tree.collect_visible_declarations(&innermost_scope.range, header);
            if let Some(declarations) = declarations {
                for decl in declarations {
                    items.push(declaration_to_completion(&decl));
                }
            }
        }
    }

    // Remove duplicates
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);

    items
}

/// Recursively extracts ports from a symbol for LHS completion.
fn collect_ports(symbol: &Symbol, items: &mut Vec<CompletionItem>) {
    if symbol.kind == OxideSymbolKind::Port
        && let Some(item) = symbol_to_completion(symbol)
    {
        items.push(item);
    }
    for child in &symbol.children {
        collect_ports(child, items);
    }
}

/// Recursively extracts generics from a symbol for LHS completion.
fn collect_generics(symbol: &Symbol, items: &mut Vec<CompletionItem>) {
    if symbol.kind == OxideSymbolKind::Generic
        && let Some(item) = symbol_to_completion(symbol)
    {
        items.push(item);
    }
    for child in &symbol.children {
        collect_generics(child, items);
    }
}

/// Converts a Symbol to a CompletionItem.
fn symbol_to_completion(symbol: &Symbol) -> Option<CompletionItem> {
    let kind = match symbol.kind {
        OxideSymbolKind::Entity => CompletionItemKind::INTERFACE,
        OxideSymbolKind::Architecture => return None,
        OxideSymbolKind::Port => CompletionItemKind::FIELD,
        OxideSymbolKind::Signal => CompletionItemKind::VARIABLE,
        OxideSymbolKind::Constant => CompletionItemKind::CONSTANT,
        OxideSymbolKind::Struct => CompletionItemKind::STRUCT,
        OxideSymbolKind::Function => CompletionItemKind::FUNCTION,
        OxideSymbolKind::Process => return None,
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

fn declaration_to_completion(decl: &Declaration) -> CompletionItem {
    let kind = match decl.decl_type {
        DeclType::Alias => CompletionItemKind::VARIABLE,
        DeclType::Component => CompletionItemKind::INTERFACE,
        DeclType::Constant => CompletionItemKind::CONSTANT,
        DeclType::Variable => CompletionItemKind::VARIABLE,
        DeclType::Signal => CompletionItemKind::VARIABLE,
        DeclType::Generic => CompletionItemKind::CONSTANT,
        DeclType::Port(_) => CompletionItemKind::FIELD,
        DeclType::Parameter(_, _) => CompletionItemKind::FIELD,
        DeclType::Function => CompletionItemKind::FUNCTION,
        DeclType::Type => CompletionItemKind::STRUCT,
        DeclType::Subtype => CompletionItemKind::STRUCT,
        DeclType::Procedure => CompletionItemKind::FUNCTION,
    };

    let mut details = decl.type_info.base_type.clone();
    if let Some(constraints) = &decl.type_info.constraints {
        details.push_str(constraints);
    }

    CompletionItem {
        label: decl.name.clone(),
        kind: Some(kind),
        label_details: Some(tower_lsp::lsp_types::CompletionItemLabelDetails {
            detail: None,
            description: Some(details.to_string()),
        }),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: hover::format_declaration_hover(decl),
        })),
        ..CompletionItem::default()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
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
}
