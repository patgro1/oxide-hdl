//! Analysis module for Oxide HDL.
//!
//! This module defines the core data structures used to represent the VHDL syntax tree
//! and symbol table. It acts as the "Model" in the MVC architecture of the LSP.
//!
//! The central struct is [`Analysis`], which holds the symbol table for a single file.
//! The hierarchical structure of VHDL (Entities containing Ports, Architectures containing Signals)
//! is represented by the recursive [`Symbol`] struct.
//!
use core::fmt;
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::{Position, Range, SymbolKind};
use tree_sitter::{Language, Node};

#[allow(dead_code)]
unsafe extern "C" {
    /// External declaration for the tree-sitter-vhdl language function.
    /// This is required to initialize the Tree-sitter parser with the VHDL grammar.
    fn tree_sitter_vhdl() -> Language;
}

/// Represents the way the analysis was made
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseLevel {
    /// Analysis obtained quickly via regex on high-level stuff  (entities, components, packages,
    /// functions)
    Shallow, // Regex based parseing
    /// Deep tree-sitter analysis was made on the file
    Deep, // Tree-sitter parsing
}

/// Represents the semantic kind of a VHDL symbol.
///
/// This enum maps VHDL constructs (like Entity, Signal, Process) to an internal representation
/// that can be easily converted to LSP `SymbolKind` for editor display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OxideSymbolKind {
    /// A VHDL Entity declaration (Interface).
    Entity,
    /// A VHDL Package declaration (Collection of types/constants).
    Package,
    /// A Component declaration.
    Component,
    /// A Component Instantiation statement (Usage of a component).
    ComponentInstantiation, // Note: You might want to rename this to 'Instantiation' to match your other files if needed.
    /// An Interface Port (Input/Output).
    Port,
    /// A Generic parameter.
    Generic,
    /// A VHDL Architecture body (Implementation).
    Architecture,
    /// A Process block.
    Process,
    /// A Block statement.
    Block,
    /// A Generate statement (If/For).
    Generate,
    /// A Record or Type definition.
    Struct,
    /// A Constant value.
    Constant,
    /// A Function or Procedure definition.
    Function,
    /// An internal Signal or Variable.
    Signal,
    /// Variable withing a process
    Variable,
    /// Fallback for generic classes.
    Class,
}

impl fmt::Display for OxideSymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            OxideSymbolKind::Entity => "entity",
            OxideSymbolKind::Package => "package",
            OxideSymbolKind::Component => "component",
            OxideSymbolKind::ComponentInstantiation => "instantiation",
            OxideSymbolKind::Port => "port",
            OxideSymbolKind::Generic => "generic",
            OxideSymbolKind::Constant => "constant",
            OxideSymbolKind::Architecture => "architecture",
            OxideSymbolKind::Block => "block",
            OxideSymbolKind::Generate => "generate",
            OxideSymbolKind::Process => "process",
            OxideSymbolKind::Function => "function",
            OxideSymbolKind::Struct => "record",
            OxideSymbolKind::Signal => "signal",
            OxideSymbolKind::Variable => "variable",
            OxideSymbolKind::Class => "class",
        };
        write!(f, "{}", s)
    }
}

impl From<OxideSymbolKind> for SymbolKind {
    fn from(kind: OxideSymbolKind) -> Self {
        match kind {
            OxideSymbolKind::Entity => SymbolKind::INTERFACE,
            OxideSymbolKind::Package => SymbolKind::MODULE,
            OxideSymbolKind::Component => SymbolKind::INTERFACE,
            OxideSymbolKind::ComponentInstantiation => SymbolKind::FIELD,
            OxideSymbolKind::Port => SymbolKind::FIELD,
            OxideSymbolKind::Generic => SymbolKind::CONSTANT,
            OxideSymbolKind::Constant => SymbolKind::CONSTANT,
            OxideSymbolKind::Architecture => SymbolKind::CLASS,
            OxideSymbolKind::Block => SymbolKind::NAMESPACE,
            OxideSymbolKind::Generate => SymbolKind::NAMESPACE,
            OxideSymbolKind::Process => SymbolKind::METHOD,
            OxideSymbolKind::Function => SymbolKind::FUNCTION,
            OxideSymbolKind::Struct => SymbolKind::STRUCT,
            OxideSymbolKind::Signal => SymbolKind::VARIABLE,
            OxideSymbolKind::Variable => SymbolKind::VARIABLE,
            OxideSymbolKind::Class => SymbolKind::CLASS,
        }
    }
}

