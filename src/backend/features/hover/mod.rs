use tower_lsp::lsp_types::{Position, Url};
use tree_sitter::{Node, Point};

use crate::{
    analysis::{DeclType, Declaration, OxideSymbolKind, Symbol, TypeInfo},
    backend::{
        AnalysisMap,
        features::lookup::{ResolvedItem, lookup_symbol, resolve_path_chain},
    },
};

/// Represents the result of a hover lookup operation.
///
/// This struct acts as a bridge between the initial lookup (which might be based on a shallow
/// Regex scan) and the final display (which requires a deep Tree-sitter parse).
///
/// It carries the symbol found, and crucially, the location (`definition_uri`) where the
/// rich definition resides, allowing the backend to trigger a JIT parse if needed.
pub struct HoverResolution {
    /// Declaration  or symbol issued from the lookup
    pub item: ResolvedItem,
    /// Uri of the item location
    pub definition_uri: Option<Url>,
}

/// Dispatcher for the different hover resolution formatter
///
/// # Arguments
/// `res` - HoverResolution to format for Hover
///
/// # Returns
/// Formatted hover markdown string
pub fn format_hover_result(res: &HoverResolution) -> String {
    match &res.item {
        ResolvedItem::Declaration(d) => format_rich_declaration(d),
        ResolvedItem::Symbol(s) => match s.kind {
            OxideSymbolKind::ComponentInstantiation => format_instantiation_hover(&s.name, s),
            OxideSymbolKind::Function | OxideSymbolKind::Process => format_function_hover(s),
            OxideSymbolKind::Entity => format_entity_hover(s),
            _ => format_basic(s),
        },
    }
}

/// Generate markdown for a [`Declaration`]
///
/// # Arguments
/// `decl`: [`Declaration`] to format
///
/// # Returns
/// String containing the markdown
pub fn format_rich_declaration(decl: &Declaration) -> String {
    let mut md = String::new();
    // Doc comments
    if let Some(doc) = &decl.doc_comment {
        md.push_str(doc);
        md.push_str("\n\n---\n\n");
    }

    md.push_str("```vhdl\n");

    match &decl.decl_type {
        // Handle Functions/Procedures with parameters
        DeclType::Function
        | DeclType::FunctionDeclaration
        | DeclType::Procedure
        | DeclType::ProcedureDeclaration => {
            md.push_str(&format_subprogram_signature(decl));
        }
        DeclType::Component => {
            md.push_str(&format_component_hover(decl));
        }
        // Handle Everything Else (Signals, Ports, Vars)
        _ => {
            md.push_str(&format_data_declaration(decl));
        }
    }

    md.push_str("\n```");
    md
}

/// Formats a basic hover tooltip for generic symbols (Signals, Variables, etc.).
///
/// Displays the name, kind, and type detail in a simple Markdown block.
///
/// # Arguments
///
/// * `sym` - The symbol to format.
///
/// # Returns
///
/// A `String` containing the Markdown formatted hover text.
///
/// # Example Output
/// ```markdown
/// **clk**
///
/// ```vhdl
/// port : in std_logic
/// ```
pub fn format_basic(sym: &Symbol) -> String {
    let type_info = sym.detail.as_deref().unwrap_or("void");
    format!(
        "**{}**\n\n```vhdl\n{}  :  {}\n```",
        sym.name, sym.kind, type_info
    )
}

/// Formats a rich hover tooltip for Component Instantiations.
///
/// Reconstructs the `entity` interface (Generics and Ports) from the definition symbol
/// to show the user exactly what they are instantiating.
///
/// # Arguments
///
/// * `instance_name` - The label of the instantiation (e.g., "u_uart").
/// * `definition` - The `Entity` or `Component` symbol that defines the interface.
///
/// # Returns
///
/// A `String` containing the Markdown formatted VHDL interface.
pub fn format_instantiation_hover(instance_name: &str, definition: &Symbol) -> String {
    let mut md = String::new();
    // Title: "inst_ent (Instance of entity)"
    md.push_str(&format!(
        "**{}** (Instance of `{}`)\n\n",
        instance_name, definition.name
    ));
    md.push_str("```vhdl\n");
    // Pseudo header "entity ent is"
    md.push_str(&format!("entity {} is\n", definition.name));
    md.push_str(&format_entity_interface_body(&definition.children));
    md.push_str("end entity;\n");
    md.push_str("\n```");
    md
}

