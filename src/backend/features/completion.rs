use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind, Position, Range,
    Url,
};
use tree_sitter::{Node, Point};

use crate::analysis::{OxideSymbolKind, Symbol};
use crate::backend::AnalysisMap;

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
    Unknown,
}

/// Helper function to extract the instantiated unit name from a component instantiation node.
/// e.g. "u0: entity work.my_comp(rtl)" -> returns "my_comp"
///
/// # Arguments
/// * `node` - The component instantiation node.
/// * `text` - The text representing the parsed file.
///
/// # Return
/// `Some(String)` with the valid instantiated unit name, or `None` if the extraction failed.
fn get_instantiated_name(node: Node, text: &str) -> Option<String> {
    // 1. Try to find the complex "instantiated_unit" (e.g., "entity work.comp")
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
    let mut cursor = node.walk();
    if let Some(name_node) = node.children(&mut cursor).find(|c| c.kind() == "name") {
        // --- CASE 2: Simple Component Instantiation ---
        let raw_text = name_node.utf8_text(text.as_bytes()).ok()?;

        // Even simple components might have an architecture suffix: "my_comp(rtl)"
        let text_no_arch = raw_text.split('(').next().unwrap_or(raw_text);

        let res = text_no_arch.trim().to_string();
        return Some(res);
    }

    None
}