// Represents a single symbol in the VHDL source code.
///
/// Symbols are hierarchical. For example, an `Architecture` symbol will contain
/// `Signal` and `Process` symbols in its `children` vector.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The name of the symbol as it appears in the source code (original casing).
    pub name: String,
    /// The semantic kind of the symbol.
    pub kind: OxideSymbolKind,
    /// Additional details, such as the type signature (e.g., `std_logic_vector(7 downto 0)`).
    pub detail: Option<String>,
    /// The range in the source document where this symbol is defined.
    pub range: Range,
    /// Nested symbols defined within this symbol's scope.
    pub children: Vec<Symbol>,
}

/// Represents the analysis result for a single VHDL file.
///
/// Contains a lookup map of all top-level symbols found in the file.
#[derive(Debug, Clone)]
pub struct Analysis {
    /// Map of top-level symbol names (normalized to lowercase) to the `Symbol` struct.
    ///
    /// Using lowercase keys ensures case-insensitive lookup, while the `Symbol` struct
    /// preserves the original display name.
    pub symbols: HashMap<String, Symbol>,

    /// How the file was parsed
    pub parse_level: ParseLevel,

    /// Scope tree with signals, constants, types declaration and usage
    pub scope_trees: Vec<ScopeTree>,
}

impl Symbol {
    /// Recursively searches this symbol's children for a specific name.
    ///
    /// This method performs a case-insensitive search.
    ///
    /// # Arguments
    ///
    /// * `target` - The name of the symbol to find (must be lowercase).
    ///
    /// # Returns
    ///
    /// * `Some(&Symbol)` - A reference to the found symbol.
    /// * `None` - If the symbol was not found in the children.
    fn find_recursive<'a>(&'a self, target: &str) -> Option<&'a Symbol> {
        for child in &self.children {
            if child.name.to_lowercase() == target {
                return Some(child);
            }
            if let Some(found) = child.find_recursive(target) {
                return Some(found);
            }
        }
        None
    }

    /// Alias for `find_recursive` to match the API used in features.
    /// Recursively searches this symbol's children for a specific name.
    pub fn find_child<'a>(&'a self, target: &str) -> Option<&'a Symbol> {
        self.find_recursive(target)
    }

    /// Recursively dumps the symbol hierarchy to a string for debugging purposes.
    #[allow(dead_code)]
    pub fn dump_symbol_recursive(&self, depth: usize, output: &mut String) {
        let indent = "  ".repeat(depth);
        output.push_str(&format!("{}{:?} {}\n", indent, self.kind, self.name));
        for child in self.children.clone() {
            child.dump_symbol_recursive(depth + 1, output);
        }
    }
}

impl Analysis {
    /// Creates a new, empty Analysis struct.
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
            parse_level: ParseLevel::Shallow,
            scope_trees: Vec::new(),
        }
    }

    /// Recursively searches for a symbol by name anywhere in the file's hierarchy.
    ///
    /// This method checks the top-level symbols first, and then recursively searches
    /// the children of all top-level symbols. This allows finding nested signals
    /// inside Architectures or variables inside Processes.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the symbol to find (case-insensitive).
    ///
    /// # Returns
    ///
    /// * `Some(&Symbol)` - A reference to the found symbol.
    /// * `None` - If the symbol was not found.
    pub fn find_symbol(&self, name: &str) -> Option<&Symbol> {
        let target = name.to_lowercase();
        if let Some(s) = self.symbols.get(&target) {
            return Some(s);
        }
        for s in self.symbols.values() {
            if let Some(found) = s.find_recursive(&target) {
                return Some(found);
            }
        }
        None
    }
}

