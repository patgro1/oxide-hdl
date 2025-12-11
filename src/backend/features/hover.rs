use tower_lsp::lsp_types::Url;

use crate::{
    analysis::{OxideSymbolKind, Symbol},
    backend::AnalysisMap,
};

pub struct HoverResolution {
    pub symbol: Symbol,
    pub definition_uri: Option<Url>,
    pub target_definition_key: Option<String>,
}

pub fn format_basic(sym: &Symbol) -> String {
    let type_info = sym.detail.as_deref().unwrap_or("void");
    format!(
        "**{}**\n\n```vhdl\n{}  :  {}\n```",
        sym.name, sym.kind, type_info
    )
}

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
                && let Some(child) = root_sym.find_recursive(&lower_target)
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