/// Walks up the tree (or uses text heuristic) to find the component name and the map kind (Generic/Port).
/// This function encapsulates the complexity of handling broken ASTs and extracting the component name.
///
/// # Arguments
/// * `node` - The starting AST node (usually an association list, ERROR, or statement).
/// * `text` - The full source code.
/// * `cursor_offset` - The byte offset of the cursor.
///
/// # Return
/// `Some((String, DetectedMapKind))` containing the component name and the map type, or `None`.
fn find_component_declaration(
    node: Node,
    text: &str,
    cursor_offset: usize,
) -> Option<(String, DetectedMapKind)> {
    eprintln!(
        "  [DEBUG][find_component_declaration] Start. Node kind: {}",
        node.kind()
    );

    // 1. Try Tree Traversal
    let mut map_kind = DetectedMapKind::Unknown;
    let mut current = Some(node);

    while let Some(n) = current {
        let kind = n.kind();

        if kind == "generic_map_aspect" {
            map_kind = DetectedMapKind::Generic;
        }
        if kind == "port_map_aspect" {
            map_kind = DetectedMapKind::Port;
        }

        // CASE A: Normal Instantiation
        if kind == "component_instantiation_statement" {
            if let Some(name) = get_instantiated_name(n, text) {
                eprintln!(
                    "  [DEBUG][find_component_declaration] Found via tree: {}",
                    name
                );
                return Some((name, map_kind));
            }
            break;
        }

        // CASE B: Mis-parsed as Signal Assignment (Handles broken trees where instantiation is wrapped)
        if kind == "concurrent_simple_signal_assignment" {
            let has_generic = has_descendant_kind(n, "generic_map_aspect");
            let has_port = has_descendant_kind(n, "port_map_aspect");

            if map_kind == DetectedMapKind::Unknown {
                if has_generic && !has_port {
                    map_kind = DetectedMapKind::Generic;
                } else if has_port {
                    map_kind = DetectedMapKind::Port;
                }
            }

            if has_generic || has_port {
                let mut start_search = n.start_byte();
                if let Some(label) = n.child_by_field_name("label") {
                    start_search = label.end_byte();
                }

                let mut cursor = n.walk();
                for child in n.children(&mut cursor) {
                    if child.start_byte() >= start_search {
                        if child.kind() == "ERROR" {
                            let mut err_cursor = child.walk();
                            for err_child in child.children(&mut err_cursor) {
                                if err_child.kind() == "name" || err_child.kind() == "identifier" {
                                    let name =
                                        err_child.utf8_text(text.as_bytes()).ok()?.to_string();
                                    return Some((name, map_kind));
                                }
                            }
                        }
                        if child.kind() == "name" || child.kind() == "identifier" {
                            let name = child.utf8_text(text.as_bytes()).ok()?.to_string();
                            return Some((name, map_kind));
                        }
                    }
                }
            }
        }
        current = n.parent();
    }

    // 2. Fallback: Text Search (Used when tree structure is too broken to traverse)
    let search_limit = cursor_offset.min(text.len());
    let text_before = &text[..search_limit];

    eprintln!(
        "  [DEBUG][find_component_declaration] Text fallback. Limit: {}. Tail: {:?}",
        search_limit,
        text_before.lines().last().unwrap_or("")
    );

    if let Some(colon_idx) = text_before.rfind(':') {
        let after_colon = &text_before[colon_idx + 1..];
        let lower = after_colon.to_lowercase();

        let gmap_idx = lower.rfind("generic map");
        let pmap_idx = lower.rfind("port map");

        let (cut_idx, found_kind) = match (gmap_idx, pmap_idx) {
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

        let final_kind = if map_kind == DetectedMapKind::Unknown {
            found_kind
        } else {
            map_kind
        };

        let raw_decl = &after_colon[..cut_idx];
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

        if !final_clean.is_empty() && final_clean.chars().all(|c| c.is_alphanumeric() || c == '_') {
            eprintln!(
                "  [DEBUG][find_component_declaration] Found via text: {} ({:?})",
                final_clean, final_kind
            );
            return Some((final_clean.to_string(), final_kind));
        }
    }

    None
}

/// Helper to check deep structure without cloning everything
///
/// # Arguments
/// * `node` - The node we want to traverse.
/// * `kind` - The kind we want to find in the node.
///
/// # Return
/// `true` if a child or grand-child (via ERROR node) of the node is of kind `kind`.
fn has_descendant_kind(node: Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return true;
        }
        // Check 1 level deep inside ERROR nodes
        if child.kind() == "ERROR" {
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

/// Converts an LSP position (line/character) into a byte offset within the source text.
///
/// # Arguments
/// * `text` - The full source code.
/// * `pos` - The LSP `Position`.
///
/// # Return
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

/// Recursively traverses descendants to find map context in broken trees (starting at 'design_file').
///
/// # Arguments
/// * `start_node` - The node to start the recursive descent from.
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
        eprintln!(
            "[DEBUG] Deep Scan child: '{}' at ({},{})-({},{})",
            child.kind(),
            child.start_position().row,
            child.start_position().column,
            child.end_position().row,
            child.end_position().column
        );

        // 1. Check if the current child spans the point
        let child_start = child.start_position();
        let child_end = child.end_position();

        let is_point_contained = (point.row > child_start.row
            || (point.row == child_start.row && point.column >= child_start.column))
            && (point.row < child_end.row
                || (point.row == child_end.row && point.column <= child_end.column));

        // 2. For ERROR nodes, ALWAYS try to extract map context regardless of point containment
        //    This handles cases where the ERROR node's boundaries might be off due to broken syntax
        if child.kind() == "ERROR" {
            eprintln!("[DEBUG] Deep Scan: Found ERROR node, checking for map context");
            if let Some((name, map_kind)) = find_component_declaration(child, text, cursor_offset) {
                let limit = cursor_offset.min(text.len());
                let prefix = &text[..limit];
                let last_comma = prefix.rfind(',');
                let last_paren = prefix.rfind('(');
                let start_idx = match (last_comma, last_paren) {
                    (Some(c), Some(p)) => c.max(p),
                    (Some(c), None) => c,
                    (None, Some(p)) => p,
                    (None, None) => 0,
                };

                let current_segment = &prefix[start_idx..];
                let is_rhs = current_segment.contains("=>");
                let is_generic = map_kind == DetectedMapKind::Generic;

                eprintln!(
                    "[DEBUG] Deep Scan ERROR: Name: {}, is_rhs: {}, is_generic: {}",
                    name, is_rhs, is_generic
                );

                if is_rhs {
                    return Some(if is_generic {
                        CompletionContext::GenericMapRhs
                    } else {
                        CompletionContext::PortMapRhs
                    });
                } else {
                    return Some(if is_generic {
                        CompletionContext::GenericMapLhs(name)
                    } else {
                        CompletionContext::PortMapLhs(name)
                    });
                }
            }
            // Also recurse into ERROR node
            if let Some(ctx) = deep_scan_for_map_context(child, text, point, cursor_offset) {
                return Some(ctx);
            }
        }

        if !is_point_contained {
            // If the cursor is not in this node, check its children if it's a scope node
            if (child.kind() == "architecture_body"
                || child.kind() == "block_statement"
                || child.kind() == "generate_statement"
                || child.kind() == "process_statement")
                && let Some(ctx) = deep_scan_for_map_context(child, text, point, cursor_offset)
            {
                return Some(ctx);
            }
            continue;
        }

        // 3. Point is contained: check if it's a map context
        let is_map_context = child.kind() == "component_instantiation_statement"
            || child.kind() == "concurrent_simple_signal_assignment";

        if is_map_context {
            eprintln!("[DEBUG] Deep Scan Found matching node: '{}'", child.kind());
            if let Some((name, map_kind)) = find_component_declaration(child, text, cursor_offset) {
                let limit = cursor_offset.min(text.len());
                let prefix = &text[..limit];
                let last_comma = prefix.rfind(',');
                let last_paren = prefix.rfind('(');
                let start_idx = match (last_comma, last_paren) {
                    (Some(c), Some(p)) => c.max(p),
                    (Some(c), None) => c,
                    (None, Some(p)) => p,
                    (None, None) => 0,
                };

                let current_segment = &prefix[start_idx..];
                let is_rhs = current_segment.contains("=>");
                let is_generic = map_kind == DetectedMapKind::Generic;

                eprintln!(
                    "[DEBUG] Deep Scan Final Context: Name: {}, is_rhs: {}, is_generic: {}",
                    name, is_rhs, is_generic
                );

                if is_rhs {
                    return Some(if is_generic {
                        CompletionContext::GenericMapRhs
                    } else {
                        CompletionContext::PortMapRhs
                    });
                } else {
                    return Some(if is_generic {
                        CompletionContext::GenericMapLhs(name)
                    } else {
                        CompletionContext::PortMapLhs(name)
                    });
                }
            }
        }

        // 4. If the point is contained and it's a scope node, recurse deeper
        if (child.kind() == "architecture_body"
            || child.kind() == "block_statement"
            || child.kind() == "generate_statement"
            || child.kind() == "process_statement")
            && let Some(ctx) = deep_scan_for_map_context(child, text, point, cursor_offset)
        {
            return Some(ctx);
        }
    }
    None
}

