//! Sensitivity list validation for VHDL processes.
//!
//! This module validates that process sensitivity lists are complete and correct:
//! - **Synchronous processes**: Clock signals must be in sensitivity list
//! - **Combinatorial processes**: All read signals must be in sensitivity list
//!
//! # Validation Rules
//!
//! ## Missing Signals (WARNING)
//! - Combinatorial: Any signal read in the process body must appear in sensitivity list
//! - Synchronous: Clock signals (from `rising_edge`/`falling_edge`/`'event`) must be present
//!
//! ## Unnecessary Signals (HINT)
//! - Combinatorial: Signals in sensitivity list but never read in process body
//! - Constants/Generics: Always unnecessary in sensitivity lists (they never change)
//!
//! ## Special Cases
//! - `process(all)`: VHDL-2008 keyword - validation skipped (compiler handles it)
//! - Async resets: Not yet validated (TODO v0.5)
//! - Package constants: Conservatively assumed valid if undeclared

use crate::analysis::{
    Analysis, DeclType, ScopeTree, Usage, UsageContext, collect_identifiers_recursive,
};
use crate::backend::features::diagnostics::{DiagnosticCollectors, messages};
use crate::utils::ast::{find_child, find_descendant};
use crate::utils::node_to_range;
use std::collections::HashSet;
use tower_lsp::lsp_types::Diagnostic;
use tree_sitter::Node;

/// Classification of a VHDL process based on its structure.
///
/// Processes are classified by analyzing their first-level if statement conditions:
/// - If edge detection (rising_edge/falling_edge/'event) is found → Synchronous
/// - Otherwise → Combinatorial
#[derive(Debug)]
enum ProcessType {
    /// Synchronous (clocked) process with edge-triggered behavior.
    ///
    /// Identified by presence of `rising_edge()`, `falling_edge()`, or `clk'event`
    /// in the first-level if condition.
    Synchronous {
        /// Clock signals extracted from edge checks
        clock_signals: Vec<Usage>,
        // TODO v0.5: async_resets: Vec<String>,
    },
    /// Combinatorial (unclocked) process with no edge-triggered behavior.
    ///
    /// All signals read in the process body should be in the sensitivity list.
    Combinatorial,
}