/// Formats a rich hover tooltip for the entity name inside a direct
/// instantiation (`label: entity lib.name`), e.g. hovering `uart_rx` itself
/// rather than the label `u0`.
///
/// # Arguments
/// * `sym` - An `Entity`-kind symbol with its generics/ports as `children`
///   (built by `build_entity_symbol`).
///
/// # Empty-children fallback
/// `format_hover_result` dispatches *every* `Entity`-kind symbol here, not just
/// the ones `resolve_instantiated_entity_hover` builds. Symbols reaching hover
/// via the bare-word path (`get_identifier_from_ast` → `resolve_rich_hover` →
/// `lookup_symbol`) carry no `children`, and so does the shallow synthetic
/// symbol this feature builds before the JIT re-parse. Rendering those as
/// `entity <name> is / end entity;` would be a confidently-wrong claim that the
/// entity has no generics and no ports, so an empty `children` list falls back
/// to [`format_basic`] instead.
pub fn format_entity_hover(sym: &Symbol) -> String {
    if sym.children.is_empty() {
        return format_basic(sym);
    }
    let mut md = String::new();
    md.push_str(&format!("**{}**\n\n", sym.name));
    md.push_str("```vhdl\n");
    md.push_str(&format!("entity {} is\n", sym.name));
    md.push_str(&format_entity_interface_body(&sym.children));
    md.push_str("end entity;\n");
    md.push_str("\n```");
    md
}

/// Renders the `generics (...)`/`ports (...)` body shared by
/// `format_instantiation_hover` and `format_entity_hover` — both work from a
/// flat list of `Generic`/`Constant`/`Port`-kind child symbols.
fn format_entity_interface_body(children: &[Symbol]) -> String {
    let mut md = String::new();
    let generics: Vec<&Symbol> = children
        .iter()
        .filter(|c| c.kind == OxideSymbolKind::Generic || c.kind == OxideSymbolKind::Constant)
        .collect();
    if !generics.is_empty() {
        md.push_str("generics (\n");
        for (i, g) in generics.iter().enumerate() {
            let type_info = g.detail.as_deref().unwrap_or("?");
            let sep = if i < generics.len() - 1 { ";" } else { "" };
            md.push_str(&format!("    {} : {}{}\n", g.name, type_info, sep));
        }
        md.push_str(");\n");
    }
    let ports: Vec<&Symbol> = children
        .iter()
        .filter(|c| c.kind == OxideSymbolKind::Port)
        .collect();
    if !ports.is_empty() {
        md.push_str("ports (\n");
        for (i, p) in ports.iter().enumerate() {
            let type_info = p.detail.as_deref().unwrap_or("?");
            let sep = if i < ports.len() - 1 { ";" } else { "" };
            md.push_str(&format!("    {} : {}{}\n", p.name, type_info, sep));
        }
        md.push_str(");\n");
    }
    md
}

/// Formats a rich hover tooltip for Function or Procedure calls.
///
/// Reconstructs the function signature (parameters and return type) from the
/// definition symbol's children and details.
///
/// # Arguments
///
/// * `sym` - The Function/Procedure symbol containing parameters as children.
///
/// # Returns
///
/// A `String` containing the Markdown formatted VHDL function signature.
pub fn format_function_hover(sym: &Symbol) -> String {
    let mut md = String::new();
    // Header
    md.push_str(&format!("**{}** (Function)\n\n", sym.name));
    md.push_str("```vhdl\n");
    let params: Vec<&Symbol> = sym
        .children
        .iter()
        .filter(|c| c.kind == OxideSymbolKind::Port)
        .collect();

    // params
    if !params.is_empty() {
        md.push_str(" (\n");
        for (i, p) in params.iter().enumerate() {
            let type_info = p.detail.as_deref().unwrap_or("?");
            let sep = if i < params.len() - 1 { ";" } else { "" };
            md.push_str(&format!("    {} : {}{}\n", p.name, type_info, sep));
        }
        md.push_str(")\n");
    }

    // return type
    if let Some(ret_type) = &sym.detail {
        md.push_str(&format!("\nreturn: {};\n", ret_type));
    } else {
        md.push(';');
    }

    md.push_str("\n```");
    md
}