/// Hierarchical scope tree representing VHDL code structure.
///
/// Each node in the tree represents a scope (architecture, process, generate, or block)
/// and contains:
/// - Declarations made in that scope
/// - Identifiers used in that scope (not in child scopes)
/// - Child scopes (nested processes, generates, blocks)
///
/// The tree structure allows accurate tracking of:
/// - What's declared at each level
/// - What's used at each level
/// - Which parent declarations are accessible to child scopes
#[derive(Debug, Clone)]
pub struct ScopeTree {
    /// Kind of scope this node represents
    #[allow(dead_code)]
    pub kind: ScopeKind,

    /// Declarations made in this scope
    pub declarations: Vec<Declaration>,

    /// Identifiers referenced directly in this scope (not including children)
    pub local_usage: HashSet<Usage>,

    /// Child scopes nested within this scope
    pub children: Vec<ScopeTree>,
}

/// Type of declaration in VHDL code.
///
/// Distinguishes between different kinds of declarations to provide
/// more specific diagnostic messages.
#[derive(Debug, Clone)]
pub enum DeclType {
    /// Constant declaration (value cannot change)
    Constant,
    /// Signal declaration (architecture/generate/block level)
    Signal,
    /// Variable declaration (process/function/procedure level)
    Variable,
}

/// Kind of scope in the VHDL hierarchy.
///
/// Each scope level has different rules about what can be declared
/// and how visibility works.
#[derive(Debug, Clone)]
pub enum ScopeKind {
    /// Architecture scope - can declare signals and constants
    Architecture,
    /// Process scope - can declare variables and constants
    Process,
    /// Generate scope - can declare signals and constants
    Generate,
    /// Block scope - can declare signals and constants
    Block,
}

/// Define where the usage is done...
/// A usage inside a decl is not necessary a valid usage depending on
/// what is used so we keep track of where it is used
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageContext {
    /// Used in specification (i.e. signal type, constant expression)
    TypeSpec,

    /// Used in behavioral code (assignment, expressions)
    Behavioral,
}

/// Data structure to keep track of the identifier usage
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Usage {
    // Name of the signal, variable, constant
    pub name: String,
    // Context in which it was used
    pub context: UsageContext,
}

/// A declaration of a signal, variable, or constant.
///
/// Contains all information needed to create a diagnostic if the
/// declaration is determined to be unused.
#[derive(Debug, Clone)]
pub struct Declaration {
    /// Name of the declared identifier (lowercase)
    pub name: String,
    /// Type of declaration
    pub decl_type: DeclType,
    /// Source location information
    pub node_info: NodeInfo,
}

/// Source location information for a declaration.
///
/// Used to create properly positioned diagnostics.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// Line number (0-indexed)
    pub line: u32,
    /// Column number (0-indexed)
    pub column: u32,
}

impl NodeInfo {
    /// Creates NodeInfo from a Tree-sitter node.
    ///
    /// # Arguments
    ///
    /// * `node` - Tree-sitter node containing position information
    pub fn from_node(node: Node) -> Self {
        Self {
            line: node.start_position().row as u32,
            column: node.start_position().column as u32,
        }
    }

    /// Converts NodeInfo to an LSP Range.
    ///
    /// Creates a range spanning the length of the identifier name.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the identifier (used to calculate end position)
    pub fn to_range(&self, name: &str) -> Range {
        Range {
            start: Position {
                line: self.line,
                character: self.column,
            },
            end: Position {
                line: self.line,
                character: self.column + (name.len() as u32),
            },
        }
    }
}

impl ScopeTree {
    /// Creates a new empty scope tree node.
    ///
    /// # Arguments
    ///
    /// * `kind` - The type of scope this node represents
    pub fn new(kind: ScopeKind) -> Self {
        Self {
            kind,
            declarations: Vec::new(),
            local_usage: HashSet::new(),
            children: Vec::new(),
        }
    }

    /// Recursively checks for unused declarations in this scope and all children.
    ///
    /// Declarations from parent scopes are passed down so children can check
    /// against them. A declaration is considered used if it appears in:
    /// - The local_usage set of this scope
    /// - The local_usage set of any descendant scope
    ///
    /// # Arguments
    ///
    /// * `parent_declarations` - Set of declaration names accessible from parent scopes
    ///
    /// # Returns
    ///
    /// Vector of unused declarations found in this scope tree
    pub fn check_unused(&self, parent_declarations: &HashSet<String>) -> Vec<Declaration> {
        let mut unused = Vec::new();

        // Build combined set of all accessible declarations
        let mut all_available: HashSet<String> = parent_declarations.clone();
        for decl in &self.declarations {
            all_available.insert(decl.name.clone().to_lowercase());
        }

        // Check local declarations for usage
        for decl in &self.declarations {
            if !self.is_used_anywhere(decl) {
                unused.push(decl.clone());
            }
        }

        // Recursively check child scopes
        for child in &self.children {
            unused.extend(child.check_unused(&all_available));
        }

        unused
    }

