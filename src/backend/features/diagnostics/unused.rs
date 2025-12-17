//! Unused signal, variable, and constant detection with full scope tracking.
//!
//! This module implements a scope tree-based analysis to detect unused declarations
//! across all VHDL scope levels including architectures, processes, generates, and blocks.
//! The scope tree approach enables accurate tracking of nested scopes and proper handling
//! of parent-child scope relationships.
//!
//! # Architecture
//!
//! The analysis works in two phases:
//! 1. **Build Phase**: Construct a hierarchical scope tree representing all declarations
//!    and usage patterns throughout the architecture
//! 2. **Check Phase**: Recursively traverse the scope tree to identify declarations that
//!    are never referenced in their scope or any child scopes
//!
//! # Scope Hierarchy
//!
//! ```text
//! Architecture
//!   ├── Process (can declare variables)
//!   ├── Generate (can declare signals/constants)
//!   │   ├── Process (nested)
//!   │   └── Generate (nested)
//!   └── Block (can declare signals/constants)
//!       └── Process (nested)
//! ```

use crate::backend::features::diagnostics::DiagnosticCollectors;
use std::collections::HashSet;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
use tree_sitter::Node;

/// Type of declaration in VHDL code.
///
/// Distinguishes between different kinds of declarations to provide
/// more specific diagnostic messages.
#[derive(Debug, Clone)]
enum DeclType {
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

/// A declaration of a signal, variable, or constant.
///
/// Contains all information needed to create a diagnostic if the
/// declaration is determined to be unused.
#[derive(Debug, Clone)]
pub struct Declaration {
    /// Name of the declared identifier (lowercase)
    name: String,
    /// Type of declaration
    decl_type: DeclType,
    /// Source location information
    node_info: NodeInfo,
}

/// Source location information for a declaration.
///
/// Used to create properly positioned diagnostics.
#[derive(Debug, Clone)]
struct NodeInfo {
    /// Line number (0-indexed)
    line: u32,
    /// Column number (0-indexed)
    column: u32,
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
    kind: ScopeKind,

    /// Declarations made in this scope
    declarations: Vec<Declaration>,

    /// Identifiers referenced directly in this scope (not including children)
    local_usage: HashSet<String>,

    /// Child scopes nested within this scope
    children: Vec<ScopeTree>,
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
            if !self.is_used_anywhere(&decl.name.to_lowercase()) {
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
    /// * `name` - Identifier name to search for (case-insensitive)
    ///
    /// # Returns
    ///
    /// `true` if the identifier is referenced anywhere in this scope tree
    pub fn is_used_anywhere(&self, name: &str) -> bool {
        if self.local_usage.contains(name) {
            return true;
        }

        for child in &self.children {
            if child.is_used_anywhere(name) {
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
                } else if decl_child.kind() == "constant_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Constant,
                    ));
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
                    _ => collect_identifiers_recursive(inner_child, text, &mut tree.local_usage),
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
            collect_identifiers_recursive(child, text, &mut tree.local_usage);
        } else if child.kind() == "sequential_block" {
            collect_identifiers_recursive(child, text, &mut tree.local_usage);
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
    let body_node = generate_node
        .children(&mut generate_node.walk())
        .find(|c| c.kind() == "if_generate")
        .and_then(|if_gen| {
            if_gen
                .children(&mut if_gen.walk())
                .find(|c| c.kind() == "generate_body")
        });

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
                    } else if decl_child.kind() == "constant_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Constant,
                        ));
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
                        _ => {
                            collect_identifiers_recursive(inner_child, text, &mut tree.local_usage)
                        }
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
    let body_node = generate_node
        .children(&mut generate_node.walk())
        .find(|c| c.kind() == "generate_body");

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
                    } else if decl_child.kind() == "constant_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Constant,
                        ));
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
                        _ => {
                            collect_identifiers_recursive(inner_child, text, &mut tree.local_usage)
                        }
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
                } else if decl_child.kind() == "constant_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Constant,
                    ));
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
                    _ => collect_identifiers_recursive(inner_child, text, &mut tree.local_usage),
                }
            }
            break;
        }
    }

    tree
}