/// Extracts the dot-separated identifier chain at the cursor position from the AST.
///
/// Walks upward from the cursor leaf to find a `name` or `selected_name` node that
/// contains dot-notation selections. When found, delegates to `extract_chain_at_pos`
/// to build the identifier list up to the cursor (e.g. `["pkg_a", "my_func"]` for
/// `pkg_a.my_func` or `["work", "pkg_a", "my_func"]` for `work.pkg_a.my_func`).
///
/// Returns an empty vec when the cursor is not on a dot-notation expression.
///
/// Used by both hover and goto handlers to detect qualified names before falling
/// back to simple word-at-cursor lookup.
pub fn get_qualified_chain_at_pos(root_node: Node, text: &str, pos: Position) -> Vec<String> {
    let point = Point::new(pos.line as usize, pos.character as usize);
    let mut current_node = root_node.descendant_for_point_range(point, point);

    while let Some(node) = current_node {
        if (node.kind() == "selected_name" || node.kind() == "name") && has_selections(node) {
            let chain = extract_chain_at_pos(node, text, pos);
            if !chain.is_empty() {
                return chain;
            }
        }
        if node.kind() == "process_statement" || node.kind() == "architecture_body" {
            break;
        }
        current_node = node.parent();
    }

    vec![]
}

/// Converts a generic/port `Declaration` from a deep-parsed entity's scope
/// tree into a child `Symbol`, with its direction/type/default pre-formatted
/// into `detail` — the shape `format_entity_interface_body` expects.
fn declaration_to_child_symbol(decl: &Declaration) -> Symbol {
    let type_str = format_type_info(&decl.type_info);
    let default_part = decl
        .default_value
        .as_ref()
        .map(|v| format!(" := {}", v))
        .unwrap_or_default();
    let dir_str = if let DeclType::Port(d) = decl.decl_type {
        format!("{} ", d)
    } else {
        String::new()
    };
    let kind = if matches!(decl.decl_type, DeclType::Generic) {
        OxideSymbolKind::Generic
    } else {
        OxideSymbolKind::Port
    };
    Symbol {
        name: decl.name.clone(),
        kind,
        detail: Some(format!("{}{}{}", dir_str, type_str, default_part)),
        range: decl.range,
        children: Vec::new(),
    }
}

/// Builds an `Entity`-kind `Symbol` for `name` from a deep-parsed entity's
/// `ScopeTree`, with its generics/ports copied in as children so
/// `format_entity_hover` can render the real interface.
fn build_entity_symbol(name: &str, tree: &crate::analysis::ScopeTree) -> Symbol {
    let children = tree
        .declarations
        .iter()
        .filter(|d| matches!(d.decl_type, DeclType::Generic | DeclType::Port(_)))
        .map(declaration_to_child_symbol)
        .collect();
    Symbol {
        name: name.to_string(),
        kind: OxideSymbolKind::Entity,
        detail: None,
        range: tree.range,
        children,
    }
}

/// Resolves hover when the cursor sits on the unit-name of a direct entity
/// instantiation (`label: entity lib.name`).
///
/// This runs before *both* of `resolve_hover`'s generic paths, because neither
/// has any concept of instantiation syntax:
/// * the dotted-name chain resolver (`get_qualified_chain_at_pos` /
///   `resolve_path_chain`) — at this cursor position the chain comes back
///   empty, so it contributes nothing rather than something wrong; and
/// * the bare-word fallback (`get_identifier_from_ast` → `resolve_rich_hover`
///   → `lookup_symbol`) — this is the path that actually produced the old,
///   meaningless `entity : void` output, by finding the entity's childless
///   `Symbol` and rendering it through `format_basic`.
///
/// When the target entity is still shallow-indexed, returns a minimal result
/// that still carries the correct `definition_uri` — `Backend::hover`'s
/// existing needs-JIT check upgrades it and calls `resolve_hover` again,
/// at which point this function finds the now-deep entity and returns the
/// full signature. No new JIT-parse plumbing needed.
///
/// Returns `None` when the cursor isn't on such a position, or the
/// instantiation's library/name doesn't resolve to a known file — the caller
/// falls through to the existing chain/bare-word resolution unchanged.
fn resolve_instantiated_entity_hover(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    pos: Position,
) -> Option<HoverResolution> {
    let analysis = analysis_map.get(current_uri)?;
    let inst = crate::analysis::find_instance_at(&analysis.scope_trees, pos)?;
    if inst.unit_kind != crate::analysis::InstantiatedUnitKind::Entity {
        return None;
    }
    let target_uri =
        crate::backend::units::resolve_entity_uris(analysis_map, inst, &analysis.library)
            .into_iter()
            .next()?;
    let target_analysis = analysis_map.get(&target_uri)?;
    let name_lc = inst.component.to_lowercase();
    let sym = match target_analysis.entity_scope_trees.get(&name_lc) {
        Some(tree) => build_entity_symbol(&inst.component, tree),
        None => Symbol {
            name: inst.component.clone(),
            kind: OxideSymbolKind::Entity,
            detail: Some("Entity".to_string()),
            range: tower_lsp::lsp_types::Range::default(),
            children: Vec::new(),
        },
    };
    Some(HoverResolution {
        item: ResolvedItem::Symbol(sym),
        definition_uri: Some(target_uri),
    })
}