/// Validates the sensitivity list of a VHDL process statement.
///
/// Performs comprehensive checking for both missing and unnecessary signals in
/// the process sensitivity list. The validation rules differ based on whether
/// the process is synchronous (clocked) or combinatorial.
///
/// # Validation Steps
///
/// 1. Extract sensitivity list from process statement
/// 2. Check for VHDL-2008 `all` keyword (skip validation if present)
/// 3. Classify process as synchronous or combinatorial
/// 4. Extract signals that are read in the process body
/// 5. Filter out variables, constants, and undeclared identifiers
/// 6. Check for missing signals (should be in sensitivity but aren't)
/// 7. Check for unnecessary signals (in sensitivity but not read)
///
/// # Arguments
///
/// * `process_node` - Tree-sitter node of type `process_statement`
/// * `text` - Full source text of the file
/// * `scope_tree` - Scope tree containing the process (for declaration lookup)
/// * `analysis` - Full analysis context (for entity scope resolution)
/// * `collectors` - Diagnostic collectors to append findings to
///
/// # Diagnostics Produced
///
/// - **WARNING**: Signal read but not in sensitivity list (functional issue)
/// - **HINT**: Signal in sensitivity list but not read (unnecessary, tagged)
///
/// # Examples
///
/// ```vhdl
/// -- Missing 'b' in sensitivity list → WARNING
/// process(a)
/// begin
///     result <= a and b;
/// end process;
///
/// -- Unnecessary 'c' in sensitivity list → HINT
/// process(a, b, c)
/// begin
///     result <= a and b;
/// end process;
/// ```
pub fn check_process_sensitivity(
    process_node: Node,
    text: &str,
    scope_tree: Option<&ScopeTree>,
    analysis: &Analysis,
    collectors: &mut DiagnosticCollectors,
) {
    let sensitivity_list = extract_sensitivity_list(process_node, text);

    // Skip validation if 'all' keyword is used (VHDL-2008)
    if sensitivity_list
        .iter()
        .any(|s| s.name.to_lowercase() == "all")
    {
        return;
    }

    let mut read_signals = HashSet::new();

    if let Some(sequential_block) = process_node
        .children(&mut process_node.walk())
        .find(|c| c.kind() == "sequential_block")
        && let Some(scope_tree) = scope_tree
    {
        let process_type = classify_process(sequential_block, text);

        // Extract signals based on process type
        match process_type {
            ProcessType::Combinatorial => {
                extract_signals_read(sequential_block, text, &mut read_signals, false)
            }
            ProcessType::Synchronous {
                clock_signals: ref clocks,
            } => read_signals.extend(clocks.clone()),
        };

        // Filter to only signals/ports (exclude variables, constants, generics)
        if let Some(visible_decl) =
            analysis.collect_visible_declarations(scope_tree, node_to_range(process_node))
        {
            let read_signals: Vec<&Usage> = read_signals
                .iter()
                .filter(|s| {
                    if let Some(decl) = visible_decl
                        .iter()
                        .find(|n| *n.name.to_lowercase() == s.name.to_lowercase())
                    {
                        matches!(decl.decl_type, DeclType::Port(_) | DeclType::Signal)
                    } else {
                        // Conservative: assume undeclared identifiers are package constants
                        false
                    }
                })
                .collect();

            // Check for missing signals
            for read_signal in &read_signals {
                let lower_name = read_signal.name.to_lowercase();
                if !sensitivity_list
                    .iter()
                    .any(|v| v.name.to_lowercase() == lower_name)
                {
                    collectors.sensitivity.push(messages::missing_sensitivity(
                        &read_signal.name,
                        &process_node,
                    ));
                }
            }

            // Check for unnecessary signals
            // NOTE: Skip for synchronous processes (async resets not yet detected)
            match process_type {
                ProcessType::Combinatorial => {}
                ProcessType::Synchronous {
                    clock_signals: _clocks,
                } => return,
            };

            for sensitive in sensitivity_list {
                if !read_signals
                    .iter()
                    .any(|s| s.name.to_lowercase() == sensitive.name.to_lowercase())
                {
                    collectors
                        .sensitivity
                        .push(messages::unnecessary_sensitivity(
                            &sensitive.name,
                            &sensitive.range,
                        ));
                }
            }
        }
    }
}

/// Classifies a process as synchronous or combinatorial.
///
/// Examines the first-level if statement in the process body to determine
/// if it contains edge-triggered conditions (rising_edge, falling_edge, or 'event).
///
/// # Arguments
///
/// * `sequential_block` - The `sequential_block` node inside the process
/// * `text` - Full source text
///
/// # Returns
///
/// * `ProcessType::Synchronous` - If edge checks are found (with clock signal list)
/// * `ProcessType::Combinatorial` - If no edge checks are found
///
/// # Examples
///
/// ```vhdl
/// -- Synchronous (returns clock signal)
/// if rising_edge(clk) then
///     ...
/// end if;
///
/// -- Combinatorial (no edge check)
/// if sel = '1' then
///     ...
/// end if;
/// ```
fn classify_process(sequential_block: Node, text: &str) -> ProcessType {
    let clocks = find_edge_checks(sequential_block, text);
    if clocks.is_empty() {
        ProcessType::Combinatorial
    } else {
        ProcessType::Synchronous {
            clock_signals: clocks,
        }
    }
}

/// Finds clock signals used in edge detection expressions.
///
/// Searches the first-level if statement for edge-triggered conditions.
/// Only checks top-level conditions - nested edge checks are ignored as they
/// don't represent proper synchronous design patterns.
///
/// # Arguments
///
/// * `sequential_block` - The `sequential_block` node to search
/// * `text` - Full source text
///
/// # Returns
///
/// Vector of clock signal usages found in edge checks. Empty if no edges found.
///
/// # Recognized Patterns
///
/// - `rising_edge(clk)` / `falling_edge(clk)`
/// - `clk'event and clk = '1'`
/// - Async reset pattern: `if rst = '1' then ... elsif rising_edge(clk) then ...`
fn find_edge_checks(sequential_block: Node, text: &str) -> Vec<Usage> {
    let mut clocks = vec![];
    if let Some(if_statement_block) = find_child(sequential_block, "if_statement_block")
        && let Some(if_statement) = find_child(if_statement_block, "if_statement")
    {
        clocks.extend(extract_clocks_from_if_statement(if_statement, text));
    }
    clocks
}