    /// Checks if an identifier is used anywhere in this scope tree.
    ///
    /// Recursively searches this scope and all descendant scopes for
    /// usage of the given identifier.
    ///
    /// # Arguments
    ///
    /// * `decl` - Declaration to search for (case-insensitive)
    ///
    /// # Returns
    ///
    /// `true` if the identifier is referenced anywhere in this scope tree
    pub fn is_used_anywhere(&self, decl: &Declaration) -> bool {
        let decl_name_lower = decl.name.to_lowercase();
        let used_locally = match decl.decl_type {
            DeclType::Constant => self.local_usage.iter().any(|u| u.name == decl_name_lower),
            DeclType::Signal | DeclType::Variable => self
                .local_usage
                .iter()
                .any(|u| u.name == decl_name_lower && u.context == UsageContext::Behavioral),
        };
        if used_locally {
            return true;
        }
        for child in &self.children {
            if child.is_used_anywhere(decl) {
                return true;
            }
        }
        false
    }
}

/// Builds a complete scope tree for an architecture.
///
/// Constructs the root scope node representing the architecture, then
/// recursively builds child scopes for all processes, generates, and blocks
/// found within.
///
/// # Arguments
///
/// * `arch_node` - Tree-sitter node of type `architecture_definition`
/// * `text` - Full source text of the file
///
/// # Returns
///
/// Root node of the scope tree representing the entire architecture
pub fn build_arch_scope_tree(arch_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Architecture);

    // Collect architecture-level declarations from architecture_head
    let mut cursor = arch_node.walk();
    for child in arch_node.children(&mut cursor) {
        if child.kind() == "architecture_head" {
            let mut head_cursor = child.walk();
            for decl_child in child.children(&mut head_cursor) {
                if decl_child.kind() == "signal_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Signal,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                } else if decl_child.kind() == "constant_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Constant,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                } else if decl_child.kind() == "type_declaration"
                    || decl_child.kind() == "subtype_declaration"
                {
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                }
            }
            break;
        }
    }

    // Process concurrent_block to find usage and child scopes
    let mut cursor = arch_node.walk();
    for child in arch_node.children(&mut cursor) {
        if child.kind() == "concurrent_block" {
            let mut inner_cursor = child.walk();
            for inner_child in child.children(&mut inner_cursor) {
                match inner_child.kind() {
                    "process_statement" => tree
                        .children
                        .push(build_process_scope_tree(inner_child, text)),
                    "if_generate_statement" => tree
                        .children
                        .push(build_if_generate_scope_tree(inner_child, text)),
                    "for_generate_statement" => tree
                        .children
                        .push(build_for_generate_scope_tree(inner_child, text)),
                    "block_statement" => tree
                        .children
                        .push(build_block_scope_tree(inner_child, text)),
                    _ => collect_identifiers_recursive(
                        inner_child,
                        text,
                        UsageContext::Behavioral,
                        &mut tree.local_usage,
                    ),
                }
            }
            break;
        }
    }
    tree
}

/// Builds a scope tree for a process.
///
/// Processes can declare variables (not signals) and have a sensitivity list
/// that marks signals as used.
///
/// # Arguments
///
/// * `process_node` - Tree-sitter node of type `process_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the process
pub fn build_process_scope_tree(process_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Process);

    // Collect variable declarations from process_head
    let mut cursor = process_node.walk();
    for child in process_node.children(&mut cursor) {
        if child.kind() == "process_head" {
            let mut head_cursor = child.walk();
            for decl_child in child.children(&mut head_cursor) {
                if decl_child.kind() == "variable_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Variable,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                }
            }
            break;
        }
    }

    // Collect usage from sensitivity list and sequential block
    let mut cursor = process_node.walk();
    for child in process_node.children(&mut cursor) {
        if child.kind() == "sensitivity_specification" {
            // Signals in sensitivity list are considered used
            collect_identifiers_recursive(
                child,
                text,
                UsageContext::Behavioral,
                &mut tree.local_usage,
            );
        } else if child.kind() == "sequential_block" {
            collect_identifiers_recursive(
                child,
                text,
                UsageContext::Behavioral,
                &mut tree.local_usage,
            );
            break;
        }
    }
    tree
}