/// Main entry point for hover resolution. Handles both dot notation and standard lookups.
///
/// Walks the AST upward from the cursor position looking for `selected_name` or `name`
/// nodes with dot selections. If found, resolves the full chain (e.g., `rec.field`) via
/// `resolve_path_chain`. Otherwise falls back to a simple identifier lookup.
///
/// # Arguments
/// * `analysis_map` - The global analysis map for cross-file lookups.
/// * `current_uri` - The URI of the file containing the cursor.
/// * `text` - The full source text of the document.
/// * `root_node` - The root node of the parsed syntax tree.
/// * `pos` - The cursor position.
///
/// # Returns
/// A vector of `HoverResolution` candidates for the symbol under the cursor.
pub fn resolve_hover(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    text: &str,
    root_node: Node,
    pos: Position,
) -> Vec<HoverResolution> {
    if let Some(res) = resolve_instantiated_entity_hover(analysis_map, current_uri, pos) {
        return vec![res];
    }

    let chain = get_qualified_chain_at_pos(root_node, text, pos);
    if !chain.is_empty() {
        let results = resolve_path_chain(&chain, current_uri, analysis_map, &pos);
        if !results.is_empty() {
            return results
                .into_iter()
                .map(|res| HoverResolution {
                    item: res.item,
                    definition_uri: Some(res.source_uri),
                })
                .collect();
        }
    }

    if let Some(token) = get_identifier_from_ast(root_node, pos, text) {
        resolve_rich_hover(&token, current_uri, analysis_map, pos)
    } else {
        vec![]
    }
}

/// Checks whether a syntax node contains dot-notation children (`selection` or `selected_name`).
///
/// # Arguments
/// * `node` - The syntax node to inspect.
///
/// # Returns
/// `true` if the node has at least one `selection` or `selected_name` child.
fn has_selections(node: Node) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "selection" || child.kind() == "selected_name" {
            return true;
        }
    }
    false
}

/// Extracts the dot-separated identifier chain from a `selected_name` node up to the cursor.
///
/// Walks the direct children of the node, collecting identifiers and stopping once the
/// cursor position is reached. This ensures that hovering over `rec.a` in `rec.a.b` only
/// returns `["rec", "a"]`.
///
/// # Arguments
/// * `node` - A `selected_name` or `name` syntax node containing selections.
/// * `text` - The full source text.
/// * `pos` - The cursor position (used to truncate the chain).
///
/// # Returns
/// An ordered vector of identifier strings forming the chain up to the cursor.
fn extract_chain_at_pos(node: Node, text: &str, pos: Position) -> Vec<String> {
    let mut chain = Vec::new();
    let point = Point::new(pos.line as usize, pos.character as usize);

    let get_text = |n: Node| text[n.byte_range()].to_string();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // STOP if the child starts AFTER the cursor (we haven't typed that part yet)
        if child.start_position() > point {
            break;
        }

        match child.kind() {
            "identifier" => chain.push(get_text(child)),
            "selection" => {
                // selection usually wraps { dot, identifier }
                if let Some(id) = child.child_by_field_name("identifier").or_else(|| {
                    child
                        .children(&mut child.walk())
                        .find(|c| c.kind() == "identifier")
                }) {
                    chain.push(get_text(id));
                }
            }
            "selected_name" => {
                // Recursive selected_names (if any) could be handled here,
                // but the linear walk usually covers it.
            }
            _ => {}
        }

        // CRITICAL: If the cursor is INSIDE this child (e.g. hovering 'a' in 'rec.a.b'),
        // we stop here. The chain is just ["rec", "a"]. We don't want 'b' yet.
        if child.end_position() >= point {
            break;
        }
    }
    chain
}