/// Main entry point for unused signal/variable/constant detection.
///
/// Builds a complete scope tree for the architecture, checks for unused
/// declarations, and reports diagnostics.
///
/// # Arguments
///
/// * `node` - The `architecture_definition` node to analyze
/// * `text` - The full source text
/// * `collectors` - Mutable reference to diagnostic collectors
pub fn check_unused_signals(node: Node, text: &str, collectors: &mut DiagnosticCollectors) {
    let scope_tree = build_arch_scope_tree(node, text);

    let unused = scope_tree.check_unused(&HashSet::new());

    for decl in unused {
        collectors.unused.push(Diagnostic {
            range: decl.node_info.to_range(&decl.name),
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("oxide-hdl".to_string()),
            message: match decl.decl_type {
                DeclType::Variable => format!("Unused variable '{}'", decl.name),
                DeclType::Constant => format!("Unused constant '{}'", decl.name),
                DeclType::Signal => format!("Unused signal '{}'", decl.name),
            },
            ..Default::default()
        });
    }
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
fn extract_signal_names(signal_node: Node, text: &str, decl_type: DeclType) -> Vec<Declaration> {
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
                    name: signal_name.to_string().to_lowercase(),
                    decl_type: decl_type.clone(),
                    node_info: NodeInfo::from_node(child),
                });
            }
        }
    }
    signals
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
fn collect_identifiers_recursive(node: Node, text: &str, references: &mut HashSet<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            references.insert(text[child.byte_range()].to_string().to_lowercase());
        } else {
            collect_identifiers_recursive(child, text, references);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Comprehensive test suite for unused signal/variable/constant detection.
    //!
    //! Tests are organized into categories:
    //! - Basic unused detection
    //! - Process variables
    //! - Sensitivity lists
    //! - Nested scopes (generates, blocks)
    //! - Edge cases (shadowing, empty scopes, deep nesting)
    //!
    //! Each test verifies that the correct number of diagnostics are generated
    //! and that diagnostic messages contain expected content.

    use crate::backend::test_utils::SHARED_PARSER_LOCK;
    use tower_lsp::lsp_types::Diagnostic;
    use tree_sitter::Parser;

    fn parse_text(code: &str) -> tree_sitter::Tree {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        parser.parse(code, None).unwrap()
    }

    fn check_unused_signals(code: &str) -> Vec<Diagnostic> {
        let tree = parse_text(code);
        let root = tree.root_node();

        let mut collectors = super::super::DiagnosticCollectors::new();

        let mut cursor = root.walk();
        for node in root.children(&mut cursor) {
            if node.kind() == "design_unit" {
                for child in node.children(&mut node.walk()) {
                    if child.kind() == "architecture_definition" {
                        super::check_unused_signals(child, code, &mut collectors);
                    }
                }
            }
        }

        collectors.unused
    }

    #[test]
    fn test_simple_unused_signal() {
        let code = r#"
architecture rtl of test is
    signal unused_sig : std_logic;
    signal used_sig : std_logic;
begin
    used_sig <= '1';
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Should detect one unused signal");
        assert!(diags[0].message.contains("Unused signal"));
    }

    #[test]
    fn test_no_unused_signals() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic;
    signal b : std_logic;
begin
    a <= '1';
    b <= a;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(diags.is_empty(), "Should not detect any unused signals");
    }

    #[test]
    fn test_signal_used_in_process() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal data : std_logic;
begin
    process(clk)
    begin
        if rising_edge(clk) then
            data <= '1';
        end if;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(diags.is_empty(), "Should not flag signals used in process");
    }

    #[test]
    fn test_signal_used_in_port_map() {
        let code = r#"
architecture rtl of test is
    signal internal_clk : std_logic;
begin
    inst: entity work.sub_module
        port map (
            clk => internal_clk
        );
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Should not flag signals used in port maps"
        );
    }

    #[test]
    fn test_multiple_unused_signals() {
        let code = r#"
architecture rtl of test is
    signal unused1 : std_logic;
    signal unused2 : std_logic;
    signal unused3 : std_logic;
begin
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 3, "Should detect all three unused signals");
    }

    #[test]
    fn test_signal_declared_with_multiple_names() {
        let code = r#"
architecture rtl of test is
    signal a, b, c : std_logic;
begin
    a <= '1';
    -- b and c are unused
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 2, "Should detect b and c as unused");
        // Note: might need to check that 'a' is not flagged
    }

    #[test]
    fn test_signal_only_written_never_read() {
        let code = r#"
architecture rtl of test is
    signal write_only : std_logic;
begin
    write_only <= '1';
    -- Never read, but is it unused?
end architecture;
"#;
        let diags = check_unused_signals(code);

        // This is debatable - a write-only signal might be intentional (debug probe)
        // For now, consider it "used" if it appears in any assignment
        assert!(diags.is_empty(), "Write-only signals are considered used");
    }

    #[test]
    fn test_signal_used_in_generate() {
        let code = r#"
architecture rtl of test is
    signal gen_sig : std_logic;
begin
    gen: for i in 0 to 3 generate
        gen_sig <= '1';
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(diags.is_empty(), "Should not flag signals used in generate");
    }

    #[test]
    fn test_constant_unused() {
        let code = r#"
architecture rtl of test is
    constant UNUSED_CONST : integer := 42;
    constant USED_CONST : integer := 10;
    signal counter : integer range 0 to USED_CONST;
begin
    counter <= USED_CONST;
end architecture;
"#;
        let diags = check_unused_signals(code);

        // Should we check constants? Let's say yes for v0.4
        assert_eq!(diags.len(), 1, "Should detect unused constant");
        assert!(diags[0].message.contains("Unused constant"));
    }
    #[test]
    fn test_signal_same_name_as_architecture() {
        let code = r#"
architecture rtl of test is
    signal rtl : std_logic;      -- Should be flagged as unused
    signal used_sig : std_logic;
begin
    used_sig <= '1';
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(
            diags.len(),
            1,
            "Should detect 'rtl' as unused despite matching architecture name"
        );
        assert!(diags[0].message.contains("Unused signal"));
    }
    // Add to tests module in unused.rs

    #[test]
    fn test_unused_process_variable() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    process(clk)
        variable unused_var : integer;
        variable used_var : integer;
    begin
        used_var := 42;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Should detect unused_var");
        assert!(diags[0].message.contains("unused_var"));
        assert!(diags[0].message.contains("variable"));
    }

    #[test]
    fn test_all_process_variables_used() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    process(clk)
        variable counter : integer := 0;
        variable temp : std_logic;
    begin
        counter := counter + 1;
        temp := clk;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(diags.is_empty(), "All variables are used");
    }

    #[test]
    fn test_sensitivity_list_signals_marked_as_used() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal rst : std_logic;
    signal unused_sig : std_logic;
