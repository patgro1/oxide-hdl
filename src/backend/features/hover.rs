use tower_lsp::lsp_types::Url;

use crate::{
    analysis::{DeclType, Declaration, OxideSymbolKind, Symbol},
    backend::AnalysisMap,
};

/// Represents the result of a hover lookup operation.
///
/// This struct acts as a bridge between the initial lookup (which might be based on a shallow
/// Regex scan) and the final display (which requires a deep Tree-sitter parse).
///
/// It carries the symbol found, and crucially, the location (`definition_uri`) where the
/// rich definition resides, allowing the backend to trigger a JIT parse if needed.
pub struct HoverResolution {
    pub symbol: Symbol,
    pub definition_uri: Option<Url>,
    pub target_definition_key: Option<String>,
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
    // Title: "inst_ent (Instaance of entity)"
    md.push_str(&format!(
        "**{}** (Instance of `{}`)\n\n",
        instance_name, definition.name
    ));
    md.push_str("```vhdl\n");
    // Pseudo header "entity ent is"
    md.push_str(&format!("entity {} is\n", definition.name));

    // Generics
    let generics: Vec<&Symbol> = definition
        .children
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
    // Ports
    let ports: Vec<&Symbol> = definition
        .children
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

    md.push_str("end entity;\n");
    md.push_str("\n```");
    md
}

/// Format a rich hover tooltip for a declaration
///
/// # Arguments
///
/// * `decl` - The declaration to be formatted
///
/// # Returns
///
/// A `String` containing the markdown formatted VHDL declaration
pub fn format_declaration_hover(decl: &Declaration) -> String {
    let mut md = String::new();
    if let Some(doc_comment) = &decl.doc_comment {
        doc_comment.lines().for_each(|line| {
            md.push_str(&format!("-- {}\n", line));
        })
    }
    md.push_str(&format!("**{}**\n", &decl.name).to_string());
    match decl.decl_type {
        DeclType::Port(direction) => {
            md.push_str(&format!("```vhdl\nport {} : {} ", &decl.name, direction));
        }
        DeclType::Generic => {
            md.push_str(&format!("```vhdl\ngeneric {} : ", &decl.name));
        }
        DeclType::Constant => {
            md.push_str(&format!("```vhdl\nconstant {} : ", &decl.name));
        }
        DeclType::Signal => {
            md.push_str(&format!("```vhdl\nsignal {} : ", &decl.name));
        }
        DeclType::Variable => {
            md.push_str(&format!("```vhdl\nvariable {} : ", &decl.name));
        }
        DeclType::Type => {
            md.push_str(&format!("```vhdl\ntype {} : ", &decl.name));
        }
        DeclType::Subtype => {
            md.push_str(&format!("```vhdl\nsubtype {} : ", &decl.name));
        }
        DeclType::Function => {
            md.push_str(&format!("```vhdl\nfunction {} : ", &decl.name));
        }
        DeclType::Procedure => {
            md.push_str(&format!("```vhdl\nprocedure {} : ", &decl.name));
        }
    };
    md.push_str(&decl.type_info.base_type);
    if let Some(constraint) = &decl.type_info.constraints {
        md.push_str(constraint);
    }
    if let Some(default_val) = &decl.default_value {
        md.push_str(&format!(" := {}", default_val).to_string());
    }
    md.push_str(";\n```");
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
/// * `map` - The global `AnalysisMap` containing all indexed files.
///
/// # Returns
///
/// A `Vec<HoverResolution>`. A vector is returned to support cases like function
/// overloading where multiple valid definitions might exist globally.
pub fn resolve_rich_hover(
    target: &str,
    current_uri: &Url,
    map: &AnalysisMap,
) -> Vec<HoverResolution> {
    let lower_target = target.to_lowercase();
    // Check locally:
    if let Some(analysis) = map.get(current_uri)
        && let Some(sym) = analysis.find_symbol(&lower_target)
    {
        let mut resolution = HoverResolution {
            symbol: sym.clone(),
            definition_uri: None,
            target_definition_key: None,
        };

        // If it is an instantiation symbol, we seek the definition location
        // to get generics and ports
        if sym.kind == OxideSymbolKind::ComponentInstantiation {
            if let Some(target_name) = &sym.detail {
                let def_key = target_name.to_lowercase();
                for (f_uri, f_analysis) in map.iter() {
                    if f_analysis.symbols.contains_key(&def_key) {
                        resolution.definition_uri = Some(f_uri.clone());
                        resolution.target_definition_key = Some(def_key);
                        // NOTE: Usually, when we find a component instatiation,
                        // it makes sense to assume that all entites implementing
                        // this compoenent will share the same interfaces so for
                        // performance it makes sense to stop at the first we
                        // find.
                        break;
                    }
                }
            }
        }
        // If the symbol is an entity or a function, the definition uri is the current uri
        else if sym.kind == OxideSymbolKind::Entity || sym.kind == OxideSymbolKind::Function {
            resolution.definition_uri = Some(current_uri.clone());
            resolution.target_definition_key = Some(lower_target);
        }
        return vec![resolution];
    }
    // Global check
    let mut results = Vec::new();
    for (f_uri, f_analysis) in map.iter() {
        if let Some(sym) = f_analysis.symbols.get(&lower_target) {
            results.push(HoverResolution {
                symbol: sym.clone(),
                definition_uri: Some(f_uri.clone()),
                target_definition_key: Some(lower_target.clone()),
            });
        }
        // Nested match
        for root_sym in f_analysis.symbols.values() {
            if root_sym.kind == OxideSymbolKind::Package
                && let Some(child) = root_sym.find_child(&lower_target)
            {
                results.push(HoverResolution {
                    symbol: child.clone(),
                    definition_uri: Some(f_uri.clone()),
                    target_definition_key: Some(root_sym.name.to_lowercase()),
                });
            }
        }
    }
    results
}