/// Builds a scope tree for an if-generate statement.
///
/// If-generates can declare signals and constants, and can contain
/// nested processes, generates, and blocks.
///
/// # Arguments
///
/// * `generate_node` - Tree-sitter node of type `if_generate_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the generate block
pub fn build_if_generate_scope_tree(generate_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Generate);

    // Find the generate_body nested inside if_generate
    let mut body_node: Option<Node> = None;
    let if_generate_node = generate_node
        .children(&mut generate_node.walk())
        .find(|c| c.kind() == "if_generate");
    if let Some(if_generate_node) = if_generate_node {
        // Find everything that was used in the if statement
        for inner in if_generate_node.children(&mut if_generate_node.walk()) {
            if inner.kind() == "generate_body" {
                body_node = Some(inner);
            } else {
                collect_identifiers_recursive(
                    inner,
                    text,
                    UsageContext::Behavioral,
                    &mut tree.local_usage,
                );
            }
        }
    }

    if let Some(body_node) = body_node {
        let mut cursor = body_node.walk();
        for child in body_node.children(&mut cursor) {
            if child.kind() == "generate_head" {
                // Collect signal/constant declarations
                let mut decl_cursor = child.walk();
                for decl_child in child.children(&mut decl_cursor) {
                    if decl_child.kind() == "signal_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Signal,
                        ));
                        collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                    } else if decl_child.kind() == "constant_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Constant,
                        ));
                        collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                    }
                }
            } else if child.kind() == "generate_block" {
                // Process generate body - may contain nested scopes
                let mut inner_cursor = child.walk();
                for inner_child in child.children(&mut inner_cursor) {
                    match inner_child.kind() {
                        "process_statement" => tree
                            .children
                            .push(build_process_scope_tree(inner_child, text)),
                        "if_generate_statement" => tree
                            .children
                            .push(build_if_generate_scope_tree(inner_child, text)),
                        "for_generate_statement" => tree
                            .children
                            .push(build_for_generate_scope_tree(inner_child, text)),
                        "block_statement" => tree
                            .children
                            .push(build_block_scope_tree(inner_child, text)),
                        _ => collect_identifiers_recursive(
                            inner_child,
                            text,
                            UsageContext::Behavioral,
                            &mut tree.local_usage,
                        ),
                    }
                }
            }
        }
    }
    tree
}

/// Builds a scope tree for a for-generate statement.
///
/// For-generates have a simpler structure than if-generates (no nested
/// `if_generate` wrapper), but otherwise function identically.
///
/// # Arguments
///
/// * `generate_node` - Tree-sitter node of type `for_generate_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the generate block
pub fn build_for_generate_scope_tree(generate_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Generate);

    // Find generate_body directly (no if_generate wrapper)
    let mut body_node: Option<Node> = None;
    for child in generate_node.children(&mut generate_node.walk()) {
        if child.kind() == "generate_body" {
            body_node = Some(child);
        } else if child.kind() == "for_loop" {
            collect_identifiers_recursive(
                child,
                text,
                UsageContext::Behavioral,
                &mut tree.local_usage,
            );
        }
    }

    if let Some(body_node) = body_node {
        let mut cursor = body_node.walk();
        for child in body_node.children(&mut cursor) {
            if child.kind() == "generate_head" {
                let mut decl_cursor = child.walk();
                for decl_child in child.children(&mut decl_cursor) {
                    if decl_child.kind() == "signal_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Signal,
                        ));
                        collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                    } else if decl_child.kind() == "constant_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Constant,
                        ));
                        collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                    }
                }
            } else if child.kind() == "generate_block" {
                let mut inner_cursor = child.walk();
                for inner_child in child.children(&mut inner_cursor) {
                    match inner_child.kind() {
                        "process_statement" => tree
                            .children
                            .push(build_process_scope_tree(inner_child, text)),
                        "if_generate_statement" => tree
                            .children
                            .push(build_if_generate_scope_tree(inner_child, text)),
                        "for_generate_statement" => tree
                            .children
                            .push(build_for_generate_scope_tree(inner_child, text)),
                        "block_statement" => tree
                            .children
                            .push(build_block_scope_tree(inner_child, text)),
                        _ => collect_identifiers_recursive(
                            inner_child,
                            text,
                            UsageContext::Behavioral,
                            &mut tree.local_usage,
                        ),
                    }
                }
            }
        }
    }
    tree
}

