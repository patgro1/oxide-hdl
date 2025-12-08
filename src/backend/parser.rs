use std::collections::HashMap;

use crate::analysis::{OxideSymbolKind, Symbol};
use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

// NOTE: This should match the order in shallow.scm
#[repr(usize)]
enum PatternIndex {
    Entity = 0,
    // Package = 1,
}

pub fn node_to_range(node: Node) -> Range {
    let start = node.start_position();
    let end = node.end_position();
    Range {
        start: Position {
            line: start.row as u32,
            character: start.column as u32,
        },
        end: Position {
            line: end.row as u32,
            character: end.column as u32,
        },
    }
}

pub fn parse_design_file(source: &str, query: &Query, root_node: Node) -> Vec<Symbol> {
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root_node, source.as_bytes());

    let mut entity_map: HashMap<usize, Symbol> = HashMap::new();
    // temporary buffer for lists like declaration, i.e. IN_WIDTH, OUT_WIDTH: integer;
    let mut current_mode: Option<String> = None;
    let mut current_names: Vec<(String, Range)> = Vec::new();
    let mut current_kind = OxideSymbolKind::Port;

    while let Some(m) = matches.next() {
        let mut entity_id = 0;
        match m.pattern_index {
            val if val == PatternIndex::Entity as usize => {
                if let Some(name_cap) = m
                    .captures
                    .iter()
                    .find(|c| query.capture_names()[c.index as usize] == "entity.name")
                {
                    entity_id = name_cap.node.parent().unwrap().id();
                    entity_map.entry(entity_id).or_insert_with(|| {
                        let entity_name = name_cap
                            .node
                            .utf8_text(source.as_bytes())
                            .unwrap_or("")
                            .to_string();
                        Symbol {
                            name: entity_name,
                            kind: OxideSymbolKind::Entity,
                            detail: None,
                            range: node_to_range(name_cap.node),
                            children: Vec::new(),
                        }
                    });
                    // We did not find an entity so skip
                    if entity_id == 0 {
                        continue;
                    }
                    let entity = entity_map.get_mut(&entity_id).unwrap();
                    for capture in m.captures {
                        let capture_name = query.capture_names()[capture.index as usize];
                        let node = capture.node;
                        let text = node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                        let range = node_to_range(node);

                        match capture_name {
                            "generic.name" | "port.name" => {
                                let new_kind = if capture_name.starts_with("generic") {
                                    OxideSymbolKind::Generic
                                } else {
                                    OxideSymbolKind::Port
                                };
                                if new_kind != current_kind {
                                    current_names.clear();
                                    current_mode = None;
                                }
                                current_kind = new_kind;
                                current_names.push((text, range))
                            }
                            "generic.type" | "port.type" => {
                                let kind = current_kind;
                                let type_detail = if kind == OxideSymbolKind::Port {
                                    // Add the mode, if no mode we it is an input
                                    format!("{} {}", current_mode.as_deref().unwrap_or("in"), text)
                                } else {
                                    text.clone()
                                };
                                for (name, range) in current_names.drain(..) {
                                    entity.children.push(Symbol {
                                        name,
                                        kind,
                                        detail: Some(type_detail.clone()),
                                        range,
                                        children: Vec::new(),
                                    });
                                }
                            }
                            "port.mode" => {
                                current_mode = Some(text.to_lowercase());
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        };
    }
    entity_map.into_values().collect()
}
