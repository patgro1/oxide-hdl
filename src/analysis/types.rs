//! Core types for VHDL analysis and scope management.
//!
//! This module defines the fundamental data structures used throughout
//! the analysis pipeline, including declarations, scope trees, and usage tracking.

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
    /// A VHDL Package body (Implementation).
    PackageBody,
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
    /// Variable within a process
    Variable,
    /// Fallback for generic classes.
    Class,
}

impl fmt::Display for OxideSymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            OxideSymbolKind::Entity => "entity",
            OxideSymbolKind::Package => "package",
            OxideSymbolKind::PackageBody => "package body",
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
            OxideSymbolKind::PackageBody => SymbolKind::MODULE,
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

/// Represents a single symbol in the VHDL source code.
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
    /// Component Declaration
    Component,
    /// Entity Generic
    Generic,
    /// Entity port with direction
    Port(PortDirection),
    /// Subprogram parameter
    Parameter(PortDirection, Option<ParameterClass>),
    /// Constant declaration (value cannot change)
    Constant,
    /// Attributes
    Attribute,
    /// Signal declaration (architecture/generate/block level)
    Signal,
    /// Variable declaration (process/function/procedure level)
    Variable,
    /// Type declaration
    Type,
    /// Subtype declaration
    Subtype,
    /// Record fields
    RecordField,
    /// Function subprogram implementation (the body)
    Function,
    /// Function subprogram declaration (the signature/specification)
    FunctionDeclaration,
    /// Procedure subprogram implementation (the body)
    Procedure,
    /// Procedure subprogram declaration (the signature/specification)
    ProcedureDeclaration,
    /// Alias
    Alias,
    /// Enumeration literal (e.g., `IDLE` from `type t_state is (IDLE, RUN, STOP)`)
    EnumLiteral,
}

impl fmt::Display for DeclType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            DeclType::Component => "component",
            DeclType::Generic => "generic",
            DeclType::Port(direction) => &format!("port({})", direction),
            DeclType::Parameter(direction, _) => &format!("parameter({})", direction),
            DeclType::Constant => "constant",
            DeclType::Signal => "signal",
            DeclType::Variable => "variable",
            DeclType::Type => "type",
            DeclType::Subtype => "subtype",
            DeclType::Function | DeclType::FunctionDeclaration => "function",
            DeclType::Procedure | DeclType::ProcedureDeclaration => "procedure",
            DeclType::Alias => "alias",
            DeclType::Attribute => "attribute",
            DeclType::RecordField => "field",
            DeclType::EnumLiteral => "enum",
        };
        write!(f, "{}", s)
    }
}

impl From<DeclType> for SymbolKind {
    fn from(kind: DeclType) -> Self {
        match kind {
            DeclType::Component => SymbolKind::INTERFACE,
            DeclType::Generic => SymbolKind::CONSTANT,
            DeclType::Port(_) => SymbolKind::FIELD,
            DeclType::Parameter(_, _) => SymbolKind::FIELD,
            DeclType::Constant => SymbolKind::CONSTANT,
            DeclType::Variable => SymbolKind::VARIABLE,
            DeclType::Signal => SymbolKind::VARIABLE,
            DeclType::Type => SymbolKind::STRUCT,
            DeclType::Subtype => SymbolKind::STRUCT,
            DeclType::Function | DeclType::FunctionDeclaration => SymbolKind::FUNCTION,
            DeclType::Procedure | DeclType::ProcedureDeclaration => SymbolKind::FUNCTION,
            DeclType::Alias => SymbolKind::VARIABLE,
            DeclType::Attribute => SymbolKind::PROPERTY,
            DeclType::RecordField => SymbolKind::FIELD,
            DeclType::EnumLiteral => SymbolKind::ENUM_MEMBER,
        }
    }
}

impl From<DeclType> for OxideSymbolKind {
    fn from(kind: DeclType) -> Self {
        match kind {
            DeclType::Component => OxideSymbolKind::Component,
            DeclType::Generic => OxideSymbolKind::Generic,
            DeclType::Port(_) => OxideSymbolKind::Port,
            DeclType::Parameter(_, _) => OxideSymbolKind::Port,
            DeclType::Constant => OxideSymbolKind::Constant,
            DeclType::Variable => OxideSymbolKind::Variable,
            DeclType::Signal => OxideSymbolKind::Signal,
            DeclType::Type => OxideSymbolKind::Struct,
            DeclType::Subtype => OxideSymbolKind::Struct,
            DeclType::Function | DeclType::FunctionDeclaration => OxideSymbolKind::Function,
            DeclType::Procedure | DeclType::ProcedureDeclaration => OxideSymbolKind::Function,
            DeclType::Alias => OxideSymbolKind::Variable,
            DeclType::Attribute => OxideSymbolKind::Generic,
            DeclType::RecordField => OxideSymbolKind::Port,
            DeclType::EnumLiteral => OxideSymbolKind::Port,
        }
    }
}

