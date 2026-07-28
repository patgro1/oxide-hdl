use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Documentation,
    InsertTextFormat, MarkupContent, MarkupKind, Position, Url,
};
use tree_sitter::{Node, Point};

use crate::analysis::{Analysis, DeclType, Declaration, ScopeTree, UseClause};
use crate::backend::AnalysisMap;
use crate::backend::features::hover;
use crate::backend::features::lookup::{ResolvedItem, lookup_symbol, resolve_path_chain};

lazy_static::lazy_static! {
    /// Matches `entity <lib>.<partial>` at the end of the text before the cursor.
    /// Captures the library name.
    static ref RE_INST_LIB_DOT: regex::Regex =
        regex::Regex::new(r"(?i)\bentity\s+([A-Za-z]\w*)\s*\.\s*(\w*)$").unwrap();

    /// Matches `entity <partial>` at the end of the text before the cursor, with no dot.
    static ref RE_INST_ENTITY_KW: regex::Regex =
        regex::Regex::new(r"(?i)\bentity\s+(\w*)$").unwrap();
}

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
    #[allow(dead_code)]
    pub const FUNCTION_CALL: &str = "function_call";
    pub const PARENTHESIS_GROUP: &str = "parenthesis_group";
    pub const ASSOCIATION_OR_RANGE_LIST: &str = "association_or_range_list";
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

    /// Cursor sits right after the `entity` keyword of an instantiation, before any
    /// library name. Suggests: library names present in the workspace.
    InstantiationLibrary,

    /// Cursor sits after `<library>.` in an instantiation. Payload: lowercase library
    /// name as typed (`work` is resolved against the current file's library at
    /// completion time, not here). Suggests: entities declared in that library.
    LibraryUnits(String),

    /// Inside a subprogram call argument list with no arguments yet (empty or whitespace only).
    /// Suggests both parameter names (LHS) and in-scope values (RHS), params first.
    /// Payload: subprogram name (lowercase).
    SubprogramCallBoth(String),

    /// Inside a subprogram call argument list, before `=>` in named association mode.
    /// Suggests: parameter names not yet supplied.
    /// Payload: subprogram name (lowercase).
    SubprogramCallLhs(String),

    /// Inside a subprogram call argument list after `=>`, or in positional mode (args present, no `=>`).
    /// Suggests: in-scope signals, variables, constants.
    SubprogramCallRhs,

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

/// Collects the names of parameters already bound in a subprogram call or map argument list.
///
/// Scans `text[open_paren_offset..cursor_offset]` tracking paren depth.
/// Only collects `identifier =>` patterns at depth 1 (directly inside the call's `(`),
/// ignoring `=>` tokens inside nested aggregates, inner calls, or qualified expressions.
///
/// # Arguments
/// * `text` - The full source text.
/// * `open_paren_offset` - Byte offset of the `(` that opens the argument list.
/// * `cursor_offset` - Byte offset of the cursor.
///
/// # Returns
/// A `HashSet<String>` of lowercased parameter names already supplied.
fn collect_used_param_names(
    text: &str,
    open_paren_offset: usize,
    cursor_offset: usize,
) -> std::collections::HashSet<String> {
    let mut used = std::collections::HashSet::new();
    let limit = cursor_offset.min(text.len());
    if open_paren_offset >= limit {
        return used;
    }
    let slice = &text[open_paren_offset..limit];
    let bytes = slice.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    let mut depth: usize = 0;

    while i < n {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b if depth == 1 && (b.is_ascii_alphabetic() || b == b'_') => {
                // Potential identifier at top level of the argument list
                let start = i;
                while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = &slice[start..i];
                // Skip whitespace, then check for =>
                let mut j = i;
                while j < n && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                if j + 1 < n && bytes[j] == b'=' && bytes[j + 1] == b'>' {
                    used.insert(ident.to_ascii_lowercase());
                }
                // Do NOT advance i here; the loop will continue from i (after ident chars consumed)
            }
            _ => {
                i += 1;
            }
        }
    }
    used
}

/// Returns `true` if the text contains a `=>` token at paren depth 0.
///
/// Used to detect whether a call's argument list is using named association.
/// Ignores `=>` tokens inside nested parentheses (aggregates, inner calls).
///
/// # Arguments
/// * `text` - The inner content of a call's argument list (after the opening `(`).
fn has_top_level_arrow(text: &str) -> bool {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < n {
        match bytes[i] {
            b'(' => { depth += 1; i += 1; }
            b')' => { depth = depth.saturating_sub(1); i += 1; }
            b'=' if depth == 0 && i + 1 < n && bytes[i + 1] == b'>' => return true,
            _ => { i += 1; }
        }
    }
    false
}

/// Returns `true` if the text contains a `,` at paren depth 0.
///
/// Used to confirm positional argument mode: once a top-level comma exists the
/// user has passed the first argument without using `=>`, so the call is positional.
///
/// # Arguments
/// * `text` - The inner content of a call's argument list (after the opening `(`).
fn has_top_level_comma(text: &str) -> bool {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < n {
        match bytes[i] {
            b'(' => { depth += 1; i += 1; }
            b')' => { depth = depth.saturating_sub(1); i += 1; }
            b',' if depth == 0 => return true,
            _ => { i += 1; }
        }
    }
    false
}

/// Determines the appropriate `CompletionContext` for a cursor inside a subprogram call.
///
/// Three modes:
/// - **Both**: argument list is empty/whitespace, OR the user is typing the first argument
///   with no `=>` and no top-level comma yet (ambiguous: could be named or positional).
///   Offers param names first, then in-scope values.
/// - **Lhs**: named association mode (`=>` present at top level), cursor is before `=>`.
///   Offers only unbound parameter names.
/// - **Rhs**: after `=>` in named mode, OR positional mode confirmed by a top-level comma
///   without any `=>`. Offers only in-scope values.
///
/// The key rule for the no-`=>` case: we only commit to `SubprogramCallRhs` (positional)
/// once there is a top-level comma in the content before the cursor. A single partial token
/// like `"cond"` is ambiguous — the user may be starting a named param — so we keep `Both`
/// and let the editor's own filtering narrow the list.
///
/// # Arguments
/// * `name` - The subprogram name (lowercase), used as the context payload.
/// * `text` - The full source text.
/// * `open_paren_offset` - Byte offset of the `(` opening the argument list.
/// * `cursor_offset` - Byte offset of the cursor.
fn classify_call_args(
    name: String,
    text: &str,
    open_paren_offset: usize,
    cursor_offset: usize,
) -> CompletionContext {
    let limit = cursor_offset.min(text.len());
    let inner_start = (open_paren_offset + 1).min(limit);

    if inner_start >= limit || text[inner_start..limit].chars().all(char::is_whitespace) {
        return CompletionContext::SubprogramCallBoth(name);
    }

    let inner = &text[inner_start..limit];

    if has_top_level_arrow(inner) {
        if is_rhs_of_association(text, cursor_offset) {
            CompletionContext::SubprogramCallRhs
        } else {
            CompletionContext::SubprogramCallLhs(name)
        }
    } else if has_top_level_comma(inner) {
        // Multiple args, no `=>` anywhere → positional mode confirmed.
        CompletionContext::SubprogramCallRhs
    } else {
        // Single partial token, no `=>`, no comma yet — ambiguous.
        // Keep Both so param names stay visible while the user types.
        CompletionContext::SubprogramCallBoth(name)
    }
}