/// Handles the upward traversal of the AST for contexts that are not at the root level.
/// This applies the main scope logic (Architecture, Process) and the map logic for valid structures.
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
        eprintln!("[DEBUG] Walking Up... Current Kind: '{}'", n.kind());
        match n.kind() {
            "process_statement" | "subprogram_body" => return CompletionContext::Process,
            "architecture_definition"
            | "architecture_body"
            | "block_statement"
            | "generate_statement"
            | "concurrent_block" => return CompletionContext::Architecture,

            // Handle Broken/Mis-parsed Trees (If found via UPWARD walk)
            "ERROR" | "concurrent_simple_signal_assignment" => {
                if let Some((name, map_kind)) = find_component_declaration(n, text, cursor_offset) {
                    // Heuristic: Check for arrow '=>' in the current text segment
                    let limit = cursor_offset.min(text.len());
                    let prefix = &text[..limit];

                    let last_comma = prefix.rfind(',');
                    let last_paren = prefix.rfind('(');
                    let start_idx = match (last_comma, last_paren) {
                        (Some(c), Some(p)) => c.max(p),
                        (Some(c), None) => c,
                        (None, Some(p)) => p,
                        (None, None) => 0,
                    };

                    let current_segment = &prefix[start_idx..];
                    let is_rhs = current_segment.contains("=>");

                    eprintln!(
                        "[DEBUG] UPWARD ERROR/Assign Logic: Name: {}, is_rhs: {}",
                        name, is_rhs
                    );

                    let is_generic = map_kind == DetectedMapKind::Generic;

                    if is_rhs {
                        return if is_generic {
                            CompletionContext::GenericMapRhs
                        } else {
                            CompletionContext::PortMapRhs
                        };
                    } else {
                        return if is_generic {
                            CompletionContext::GenericMapLhs(name)
                        } else {
                            CompletionContext::PortMapLhs(name)
                        };
                    }
                }
            }

            "association_element" => {
                let cursor_col = point.column;
                let mut is_rhs = false;
                let mut cursor = n.walk();
                for child in n.children(&mut cursor) {
                    if child.kind() == "=>" {
                        let arrow_end = child.end_position().column;
                        if cursor_col >= arrow_end {
                            is_rhs = true;
                        }
                    }
                }

                if let Some((name, map_kind)) = find_component_declaration(n, text, cursor_offset) {
                    let is_generic = match map_kind {
                        DetectedMapKind::Generic => true,
                        DetectedMapKind::Port => false,
                        DetectedMapKind::Unknown => {
                            let p_kind = n
                                .parent()
                                .map(|p| p.parent().map(|pp| pp.kind()).unwrap_or(""))
                                .unwrap_or("");
                            p_kind == "generic_map_aspect"
                        }
                    };
                    eprintln!(
                        "[DEBUG] association_element Logic: Name: {}, is_rhs: {}",
                        name, is_rhs
                    );

                    if is_rhs {
                        return if is_generic {
                            CompletionContext::GenericMapRhs
                        } else {
                            CompletionContext::PortMapRhs
                        };
                    } else {
                        return if is_generic {
                            CompletionContext::GenericMapLhs(name)
                        } else {
                            CompletionContext::PortMapLhs(name)
                        };
                    }
                }
                return CompletionContext::PortMapRhs;
            }

            "association_list" => {
                eprintln!("[DEBUG] Inside association_list.");
                let mut is_rhs = false;
                let mut cursor_chk = n.walk();
                for child in n.children(&mut cursor_chk) {
                    let start = child.start_position();
                    let end = child.end_position();
                    let strictly_after = start.row > point.row
                        || (start.row == point.row && start.column > point.column);
                    let at_start = start.row == point.row && start.column == point.column;

                    if strictly_after || (at_start && child.kind() == ",") {
                        if child.kind() == "ERROR" {
                            let mut sub = child.walk();
                            for sub_child in child.children(&mut sub) {
                                if sub_child.kind() == "=>" {
                                    is_rhs = false;
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
                        let mut sub = child.walk();
                        for sub_child in child.children(&mut sub) {
                            if sub_child.kind() == "=>" {
                                has_arrow = true;
                                if sub_child.end_position().column <= point.column {
                                    is_rhs = true;
                                }
                            }
                        }
                        // If we are inside the element and found an arrow, check position relative to arrow end
                        let is_inside = !(point.row > end.row
                            || (point.row == end.row && point.column > end.column));
                        if is_inside && has_arrow {
                            // is_rhs already calculated above relative to arrow end
                        } else if is_inside && !has_arrow {
                            is_rhs = false; // LHS or no arrow yet
                        } else if !is_inside && has_arrow {
                            is_rhs = true; // After the element but before the next comma, treat as RHS
                        }
                    }
                    if child.kind() == "ERROR" {
                        let mut sub = child.walk();
                        for sub_child in child.children(&mut sub) {
                            if sub_child.kind() == "=>"
                                && point.column >= sub_child.end_position().column
                            {
                                is_rhs = true;
                            }
                        }
                    }
                }

                if let Some((name, map_kind)) = find_component_declaration(n, text, cursor_offset) {
                    let final_map_kind = if map_kind == DetectedMapKind::Unknown {
                        let parent_kind = n.parent().map(|p| p.kind()).unwrap_or("");
                        if parent_kind == "generic_map_aspect" {
                            DetectedMapKind::Generic
                        } else {
                            DetectedMapKind::Port
                        }
                    } else {
                        map_kind
                    };
                    let is_generic = final_map_kind == DetectedMapKind::Generic;

                    if is_rhs {
                        return if is_generic {
                            CompletionContext::GenericMapRhs
                        } else {
                            CompletionContext::PortMapRhs
                        };
                    } else {
                        return if is_generic {
                            CompletionContext::GenericMapLhs(name)
                        } else {
                            CompletionContext::PortMapLhs(name)
                        };
                    }
                }
                return CompletionContext::PortMapRhs;
            }

            "component_instantiation_statement" | "port_map_aspect" | "generic_map_aspect" => {
                if let Some((name, _)) = find_component_declaration(n, text, cursor_offset) {
                    return CompletionContext::PortMapLhs(name);
                }
            }
            _ => {}
        }
        current = n.parent();
    }

    eprintln!("[DEBUG] Walk finished. Returning Unresolved.");
    CompletionContext::Unresolved
}

/// Determines the semantic context of the cursor position by analyzing the AST.
///
/// This function converts the LSP position into a Tree-sitter point, finds the
/// smallest syntax node at that location, and dispatches the request to the
/// appropriate handling function (Dot Access, Deep Scan, or Upward Traversal).
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

    // --- 1. Handle Dot Access (Needs to be first and fast) ---
    let kind = node.kind();
    let parent_kind = node.parent().map(|p| p.kind()).unwrap_or("");
    if kind == "." || parent_kind == "selected_name" || parent_kind == "selection" {
        return CompletionContext::DotAccess;
    }
    // Text-based fallback for dot access, e.g., typing `r.f|`
    if let Some(line) = text.lines().nth(pos.line as usize)
        && pos.character as usize <= line.len()
    {
        let prefix = &line[..pos.character as usize];
        let trimmed = prefix.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
        if trimmed.ends_with('.') {
            return CompletionContext::DotAccess;
        }
    }

    // --- 2. Handle Broken Tree (Deep Scan from Root) ---
    if node.kind() == "design_file"
        && let Some(context) = deep_scan_for_map_context(node, text, point, cursor_offset)
    {
        return context;
    }

    // --- 3. Walk the tree UPWARDS (Default/Valid Tree Logic) ---
    handle_upward_traversal(node, text, point, cursor_offset)
}