/// Port Direction
///
/// Distinguishes between mode indications
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// Package scope - can declare components, functions, procedure, types, constants subtypes
    Package,
    /// Package implementation - can declare functions, procedure, types and constants
    PackageBody,
    /// Function scope - can declare variables and constants
    Function,
    /// Procedure scope - can declare variables and constants
    Procedure,
}

impl fmt::Display for ScopeKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            ScopeKind::Entity => "entity",
            ScopeKind::Architecture => "architecture",
            ScopeKind::Process => "process",
            ScopeKind::Generate => "generate",
            ScopeKind::Block => "block",
            ScopeKind::Package => "package",
            ScopeKind::PackageBody => "package body",
            ScopeKind::Function => "function",
            ScopeKind::Procedure => "procedure",
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
            ScopeKind::Package => SymbolKind::PACKAGE,
            ScopeKind::PackageBody => SymbolKind::PACKAGE,
            ScopeKind::Function => SymbolKind::FUNCTION,
            ScopeKind::Procedure => SymbolKind::FUNCTION,
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
        // Hash name, context, AND range to distinguish multiple usages of same identifier
        self.name.hash(state);
        self.context.hash(state);
        self.range.start.line.hash(state);
        self.range.start.character.hash(state);
        self.range.end.line.hash(state);
        self.range.end.character.hash(state);
    }
}

impl PartialEq for Usage {
    fn eq(&self, other: &Self) -> bool {
        // Compare name, context, AND range
        self.name == other.name && self.context == other.context && self.range == other.range
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
    /// Parameters stored for function and procedures
    pub parameters: Option<Vec<Declaration>>,
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

/// Parameter class to differentiate procedure parameter
#[derive(Debug, Clone, Copy)]
pub enum ParameterClass {
    /// Has to be explicit, is the default for inout
    Variable,
    /// Signals has to be explicit all the time
    Signal,
    /// Constant are implicit and default for in
    Constant,
    /// File specifier
    File,
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

/// Which flavour of instantiation statement produced an [`Instance`].
///
/// VHDL allows three: a direct entity instantiation (`entity lib.name`), a
/// component instantiation (bare `name`, or the explicit `component name`
/// form), and a configuration instantiation (`configuration lib.name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstantiatedUnitKind {
    /// `u0: entity work.cpu` — resolves against entity declarations.
    Entity,
    /// `u0: cpu` or `u0: component cpu` — resolves against component declarations.
    Component,
    /// `u0: configuration work.cfg` — not resolved by Oxide HDL yet.
    Configuration,
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
    /// Library prefix exactly as written in the source, case preserved.
    /// `None` for component instantiations, which carry no library.
    ///
    /// `work` here is a self-reference to the library of the file containing the
    /// instantiation, not a library literally named `work`. Compare it
    /// case-insensitively — see `backend::units::resolve_entity_uris`.
    pub library: Option<String>,
    /// Architecture named in `entity work.cpu(behavioral)`, case preserved.
    pub architecture: Option<String>,
    /// Which instantiation form this is.
    pub unit_kind: InstantiatedUnitKind,
    /// Source location information
    pub range: Range,
    /// Range of the label for jumps
    pub selection_range: Range,
}

/// Structure representing a use clause for package extractions
#[derive(Debug, Clone)]
pub struct UseClause {
    #[allow(dead_code)]
    /// Import location
    pub range: Range,
    /// Library containing the package
    pub library: String,
    /// Package Name
    pub name: String,
    /// Full import flag
    pub all_import: bool,
    /// Symbol imported from the package (if not all)
    pub imported_symbol: Option<String>,
}

/// A VHDL design unit: a context clause paired with the library unit it governs.
///
/// In VHDL, every design unit in a file is preceded by an optional context clause
/// (library/use statements). Those clauses apply *only* to the immediately following
/// library unit — they do not leak to subsequent design units in the same file.
///
/// This struct captures that pairing so that lookup and completion can use exactly
/// the right context for each unit, rather than a file-wide flat list.
#[derive(Debug, Clone)]
pub struct DesignUnit {
    /// Context clauses (use/library) that immediately precede this design unit.
    /// Visible within this unit and, for entities, inherited by all implementing
    /// architectures regardless of file location.
    pub context_clauses: Vec<UseClause>,

    /// The scope tree for this design unit (entity, architecture, package, or package body).
    pub scope_tree: crate::analysis::ScopeTree,
}
