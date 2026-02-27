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

use crate::analysis::{DeclType, Declaration, Usage, UsageContext, collect_identifiers_recursive};
use crate::backend::AnalysisMap;
use crate::backend::features::code_actions::{MissingSensitivityData, UnnecessarySensitivityData};
use crate::backend::features::diagnostics::{DiagnosticCollectors, DiagnosticContext, messages};
use crate::backend::features::lookup::lookup_all_procedure_declarations;
use crate::utils::ast::{find_child, find_descendant};
use crate::utils::node_to_range;
use std::collections::HashSet;
use tower_lsp::lsp_types::{Diagnostic, Position, Url};
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

struct SignalExtractionContext<'a> {
    text: &'a str,
    global_map: &'a AnalysisMap,
    current_uri: &'a Url,
    /// Signals that are definitely read (used for "missing" and "unnecessary" checks).
    read_signals: &'a mut HashSet<Usage>,
    /// Signals that appear as arguments to a procedure call whose declaration
    /// could not be resolved.  We don't know their direction, so they are
    /// excluded from both the "missing" check and the "unnecessary" check.
    maybe_read_signals: &'a mut HashSet<Usage>,
}

impl<'a> SignalExtractionContext<'a> {
    fn extract(&mut self, start_node: Node, is_lhs: bool) {
        match start_node.kind() {
            // Nodes where all identifiers are reads
            "conditional_or_unaffected_expression"
            | "case_expression"
            | "relational_expression"
            | "when_element"
            | "when_expression"
            | "initialiser" => collect_identifiers_recursive(
                start_node,
                self.text,
                UsageContext::Behavioral,
                self.read_signals,
            ),

            // Assignment statements - need to distinguish LHS from RHS
            "simple_waveform_assignment"
            | "conditional_waveform_assignment"
            | "conditional_signal_assignment"
            | "simple_variable_assignment" => {
                for child in start_node.children(&mut start_node.walk()) {
                    if child.kind() == "name" {
                        // First 'name' is LHS (write target)
                        self.extract(child, true);
                    } else {
                        // Everything else is RHS (read)
                        self.extract(child, false);
                    }
                }
            }
            "conditional_expression"
            | "conditional_waveform"
            | "simple_expression"
            | "term"
            | "factor"
            | "primary"
            | "parenthesis_group" => {
                for child in start_node.children(&mut start_node.walk()) {
                    // Propagate 'is_lhs' down!
                    self.extract(child, is_lhs);
                }
            }

            // Name nodes (signal references) - check if read or write
            "name" => {
                if find_child(start_node, "attribute").is_none() {
                    for child in start_node.children(&mut start_node.walk()) {
                        // If the name contains an attribute node, we should skip
                        if child.kind() == "parenthesis_group" {
                            // Array indices are always reads, even on LHS
                            self.extract(child, false);
                        } else if child.kind() == "identifier" && !is_lhs {
                            // Identifier on RHS is a read
                            self.read_signals.insert(Usage {
                                name: self.text[child.byte_range()].to_string(),
                                context: UsageContext::Behavioral,
                                range: node_to_range(child),
                            });
                        }
                    }
                }
            }
            "procedure_call_statement" => {
                let name_node = find_child(start_node, "name");

                if let Some(name_node) = name_node {
                    let mut proc_name = None;
                    let mut args_node = None;
                    for child in name_node.children(&mut name_node.walk()) {
                        match child.kind() {
                            "identifier" | "selected_name" => {
                                proc_name = Some(self.text[child.byte_range()].to_string());
                            }
                            "parenthesis_group" => {
                                args_node = Some(child);
                            }
                            _ => {}
                        }
                    }

                    let pos: Position = Position {
                        line: start_node.range().start_point.row as u32,
                        character: start_node.range().start_point.column as u32,
                    };

                    let declarations = proc_name
                        .map(|name| {
                            lookup_all_procedure_declarations(
                                &name,
                                self.current_uri,
                                self.global_map,
                                &pos,
                            )
                        })
                        .unwrap_or_default();

                    match declarations.len() {
                        0 => {
                            // Lookup failed (procedure defined in another file or not yet
                            // parsed).  We don't know the directions, so add all identifiers
                            // to `maybe_read_signals`: suppresses false "unnecessary" warnings
                            // without generating false "missing" warnings.
                            if let Some(args) = args_node {
                                collect_identifiers_recursive(
                                    args,
                                    self.text,
                                    UsageContext::Behavioral,
                                    self.maybe_read_signals,
                                );
                            }
                        }
                        1 => {
                            // Exactly one overload – we know the parameter directions.
                            let declaration = &declarations[0];
                            match declaration.decl_type {
                                DeclType::Function => {
                                    if let Some(args) = args_node {
                                        self.extract(args, false);
                                    }
                                }
                                DeclType::Procedure | DeclType::ProcedureDeclaration => {
                                    if let Some(args) = args_node {
                                        self.analyze_procedure_arguments(args, declaration);
                                    }
                                }
                                _ => {
                                    if let Some(args) = args_node {
                                        self.extract(args, false);
                                    }
                                }
                            }
                        }
                        _ => {
                            // Multiple overloads – directions may differ per overload so we
                            // cannot determine them without resolving the call.  Fall back to
                            // the same conservative treatment as a failed lookup.
                            if let Some(args) = args_node {
                                collect_identifiers_recursive(
                                    args,
                                    self.text,
                                    UsageContext::Behavioral,
                                    self.maybe_read_signals,
                                );
                            }
                        }
                    }
                }
            }

            // Default: recurse into children
            _ => {
                for child in start_node.children(&mut start_node.walk()) {
                    self.extract(child, false);
                }
            }
        }
    }
    /// Analyzes arguments of a procedure call to determine if they are Reads or Writes.
    ///
    /// Maps the "Actual" arguments (in the call) to the "Formal" parameters (in the definition)
    /// to check their direction (IN vs OUT).
    fn analyze_procedure_arguments(
        &mut self,
        parenthesis_group: Node,
        decl: &Declaration,
        // Context needed for recursion
    ) {
        let mut cursor = parenthesis_group.walk();
        let list_node = parenthesis_group
            .children(&mut cursor)
            .find(|c| c.kind() == "association_or_range_list")
            .unwrap_or(parenthesis_group);

        let mut param_cursor = 0;

        for child in list_node.children(&mut list_node.walk()) {
            if child.kind() == "association_element" {
                let mut target_param = None;
                let mut actual_node = None;

                let children: Vec<Node> = child
                    .children(&mut child.walk())
                    .filter(|c| !c.kind().contains("comment"))
                    .collect();
                if children.len() >= 2 {
                    // Named association list
                    let formal_node = children[0];
                    let formal_name = self.text[formal_node.byte_range()].to_string();
                    if let Some(params) = &decl.parameters {
                        target_param = params
                            .iter()
                            .find(|p| p.name.eq_ignore_ascii_case(&formal_name));
                    }
                    actual_node = children.last().cloned();
                } else if children.len() == 1 {
                    // Positional assoc
                    actual_node = Some(children[0]);
                    if let Some(params) = &decl.parameters
                        && param_cursor < params.len()
                    {
                        target_param = Some(&params[param_cursor]);
                        param_cursor += 1;
                    }
                }

                // Resolution
                if let Some(param) = target_param {
                    let is_write_only = match &param.decl_type {
                        DeclType::Parameter(direction, _) => {
                            let dir_str = direction.to_string().to_lowercase();
                            dir_str == "out"
                        }
                        _ => false,
                    };

                    if let Some(actual) = actual_node {
                        self.extract(actual, is_write_only);
                    }
                } else {
                    // Fallback, we assume inputs
                    if let Some(actual) = actual_node {
                        self.extract(actual, false);
                    }
                }
            }
        }
    }
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
/// * `ctx` - Diagnostic context containing all read-only validation parameters
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
    ctx: &DiagnosticContext,
    collectors: &mut DiagnosticCollectors,
) {
    let sensitivity_list = extract_sensitivity_list(process_node, ctx.text);

    // Skip validation if 'all' keyword is used (VHDL-2008)
    if sensitivity_list
        .iter()
        .any(|s| s.name.to_lowercase() == "all")
    {
        return;
    }

    let mut read_signals = HashSet::new();
    let mut maybe_read_signals = HashSet::new();

    if let Some(sequential_block) = process_node
        .children(&mut process_node.walk())
        .find(|c| c.kind() == "sequential_block")
        && let Some(scope_tree) = ctx.scope_tree
    {
        let process_type = classify_process(sequential_block, ctx.text);

        // Extract signals based on process type
        match process_type {
            ProcessType::Combinatorial => {
                let mut extraction_ctx = SignalExtractionContext {
                    text: ctx.text,
                    read_signals: &mut read_signals,
                    maybe_read_signals: &mut maybe_read_signals,
                    global_map: ctx.global_map,
                    current_uri: ctx.current_uri,
                };
                extraction_ctx.extract(sequential_block, false)
            }
            ProcessType::Synchronous {
                clock_signals: ref clocks,
            } => read_signals.extend(clocks.clone()),
        };

        let has_wait = process_node
            .children(&mut process_node.walk())
            .any(|child| find_descendant(child, "wait_statement").is_some());

        // Filter to only signals/ports (exclude variables, constants, generics)
        if let Some(visible_decl) = ctx
            .analysis
            .collect_visible_declarations(scope_tree, node_to_range(process_node))
        {
            let is_signal_or_port = |name: &str| {
                if let Some(decl) = visible_decl
                    .iter()
                    .find(|n| n.name.to_lowercase() == name.to_lowercase())
                {
                    matches!(decl.decl_type, DeclType::Port(_) | DeclType::Signal)
                } else {
                    // Conservative: assume undeclared identifiers are package constants
                    false
                }
            };

            let read_signals: Vec<&Usage> = read_signals
                .iter()
                .filter(|s| is_signal_or_port(&s.name))
                .collect();

            // `maybe_read_signals`: args of unresolved procedure calls – direction unknown.
            // Used only to suppress false "unnecessary" warnings; never used for "missing".
            let maybe_read_names: HashSet<String> = maybe_read_signals
                .iter()
                .filter(|s| is_signal_or_port(&s.name))
                .map(|s| s.name.to_lowercase())
                .collect();

            // ── Gather shared context for code-action data ────────────────
            let sensitivity_spec_range = find_descendant(process_node, "sensitivity_specification")
                .map(|n| node_to_range(n));

            let process_kw_end = find_process_keyword_end(process_node);

            // Sort by source position so existing_signals always reflects the
            // left-to-right order in the file, regardless of HashSet iteration order.
            let mut existing_signals_ordered: Vec<(&str, (u32, u32))> = sensitivity_list
                .iter()
                .map(|u| {
                    (
                        u.name.as_str(),
                        (u.range.start.line, u.range.start.character),
                    )
                })
                .collect();
            existing_signals_ordered.sort_by_key(|(_, pos)| *pos);
            let existing_signals: Vec<String> = existing_signals_ordered
                .into_iter()
                .map(|(name, _)| name.to_string())
                .collect();

            // ── Collect all missing / all unnecessary up-front ────────────
            // (so every emitted diagnostic carries the full set for "fix all")

            let all_missing: Vec<String> = if !has_wait {
                read_signals
                    .iter()
                    .filter(|s| {
                        !sensitivity_list
                            .iter()
                            .any(|v| v.name.to_lowercase() == s.name.to_lowercase())
                    })
                    .map(|s| s.name.clone())
                    .collect()
            } else {
                vec![]
            };

            // Unnecessary only applies to combinatorial processes.
            let all_unnecessary: Vec<String> = match process_type {
                ProcessType::Combinatorial => sensitivity_list
                    .iter()
                    .filter(|s| {
                        let lc = s.name.to_lowercase();
                        let is_read = read_signals.iter().any(|r| r.name.to_lowercase() == lc);
                        let is_maybe = maybe_read_names.contains(&lc);
                        (!is_read && !is_maybe) || has_wait
                    })
                    .map(|s| s.name.clone())
                    .collect(),
                ProcessType::Synchronous { .. } => vec![],
            };

            // ── Emit diagnostics ──────────────────────────────────────────

            for signal in &all_missing {
                collectors.sensitivity.push(messages::missing_sensitivity(
                    &process_node,
                    MissingSensitivityData {
                        signal: signal.clone(),
                        sensitivity_spec_range,
                        process_kw_end,
                        existing_signals: existing_signals.clone(),
                        all_missing: all_missing.clone(),
                    },
                ));
            }

            // Skip unnecessary check for synchronous processes.
            if matches!(process_type, ProcessType::Synchronous { .. }) {
                return;
            }

            for signal_usage in sensitivity_list.iter().filter(|s| {
                all_unnecessary
                    .iter()
                    .any(|u| u.to_lowercase() == s.name.to_lowercase())
            }) {
                collectors
                    .sensitivity
                    .push(messages::unnecessary_sensitivity(
                        &signal_usage.range,
                        UnnecessarySensitivityData {
                            signal: signal_usage.name.clone(),
                            sensitivity_spec_range: sensitivity_spec_range.unwrap_or_default(),
                            existing_signals: existing_signals.clone(),
                            all_unnecessary: all_unnecessary.clone(),
                        },
                    ));
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

/// Returns the position immediately after the `process` keyword token.
///
/// Used by code actions to know where to insert a brand-new sensitivity list
/// when the process has none.  Falls back to the start of the process node if
/// the keyword token cannot be found (should not happen in valid VHDL).
fn find_process_keyword_end(process_node: Node) -> Position {
    for child in process_node.children(&mut process_node.walk()) {
        if child.kind() == "process" {
            return Position {
                line: child.end_position().row as u32,
                character: child.end_position().column as u32,
            };
        }
    }
    // Fallback: start of the process node.
    Position {
        line: process_node.start_position().row as u32,
        character: process_node.start_position().column as u32,
    }
}

/// Placeholder for future synchronous process validation.
///
/// Will validate that all clock signals are present in the sensitivity list
/// and that async rese signals (when detected) are also included.
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
    use crate::{
        analysis::{Analysis, ScopeTree},
        backend::test_utils::parse_text,
    };
    use tower_lsp::lsp_types::Url;

    fn check_sensitivity(code: &str) -> Vec<Diagnostic> {
        let tree = parse_text(code);
        let root = tree.root_node();
        let dummy_uri = Url::parse("file:///test.vhd").unwrap();

        // Build full analysis (includes entity scopes + arch scope trees)
        let analysis = crate::backend::syntax::parser::extract_document_symbols(code, root);

        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(dummy_uri.clone(), analysis.clone());

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
                                &analysis_map,
                                &dummy_uri,
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
        analysis: &Analysis,
        collectors: &mut DiagnosticCollectors,
        global_map: &AnalysisMap,
        current_uri: &Url,
    ) {
        if node.kind() == "process_statement" {
            let ctx = super::super::DiagnosticContext {
                text,
                scope_tree: Some(scope_tree),
                analysis,
                global_map,
                current_uri,
                ignored_patterns: &[],
            };
            check_process_sensitivity(node, &ctx, collectors);
        }

        for child in node.children(&mut node.walk()) {
            find_and_check_processes(
                child,
                text,
                scope_tree,
                analysis,
                collectors,
                global_map,
                current_uri,
            );
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
    fn test_process_with_all_keyword_skips_validation() {
        // VHDL-2008 'all' keyword should skip both missing and unnecessary checks
        let code = r#"
architecture rtl of test is
    signal a, b, result, unused : std_logic;
begin
    process(all)
    begin
        result <= a and b;
        -- 'unused' not referenced, but 'all' means no unnecessary warning
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert!(
            diags.is_empty(),
            "'all' keyword should skip all sensitivity validation"
        );
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
    fn test_function_output_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal result : std_logic_vector(31 downto 0);
    procedure std_extract(variable v_in: inout std_logic_vector; signal v_out : out std_logic_vector) is
    begin
        v_out <= v_in;
    end;
begin
    p_the_process: process is 
        variable var: std_logic_vector(31 downto 0);
    begin
        std_extract(var, result);
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 0, "Sensitivity list must be empty");
    }

    #[test]
    fn test_function_output_not_flagged_input_flagged() {
        let code = r#"
architecture rtl of test is
    signal result : std_logic_vector(31 downto 0);
    signal inp: std_logic_vector(31 downto 0);
    procedure std_extract(signal v_in: in std_logic_vector; signal v_out : out std_logic_vector) is
    begin
        v_out <= v_in;
    end;
begin
    p_the_process: process is 
    begin
        std_extract(inp, result);
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 1, "inp should be in sensitivity list");
    }

    #[test]
    fn test_signal_attribute_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal result : std_logic_vector(31 downto 0);
    signal inp: std_logic_vector(31 downto 0);
begin
    p_the_process: process is 
    begin
        inp <= result'length;
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 0, "No signal should be in sensitivity list");
    }
    #[test]
    fn test_signal_attribute_in_function_call_not_flagged() {
        let code = r#"
architecture rtl of test is
    signal result : std_logic_vector(31 downto 0);
    signal inp: std_logic_vector(31 downto 0);
begin
    p_the_process: process is 
    begin
        inp <= std_inc(result'length);
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 0, "No signal should be in sensitivity list");
    }

    #[test]
    fn test_signal_in_when_clause() {
        let code = r#"
architecture rtl of test is
    signal result : std_logic_vector(31 downto 0);
    signal inp: std_logic_vector(31 downto 0);
    signal toto: std_logic;
begin
    p_the_process: process(toto, inp) is 
    begin
        result <= inp when toto = '1' else (others => '0');
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 0, "No signal should be in sensitivity list");
    }
    #[test]
    fn test_array_access_should_be_flagged() {
        let code = r#"
architecture rtl of test is
    signal result : std_logic_vector(31 downto 0);
    signal inp: std_logic_vector(31 downto 0);
    signal toto: std_logic;
begin
    p_the_process: process is 
    begin
        result <= inp(0);
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 1, "inp(0) should be flagged");
    }
    #[test]
    fn test_empty_parenthesis_should_be_ok() {
        let code = r#"
architecture rtl of test is
    signal result : std_logic_vector(31 downto 0);
    signal inp: std_logic_vector(31 downto 0);
    signal toto: std_logic;
begin
    p_the_process: process() is
    begin
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(diags.len(), 0, "nothing should be flagged");
    }

    // Regression tests: signals used as in/inout procedure params must not be
    // flagged as "unnecessary" when they appear in the sensitivity list.

    #[test]
    fn test_in_param_in_sensitivity_not_unnecessary() {
        // sig_in is used as an `in` parameter → it IS read → should NOT be
        // reported as unnecessary in the sensitivity list.
        let code = r#"
architecture rtl of test is
    signal sig_in  : std_logic;
    signal sig_out : std_logic;
    procedure my_proc(signal v_in: in std_logic; signal v_out: out std_logic) is
    begin
        v_out <= v_in;
    end;
begin
    process(sig_in)
    begin
        my_proc(sig_in, sig_out);
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(
            diags.len(),
            0,
            "sig_in is read via 'in' param – must not be flagged as unnecessary; got: {:?}",
            diags
        );
    }

    #[test]
    fn test_unknown_proc_no_false_positives() {
        // Procedure declaration is not visible (external library etc.).
        // sig_in is in the sensitivity list and passed as first arg (may be `in`).
        // sig_out is NOT in the sensitivity list and passed as second arg (may be `out`).
        // Expected: zero diagnostics – we cannot know the directions so we emit nothing.
        let code = r#"
architecture rtl of test is
    signal sig_in  : std_logic;
    signal sig_out : std_logic;
begin
    process(sig_in)
    begin
        some_external_proc(sig_in, sig_out);
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        let unnecessary_sig_in = diags.iter().any(|d| {
            d.message.to_lowercase().contains("sig_in")
                && d.message.to_lowercase().contains("not needed")
        });
        let missing_sig_out = diags.iter().any(|d| {
            d.message.to_lowercase().contains("sig_out")
                && d.message.to_lowercase().contains("not in")
        });
        assert!(
            !unnecessary_sig_in,
            "sig_in should not be flagged as unnecessary (unknown proc); got: {:?}",
            diags
        );
        assert!(
            !missing_sig_out,
            "sig_out should not be flagged as missing (unknown proc); got: {:?}",
            diags
        );
    }

    #[test]
    fn test_inout_param_in_sensitivity_not_unnecessary() {
        // sig_inout is used as an `inout` parameter → it IS read → should NOT
        // be reported as unnecessary in the sensitivity list.
        let code = r#"
architecture rtl of test is
    signal sig_inout : std_logic;
    procedure my_proc(signal v_inout: inout std_logic) is
    begin
        v_inout <= '0';
    end;
begin
    process(sig_inout)
    begin
        my_proc(sig_inout);
    end process;
end architecture;
"#;
        let diags = check_sensitivity(code);
        assert_eq!(
            diags.len(),
            0,
            "sig_inout is read via 'inout' param – must not be flagged as unnecessary; got: {:?}",
            diags
        );
    }

    // Cross-file: procedure declared in a package, called from an architecture.
    // This is the realistic scenario the user reported.
    fn check_sensitivity_with_package(pkg_code: &str, arch_code: &str) -> Vec<Diagnostic> {
        let pkg_uri = Url::parse("file:///pkg.vhd").unwrap();
        let arch_uri = Url::parse("file:///arch.vhd").unwrap();

        let pkg_tree = parse_text(pkg_code);
        let pkg_analysis = crate::backend::syntax::parser::extract_document_symbols(
            pkg_code,
            pkg_tree.root_node(),
        );

        let arch_tree = parse_text(arch_code);
        let arch_root = arch_tree.root_node();
        let arch_analysis =
            crate::backend::syntax::parser::extract_document_symbols(arch_code, arch_root);

        let mut analysis_map = crate::backend::AnalysisMap::new();
        analysis_map.insert(pkg_uri.clone(), pkg_analysis);
        analysis_map.insert(arch_uri.clone(), arch_analysis.clone());

        crate::backend::features::diagnostics::collect_all_diagnostics(
            arch_root,
            &arch_analysis,
            arch_code,
            &analysis_map,
            &arch_uri,
            &crate::config::OxideConfig::default(),
        )
        .into_iter()
        .filter(|d| d.source.as_deref() == Some("oxide-hdl-sensitivity"))
        .collect()
    }

    #[test]
    fn test_cross_file_in_param_in_sensitivity_not_unnecessary() {
        // The procedure is declared in a separate package file.  This is the
        // real-world scenario the user hit: hover/goto work (lookup succeeds)
        // but the sensitivity checker was still flagging sig_in as unnecessary.
        let pkg_code = r#"
package my_pkg is
    procedure my_proc(signal v_in: in std_logic; signal v_out: out std_logic);
end package;
"#;
        let arch_code = r#"
use work.my_pkg.all;
architecture rtl of test is
    signal sig_in  : std_logic;
    signal sig_out : std_logic;
begin
    process(sig_in)
    begin
        my_proc(sig_in, sig_out);
    end process;
end architecture;
"#;
        let diags = check_sensitivity_with_package(pkg_code, arch_code);
        let unnecessary_sig_in = diags.iter().any(|d| {
            d.message.to_lowercase().contains("sig_in")
                && d.message.to_lowercase().contains("not needed")
        });
        assert!(
            !unnecessary_sig_in,
            "sig_in is an 'in' param from a package proc – must not be flagged as unnecessary; got: {:?}",
            diags
        );
    }

    #[test]
    fn test_cross_file_inout_param_in_sensitivity_not_unnecessary() {
        let pkg_code = r#"
package my_pkg is
    procedure my_proc(signal v_inout: inout std_logic);
end package;
"#;
        let arch_code = r#"
use work.my_pkg.all;
architecture rtl of test is
    signal sig_inout : std_logic;
begin
    process(sig_inout)
    begin
        my_proc(sig_inout);
    end process;
end architecture;
"#;
        let diags = check_sensitivity_with_package(pkg_code, arch_code);
        let unnecessary = diags.iter().any(|d| {
            d.message.to_lowercase().contains("sig_inout")
                && d.message.to_lowercase().contains("not needed")
        });
        assert!(
            !unnecessary,
            "sig_inout is an 'inout' param from a package proc – must not be flagged as unnecessary; got: {:?}",
            diags
        );
    }
}