/// Builds a scope tree for a block statement.
///
/// Blocks have a structure similar to architectures with a declarative
/// region (block_head) and a concurrent region (concurrent_block).
///
/// # Arguments
///
/// * `block_node` - Tree-sitter node of type `block_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the block
pub fn build_block_scope_tree(block_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Block);

    // Collect declarations from block_head
    let mut cursor = block_node.walk();
    for child in block_node.children(&mut cursor) {
        if child.kind() == "block_head" {
            let mut head_cursor = child.walk();
            for decl_child in child.children(&mut head_cursor) {
                if decl_child.kind() == "signal_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Signal,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                } else if decl_child.kind() == "constant_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Constant,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                }
            }
            break;
        }
    }

    // Process concurrent_block
    let mut cursor = block_node.walk();
    for child in block_node.children(&mut cursor) {
        if child.kind() == "concurrent_block" {
            let mut inner_cursor = child.walk();
            for inner_child in child.children(&mut inner_cursor) {
                match inner_child.kind() {
                    "process_statement" => tree
                        .children
                        .push(build_process_scope_tree(inner_child, text)),
                    "if_generate_statement" => tree
                        .children
                        .push(build_if_generate_scope_tree(inner_child, text)),
                    "for_generate_statement" => tree
                        .children
                        .push(build_for_generate_scope_tree(inner_child, text)),
                    "block_statement" => tree
                        .children
                        .push(build_block_scope_tree(inner_child, text)),
                    _ => collect_identifiers_recursive(
                        inner_child,
                        text,
                        UsageContext::Behavioral,
                        &mut tree.local_usage,
                    ),
                }
            }
            break;
        }
    }

    tree
}

/// Extracts all declared names from a signal/variable/constant declaration.
///
/// Handles declarations with multiple names (e.g., `signal a, b, c : std_logic`).
///
/// # Arguments
///
/// * `signal_node` - Declaration node (signal_declaration, variable_declaration, etc.)
/// * `text` - Full source text
/// * `decl_type` - Type of declaration being processed
///
/// # Returns
///
/// Vector of Declaration objects, one for each identifier in the declaration
pub fn extract_signal_names(
    signal_node: Node,
    text: &str,
    decl_type: DeclType,
) -> Vec<Declaration> {
    let mut signals: Vec<Declaration> = Vec::new();

    // Find identifier_list child
    let mut cursor = signal_node.walk();
    let mut identifier_list: Option<Node> = None;
    for child in signal_node.children(&mut cursor) {
        if child.kind() == "identifier_list" {
            identifier_list = Some(child);
            break;
        }
    }

    // Extract each identifier
    if let Some(identifier_list) = identifier_list {
        let mut cursor = identifier_list.walk();
        for child in identifier_list.children(&mut cursor) {
            if child.kind() == "identifier" {
                let signal_name = &text[child.byte_range()];
                signals.push(Declaration {
                    name: signal_name.to_string(),
                    decl_type: decl_type.clone(),
                    node_info: NodeInfo::from_node(child),
                });
            }
        }
    }
    signals
}

/// Extract identifiers from declartion
///
/// Will extract every identifier on the right side of a declaration
///
/// # Arguments
///
/// `node` - Root node to search from
/// `text` - Full source text
/// `references` - Mutable set to collect identifer names into
pub fn collect_identifier_from_decl(node: Node, text: &str, references: &mut HashSet<Usage>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier_list" => {}
            _ => collect_identifiers_recursive(child, text, UsageContext::TypeSpec, references),
        }
    }
}