/// Extracts clock signals from an if statement and its elsif branches.
///
/// Recursively searches if and elsif conditions for edge detection patterns.
///
/// # Arguments
///
/// * `if_stmt` - The `if_statement` node
/// * `text` - Full source text
///
/// # Returns
///
/// Vector of clock signal usages found in the if/elsif conditions
fn extract_clocks_from_if_statement(if_stmt: Node, text: &str) -> Vec<Usage> {
    let mut clocks = Vec::new();

    for child in if_stmt.children(&mut if_stmt.walk()) {
        match child.kind() {
            kind if kind.contains("expression") => {
                clocks.extend(find_edge_in_condition(child, text));
            }
            "elsif_statement" => {
                clocks.extend(extract_clocks_from_elsif(child, text));
            }
            _ => {}
        }
    }

    clocks
}

/// Extracts clock signals from an elsif statement condition.
///
/// # Arguments
///
/// * `elsif_stmt` - The `elsif_statement` node
/// * `text` - Full source text
///
/// # Returns
///
/// Vector of clock signal usages found in the elsif condition
fn extract_clocks_from_elsif(elsif_stmt: Node, text: &str) -> Vec<Usage> {
    let mut clocks = Vec::new();

    for child in elsif_stmt.children(&mut elsif_stmt.walk()) {
        if child.kind().contains("expression") {
            clocks.extend(find_edge_in_condition(child, text));
        }
    }

    clocks
}

/// Recursively searches a condition for edge detection patterns.
///
/// Looks for:
/// - Function calls: `rising_edge(clk)`, `falling_edge(clk)`
/// - Attribute usage: `clk'event`
///
/// # Arguments
///
/// * `condition` - Expression node containing the condition
/// * `text` - Full source text
///
/// # Returns
///
/// Vector of clock signal usages found in the condition
fn find_edge_in_condition(condition: Node, text: &str) -> Vec<Usage> {
    let mut clocks = Vec::new();
    let mut cursor = condition.walk();

    for child in condition.children(&mut cursor) {
        match child.kind() {
            "name" => {
                if let Some(clock) = extract_clock_from_function_call(child, text) {
                    clocks.push(Usage {
                        name: clock,
                        context: UsageContext::Behavioral,
                        range: node_to_range(child),
                    });
                } else if let Some(clock) = extract_clock_from_attribute(child, text) {
                    clocks.push(Usage {
                        name: clock,
                        context: UsageContext::Behavioral,
                        range: node_to_range(child),
                    });
                }
            }
            _ => {
                if !child.kind().contains("statement") && !child.kind().contains("sequence") {
                    clocks.extend(find_edge_in_condition(child, text));
                }
            }
        }
    }

    clocks
}

/// Extracts clock signal from rising_edge() or falling_edge() function calls.
///
/// # Arguments
///
/// * `function` - Name node that might be a function call
/// * `text` - Full source text
///
/// # Returns
///
/// * `Some(clock_name)` - If this is a rising_edge/falling_edge call
/// * `None` - Otherwise
///
/// # Example
///
/// ```vhdl
/// rising_edge(clk)  -- Returns Some("clk")
/// my_func(sig)      -- Returns None
/// ```
fn extract_clock_from_function_call(function: Node, text: &str) -> Option<String> {
    let mut function_name: Option<String> = None;
    let mut params_node: Option<Node> = None;

    for child in function.children(&mut function.walk()) {
        match child.kind() {
            "library_function" => {
                function_name = Some(text[child.byte_range()].to_lowercase().clone())
            }
            "parenthesis_group" => params_node = Some(child),
            _ => {}
        }
    }

    if let Some(function_name) = function_name
        && (function_name == "rising_edge" || function_name == "falling_edge")
        && let Some(params_node) = params_node
    {
        return extract_first_identifier_recurse(params_node, text);
    }
    None
}