/// Extracts the subprogram name from a `name` node that is the callee of a call expression.
///
/// In the VHDL tree-sitter grammar, a function call is represented as a `name` node
/// whose first `identifier` child holds the callee name and whose `parenthesis_group`
/// child holds the argument list. This function extracts the callee name — the last
/// dot-separated segment of the `name` node's text (lowercased).
///
/// For example, `pkg.my_func(...)` yields `"my_func"`.
///
/// # Arguments
/// * `name_node` - The `name` tree-sitter node that is the callee.
/// * `text` - The full source text.
fn extract_subprogram_name(name_node: Node, text: &str) -> String {
    // Extract the text of the name node up to (but not including) the parenthesis_group.
    // We grab the first identifier child, since selected_name nodes also have identifiers.
    let mut cursor = name_node.walk();
    for child in name_node.children(&mut cursor) {
        if child.kind() == IDENTIFIER || child.kind() == SELECTED_NAME {
            let name_text = &text[child.start_byte()..child.end_byte()];
            return name_text
                .split('.')
                .next_back()
                .unwrap_or(name_text)
                .trim()
                .to_ascii_lowercase();
        }
    }
    // Fallback: use the full name node text before any '('
    let name_text = &text[name_node.start_byte()..name_node.end_byte()];
    let before_paren = name_text.split('(').next().unwrap_or(name_text);
    before_paren
        .split('.')
        .next_back()
        .unwrap_or(before_paren)
        .trim()
        .to_ascii_lowercase()
}

/// Walks up from `node` looking for a `parenthesis_group` whose parent is `name`.
///
/// In the VHDL tree-sitter grammar, a subprogram call is represented as:
/// ```text
/// name
///   identifier  ("my_func")
///   parenthesis_group
///     association_or_range_list
///       association_element ...
/// ```
///
/// Returns `Some(CompletionContext)` if a subprogram call context is found.
/// Returns `None` if `port_map_aspect` or `generic_map_aspect` is encountered first
/// (meaning we are inside an instantiation map, not a subprogram call).
///
/// # Arguments
/// * `node` - Starting node (the current node in the upward traversal).
/// * `text` - Full source text.
/// * `cursor_offset` - Byte offset of the cursor.
fn try_subprogram_call_context(
    node: Node,
    text: &str,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    // Skip labeled statements (label: process, label: function_call, etc.)
    // Detect by checking if node is a NAME and is preceded by a colon
    if node.kind() == NAME && is_label_identifier(node, text) {
        return None;
    }

    let mut current = Some(node);
    while let Some(n) = current {
        let kind = n.kind();
        // Stop — this is an instantiation map, not a subprogram call
        if kind == PORT_MAP_ASPECT || kind == GENERIC_MAP_ASPECT {
            return None;
        }
        if kind == PARENTHESIS_GROUP
            && let Some(parent) = n.parent()
                && parent.kind() == NAME {
                    let name = extract_subprogram_name(parent, text);
                    let open_paren_offset = n.start_byte();
                    let ctx = classify_call_args(name.clone(), text, open_paren_offset, cursor_offset);
                    // Edge case: cursor is right after '(' with no text before it, so
                    // classify_call_args returns Both. But if the AST shows named
                    // associations exist in the full arg list, the user is on an LHS.
                    if matches!(ctx, CompletionContext::SubprogramCallBoth(_))
                        && parenthesis_group_has_named_assoc(n)
                    {
                        return Some(CompletionContext::SubprogramCallLhs(name));
                    }
                    return Some(ctx);
                }
        current = n.parent();
    }
    None
}

/// Checks if a NAME node is a label identifier (preceded by a colon).
/// Excludes labels from subprogram call detection to preserve snippet completion.
fn is_label_identifier(node: Node, text: &str) -> bool {
    let start_byte = node.start_byte();
    if start_byte == 0 {
        return false;
    }
    let bytes = text.as_bytes();
    let mut i = start_byte - 1;
    // Skip whitespace backward
    while i > 0 && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i -= 1;
    }
    // Check for colon
    i < bytes.len() && bytes[i] == b':'
}