/// Gets the simple identifier token under the cursor from the AST.
///
/// Used as a fallback when the cursor is not on a dot-notation expression.
///
/// # Arguments
/// * `root` - The root node of the syntax tree.
/// * `pos` - The cursor position.
/// * `text` - The full source text.
///
/// # Returns
/// `Some(identifier)` if the cursor is on an identifier node, `None` otherwise.
fn get_identifier_from_ast(root: Node, pos: Position, text: &str) -> Option<String> {
    let point = Point::new(pos.line as usize, pos.character as usize);
    let node = root.descendant_for_point_range(point, point)?;

    if node.kind() == "identifier" || node.kind() == "basic_identifier" {
        return Some(text[node.byte_range()].to_string());
    }
    None
}

/// Resolves the symbol under the cursor to one or more candidate definitions.
///
/// This function performs the core "Lookup Logic" for hover requests:
/// 1. **Local Search:** Checks the current file first (shadowing global symbols).
///    If found locally, it returns immediately (Winner Takes All).
/// 2. **Global Search:** If not found locally, it scans the entire workspace index.
///    This handles overloads (multiple functions with same name) and split packages
///    (finding functions inside package bodies or headers).
///
/// # Arguments
///
/// * `target` - The identifier string to look up (e.g. "clk", "uart_tx").
/// * `current_uri` - The URI of the file where the cursor is located.
/// * `analysis_map` - The global `AnalysisMap` containing all indexed files.
/// * `pos` - The cursor position for scope resolution.
///
/// # Returns
///
/// A `Vec<HoverResolution>`. A vector is returned to support cases like function
/// overloading where multiple valid definitions might exist globally.
pub fn resolve_rich_hover(
    target: &str,
    current_uri: &Url,
    analysis_map: &AnalysisMap,
    pos: Position,
) -> Vec<HoverResolution> {
    let results = lookup_symbol(target, current_uri, analysis_map, &pos, false);
    results
        .into_iter()
        .map(|res| HoverResolution {
            item: res.item,
            definition_uri: Some(res.source_uri),
        })
        .collect()
}
// --- Formatting Functions ---

/// Formats a rich hover tooltip for a declaration.
///
/// Distinguishes between data objects (signals, vars) and subprograms (functions)
/// to render standard VHDL syntax.
pub fn format_declaration_hover(decl: &Declaration) -> String {
    let mut md = String::new();

    // 1. Documentation (Markdown Text Top)
    if let Some(doc) = &decl.doc_comment {
        md.push_str(doc);
        md.push_str("\n\n---\n\n"); // Horizontal rule separator
    }

    // 2. Code Block
    md.push_str("```vhdl\n");

    match &decl.decl_type {
        DeclType::Function
        | DeclType::FunctionDeclaration
        | DeclType::Procedure
        | DeclType::ProcedureDeclaration => {
            md.push_str(&format_subprogram_signature(decl));
        }
        _ => {
            md.push_str(&format_data_declaration(decl));
        }
    }

    md.push_str("\n```");
    md
}

/// Helper to format Signals, Variables, Ports, Constants, Types
fn format_data_declaration(decl: &Declaration) -> String {
    let type_str = format_type_info(&decl.type_info);
    let default_part = decl
        .default_value
        .as_ref()
        .map(|v| format!(" := {}", v))
        .unwrap_or_default();

    match decl.decl_type {
        DeclType::Port(dir) => {
            format!("port {} : {} {}{};", decl.name, dir, type_str, default_part)
        }

        DeclType::Generic => format!("generic {} : {}{};", decl.name, type_str, default_part),

        DeclType::Alias => format!("alias {} : {}{};", decl.name, type_str, default_part),

        DeclType::Signal => format!("signal {} : {}{};", decl.name, type_str, default_part),

        DeclType::Variable => format!("variable {} : {}{};", decl.name, type_str, default_part),

        DeclType::Constant => format!("constant {} : {}{};", decl.name, type_str, default_part),

        DeclType::Type => format!("type {} is {};", decl.name, type_str), // "is" for types
        //
        DeclType::Attribute => format!("attribute {} is {};", decl.name, type_str), // "is" for types

        DeclType::Subtype => format!("subtype {} is {};", decl.name, type_str),

        DeclType::RecordField => format!("field {} is {};", decl.name, type_str),

        DeclType::EnumLiteral => format!("enum field {};", decl.name),

        DeclType::Parameter(dir, _) => {
            format!("{} : {} {}{}", decl.name, dir, type_str, default_part)
        } // Inside parameter list

        // Fallback (should be handled by subprogram formatter)
        DeclType::Function
        | DeclType::FunctionDeclaration
        | DeclType::Procedure
        | DeclType::ProcedureDeclaration
        | DeclType::Component => String::new(),
    }
}