/// Extracts clock signal from attribute usage like `clk'event`.
///
/// Specifically looks for the `'event` attribute which is commonly used
/// in edge detection patterns like `clk'event and clk = '1'`.
///
/// # Arguments
///
/// * `attr` - Name node that might contain an attribute
/// * `text` - Full source text
///
/// # Returns
///
/// * `Some(clock_name)` - If this is a signal with 'event attribute
/// * `None` - Otherwise
///
/// # Example
///
/// ```vhdl
/// clk'event    -- Returns Some("clk")
/// sig'stable   -- Returns None (not 'event)
/// ```
fn extract_clock_from_attribute(attr: Node, text: &str) -> Option<String> {
    let mut clock_name: Option<String> = None;
    let mut attr_name: Option<String> = None;

    for child in attr.children(&mut attr.walk()) {
        match child.kind() {
            "identifier" => clock_name = Some(text[child.byte_range()].to_string()),
            "attribute" => {
                if let Some(attr_name_node) = child.child_by_field_name("attribute") {
                    attr_name = Some(text[attr_name_node.byte_range()].to_lowercase());
                }
            }
            _ => {}
        }
    }

    if let Some(clock) = clock_name
        && let Some(attr_name) = attr_name
        && attr_name == "event"
    {
        return Some(clock);
    }
    None
}

/// Recursively finds the first identifier in a node tree.
///
/// Used to extract the clock signal name from function call parameters.
///
/// # Arguments
///
/// * `node` - Node to search
/// * `text` - Full source text
///
/// # Returns
///
/// * `Some(identifier)` - First identifier found
/// * `None` - No identifier found
fn extract_first_identifier_recurse(node: Node, text: &str) -> Option<String> {
    if node.kind() == "identifier" {
        return Some(text[node.byte_range()].to_string());
    }

    for child in node.children(&mut node.walk()) {
        if let Some(clock) = extract_first_identifier_recurse(child, text) {
            return Some(clock);
        }
    }
    None
}

/// Extracts all signals from a process sensitivity list.
///
/// Parses the sensitivity specification (the part in parentheses after `process`)
/// and collects all signal identifiers, preserving their source locations.
///
/// # Arguments
///
/// * `process_node` - Tree-sitter node of type `process_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// HashSet of Usage objects representing signals in the sensitivity list.
/// Returns empty set if no sensitivity list present (process uses `wait` statements).
///
/// # Special Cases
///
/// - **VHDL-2008 `all` keyword**: Returns immediately with single "all" usage
/// - **No sensitivity list**: Returns empty set
///
/// # Examples
///
/// ```vhdl
/// process(clk, rst)     -- Returns {clk, rst}
/// process(all)          -- Returns {all}
/// process               -- Returns {} (no sensitivity list)
/// ```
fn extract_sensitivity_list(process_node: Node, text: &str) -> HashSet<Usage> {
    let mut sensitivity_signals = HashSet::new();

    if let Some(sensitivity_spec) = find_descendant(process_node, "sensitivity_specification") {
        // Check for VHDL-2008 'all' keyword
        if let Some(sensitivity_all) = find_child(sensitivity_spec, "ALL") {
            sensitivity_signals.insert(Usage {
                name: "all".to_string(),
                context: UsageContext::Behavioral,
                range: node_to_range(sensitivity_all),
            });
            return sensitivity_signals;
        }

        // Collect all identifier nodes
        collect_identifiers_recursive(
            sensitivity_spec,
            text,
            UsageContext::Behavioral,
            &mut sensitivity_signals,
        );
    }

    sensitivity_signals
}