/// Returns `true` if the `parenthesis_group` node contains at least one `association_element`
/// with an arrow (`=>`), indicating named association mode.
fn parenthesis_group_has_named_assoc(paren_group: Node) -> bool {
    let mut cursor = paren_group.walk();
    for child in paren_group.children(&mut cursor) {
        if child.kind() == ASSOCIATION_OR_RANGE_LIST || child.kind() == ASSOCIATION_LIST {
            let mut list_cursor = child.walk();
            for elem in child.children(&mut list_cursor) {
                if elem.kind() == ASSOCIATION_ELEMENT {
                    let mut elem_cursor = elem.walk();
                    for token in elem.children(&mut elem_cursor) {
                        if token.kind() == ARROW {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
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

/// Detects if the cursor is positioned after a label in the syntax tree.
///
/// Labels in VHDL can be used for various concurrent statements in architecture bodies:
/// - Component instantiations: `inst1: my_comp ...`
/// - Process statements: `proc1: process ...`
/// - Generate statements: `gen1: for ... generate` or `gen1: if ... generate`
/// - Block statements: `block1: block ...`
///
/// This function checks the tree-sitter AST for patterns indicating the user has typed
/// a label followed by a colon, and the cursor is positioned where they would start
/// typing the statement type or component name.
///
/// Returns `true` for patterns like:
/// - `inst1: |` → Tree shows: `(ERROR (label_declaration (label)))`
/// - `my_label: avl|` → Tree shows: `(concurrent_procedure_call_statement ...)`
///
/// Returns `false` when:
/// - No label pattern is detected
/// - Cursor is inside a well-formed statement body
///
/// # Arguments
/// * `_text` - Full source code text (unused, reserved for future text-based validation)
/// * `position` - LSP cursor position
/// * `tree_root` - Tree-sitter root node
///
/// # Returns
/// `true` if cursor is positioned after a label where snippets should be offered
pub fn is_after_label(_text: &str, position: Position, tree_root: Node) -> bool {
    let cursor_line = position.line as usize;

    // Strategy: Recursively search tree for ERROR nodes on same line with label_declaration
    // This works because incomplete syntax (inst1: |) creates ERROR nodes, while
    // well-formed statements (inst1: comp port map...) don't have ERROR nodes
    find_error_with_label_on_line(tree_root, cursor_line)
}

/// Recursively searches for ERROR nodes on a specific line containing label_declaration.
///
/// This helper enables detection of incomplete labeled statements where the user
/// has typed a label but hasn't completed the statement yet.
///
/// # Arguments
/// * `node` - Current node to check (starts from root, recurses through children)
/// * `target_line` - Line number where cursor is positioned
///
/// # Returns
/// `true` if an ERROR node with label_declaration is found on target line
fn find_error_with_label_on_line(node: Node, target_line: usize) -> bool {
    // Any statement-level node that starts on the cursor line and already has
    // a label_declaration child should trigger label-completion. This covers:
    // - ERROR nodes (incomplete syntax, e.g. "inst1: |")
    // - concurrent_procedure_call_statement
    // - simple_waveform_assignment / conditional_signal_assignment / etc.
    //   where the parser successfully binds the label to the next line's signal
    //   assignment, producing a valid (non-ERROR) node that still starts on the
    //   label line.
    if node.start_position().row == target_line && has_label_declaration(node) {
        return true;
    }
    // Prune branches that can't contain the target line.
    if node.end_position().row < target_line {
        return false;
    }
    for child in node.children(&mut node.walk()) {
        if find_error_with_label_on_line(child, target_line) {
            return true;
        }
    }
    false
}

/// Checks if a node contains a label_declaration child.
///
/// Used to verify that an ERROR or concurrent_procedure_call_statement node
/// actually represents a labeled statement pattern.
///
/// # Arguments
/// * `node` - The node to check for label_declaration children
///
/// # Returns
/// `true` if node has a label_declaration child
fn has_label_declaration(node: Node) -> bool {
    for child in node.children(&mut node.walk()) {
        if child.kind() == "label_declaration" {
            return true;
        }
    }
    false
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

/// Detects whether the cursor sits in the *unit name* position of a direct entity
/// instantiation — i.e. `u0: entity |`, or `u0: entity rtl_lib.my_|`.
///
/// This is text-based rather than AST-based on purpose: while the user is typing,
/// `entity rtl_lib.my_` does not parse into a clean `instantiated_unit`, and the
/// generic dot-access path would otherwise treat the dot as a record field access.
///
/// Anchored to the `entity` keyword, so `use ieee.std_logic_1164` and ordinary
/// record accesses like `my_rec.field` are correctly rejected.
///
/// # Arguments
/// * `text` - Full source text.
/// * `pos` - Cursor position.
///
/// # Returns
/// `Some(CompletionContext::LibraryUnits(lib))` after a library prefix,
/// `Some(CompletionContext::InstantiationLibrary)` right after `entity`,
/// or `None` when this is not an instantiation unit position.
fn detect_instantiation_unit_context(text: &str, pos: Position) -> Option<CompletionContext> {
    let line = text.lines().nth(pos.line as usize)?;
    let col = (pos.character as usize).min(line.len());
    let prefix = &line[..col];

    if let Some(caps) = RE_INST_LIB_DOT.captures(prefix) {
        let library = caps.get(1)?.as_str().to_lowercase();
        return Some(CompletionContext::LibraryUnits(library));
    }

    if RE_INST_ENTITY_KW.is_match(prefix) {
        return Some(CompletionContext::InstantiationLibrary);
    }

    None
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

/// Pure-text subprogram call detection — used as a fallback when the AST has
/// ERROR nodes (e.g. incomplete code while typing, no closing `)` yet).
///
/// Scans backward from the cursor to find the nearest unmatched `(`, then scans
/// further backward to extract the identifier immediately preceding it (skipping
/// whitespace). Returns `None` if:
/// - No open paren is found.
/// - The text before the paren ends with `map` (→ it's a port/generic map).
/// - No identifier precedes the paren.
fn try_text_subprogram_call_context(
    text: &str,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let open_paren = find_call_open_paren(text, cursor_offset)?;
    if open_paren == 0 {
        return None;
    }

    // Scan backward over whitespace to find the end of the name.
    let bytes = text.as_bytes();
    let mut i = open_paren;
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'\r') {
        i -= 1;
    }
    if i == 0 {
        return None;
    }

    // Collect the identifier chars (alphanumeric + underscore).
    let name_end = i;
    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }
    if i == name_end {
        return None; // nothing before the paren
    }

    let raw_name = &text[i..name_end];

    // Exclude VHDL map keywords — these are port/generic maps, not subprogram calls.
    if raw_name.eq_ignore_ascii_case("map") {
        return None;
    }

    let name = raw_name.to_ascii_lowercase();
    Some(classify_call_args(name, text, open_paren, cursor_offset))
}

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

        // Check for subprogram call context before sequential scope, because
        // name-with-parenthesis_group nodes are nested inside process_statement
        // and would otherwise be shadowed by the Process context.
        if (kind == PARENTHESIS_GROUP || kind == NAME)
            && let Some(ctx) = try_subprogram_call_context(n, text, cursor_offset) {
                return ctx;
            }

        // Check for sequential scope (process, subprogram)
        if is_sequential_scope(kind) {
            // Fallback: detect subprogram calls via text when the AST has ERROR
            // nodes (e.g. incomplete code, no closing `)` typed yet).
            if let Some(ctx) = try_text_subprogram_call_context(text, cursor_offset) {
                return ctx;
            }
            return CompletionContext::Process;
        }

        // Check for architecture/concurrent scope
        if is_scope_container(kind) {
            // Same fallback for concurrent procedure calls in architecture body.
            if let Some(ctx) = try_text_subprogram_call_context(text, cursor_offset) {
                return ctx;
            }
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

    // Check for subprogram call context first.
    // This must precede association_element / association_list handling because those
    // nodes appear inside function_call argument lists too.
    if matches!(
        kind,
        ASSOCIATION_ELEMENT | ASSOCIATION_LIST | ASSOCIATION_OR_RANGE_LIST | PARENTHESIS_GROUP
    )
        && let Some(ctx) = try_subprogram_call_context(node, text, cursor_offset) {
            return Some(ctx);
        }

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

    // 1. Instantiation unit position (`entity |`, `entity lib.|`) — must precede the
    //    dot-access fast path, which would otherwise treat the library dot as a
    //    record field access.
    if let Some(ctx) = detect_instantiation_unit_context(text, pos) {
        return ctx;
    }

    // 2. Handle Dot Access (fast path)
    if is_dot_access_context(&node, text, pos) {
        return CompletionContext::DotAccess;
    }

    // 3. Handle Broken Tree (deep scan from root)
    if node.kind() == DESIGN_FILE
        && let Some(context) = deep_scan_for_map_context(node, text, point, cursor_offset)
    {
        return context;
    }

    // 4. Walk the tree upwards (default/valid tree logic)
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
// VHDL Construct Snippet Builders
// =============================================================================

/// Creates a snippet completion item for a combinatorial process statement.
///
/// Expands to a combinatorial process template:
/// ```vhdl
/// process(${1:all}) is
/// begin
///     $0
/// end process;
/// ```
///
/// # Returns
/// CompletionItem configured as a snippet with label "process"
pub fn create_process_snippet() -> CompletionItem {
    let snippet = "process(${1:all}) is\nbegin\n\t$0\nend process;".to_string();
    CompletionItem {
        kind: Some(CompletionItemKind::SNIPPET),
        label: "process".to_string(),
        detail: Some("Combinatiorial process".to_string()),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some("Combinatiorial process".to_string()),
        }),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "** Combinatorial process **\n\n```vhdl\n{}\n```",
                snippet.clone()
            ),
        })),
        sort_text: Some("process".to_string()),
        filter_text: Some("process".to_string()),
        insert_text: Some(snippet),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Creates a snippet completion item for a synchronous clocked process.
///
/// Expands to a synchronous process template with rising edge detection:
/// ```vhdl
/// process(${1:clk}) is
/// begin
///     if rising_edge(${1:clk}) then
///         $0
///     end if;
/// end process;
/// ```
///
/// # Returns
/// CompletionItem configured as a snippet with label "sync"
pub fn create_sync_process_snippet() -> CompletionItem {
    let snippet = "process(${1:clk}) is\nbegin\n\tif rising_edge(${1:clk}) then\n\t\t$0\n\tend if;\nend process;".to_string();
    CompletionItem {
        kind: Some(CompletionItemKind::SNIPPET),
        label: "process-sync".to_string(),
        detail: Some("Synchronous process".to_string()),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some("Synchronous process".to_string()),
        }),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "** Synchronous process **\n\n```vhdl\n{}\n```",
                snippet.clone()
            ),
        })),
        sort_text: Some("process".to_string()),
        filter_text: Some("process".to_string()),
        insert_text: Some(snippet),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Creates a snippet completion item for a synchronous process with synchronous reset.