/// Generates completion items based on the global context.
///
/// # Arguments
/// * `analysis_map` - The global map of all file analyses.
/// * `current_uri` - The URI of the file being completed.
/// * `context` - The determined `CompletionContext`.
/// * `position` - The cursor position.
///
/// # Return
/// Vector of completion items appropriate for the context.
pub fn complete_scope(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    context: &CompletionContext,
    position: Position,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    if let Some(current_analysis) = analysis_map.get(current_uri) {
        // Handle Map LHS Lookups (Requires searching all files)
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

        // Handle general scope lookups (Only searches current file analysis)
        for sym in current_analysis.symbols.values() {
            collect_symbols(sym, context, position, &mut items);
        }
    }

    // Clean up duplicates
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);

    items
}

/// Recursive helper to extract ONLY the ports from a symbol (for LHS completion).
///
/// # Arguments
/// * `symbol` - The top level symbol to traverse.
/// * `items` - Mutable reference to the vector of completion items.
fn collect_ports(symbol: &Symbol, items: &mut Vec<CompletionItem>) {
    if (symbol.kind == OxideSymbolKind::Port)
        && let Some(item) = symbol_to_completion(symbol)
    {
        items.push(item);
    }
    for child in &symbol.children {
        collect_ports(child, items);
    }
}