/// Recursively extracts all signals that are read in a process body.
///
/// Traverses the AST to find all signal/variable references that represent
/// read operations (not writes). Handles VHDL assignment statements to
/// distinguish between left-hand side (write) and right-hand side (read).
///
/// # Arguments
///
/// * `start_node` - Node to start searching from (typically `sequential_block`)
/// * `text` - Full source text
/// * `read_signals` - HashSet to accumulate found signal usages into
/// * `is_lhs` - Whether we're currently on the left-hand side of an assignment
///
/// # Read Detection Rules
///
/// - **Always read**: Waveforms, expressions, conditions, case expressions
/// - **Context-dependent**: Names (read unless on LHS of assignment)
/// - **Indexing**: Always read (even on LHS, e.g., `array(index) <= value`)
///
/// # Examples
///
/// ```vhdl
/// result <= a and b;     -- Reads: a, b (not result)
/// array(idx) <= val;     -- Reads: idx, val (not array)
/// if sel = '1' then      -- Reads: sel
/// ```
fn extract_signals_read(
    start_node: Node,
    text: &str,
    read_signals: &mut HashSet<Usage>,
    is_lhs: bool,
) {
    match start_node.kind() {
        // Nodes where all identifiers are reads
        "waveform"
        | "conditional_or_unaffected_expression"
        | "case_expression"
        | "relational_expression"
        | "when_element"
        | "when_expression"
        | "initialiser" => {
            collect_identifiers_recursive(start_node, text, UsageContext::Behavioral, read_signals)
        }

        // Assignment statements - need to distinguish LHS from RHS
        "simple_waveform_assignment"
        | "conditional_waveform_assignment"
        | "simple_variable_assignment" => {
            for child in start_node.children(&mut start_node.walk()) {
                if child.kind() == "name" {
                    // First 'name' is LHS (write target)
                    extract_signals_read(child, text, read_signals, true);
                } else {
                    // Everything else is RHS (read)
                    extract_signals_read(child, text, read_signals, false);
                }
            }
        }

        // Name nodes (signal references) - check if read or write
        "name" => {
            for child in start_node.children(&mut start_node.walk()) {
                if child.kind() == "parenthesis_group" {
                    // Array indices are always reads, even on LHS
                    extract_signals_read(child, text, read_signals, false);
                } else if child.kind() == "identifier" && !is_lhs {
                    // Identifier on RHS is a read
                    read_signals.insert(Usage {
                        name: text[child.byte_range()].to_string(),
                        context: UsageContext::Behavioral,
                        range: node_to_range(child),
                    });
                }
            }
        }

        // Default: recurse into children
        _ => {
            for child in start_node.children(&mut start_node.walk()) {
                extract_signals_read(child, text, read_signals, false);
            }
        }
    }
}

/// Placeholder for future synchronous process validation.
///
/// Will validate that all clock signals are present in the sensitivity list
/// and that async reset signals (when detected) are also included.
///
/// # TODO v0.5
///
/// - Implement clock signal validation
/// - Add async reset detection and validation
#[allow(dead_code)]
fn check_sync_process(
    _sensitivity_list: &HashSet<String>,
    _clock_signals: &[String],
) -> Vec<Diagnostic> {
    vec![]
}