/// Recursively collects all identifier references in a subtree.
///
/// Walks the tree depth-first, collecting all identifier nodes (which
/// represent references to signals, variables, etc.).
///
/// # Arguments
///
/// * `node` - Root node to search from
/// * `text` - Full source text
/// * `references` - Mutable set to collect identifier names into
pub fn collect_identifiers_recursive(
    node: Node,
    text: &str,
    context: UsageContext,
    references: &mut HashSet<Usage>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            references.insert(Usage {
                name: text[child.byte_range()].to_string().to_lowercase(),
                context,
            });
        } else {
            collect_identifiers_recursive(child, text, context, references);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{Position, Range}; // Used only for Symbol struct creation

    // --- SETUP HELPERS ---

    fn dummy_range() -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        }
    }

    fn make_symbol(name: &str, kind: OxideSymbolKind, children: Vec<Symbol>) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind,
            range: dummy_range(),
            detail: None,
            children,
        }
    }

    /// Creates a structure like:
    /// Analysis ->
    ///   +-- Key: "top_ent" (Entity)
    ///       +-- Child: "port_a" (Port)
    ///       +-- Child: "port_b" (Port)
    ///   +-- Key: "arch_rtl" (Architecture)
    ///       +-- Child: "sig_x" (Signal)
    ///           +-- Grandchild: "var_z" (Signal)
    fn setup_test_analysis() -> Analysis {
        let mut analysis = Analysis::new();

        // 1. Nested Signal/Variable (Arch -> Signal -> Variable)
        let var_z = make_symbol("var_z", OxideSymbolKind::Signal, vec![]);
        let sig_x = make_symbol("sig_x", OxideSymbolKind::Signal, vec![var_z]);

        // 2. Entity Ports
        let port_a = make_symbol("port_a", OxideSymbolKind::Port, vec![]);
        let port_b = make_symbol("port_b", OxideSymbolKind::Port, vec![]);

        // 3. Architecture (Top Level)
        let arch = make_symbol("Arch_RTL", OxideSymbolKind::Architecture, vec![sig_x]);

        // 4. Entity (Top Level)
        let ent = make_symbol("Top_Ent", OxideSymbolKind::Entity, vec![port_a, port_b]);

        // Insert into the map (keys MUST be lowercase)
        analysis.symbols.insert("arch_rtl".to_string(), arch);
        analysis.symbols.insert("top_ent".to_string(), ent);

        analysis
    }

    // --- TEST CASES ---

    #[test]
    fn test_01_find_top_level() {
        let analysis = setup_test_analysis();

        // Lookup using lowercase target
        let sym = analysis
            .find_symbol("top_ent")
            .expect("Should find Entity at root");

        // Check symbol name (it should preserve original casing)
        assert_eq!(sym.name, "Top_Ent");
        assert_eq!(sym.kind, OxideSymbolKind::Entity);
    }

    #[test]
    fn test_02_find_nested_child() {
        let analysis = setup_test_analysis();

        // Search for a signal inside Architecture (Level 1)
        let sym = analysis
            .find_symbol("sig_x")
            .expect("Should find Signal nested in Architecture");

        assert_eq!(sym.name, "sig_x");
        assert_eq!(sym.kind, OxideSymbolKind::Signal);
    }

    #[test]
    fn test_03_find_deep_nested_child() {
        let analysis = setup_test_analysis();

        // Search for a variable deep inside (Level 2)
        let sym = analysis
            .find_symbol("var_z")
            .expect("Should find Variable nested two levels deep");

        assert_eq!(sym.name, "var_z");
    }

    #[test]
    fn test_04_find_case_insensitive() {
        let analysis = setup_test_analysis();

        // Search using uppercase target for a nested symbol
        let sym = analysis
            .find_symbol("PORT_A")
            .expect("Should find Port regardless of case");

        // The found symbol should preserve its original case
        assert_eq!(sym.name, "port_a");
        assert_eq!(sym.kind, OxideSymbolKind::Port);
    }

    #[test]
    fn test_05_find_not_exists() {
        let analysis = setup_test_analysis();

        // Should return None for a missing symbol
        let sym = analysis.find_symbol("missing_signal");

        assert!(sym.is_none());
    }
}