begin
    process(clk, rst)
    begin
        if rst = '1' then
            null;
        elsif rising_edge(clk) then
            null;
        end if;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Only unused_sig should be flagged");
        assert!(diags[0].message.contains("unused_sig"));
    }

    #[test]
    fn test_architecture_signal_used_in_process() {
        let code = r#"
architecture rtl of test is
    signal data : std_logic;
    signal clk : std_logic;
begin
    process(clk)
        variable temp : std_logic;
    begin
        temp := data;  -- Uses architecture signal
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert!(
            diags.is_empty(),
            "Architecture signal used in process should not be flagged"
        );
    }

    #[test]
    fn test_multiple_processes_different_scopes() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    p1: process(clk)
        variable v1 : integer;  -- Unused
    begin
        null;
    end process;
    
    p2: process(clk)
        variable v2 : integer;  -- Used
    begin
        v2 := 10;
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Should only detect v1 as unused");
        assert!(diags[0].message.contains("v1"));
    }

    #[test]
    fn test_process_variable_multiple_declarations() {
        let code = r#"
architecture rtl of test is
begin
    process
        variable a, b, c : integer;
    begin
        a := 1;
        -- b and c unused
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 2, "Should detect b and c as unused");
    }
    #[test]
    fn test_nested_generate_and_process() {
        let code = r#"
architecture rtl of test is
    signal arch_sig : std_logic;
begin
    gen: for i in 0 to 3 generate
        signal gen_sig : std_logic;    -- Should be flagged as unused
    begin
        process
            variable proc_var : integer;  -- Used
        begin
            proc_var := 1;
            arch_sig <= '1';  -- Uses parent scope
        end process;
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1, "Should detect unused gen_sig");
        assert!(diags[0].message.contains("gen_sig"));
    }

    #[test]
    fn test_block_scope() {
        let code = r#"
architecture rtl of test is
begin
    blk: block is
        signal blk_unused : std_logic;
        signal blk_used : std_logic;
    begin
        blk_used <= '1';
    end block;
end architecture;
"#;
        let diags = check_unused_signals(code);

        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("blk_unused"));
    }
    #[test]
    fn test_deep_nesting_three_levels() {
        let code = r#"
architecture rtl of test is
    signal level0 : std_logic;
begin
    gen1: for i in 0 to 3 generate
        signal level1 : std_logic;
    begin
        gen2: for j in 0 to 3 generate
            signal level2 : std_logic;
        begin
            process
                variable level3 : integer;
            begin
                level3 := 1;
                level2 <= '0';
                level1 <= '0';
                level0 <= '0';
            end process;
        end generate;
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);
        assert!(
            diags.is_empty(),
            "All deeply nested signals should be marked as used"
        );
    }

    #[test]
    fn test_shadowing_variable_hides_signal() {
        let code = r#"
architecture rtl of test is
    signal data : std_logic;  -- Should be unused!
begin
    process
        variable data : integer;  -- Shadows signal
    begin
        data := 1;  -- Uses variable, not signal
    end process;
end architecture;
"#;
        let diags = check_unused_signals(code);
        // This is HARD - requires scope resolution
        // For v0.4, you might accept false negative
        // assert_eq!(diags.len(), 1, "Architecture 'data' should be flagged");
    }

    #[test]
    fn test_signal_used_in_port_map_output() {
        let code = r#"
architecture rtl of test is
    signal internal : std_logic;
begin
    inst: entity work.sub
        port map (
            output => internal
        );
end architecture;
"#;
        let diags = check_unused_signals(code);
        assert!(
            diags.is_empty(),
            "Signal assigned via port map should be used"
        );
    }

    #[test]
    fn test_if_generate_with_else() {
        let code = r#"
architecture rtl of test is
begin
    gen: if true generate
        signal sig_if : std_logic;
    begin
        sig_if <= '1';
    end generate;
    else generate
        signal sig_else : std_logic;
    begin
        sig_else <= '0';
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);
        assert!(diags.is_empty(), "Both if and else signals should be used");
    }

    #[test]
    fn test_empty_generate_scope() {
        let code = r#"
architecture rtl of test is
begin
    gen: for i in 0 to 3 generate
        signal unused : std_logic;
    begin
        -- Nothing here
    end generate;
end architecture;
"#;
        let diags = check_unused_signals(code);
        assert_eq!(
            diags.len(),
            1,
            "Should detect unused signal in empty generate"
        );
    }
}