///
/// Uses the pattern where main logic comes first, then reset overrides:
/// ```vhdl
/// process(${1:clk}) is
/// begin
///     if rising_edge(${1:clk}) then
///         $0
///         if ${2:rst} = '1' then
///             ${3:-- reset values}
///         end if;
///     end if;
/// end process;
/// ```
///
/// # Returns
/// CompletionItem configured as a snippet with label "sync_rst"
pub fn create_sync_rst_process_snippet() -> CompletionItem {
    let snippet = "process(${1:clk}) is\nbegin\n\tif rising_edge(${1:clk}) then\n\t\t$0\n\t\tif ${2:rst} = '1' then\n\t\t\t${3:-- reset_value}\n\t\tend if;\n\tend if;\nend process;".to_string();
    CompletionItem {
        kind: Some(CompletionItemKind::SNIPPET),
        label: "process-sync-rst".to_string(),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some("Synchronous process with synchronous reset".to_string()),
        }),
        detail: Some("Synchronous process with synchronous reset".to_string()),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "** Synchronous process with synchronous reset**\n\n```vhdl\n{}\n```",
                snippet.clone()
            ),
        })),
        sort_text: Some("process".to_string()),
        filter_text: Some("process".to_string()),
        insert_text: Some(snippet),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Creates a snippet completion item for a process with asynchronous reset.
///
/// Expands to an async reset process template:
/// ```vhdl
/// process(${1:clk}, ${2:rst}) is
/// begin
///     if ${2:rst} = '1' then
///         ${3:-- reset values}
///     elsif rising_edge(${1:clk}) then
///         $0
///     end if;
/// end process;
/// ```
///
/// # Returns
/// CompletionItem configured as a snippet with label "async_rst"
pub fn create_async_rst_process_snippet() -> CompletionItem {
    let snippet = "process(${1:clk}, ${2:rst_n}) is\nbegin\n\tif ${2:rst_n} = '0' then\n\t\t${3:-- reset values}\n\telsif rising_edge(${1:clk}) then\n\t\t$0\n\tend if;\nend process;".to_string();
    CompletionItem {
        kind: Some(CompletionItemKind::SNIPPET),
        label: "process-async-rst".to_string(),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some("Synchronous process with asynchronous reset".to_string()),
        }),
        detail: Some("Synchronous process with asynchronous reset".to_string()),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "** Synchronous process with asynchronous reset**\n\n```vhdl\n{}\n```",
                snippet.clone()
            ),
        })),
        sort_text: Some("process".to_string()),
        filter_text: Some("process".to_string()),
        insert_text: Some(snippet),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Creates a snippet completion item for a for-generate statement.
///
/// Expands to a for-generate loop template:
/// ```vhdl
/// for ${1:i} in ${2:0} to ${3:N-1} generate
///     $0
/// end generate;
/// ```
///
/// # Returns
/// CompletionItem configured as a snippet with label "for"
pub fn create_for_generate_snippet() -> CompletionItem {
    let snippet = "for ${1:i} in ${2:0} to ${3:N-1} generate\n\t${4:-- local parameters}\nbegin\n\t$0\nend generate;".to_string();
    CompletionItem {
        kind: Some(CompletionItemKind::SNIPPET),
        label: "for-generate".to_string(),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some("For-generate statement".to_string()),
        }),
        detail: Some("For-generate statement".to_string()),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "** For-generate statement**\n\n```vhdl\n{}\n```",
                snippet.clone()
            ),
        })),
        sort_text: Some("generate".to_string()),
        filter_text: Some("generate".to_string()),
        insert_text: Some(snippet),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Creates a snippet completion item for an if-generate statement.
///
/// Expands to an if-generate template:
/// ```vhdl
/// if ${1:condition} generate
///     $0
/// end generate;
/// ```
///
/// # Returns
/// CompletionItem configured as a snippet with label "if"
pub fn create_if_generate_snippet() -> CompletionItem {
    let snippet =
        "if ${1:condition} generate\n\t${2:-- local parameters}\nbegin\n\t$0\nend generate;"
            .to_string();
    CompletionItem {
        kind: Some(CompletionItemKind::SNIPPET),
        label: "if-generate".to_string(),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some("if-generate statement".to_string()),
        }),
        detail: Some("if-generate statement".to_string()),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "** If-generate statement**\n\n```vhdl\n{}\n```",
                snippet.clone()
            ),
        })),
        sort_text: Some("generate".to_string()),
        filter_text: Some("generate".to_string()),
        insert_text: Some(snippet),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Creates a snippet completion item for a block statement.
///
/// Expands to a block template:
/// ```vhdl
/// block
/// begin
///     $0
/// end block;
/// ```
///
/// # Returns
/// CompletionItem configured as a snippet with label "block"
pub fn create_block_snippet() -> CompletionItem {
    let snippet = "block \n\t${1:-- local parameters}\nbegin\n\t$0\nend block;".to_string();
    CompletionItem {
        kind: Some(CompletionItemKind::SNIPPET),
        label: "block".to_string(),
        detail: Some("block statement".to_string()),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some("block statement".to_string()),
        }),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("** Block statement**\n\n```vhdl\n{}\n```", snippet.clone()),
        })),
        sort_text: Some("block".to_string()),
        filter_text: Some("block".to_string()),
        insert_text: Some(snippet),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