/// Recursive helper to extract ONLY the generics from a symbol (for LHS completion).
///
/// # Arguments
/// * `symbol` - The top level symbol to traverse.
/// * `items` - Mutable reference to the vector of completion items.
fn collect_generics(symbol: &Symbol, items: &mut Vec<CompletionItem>) {
    if (symbol.kind == OxideSymbolKind::Generic)
        && let Some(item) = symbol_to_completion(symbol)
    {
        items.push(item);
    }
    for child in &symbol.children {
        collect_generics(child, items);
    }
}

/// Recursive helper to flatten the symbol tree into suggestions based on the context.
///
/// # Arguments
/// * `symbol` - The top level symbol we want to collect symbols from.
/// * `context` - The current completion context.
/// * `position` - The current cursor position.
/// * `items` - Mutable reference to the a vector of completion items.
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
        && !is_strict_scope // Don't suggest the architecture name itself...
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
///
/// # Arguments
/// * `sym` - The symbol on which we are requesting visibility.
/// * `context` - The current completion context.
///
/// # Return
/// `true` if the symbol should be visible in the completion.
fn is_visible(sym: &Symbol, context: &CompletionContext) -> bool {
    match context {
        CompletionContext::Architecture => matches!(
            sym.kind,
            OxideSymbolKind::Component
                | OxideSymbolKind::Entity
                | OxideSymbolKind::Package
                | OxideSymbolKind::Constant
                | OxideSymbolKind::Port
                | OxideSymbolKind::Signal
        ),
        CompletionContext::Process => matches!(
            sym.kind,
            OxideSymbolKind::Signal
                | OxideSymbolKind::Variable
                | OxideSymbolKind::Constant
                | OxideSymbolKind::Port
        ),
        CompletionContext::PortMapRhs | CompletionContext::GenericMapRhs => {
            matches!(
                sym.kind,
                OxideSymbolKind::Signal | OxideSymbolKind::Constant | OxideSymbolKind::Port
            )
        }
        _ => true,
    }
}

/// Checks if a position is contained within a range.
///
/// # Arguments
/// * `range` - The range the position needs to be checked against.
/// * `position` - The position we want to check.
///
/// # Return
/// `true` if the position is within the range (inclusive of start, exclusive of end character).
fn range_contains(range: Range, position: Position) -> bool {
    if position.line < range.start.line || position.line > range.end.line {
        return false;
    }

    // edge case on start line
    if position.line == range.start.line && position.character < range.start.character {
        return false;
    }

    // edge case on end line (excluding the last character column itself for multi-line)
    if position.line == range.end.line && position.character > range.end.character {
        return false;
    }
    true
}