/// Placeholder for future combinatorial process validation.
///
/// Will be refactored once async reset detection is added.
///
/// # TODO v0.5
///
/// - May be removed in favor of inline validation
#[allow(dead_code)]
fn check_comb_process(
    _sensitivity_list: &HashSet<String>,
    _signals_read: &HashSet<String>,
) -> Vec<Diagnostic> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::test_utils::SHARED_PARSER_LOCK;
    use tree_sitter::Parser;

    fn parse_text(code: &str) -> tree_sitter::Tree {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        parser.parse(code, None).unwrap()
    }

    fn check_sensitivity(code: &str) -> Vec<Diagnostic> {
        let tree = parse_text(code);
        let root = tree.root_node();

        // Build full analysis (includes entity scopes + arch scope trees)
        let analysis = crate::backend::syntax::parser::extract_document_symbols(code, root);

        let mut collectors = super::super::DiagnosticCollectors::new();

        // Match architectures with scope trees by order
        let mut arch_index = 0;
        let mut cursor = root.walk();

        for node in root.children(&mut cursor) {
            if node.kind() == "design_unit" {
                for child in node.children(&mut node.walk()) {
                    if child.kind() == "architecture_definition" {
                        if let Some(scope_tree) = analysis.scope_trees.get(arch_index) {
                            find_and_check_processes(
                                child,
                                code,
                                scope_tree,
                                &analysis,
                                &mut collectors,
                            );
                        }
                        arch_index += 1;
                    }
                }
            }
        }

        collectors.sensitivity
    }

    fn find_and_check_processes(
        node: Node,
        text: &str,
        scope_tree: &ScopeTree,
        analysis: &Analysis, // ← ADD THIS
        collectors: &mut DiagnosticCollectors,
    ) {
        if node.kind() == "process_statement" {
            check_process_sensitivity(node, text, Some(scope_tree), analysis, collectors);
        }

        for child in node.children(&mut node.walk()) {
            find_and_check_processes(child, text, scope_tree, analysis, collectors);
        }
    }

    macro_rules! test_edge_detection {
        ($name:ident, $code:expr, $expected:expr) => {
            #[test]
            fn $name() {
                let tree = parse_text($code);
                let root = tree.root_node();

                let mut seq_block = None;
                let mut cursor = root.walk();
                for node in root.children(&mut cursor) {
                    if node.kind() == "design_unit" {
                        for child in node.children(&mut node.walk()) {
                            if child.kind() == "architecture_definition" {
                                for arch_child in child.children(&mut child.walk()) {
                                    if arch_child.kind() == "concurrent_block" {
                                        for proc in arch_child.children(&mut arch_child.walk()) {
                                            if proc.kind() == "process_statement" {
                                                for proc_child in proc.children(&mut proc.walk()) {
                                                    if proc_child.kind() == "sequential_block" {
                                                        seq_block = Some(proc_child);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                let clocks: Vec<String> = find_edge_checks(seq_block.unwrap(), $code)
                    .iter()
                    .map(|x| x.name.clone())
                    .collect();
                let expected: Vec<String> = $expected;
                assert_eq!(clocks, expected);
            }
        };
    }

    test_edge_detection!(
        test_find_rising_edge,
        r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    process(clk)
    begin
        if rising_edge(clk) then
            null;
        end if;
    end process;
end architecture;
"#,
        vec!["clk".to_string()]
    );

    test_edge_detection!(
        test_find_falling_edge,
        r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    process(clk)
    begin
        if falling_edge(clk) then
            null;
        end if;
    end process;
end architecture;
"#,
        vec!["clk".to_string()]
    );

    test_edge_detection!(
        test_find_clk_event_attribute,
        r#"
architecture rtl of test is
    signal clk : std_logic;
begin
    process(clk)
    begin
        if clk'event and clk = '1' then
            null;
        end if;
    end process;
end architecture;
"#,
        vec!["clk".to_string()]
    );

    test_edge_detection!(
        test_async_reset_pattern_edge_in_elsif,
        r#"
architecture rtl of test is
    signal clk, rst : std_logic;
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
"#,
        vec!["clk".to_string()]
    );

    test_edge_detection!(
        test_multiple_clocks,
        r#"
architecture rtl of test is
    signal clk1, clk2 : std_logic;
begin
    process(clk1, clk2)
    begin
        if rising_edge(clk1) then
            null;
        elsif falling_edge(clk2) then
            null;
        end if;
    end process;
end architecture;
"#,
        vec!["clk1".to_string(), "clk2".to_string()]
    );

    test_edge_detection!(
        test_no_edge_combinatorial,
        r#"
architecture rtl of test is
    signal a, b, sel : std_logic;
begin
    process(a, b, sel)
    begin
        if sel = '1' then
            null;
        else
            null;
        end if;
    end process;
end architecture;
"#,
        vec![]
    );

    test_edge_detection!(
        test_nested_if_with_edge_at_top_level,
        r#"
architecture rtl of test is
    signal clk, enable : std_logic;
begin
    process(clk)
    begin
        if rising_edge(clk) then
            if enable = '1' then
                null;
            end if;
        end if;
    end process;
end architecture;
"#,
        vec!["clk".to_string()]
    );

    test_edge_detection!(
        test_edge_not_at_first_level_ignored,
        r#"
architecture rtl of test is
    signal clk, enable : std_logic;
begin
    process(enable)
    begin
        if enable = '1' then
            if rising_edge(clk) then
                null;
            end if;
        end if;
    end process;
end architecture;
"#,
        vec![]
    );

    test_edge_detection!(
        test_edge_first_level_following_assignments,
        r#"
architecture rtl of test is
    signal clk, enable : std_logic;
    signal toto, titi: std_logic;
begin
    process(clk)
    begin
        toto <= titi;
        if rising_edge(clk) then
            if enable = '1' then
                null;
            end if;
        end if;
    end process;
end architecture;
"#,
        vec!["clk".to_string()]
    );
    // Add these to the sensitivity.rs test module

    #[test]
    fn test_sync_process_complete_sensitivity() {
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
        let diags = check_sensitivity(code);
        assert!(
            diags.is_empty(),
            "Complete synchronous process should be OK"
        );
    }

    #[test]
    fn test_sync_process_missing_clock() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal data : std_logic;
begin
    process  -- Missing sensitivity list!
    begin
        if rising_edge(clk) then
            data <= '1';
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 1, "Should detect missing clock in sensitivity");
        assert!(diags[0].message.to_lowercase().contains("clk"));
    }

    #[test]
    fn test_sync_process_clock_in_wrong_list() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal wrong : std_logic;
    signal data : std_logic;
    signal b: std_logic;
begin
    process(wrong)  -- Wrong signal in sensitivity!
    begin
        if rising_edge(clk) then
            data <= wrong;
            if b = '1' then
                data <= '0';
            end if;
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 1, "Should detect missing clock");
        assert!(diags[0].message.to_lowercase().contains("clk"));
    }

    #[test]
    fn test_comb_process_complete_sensitivity() {
        let code = r#"
architecture rtl of test is
    signal a, b, sel, result : std_logic;
begin
    process(a, b, sel)
    begin
        if sel = '1' then
            result <= a;
        else
            result <= b;
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert!(
            diags.is_empty(),
            "Complete combinatorial process should be OK"
        );
    }

    #[test]
    fn test_comb_process_missing_signal() {
        let code = r#"
architecture rtl of test is
    signal a, b, sel, result : std_logic;
begin
    process(a, b)  -- Missing 'sel'!
    begin
        if sel = '1' then
            result <= a;
        else
            result <= b;
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 1, "Should detect missing signal");
        assert!(diags[0].message.to_lowercase().contains("sel"));
    }

    #[test]
    fn test_comb_process_multiple_missing_signals() {
        let code = r#"
architecture rtl of test is
    signal a, b, c, sel, result : std_logic;
begin
    process(a)  -- Missing b, c, sel!
    begin
        if sel = '1' then
            result <= b;
        else
            result <= c;
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert!(diags.len() >= 2, "Should detect multiple missing signals");
    }

    #[test]
    fn test_sync_with_falling_edge() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal data : std_logic;
begin
    process(clk)
    begin
        if falling_edge(clk) then
            data <= '0';
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert!(diags.is_empty(), "falling_edge should work too");
    }

    #[test]
    fn test_sync_with_clk_event() {
        let code = r#"
architecture rtl of test is
    signal clk : std_logic;
    signal data : std_logic;
begin
    process(clk)
    begin
        if clk'event and clk = '1' then
            data <= '1';
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert!(diags.is_empty(), "clk'event should work");
    }

    #[test]
    fn test_async_reset_both_in_sensitivity() {
        let code = r#"
architecture rtl of test is
    signal clk, rst : std_logic;
    signal counter : integer;
begin
    process(clk, rst)
    begin
        if rst = '1' then
            counter <= 0;
        elsif rising_edge(clk) then
            counter <= counter + 1;
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        // For v0.4, we only check clocks, not async resets
        assert!(diags.is_empty(), "Clock is present, should be OK");
    }

    #[test]
    fn test_process_with_all_keyword() {
        let code = r#"
architecture rtl of test is
    signal a, b, result : std_logic;
begin
    process(all)  -- VHDL-2008 'all' keyword
    begin
        result <= a and b;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        // Should handle 'all' gracefully (maybe skip validation)
        assert!(diags.is_empty(), "'all' keyword should be accepted");
    }

    #[test]
    fn test_process_without_sensitivity_list() {
        let code = r#"
architecture rtl of test is
    signal data : std_logic;
begin
    process
    begin
        wait until rising_edge(clk);
        data <= '1';
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        // Process with 'wait' doesn't need sensitivity list
        // For v0.4, if no sensitivity list, skip validation
        assert!(
            diags.is_empty(),
            "Process with wait doesn't need sensitivity"
        );
    }
    #[test]
    fn test_process_missing_from_entity() {
        let code = r#"
entity test is
generic (
    GENERIC_A: integer := 0;
    GENERIC_B, GENERIC_C: integer := 0
);
port (
    valid: in std_logic;
    sig_a, sig_b: out std_logic;
);
end entity;
architecture rtl of test is
    signal data : std_logic;
begin
    process
    begin
        if valid = '1' then
            data <= '1';
        else
            data <= '0';
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        // Process with 'wait' doesn't need sensitivity list
        // For v0.4, if no sensitivity list, skip validation
        assert!(diags.len() == 1, "Missing valid in the sensitivity list");
    }
    #[test]
    fn test_unnecessary_constant_in_sensitivity() {
        let code = r#"
architecture rtl of test is
    constant MAX_VAL : integer := 100;
    signal data : integer;
begin
    process(MAX_VAL)  -- Constant never changes, unnecessary!
    begin
        data <= MAX_VAL;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 1, "Should detect unnecessary constant");
        assert!(diags[0].message.to_lowercase().contains("not needed"));
        assert!(diags[0].message.to_lowercase().contains("max_val"));
    }

    #[test]
    fn test_unnecessary_generic_in_sensitivity() {
        let code = r#"
entity test is
    generic (
        DATA_WIDTH : integer := 8
    );
end entity;
architecture rtl of test is
    signal data : std_logic_vector(DATA_WIDTH-1 downto 0);
begin
    process(DATA_WIDTH)
    begin
        data <= (others => '0');
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 1, "Should detect unnecessary generic");
        assert!(diags[0].message.to_lowercase().contains("data_width"));
        assert!(diags[0].message.to_lowercase().contains("not needed"));
    }

    #[test]
    fn test_unnecessary_signal_not_read() {
        let code = r#"
architecture rtl of test is
    signal a, b, unused, result : std_logic;
begin
    process(a, b, unused)  -- 'unused' not actually read!
    begin
        if a = '1' then
            result <= b;
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 1, "Should detect unnecessary signal");
        assert!(diags[0].message.to_lowercase().contains("unused"));
        assert!(
            diags[0].message.to_lowercase().contains("unnecessary")
                || diags[0].message.to_lowercase().contains("not needed")
        );
    }

    #[test]
    fn test_multiple_unnecessary_signals() {
        let code = r#"
architecture rtl of test is
    signal a, b, extra1, extra2, result : std_logic;
begin
    process(a, b, extra1, extra2)  -- extra1 and extra2 not needed!
    begin
        if a = '1' then
            result <= b;
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert!(
            diags.len() >= 2,
            "Should detect multiple unnecessary signals"
        );
    }

    #[test]
    fn test_no_unnecessary_all_signals_used() {
        let code = r#"
architecture rtl of test is
    signal a, b, sel, result : std_logic;
begin
    process(a, b, sel)
    begin
        if sel = '1' then
            result <= a;
        else
            result <= b;
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert!(diags.is_empty(), "All signals used, none unnecessary");
    }

    #[test]
    #[ignore]
    fn test_sync_process_with_unnecessary_signal() {
        let code = r#"
architecture rtl of test is
    signal clk, unused : std_logic;
    signal data : std_logic;
begin
    process(clk, unused)  -- 'unused' not needed in sync process
    begin
        if rising_edge(clk) then
            data <= '1';
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(
            diags.len(),
            1,
            "Should detect unnecessary signal in sync process"
        );
        assert!(diags[0].message.to_lowercase().contains("unused"));
    }

    #[test]
    fn test_mixed_missing_and_unnecessary() {
        let code = r#"
architecture rtl of test is
    signal needed, unnecessary, result : std_logic;
begin
    process(unnecessary)  -- Has 'unnecessary', missing 'needed'
    begin
        if needed = '1' then
            result <= '1';
        end if;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 2, "Should detect both missing and unnecessary");

        let has_missing = diags.iter().any(|d| {
            d.message.to_lowercase().contains("needed")
                && d.message.to_lowercase().contains("not in")
        });
        let has_unnecessary = diags
            .iter()
            .any(|d| d.message.to_lowercase().contains("unnecessary"));

        assert!(has_missing, "Should report missing 'needed'");
        assert!(has_unnecessary, "Should report unnecessary signal");
    }

    #[test]
    fn test_constant_and_signal_both_unnecessary() {
        let code = r#"
architecture rtl of test is
    constant MAX : integer := 100;
    signal unused_sig : std_logic;
    signal result : std_logic;
begin
    process(MAX, unused_sig)  -- Both unnecessary
    begin
        result <= '0';
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 2, "Should detect both unnecessary items");
    }

    #[test]
    fn test_all_keyword_no_unnecessary_check() {
        let code = r#"
architecture rtl of test is
    signal a, b : std_logic;
begin
    process(all)  -- 'all' keyword - skip unnecessary checks
    begin
        a <= '0';  -- 'b' not used, but 'all' is fine
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert!(
            diags.is_empty(),
            "'all' keyword should skip unnecessary checks"
        );
    }
}