/// Generates a component/entity instantiation snippet from a scope tree.
///
/// Takes an entity or component declaration's scope tree and builds a formatted
/// instantiation with generic map and port map, with proper `=>` alignment.
/// Tab stops start at 1 for the first generic (if any) and increment for each
/// parameter. Default placeholder is the parameter name itself.
///
/// # Arguments
///
/// * `name` - The entity/component name for the instantiation
/// * `scope_tree` - The ScopeTree containing generics and ports
///
/// # Returns
///
/// A formatted snippet string with tab stops for LSP completion
///
/// # Examples
///
/// ```text
/// // Entity with generics and ports:
/// // uart_tx
/// //     generic map (
/// //         BAUD_RATE => ${1:BAUD_RATE},
/// //         DATA_BITS => ${2:DATA_BITS}
/// //     )
/// //     port map (
/// //         clk   => ${3:clk},
/// //         reset => ${4:reset}
/// //     );
/// ```
pub fn generate_instantiation_snippet(name: &str, scope_tree: &ScopeTree) -> String {
    let mut tab_index = 1;
    let mut snippet = String::new();

    let generics: Vec<&Declaration> = scope_tree
        .declarations
        .iter()
        .filter(|d| matches!(d.decl_type, DeclType::Generic))
        .collect();

    let ports: Vec<&Declaration> = scope_tree
        .declarations
        .iter()
        .filter(|d| matches!(d.decl_type, DeclType::Port(_)))
        .collect();

    let max_generic_len = generics.iter().map(|g| g.name.len()).max().unwrap_or(0);
    let max_port_len = ports.iter().map(|p| p.name.len()).max().unwrap_or(0);

    snippet.push_str(&format!("{}\n", name).to_string());
    let generics_line: Vec<String> = generics
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let padding = " ".repeat(max_generic_len - g.name.len());
            format!(
                "\t{}{} => ${{{}:{}}}",
                g.name,
                padding,
                tab_index + i,
                g.name
            )
        })
        .collect();
    if !generics_line.is_empty() {
        snippet.push_str("generic map (\n");
        snippet.push_str(&generics_line.join(",\n"));
        snippet.push_str("\n)\n");
    }
    tab_index = generics_line.len() + 1;

    let ports_line: Vec<String> = ports
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let padding = " ".repeat(max_port_len - p.name.len());
            format!(
                "\t{}{} => ${{{}:{}}}",
                p.name,
                padding,
                tab_index + i,
                p.name
            )
        })
        .collect();
    if !ports_line.is_empty() {
        snippet.push_str("port map (\n");
        snippet.push_str(&ports_line.join(",\n"));
        snippet.push_str("\n);\n");
    }
    snippet
}

/// Generates a component instantiation snippet from a component Declaration.
///
/// Similar to `generate_instantiation_snippet()` but works with component declarations
/// found in packages, where generics and ports are stored in `declaration.parameters`
/// rather than in a separate ScopeTree.
///
/// # Arguments
///
/// * `name` - The component name for the instantiation
/// * `declaration` - The component Declaration containing parameters (generics/ports)
///
/// # Returns
///
/// A formatted snippet string with tab stops for LSP completion
///
/// # Examples
///
/// ```text
/// // Component declaration with generics and ports in parameters:
/// // uart_tx
/// //     generic map (
/// //         BAUD_RATE => ${1:BAUD_RATE}
/// //     )
/// //     port map (
/// //         clk => ${2:clk}
/// //     );
/// ```
pub fn generate_instantiation_snippet_from_declaration(
    name: &str,
    declaration: &Declaration,
) -> String {
    // TODO: Extract parameters from declaration.parameters
    // If None, return simple instantiation with just port map
    let parameters = match &declaration.parameters {
        Some(params) => params,
        None => return format!("{};\n", name), // No interface, just name
    };

    let mut tab_index = 1;
    let mut snippet = String::new();

    let generics: Vec<&Declaration> = parameters
        .iter()
        .filter(|d| matches!(d.decl_type, DeclType::Generic))
        .collect();

    let ports: Vec<&Declaration> = parameters
        .iter()
        .filter(|d| matches!(d.decl_type, DeclType::Port(_)))
        .collect();

    let max_generic_len = generics.iter().map(|g| g.name.len()).max().unwrap_or(0);
    let max_port_len = ports.iter().map(|p| p.name.len()).max().unwrap_or(0);

    snippet.push_str(&format!("{}\n", name).to_string());
    let generics_line: Vec<String> = generics
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let padding = " ".repeat(max_generic_len - g.name.len());
            format!(
                "\t{}{} => ${{{}:{}}}",
                g.name,
                padding,
                tab_index + i,
                g.name
            )
        })
        .collect();
    if !generics_line.is_empty() {
        snippet.push_str("generic map (\n");
        snippet.push_str(&generics_line.join(",\n"));
        snippet.push_str("\n)\n");
    }
    tab_index = generics_line.len() + 1;

    let ports_line: Vec<String> = ports
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let padding = " ".repeat(max_port_len - p.name.len());
            format!(
                "\t{}{} => ${{{}:{}}}",
                p.name,
                padding,
                tab_index + i,
                p.name
            )
        })
        .collect();
    if !ports_line.is_empty() {
        snippet.push_str("port map (\n");
        snippet.push_str(&ports_line.join(",\n"));
        snippet.push_str("\n);\n");
    }
    snippet
}

/// Generates completion items for entity and component instantiations.
///
/// Covers two sources, which deliberately produce different syntax:
///
/// * **Entities**, drawn from every deep-parsed file in the workspace — not just the
///   current one. A direct-instantiation codebase has no component declarations to
///   fall back on, so a current-file-only list would offer nothing. These emit the
///   direct form, `entity <lib>.<name>`, using `work` when the entity shares the
///   current file's library and the explicit library name otherwise. A bare name
///   would not be legal VHDL here without a component declaration in scope.
/// * **Components**, drawn from `use`d packages. These keep emitting a bare name,
///   which is correct for a component instantiation.
///
/// Only deep-parsed entities appear, since a snippet needs the port and generic
/// interface. Shallow-indexed entities are reached through `LibraryUnits` completion
/// instead, which offers the name alone.
///
/// # Arguments
///
/// * `analysis_map` - The global map of all file analyses
/// * `analysis` - The analysis of the current file, used for its library and use clauses
///
/// # Returns
///
/// Vector of completion items, one per available entity/component, deduplicated by name
pub fn generate_entity_completions(
    analysis_map: &AnalysisMap,
    analysis: &Analysis,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    let current_library = analysis.library.clone();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Every deep-parsed entity in the workspace, not just this file's. In a
    // direct-instantiation codebase there are no component declarations to fall
    // back on, so a current-file-only list means no completions at all.
    let mut sources: Vec<(&Analysis, &ScopeTree, String)> = Vec::new();
    for global in analysis_map.values() {
        for (key, tree) in &global.entity_scope_trees {
            sources.push((global, tree, key.clone()));
        }
    }
    sources.sort_by(|a, b| a.2.cmp(&b.2));

    for (owner, entity, key) in sources {
        if !seen.insert(key.clone()) {
            continue;
        }
        let name = entity.name.clone().unwrap_or_else(|| key.clone());

        // `work` is the correct prefix only when the target shares our library;
        // otherwise the library must be named explicitly.
        let unit_ref = if owner.library == current_library {
            format!("entity work.{}", name)
        } else {
            format!("entity {}.{}", owner.library, name)
        };

        let snippet = generate_instantiation_snippet(&unit_ref, entity);
        items.push(CompletionItem {
            kind: Some(CompletionItemKind::SNIPPET),
            label: name.clone(),
            detail: Some("Entity Instantiation".to_string()),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(format!("entity in {}", owner.library)),
            }),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!(
                    "** Generate instantiation for `{}`**\n\n```vhdl\n{}\n```",
                    name, snippet
                ),
            })),
            sort_text: Some(format!("!{}", name)),
            filter_text: Some(name),
            insert_text: Some(snippet),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        });
    }

    for clause in &analysis.use_clauses {
        let pkg_name = &clause.name;
        for (_, global_analysis) in analysis_map.iter() {
            // Primarily check declarations (headers) for completions
            if let Some(pkg_scope) = global_analysis
                .package_declaration_scope_trees
                .get(&pkg_name.to_lowercase())
            {
                let components = if clause.all_import {
                    pkg_scope
                        .declarations
                        .iter()
                        .filter(|d| matches!(d.decl_type, DeclType::Component))
                        .collect()
                } else if let Some(sym) = &clause.imported_symbol
                    && let Some(decl) = pkg_scope
                        .declarations
                        .iter()
                        .find(|d| d.name.eq_ignore_ascii_case(sym))
                {
                    vec![decl]
                } else {
                    vec![]
                };
                items.extend(components.iter().map(|c| {
                    let name = c.name.clone();
                    let snippet = generate_instantiation_snippet_from_declaration(&name, c);
                    CompletionItem {
                        kind: Some(CompletionItemKind::SNIPPET),
                        label: name.clone(),
                        detail: Some("Component Instantiation".to_string()),
                        label_details: Some(CompletionItemLabelDetails {
                            detail: None,
                            description: Some("Component Instantiation".to_string()),
                        }),
                        documentation: Some(Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!(
                                "** Generate instantiation for `{}`**\n\n```vhdl\n{}\n```",
                                name.clone(),
                                snippet.clone()
                            ),
                        })),
                        sort_text: Some(format!("!{}", name.clone())),
                        filter_text: Some(name),
                        insert_text: Some(snippet),
                        insert_text_format: Some(InsertTextFormat::SNIPPET),
                        ..Default::default()
                    }
                }));
            }
        }
    }

    items
}