/// Converts an internal `Symbol` struct into an LSP `CompletionItem`.
///
/// # Arguments
/// * `symbol` - The symbol to convert.
///
/// # Return
/// `Some(CompletionItem)` if the symbol type is appropriate for completion, otherwise `None`.
fn symbol_to_completion(symbol: &Symbol) -> Option<CompletionItem> {
    let kind = match symbol.kind {
        OxideSymbolKind::Entity => CompletionItemKind::INTERFACE,
        OxideSymbolKind::Architecture => return None, // Don't suggest Arch names in code
        OxideSymbolKind::Port => CompletionItemKind::FIELD,
        OxideSymbolKind::Signal => CompletionItemKind::VARIABLE,
        OxideSymbolKind::Constant => CompletionItemKind::CONSTANT,
        OxideSymbolKind::Struct => CompletionItemKind::STRUCT,
        OxideSymbolKind::Function => CompletionItemKind::FUNCTION,
        OxideSymbolKind::Process => return None, // Don't suggest Process labels
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
    use crate::backend::test_utils::SHARED_PARSER_LOCK;
    use tree_sitter::Parser;

    // --- HELPER FUNCTIONS ---

    /// Parses the code, extracts the cursor, and checks the completion context against an expected value.
    fn check_context(code_with_cursor: &str, expected: CompletionContext) {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();

        let (code, pos) = extract_cursor(code_with_cursor);

        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(&code, None).unwrap();
        drop(_guard); // Release lock early

        eprintln!("\n--- Running Context Check ---");
        eprintln!("Code:\n{}", code);
        eprintln!("Cursor: Line {}, Col {}", pos.line, pos.character);

        let ctx = get_completion_context(&code, tree.root_node(), pos);
        assert_eq!(ctx, expected, "\nCode:\n{}\nContext mismatch!", code);
        eprintln!("--- Context Check Passed ---");
    }

    /// Extracts the cursor position '|' and returns the clean code and the LSP Position.
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

    // --- NEW UNIT TESTS FOR HELPER FUNCTIONS ---

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

        // 1. Position at start of file - should get some node
        let pos1 = Position {
            line: 0,
            character: 0,
        };
        let node1 = get_tree_node(root, pos1);
        assert!(node1.is_some(), "Should find a node at position 0");

        // 2. Position after end of content - fallback should still work
        let pos2 = Position {
            line: 0,
            character: 18,
        };
        let _node2 = get_tree_node(root, pos2);
        // May or may not find a node at exact end, but shouldn't panic

        // 3. Position in whitespace between tokens
        let pos3 = Position {
            line: 0,
            character: 9,
        }; // between "is" and "end"
        let node3 = get_tree_node(root, pos3);
        assert!(node3.is_some(), "Should find a node via fallback");
    }

    #[test]
    fn test_find_component_declaration_tree() {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        // Use code that tree-sitter can reasonably parse (even if as ERROR)
        let code = r#"
u0: entity work.my_comp(rtl)
    port map (clk => clk);
"#;
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(code, None).unwrap();
        drop(_guard);

        // The cursor position after "clk =>"
        let cursor_offset = code.find("clk => clk").unwrap() + 7;

        // Start from root - the text fallback should find it
        let result = find_component_declaration(tree.root_node(), code, cursor_offset);

        assert!(result.is_some(), "Should find component declaration");
        let (name, kind) = result.unwrap();
        assert_eq!(name, "my_comp", "Should extract component name");
        assert_eq!(kind, DetectedMapKind::Port, "Should identify as port map");
    }

    #[test]
    fn test_find_component_declaration_text_fallback() {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        // Intentionally broken VHDL to test text fallback
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

        // Position just after 'sig'
        let cursor_offset = code.find("sig").unwrap() + 3;

        // The text fallback in find_component_declaration should work
        let result = find_component_declaration(tree.root_node(), code, cursor_offset);

        assert!(result.is_some(), "Should find component via text fallback");
        let (name, kind) = result.unwrap();
        assert_eq!(name, "my_broken_comp", "Should extract component name");
        assert_eq!(kind, DetectedMapKind::Port, "Should identify as port map");
    }

    // --- BASIC CONTEXT TESTS (RETAINED) ---

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

    // --- PORT MAP TESTS (CRITICAL FIXES) ---

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
        // This is the tricky case where the parser starts at 'design_file'
        // and the component statement is broken.
        check_context(
            r#"
            u1 : entity work.my_comp 
                port map (
                    clk => |
            "#,
            CompletionContext::PortMapRhs,
        );
    }

    // --- GENERIC MAP TESTS ---

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
        // This confirms the text fallback logic works for misparsed signal assignment
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
        // Complex instantiation name handling
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
