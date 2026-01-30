//! Core types for VHDL analysis and scope management.
//!
//! This module defines the fundamental data structures used throughout
//! the analysis pipeline, including declarations, scope trees, and usage tracking.use std::fmt;

use std::fmt;
use tower_lsp::lsp_types::{Range, SymbolKind};

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

/// Declaration Reference
///
/// Represent the scope tree resolution of a particular declaration
#[derive(Debug, Clone)]
pub struct DeclarationRef {
    /// Indexed reference of the scope tree of the declaration
    scope_tree_idx: usize,
    /// Declaration index within the scope tree
    declaration_idx: usize,
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
    pub fn find_recursive<'a>(&'a self, target: &str) -> Option<&'a Symbol> {
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

/// Represents the way the analysis was made
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseLevel {
    /// Analysis obtained quickly via regex on high-level stuff  (entities, components, packages,
    /// functions)
    Shallow, // Regex based parseing
    /// Deep tree-sitter analysis was made on the file
    Deep, // Tree-sitter parsing
}

/// Type of declaration in VHDL code.
///
/// Distinguishes between different kinds of declarations to provide
/// more specific diagnostic messages.
#[derive(Debug, Clone, Copy)]
pub enum DeclType {
    /// Entity Generic
    Generic,
    /// Entity port with direction
    Port(PortDirection),
    /// Constant declaration (value cannot change)
    Constant,
    /// Signal declaration (architecture/generate/block level)
    Signal,
    /// Variable declaration (process/function/procedure level)
    Variable,
}

impl fmt::Display for DeclType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            DeclType::Generic => "generic",
            DeclType::Port(direction) => &format!("port({})", direction),
            DeclType::Constant => "constant",
            DeclType::Signal => "signal",
            DeclType::Variable => "variable",
        };
        write!(f, "{}", s)
    }
}

impl From<DeclType> for SymbolKind {
    fn from(kind: DeclType) -> Self {
        match kind {
            DeclType::Generic => SymbolKind::CONSTANT,
            DeclType::Port(_) => SymbolKind::FIELD,
            DeclType::Constant => SymbolKind::CONSTANT,
            DeclType::Variable => SymbolKind::VARIABLE,
            DeclType::Signal => SymbolKind::VARIABLE,
        }
    }
}

/// Port Direction
///
/// Distinguishes between mode indications
#[derive(Debug, Clone, Copy)]
pub enum PortDirection {
    /// Input Port
    In,
    /// Output Port
    Out,
    /// Bidir port
    InOut,
    /// Buffer port (Out that can be read)
    Buffer,
    /// Linkage (connection with mixed language or mixed-signals)
    Linkage,
}

impl fmt::Display for PortDirection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            PortDirection::In => "in",
            PortDirection::Out => "out",
            PortDirection::InOut => "inout",
            PortDirection::Buffer => "buffer",
            PortDirection::Linkage => "linkage",
        };
        write!(f, "{}", s)
    }
}

/// Kind of scope in the VHDL hierarchy.
///
/// Each scope level has different rules about what can be declared
/// and how visibility works.
#[derive(Debug, Clone, Copy)]
pub enum ScopeKind {
    /// Entity scope - can declare ports and generics
    Entity,
    /// Architecture scope - can declare signals and constants
    Architecture,
    /// Process scope - can declare variables and constants
    Process,
    /// Generate scope - can declare signals and constants
    Generate,
    /// Block scope - can declare signals and constants
    Block,
}

impl fmt::Display for ScopeKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            ScopeKind::Entity => "entity",
            ScopeKind::Architecture => "architecture",
            ScopeKind::Process => "process",
            ScopeKind::Generate => "generate",
            ScopeKind::Block => "block",
        };
        write!(f, "{}", s)
    }
}

impl From<ScopeKind> for SymbolKind {
    fn from(kind: ScopeKind) -> Self {
        match kind {
            ScopeKind::Entity => SymbolKind::INTERFACE,
            ScopeKind::Architecture => SymbolKind::CLASS,
            ScopeKind::Process => SymbolKind::METHOD,
            ScopeKind::Generate => SymbolKind::NAMESPACE,
            ScopeKind::Block => SymbolKind::NAMESPACE,
        }
    }
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
#[derive(Debug, Clone, Eq)]
pub struct Usage {
    // Name of the signal, variable, constant
    pub name: String,
    // Context in which it was used
    pub context: UsageContext,
    // Location of this particular usage in the file
    pub range: Range,
}

impl std::hash::Hash for Usage {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Only hash name and context, not range
        self.name.hash(state);
        self.context.hash(state);
    }
}

impl PartialEq for Usage {
    fn eq(&self, other: &Self) -> bool {
        // Only compare name and context
        self.name == other.name && self.context == other.context
    }
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
    pub range: Range,
    /// Range of the name itself for jumps
    pub selection_range: Range,
    /// TypeInfo for this declaration
    pub type_info: TypeInfo,
    // Default value if any
    pub default_value: Option<String>,
    // Doc string
    pub doc_comment: Option<String>,
}

/// Type information for the VHDL declaration
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    /// name of the type (std_logic vector, integer, etc)
    pub base_type: String,
    /// Constaints of the type if any (8 downto 0), range 0 to 100
    pub constraints: Option<String>,
    /// Is it a scalar or an array
    pub is_array: bool,
}

impl TypeInfo {
    pub fn new() -> Self {
        TypeInfo {
            base_type: String::new(),
            constraints: None,
            is_array: false,
        }
    }
}

/// An instance of an entity.
///
/// Contains all the information about instance an instance
#[derive(Debug, Clone)]
pub struct Instance {
    /// Name of the instance
    pub label: String,
    /// Design unit being instantiated
    pub component: String,
    /// Source location information
    pub range: Range,
    /// Range of the label for jumps
    pub selection_range: Range,
}