// =============================================================================
// Completion Item Generation
// =============================================================================

/// Scans backward from `cursor_offset` to find the byte offset of the `(`
/// that opens the current call's or map's argument list.
///
/// Tracks paren depth: the first unmatched `(` found scanning backward is the opener.
///
/// # Arguments
/// * `text` - Full source text.
/// * `cursor_offset` - Byte offset of the cursor.
///
/// # Returns
/// `Some(offset)` of the `(`, or `None` if not found.
fn find_call_open_paren(text: &str, cursor_offset: usize) -> Option<usize> {
    let limit = cursor_offset.min(text.len());
    let bytes = text.as_bytes();
    let mut depth: usize = 0;
    let mut i = limit;
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Finds the byte offset of the `)` that closes the `(` at `open_paren_offset`.
///
/// Scans forward from the opening paren, tracking depth. Returns the offset of
/// the matching `)`, or `None` if the text ends before finding it (incomplete code).
fn find_matching_close_paren(text: &str, open_paren_offset: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut depth: usize = 0;
    let mut i = open_paren_offset;
    while i < n {
        match bytes[i] {
            b'(' => { depth += 1; i += 1; }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => { i += 1; }
        }
    }
    None
}

/// Generates completion items based on the detected context.
///
/// # Arguments
/// * `analysis_map` - The global map of all file analyses.
/// * `current_uri` - The URI of the file being completed.
/// * `context` - The determined `CompletionContext`.
/// * `position` - The cursor position.
/// * `text` - The full source text of the current document (used for dot-access chain extraction).
///
/// # Returns
/// Vector of completion items appropriate for the context.
pub fn complete_scope(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    context: &CompletionContext,
    position: Position,
    text: &str,
    tree_root: Node,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    if let Some(current_analysis) = analysis_map.get(current_uri) {
        let local_scope_tree = current_analysis.find_scope_tree_at(&position);

        // --- Subprogram Call LHS: offer filtered parameter names ---
        // Runs before the match so SubprogramCallBoth falls through to the _ arm for RHS items too.
        if let CompletionContext::SubprogramCallLhs(name) | CompletionContext::SubprogramCallBoth(name) =
            context
        {
            let cursor_offset = position_to_offset(text, position);
            let open_paren = find_call_open_paren(text, cursor_offset).unwrap_or(0);
            // Scan the FULL argument list (open to close paren) so params already present
            // after the cursor are also excluded from suggestions.
            let scan_end = find_matching_close_paren(text, open_paren).unwrap_or(text.len());
            let used = collect_used_param_names(text, open_paren, scan_end);

            let params_start = items.len();

            if let Some(tree) = &local_scope_tree {
                let innermost = tree.find_innermost_scope(&position);
                let header = tree
                    .entity
                    .as_ref()
                    .and_then(|n| {
                        current_analysis
                            .entity_scope_trees
                            .get(n)
                            .or_else(|| analysis_map.values().find_map(|a| a.entity_scope_trees.get(n)))
                    })
                    .or_else(|| {
                        tree.package.as_ref().and_then(|n| {
                            current_analysis
                                .package_declaration_scope_trees
                                .get(n)
                                .or_else(|| current_analysis.package_body_scope_trees.get(n))
                        })
                    });

                // Helper closure: push params of a matching subprogram declaration.
                let push_params = |items: &mut Vec<CompletionItem>, decl: &Declaration| {
                    if decl.name.eq_ignore_ascii_case(name)
                        && matches!(
                            decl.decl_type,
                            DeclType::Function
                                | DeclType::FunctionDeclaration
                                | DeclType::Procedure
                                | DeclType::ProcedureDeclaration
                        )
                        && let Some(params) = &decl.parameters {
                            for param in params {
                                if !used.contains(&param.name.to_ascii_lowercase()) {
                                    items.push(declaration_to_completion(param));
                                }
                            }
                        }
                };

                // 1. Local scope (architecture declarative region, process variables, etc.)
                if let Some(declarations) = tree.collect_visible_declarations(&innermost.range, header) {
                    for decl in &declarations {
                        push_params(&mut items, decl);
                    }
                }

                // 2. Imported packages — mirrors the `_` arm's use_clauses logic so that
                //    functions/procedures declared in packages are also resolved.
                let mut effective_clauses: Vec<UseClause> =
                    current_analysis.context_clauses_for(tree).to_vec();
                if let Some(entity_name) = &tree.entity {
                    let entity_key = entity_name.to_lowercase();
                    if let Some(entity_tree) = current_analysis.entity_scope_trees.get(&entity_key) {
                        effective_clauses
                            .extend_from_slice(current_analysis.context_clauses_for(entity_tree));
                    } else {
                        for (other_uri, other_analysis) in analysis_map.iter() {
                            if other_uri != current_uri
                                && let Some(entity_tree) =
                                    other_analysis.entity_scope_trees.get(&entity_key)
                                {
                                    effective_clauses.extend_from_slice(
                                        other_analysis.context_clauses_for(entity_tree),
                                    );
                                    break;
                                }
                        }
                    }
                }
                let chain = tree.collect_scope_chain(&position);
                for scope in &chain {
                    effective_clauses.extend_from_slice(&scope.use_clauses);
                }
                for clause in &effective_clauses {
                    let pkg_name = clause.name.to_lowercase();
                    for (_, analysis) in analysis_map.iter() {
                        if let Some(pkg_scope) =
                            analysis.package_declaration_scope_trees.get(&pkg_name)
                        {
                            let candidates: Vec<&Declaration> = if clause.all_import {
                                pkg_scope.declarations.iter().collect()
                            } else if let Some(sym) = &clause.imported_symbol {
                                pkg_scope
                                    .declarations
                                    .iter()
                                    .filter(|d| d.name.eq_ignore_ascii_case(sym))
                                    .collect()
                            } else {
                                vec![]
                            };
                            for decl in candidates {
                                push_params(&mut items, decl);
                            }
                        }
                    }
                }
            }

            // In SubprogramCallBoth, params share the list with generic scope items.
            // Prefix their sortText so editors float them to the top of the list.
            if matches!(context, CompletionContext::SubprogramCallBoth(_)) {
                for item in &mut items[params_start..] {
                    item.sort_text = Some(format!("0_{}", item.label));
                }
            }

            // SubprogramCallLhs: return param names only (no scope items)
            // SubprogramCallBoth: fall through to the match `_` arm which adds scope items
            if matches!(context, CompletionContext::SubprogramCallLhs(_)) {
                items.sort_by(|a, b| a.label.cmp(&b.label));
                items.dedup_by(|a, b| a.label == b.label);
                return items;
            }
        }

        if *context == CompletionContext::Architecture && is_after_label(text, position, tree_root)
        {
            // Offer process and generate snippets
            items.push(create_process_snippet());
            items.push(create_sync_process_snippet());
            items.push(create_sync_rst_process_snippet());
            items.push(create_async_rst_process_snippet());

            items.push(create_for_generate_snippet());
            items.push(create_if_generate_snippet());
            items.push(create_block_snippet());

            // Offer entity/component instantiation snippets
            items.extend(generate_entity_completions(analysis_map, current_analysis));
        }

        match context {
            CompletionContext::PortMapLhs(target_name)
            | CompletionContext::GenericMapLhs(target_name) => {
                let is_generic = matches!(context, CompletionContext::GenericMapLhs(_));
                let target_lower = target_name.to_lowercase();

                // Compute already-connected port/generic names to filter from suggestions.
                // Scan the FULL argument list (open to close paren) so names already present
                // after the cursor are also excluded when inserting in the middle of a map.
                let cursor_offset = position_to_offset(text, position);
                let open_paren = find_call_open_paren(text, cursor_offset).unwrap_or(0);
                let scan_end = find_matching_close_paren(text, open_paren).unwrap_or(text.len());
                let used = collect_used_param_names(text, open_paren, scan_end);

                //  First we look in the global lookup
                for analysis in analysis_map.values() {
                    if let Some(entity_tree) = analysis.entity_scope_trees.get(&target_lower) {
                        for decl in &entity_tree.declarations {
                            let valid = if is_generic {
                                matches!(decl.decl_type, DeclType::Generic)
                            } else {
                                matches!(decl.decl_type, DeclType::Port(_))
                            };

                            if valid && !used.contains(&decl.name.to_ascii_lowercase()) {
                                items.push(declaration_to_completion(decl));
                            }
                        }
                    }
                }

                // We need to check local scope because of direct instantiation
                if let Some(tree) = local_scope_tree {
                    let innermost = tree.find_innermost_scope(&position);
                    // Resolve implicit context (Entity or Package head).
                    // Check current file first, then fall back to analysis_map for cross-file.
                    let header = tree
                        .entity
                        .as_ref()
                        .and_then(|n| {
                            current_analysis
                                .entity_scope_trees
                                .get(n)
                                .or_else(|| {
                                    analysis_map
                                        .values()
                                        .find_map(|a| a.entity_scope_trees.get(n))
                                })
                        })
                        .or_else(|| {
                            tree.package.as_ref().and_then(|n| {
                                current_analysis
                                    .package_declaration_scope_trees
                                    .get(n)
                                    .or_else(|| current_analysis.package_body_scope_trees.get(n))
                            })
                        });

                    if let Some(decls) = tree.collect_visible_declarations(&innermost.range, header)
                    {
                        // Find the COMPONENT declaration with the target name
                        if let Some(comp_decl) = decls.iter().find(|d| {
                            d.name.eq_ignore_ascii_case(target_name)
                                && matches!(d.decl_type, DeclType::Component)
                        }) {
                            // Extract ports/generics from the component's parameters
                            if let Some(params) = &comp_decl.parameters {
                                for param in params {
                                    let valid = if is_generic {
                                        matches!(param.decl_type, DeclType::Generic)
                                    } else {
                                        matches!(param.decl_type, DeclType::Port(_))
                                    };

                                    if valid && !used.contains(&param.name.to_ascii_lowercase()) {
                                        items.push(declaration_to_completion(param));
                                    }
                                }
                            }
                        }
                    }
                }

                return items;
            }
            CompletionContext::InstantiationLibrary => {
                for library in crate::backend::units::known_libraries(analysis_map) {
                    items.push(CompletionItem {
                        kind: Some(CompletionItemKind::MODULE),
                        label: library.clone(),
                        detail: Some("VHDL library".to_string()),
                        insert_text: Some(format!("{}.", library)),
                        ..Default::default()
                    });
                }
                // `work` is always a valid prefix even if no file is stamped with it.
                if !items.iter().any(|i| i.label == "work") {
                    items.push(CompletionItem {
                        kind: Some(CompletionItemKind::MODULE),
                        label: "work".to_string(),
                        detail: Some("VHDL library (current)".to_string()),
                        insert_text: Some("work.".to_string()),
                        ..Default::default()
                    });
                }
                items.sort_by(|a, b| a.label.cmp(&b.label));
                return items;
            }

            CompletionContext::LibraryUnits(library) => {
                // `work` means the library of the file being edited. Compare
                // case-insensitively — VHDL identifiers are case-insensitive and this
                // payload comes from user-typed text.
                let target = if library.eq_ignore_ascii_case("work") {
                    current_analysis.library.clone()
                } else {
                    library.clone()
                };

                for (name, entity_uri) in
                    crate::backend::units::entities_in_library(analysis_map, &target)
                {
                    // Emit a full port-map snippet only when the entity's interface is
                    // already in memory. Deep-parsing a whole library to build this list
                    // would be unacceptable, so shallow entities get a plain name and the
                    // existing PortMapLhs path fills in ports once the user types further.
                    let deep_tree = analysis_map
                        .get(&entity_uri)
                        .and_then(|a| a.entity_scope_trees.get(&name));

                    let (insert_text, format) = match deep_tree {
                        Some(tree) => (
                            generate_instantiation_snippet(&name, tree),
                            InsertTextFormat::SNIPPET,
                        ),
                        None => (name.clone(), InsertTextFormat::PLAIN_TEXT),
                    };

                    items.push(CompletionItem {
                        kind: Some(CompletionItemKind::CLASS),
                        label: name.clone(),
                        detail: Some(format!("entity in {}", target)),
                        filter_text: Some(name.clone()),
                        insert_text: Some(insert_text),
                        insert_text_format: Some(format),
                        ..Default::default()
                    });
                }
                items.sort_by(|a, b| a.label.cmp(&b.label));
                return items;
            }

            CompletionContext::DotAccess => {
                if let Some(chain) = extract_dot_chain(text, position) {
                    let results = resolve_path_chain(&chain, current_uri, analysis_map, &position);
                    for res in results {
                        if let ResolvedItem::Declaration(decl) = res.item {
                            let type_name = &decl.type_info.base_type;
                            let type_defs = lookup_symbol(
                                type_name,
                                current_uri,
                                analysis_map,
                                &position,
                                false,
                            );
                            for type_res in type_defs {
                                if let ResolvedItem::Declaration(t) = type_res.item
                                    && let Some(fields) = &t.parameters
                                {
                                    for f in fields {
                                        items.push(declaration_to_completion(f));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // General Scope Completion (RHS, Process, Architecture body)
            _ => {
                let scope_items_start = items.len();
                if let Some(tree) = local_scope_tree {
                    let innermost = tree.find_innermost_scope(&position);
                    // Resolve entity/package header — check current file first, then
                    // fall back to analysis_map so cross-file entity+arch works.
                    let header = tree
                        .entity
                        .as_ref()
                        .and_then(|n| {
                            current_analysis
                                .entity_scope_trees
                                .get(n)
                                .or_else(|| {
                                    analysis_map
                                        .values()
                                        .find_map(|a| a.entity_scope_trees.get(n))
                                })
                        })
                        .or_else(|| {
                            tree.package.as_ref().and_then(|n| {
                                current_analysis
                                    .package_declaration_scope_trees
                                    .get(n)
                                    .or_else(|| current_analysis.package_body_scope_trees.get(n))
                            })
                        });

                    if let Some(declarations) =
                        tree.collect_visible_declarations(&innermost.range, header)
                    {
                        for decl in declarations {
                            // In SubprogramCallBoth, suppress the called function/procedure
                            // itself from the RHS scope list — it's noise when you're inside
                            // its own argument list.
                            if let CompletionContext::SubprogramCallBoth(called_name) = context
                                && decl.name.eq_ignore_ascii_case(called_name)
                                    && matches!(
                                        decl.decl_type,
                                        DeclType::Function
                                            | DeclType::FunctionDeclaration
                                            | DeclType::Procedure
                                            | DeclType::ProcedureDeclaration
                                    )
                                {
                                    continue;
                                }
                            items.push(declaration_to_completion(&decl));
                        }
                    }
                    // Build effective use_clauses:
                    //   1. Context clauses for this specific design unit (not the whole file)
                    //   2. Entity's context clauses when arch and entity are in separate files
                    //      (Rule 3: entity context is inherited by all implementing architectures)
                    //   3. Inner-scope use clauses from the scope chain (generate, block, etc.)
                    let mut effective_clauses: Vec<UseClause> =
                        current_analysis.context_clauses_for(tree).to_vec();
                    if let Some(entity_name) = &tree.entity {
                        let entity_key = entity_name.to_lowercase();
                        // Same-file (90% case): entity's context clauses are already correct
                        // via context_clauses_for on the entity's design unit — but we need
                        // to look them up from the entity's own design unit, not the arch's.
                        if let Some(entity_tree) =
                            current_analysis.entity_scope_trees.get(&entity_key)
                        {
                            effective_clauses.extend_from_slice(
                                current_analysis.context_clauses_for(entity_tree),
                            );
                        } else {
                            // Cross-file: find the entity's analysis and inherit its context.
                            for (other_uri, other_analysis) in analysis_map.iter() {
                                if other_uri != current_uri
                                    && let Some(entity_tree) =
                                        other_analysis.entity_scope_trees.get(&entity_key)
                                    {
                                        effective_clauses.extend_from_slice(
                                            other_analysis.context_clauses_for(entity_tree),
                                        );
                                        break;
                                    }
                            }
                        }
                    }
                    let chain = tree.collect_scope_chain(&position);
                    for scope in &chain {
                        effective_clauses.extend_from_slice(&scope.use_clauses);
                    }
                    // Now we add public symbols from imported packages (scope-chain-aware)
                    for clause in &effective_clauses {
                        let pkg_name = &clause.name;
                        for (_, analysis) in analysis_map.iter() {
                            if let Some(pkg_scope) = analysis
                                .package_declaration_scope_trees
                                .get(&pkg_name.to_lowercase())
                            {
                                let declarations: Vec<Declaration> = if clause.all_import {
                                    pkg_scope.declarations.clone()
                                } else if let Some(sym) = &clause.imported_symbol {
                                    pkg_scope
                                        .declarations
                                        .iter()
                                        .filter(|d| d.name.eq_ignore_ascii_case(sym))
                                        .cloned()
                                        .collect()
                                } else {
                                    vec![]
                                };
                                for decl in declarations {
                                    items.push(declaration_to_completion(&decl));
                                }
                            }
                        }
                    }
                }
                // When falling through from SubprogramCallBoth, push scope items
                // behind params in the sort order so the function's own params
                // float to the top of the completion list.
                if matches!(context, CompletionContext::SubprogramCallBoth(_)) {
                    for item in &mut items[scope_items_start..] {
                        item.sort_text = Some(format!("1_{}", item.label));
                    }
                }
            }
        }
    }

    // Deduplication
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);

    items
}

/// Extracts the dot-separated identifier chain before the cursor for record field completion.
///
/// Scans backwards from the cursor position to collect identifiers joined by dots.
/// Only returns a chain if the text immediately before the cursor ends with a dot,
/// indicating the user is requesting field completion.
///
/// Example: `"rec.sub.|"` → `Some(["rec", "sub"])`
///
/// # Arguments
/// * `text` - The full source text of the document.
/// * `pos` - The cursor position.
///
/// # Returns
/// `Some(chain)` if a valid dot-terminated path is found, `None` otherwise.
fn extract_dot_chain(text: &str, pos: Position) -> Option<Vec<String>> {
    let offset = position_to_offset(text, pos);
    let slice = &text[..offset];
    let trimmed = slice.trim_end();

    // Scan backwards for valid identifier chars and dots
    let mut start = trimmed.len();
    for (i, c) in trimmed.char_indices().rev() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            start = i;
        } else {
            break;
        }
    }

    let path_str = &trimmed[start..];

    if path_str.ends_with('.') {
        let parts: Vec<String> = path_str
            .split('.')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !parts.is_empty() {
            return Some(parts);
        }
    }
    None
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
        DeclType::Function | DeclType::FunctionDeclaration => CompletionItemKind::FUNCTION,
        DeclType::Type => CompletionItemKind::STRUCT,
        DeclType::Subtype => CompletionItemKind::STRUCT,
        DeclType::Procedure | DeclType::ProcedureDeclaration => CompletionItemKind::FUNCTION,
        DeclType::Attribute => CompletionItemKind::TYPE_PARAMETER,
        DeclType::RecordField => CompletionItemKind::FIELD,
        DeclType::EnumLiteral => CompletionItemKind::ENUM_MEMBER,
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
mod tests;