/// Helper to format Functions and Procedures with parameters
fn format_subprogram_signature(decl: &Declaration) -> String {
    let mut out = String::new();
    let keyword = if matches!(
        decl.decl_type,
        DeclType::Function | DeclType::FunctionDeclaration
    ) {
        "function"
    } else {
        "procedure"
    };

    out.push_str(&format!("{} {}", keyword, decl.name));

    // Parameters
    if let Some(params) = &decl.parameters
        && !params.is_empty()
    {
        out.push_str(" (\n");
        for (i, param) in params.iter().enumerate() {
            let sep = if i < params.len() - 1 { ";" } else { "" };
            // Recursive call to format the parameter declaration
            // Note: parameters are usually DeclType::Parameter or Constant
            let param_str = format_data_declaration(param);
            // Remove trailing semicolon from the helper output for cleaner list syntax
            let clean_param = param_str.trim_end_matches(';');
            out.push_str(&format!("    {}{}\n", clean_param, sep));
        }
        out.push(')');
    }

    // Return type (only for functions)
    if matches!(
        decl.decl_type,
        DeclType::Function | DeclType::FunctionDeclaration
    ) {
        let ret_type = format_type_info(&decl.type_info);
        out.push_str(&format!("\nreturn {};", ret_type));
    } else {
        out.push(';');
    }

    out
}

/// Combines base type and constraints (e.g., "std_logic_vector" + "(7 downto 0)")
fn format_type_info(info: &TypeInfo) -> String {
    match &info.constraints {
        Some(c) => format!("{} {}", info.base_type, c),
        None => info.base_type.clone(),
    }
}

/// Format component hover
fn format_component_hover(decl: &Declaration) -> String {
    let mut md = String::new();
    md.push_str(&format!("component {} is\n", decl.name));

    if let Some(params) = &decl.parameters {
        // 1. GENERICS
        let generics: Vec<&Declaration> = params
            .iter()
            .filter(|d| matches!(d.decl_type, DeclType::Generic))
            .collect();

        if !generics.is_empty() {
            md.push_str("    generic (\n");
            for (i, g) in generics.iter().enumerate() {
                let sep = if i < generics.len() - 1 { ";" } else { "" };

                // Format: "name : type := default"
                let type_str = format_type_info(&g.type_info);
                let default_part = g
                    .default_value
                    .as_ref()
                    .map(|v| format!(" := {}", v))
                    .unwrap_or_default();

                md.push_str(&format!(
                    "        {} : {}{}{}\n",
                    g.name, type_str, default_part, sep
                ));
            }
            md.push_str("    );\n");
        }

        // 2. PORTS
        let ports: Vec<&Declaration> = params
            .iter()
            .filter(|d| matches!(d.decl_type, DeclType::Port(_)))
            .collect();

        if !ports.is_empty() {
            md.push_str("    port (\n");
            for (i, p) in ports.iter().enumerate() {
                let sep = if i < ports.len() - 1 { ";" } else { "" };

                // Format: "name : direction type := default"
                let type_str = format_type_info(&p.type_info);
                let default_part = p
                    .default_value
                    .as_ref()
                    .map(|v| format!(" := {}", v))
                    .unwrap_or_default();

                // Extract direction
                let dir_str = if let DeclType::Port(d) = p.decl_type {
                    format!("{} ", d)
                } else {
                    String::new()
                };

                md.push_str(&format!(
                    "        {} : {}{}{}{}\n",
                    p.name, dir_str, type_str, default_part, sep
                ));
            }
            md.push_str("    );\n");
        }
    }

    md.push_str("end component;");
    md
}

#[cfg(test)]
mod tests;
