//! Code action: add `mark_debug` attribute to VHDL signals and ports.
//!
//! Produces a code action when the cursor is on an eligible symbol (Signal or Port).
//! Ineligible: variables, constants, functions, generics, types, LHS of port maps.
//!
//! Two edits are emitted per action:
//!   1. `attribute mark_debug : string;` at the arch declarative region (omitted if already present).
//!   2. `attribute mark_debug of <name> : signal is "true";` after the last declaration
//!      in the signal's own declarative scope.

use std::collections::HashMap;

use ropey::Rope;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, Position, Range, TextEdit,
    Url, WorkspaceEdit,
};

use crate::{
    analysis::{DeclType, Declaration, ScopeKind, ScopeTree},
    backend::{
        AnalysisMap,
        features::lookup::{ResolvedItem, lookup_symbol},
        syntax::utils::get_word_at_pos,
    },
    utils::position_in_range,
};

// ── Public entry point ────────────────────────────────────────────────────────

/// Produces mark_debug code actions for the cursor position or selection.
///
/// Single cursor: resolves the word at cursor, checks eligibility, emits one action.
/// Multi-line selection: collect eligible signals in the selected range and offer a batch action.
pub fn mark_debug_actions(
    params: &CodeActionParams,
    rope: &Rope,
    analysis_map: &AnalysisMap,
) -> Vec<CodeActionOrCommand> {
    let uri = &params.text_document.uri;
    let mut actions = Vec::new();

    let is_multi = params.range.start.line != params.range.end.line
        || params.range.start.character != params.range.end.character;

    if !is_multi {
        single_cursor_actions(params, rope, analysis_map, uri, &mut actions);
    } else {
        multi_line_actions(params, rope, analysis_map, uri, &mut actions);
    }

    actions
}

// ── Single cursor ─────────────────────────────────────────────────────────────

fn single_cursor_actions(
    params: &CodeActionParams,
    rope: &Rope,
    analysis_map: &AnalysisMap,
    uri: &Url,
    actions: &mut Vec<CodeActionOrCommand>,
) {
    let pos = params.range.start;

    // 1. Word at cursor.
    let Some(word) = get_word_at_pos(rope, pos) else { return };

    // 2. LHS-of-port-map guard: if the word is immediately followed by `=>`
    //    in the source text, it is the formal (component-side) port — skip it.
    if is_port_map_lhs(rope, pos, &word) {
        return;
    }

    // 3. Resolve symbol.
    let results = lookup_symbol(&word, uri, analysis_map, &pos, false);
    let Some(result) = results.into_iter().next() else { return };

    let decl = match result.item {
        ResolvedItem::Declaration(d) => d,
        _ => return,
    };

    // 4. Eligibility: Signal or Port only.
    if !matches!(decl.decl_type, DeclType::Signal | DeclType::Port(_)) {
        return;
    }

    // 5. Already marked? Check the scope containing the declaration.
    let Some(analysis) = analysis_map.get(uri) else { return };
    if let Some(scope) = find_scope_containing_pos(&decl.range.start, &analysis.scope_trees) {
        if scope.is_attr_applied("mark_debug", &decl.name) {
            return;
        }
    }

    // 6. Build edits and emit action.
    let kind_label = match decl.decl_type {
        DeclType::Port(_) => "port",
        _ => "signal",
    };
    let title = format!("Add mark_debug to {} '{}'", kind_label, decl.name);
    let edits = build_edits_for_signal(&decl, analysis, rope);
    if !edits.is_empty() {
        push_action(actions, title, uri, edits);
    }
}

// ── Edit construction ─────────────────────────────────────────────────────────

/// Builds the TextEdit(s) needed to add mark_debug for a single signal/port.
///
/// Returns 1 edit (spec only) if the arch already declares `attribute mark_debug : string;`,
/// or 2 edits (arch declaration + signal spec) if it does not.
pub(crate) fn build_edits_for_signal(
    decl: &Declaration,
    analysis: &crate::analysis::Analysis,
    rope: &Rope,
) -> Vec<TextEdit> {
    // Scope that owns the signal — used for spec insertion point and indentation.
    let Some(spec_scope) = find_scope_containing_pos(&decl.range.start, &analysis.scope_trees)
    else {
        return vec![];
    };

    let spec_indent = indent_for_scope(spec_scope, rope);
    let Some(spec_pos) = insertion_point(spec_scope) else {
        return vec![];
    };
    let spec_text = format!(
        "{}attribute mark_debug of {} : signal is \"true\";\n",
        spec_indent, decl.name
    );
    let spec_edit = TextEdit {
        range: Range { start: spec_pos, end: spec_pos },
        new_text: spec_text,
    };

    // Arch-level scope — used for the declaration edit.
    let arch = find_arch_scope_for_pos(&decl.range.start, &analysis.scope_trees);
    let has_decl = arch.map_or(false, |a| {
        a.declarations.iter().any(|d| {
            matches!(d.decl_type, DeclType::Attribute) && d.name.eq_ignore_ascii_case("mark_debug")
        })
    });

    if has_decl {
        return vec![spec_edit];
    }

    // Build the arch-level declaration edit.
    let Some(arch_scope) = arch else {
        return vec![spec_edit];
    };
    let decl_indent = indent_for_scope(arch_scope, rope);
    let Some(decl_pos) = insertion_point(arch_scope) else {
        return vec![spec_edit];
    };
    let decl_text = format!("{}attribute mark_debug : string;\n", decl_indent);
    let decl_edit = TextEdit {
        range: Range { start: decl_pos, end: decl_pos },
        new_text: decl_text,
    };

    // Two edits at the same insertion point: put spec first so when the LSP
    // client applies them bottom-up they end up in the right order.
    // If they are at different positions (signal in inner block), order by line.
    if decl_pos.line == spec_pos.line {
        vec![spec_edit, decl_edit]
    } else {
        // Different scopes: emit arch decl first (lower line number), then spec.
        if decl_pos.line < spec_pos.line {
            vec![decl_edit, spec_edit]
        } else {
            vec![spec_edit, decl_edit]
        }
    }
}

// ── Scope helpers ─────────────────────────────────────────────────────────────

/// Finds the innermost scope in `scope_trees` that contains `pos`.
pub(crate) fn find_scope_containing_pos<'a>(
    pos: &Position,
    scope_trees: &'a [ScopeTree],
) -> Option<&'a ScopeTree> {
    for root in scope_trees {
        if position_in_range(*pos, root.range) {
            return Some(root.find_innermost_scope(pos));
        }
    }
    None
}

/// Finds the architecture-level scope that contains `pos`.
fn find_arch_scope_for_pos<'a>(
    pos: &Position,
    scope_trees: &'a [ScopeTree],
) -> Option<&'a ScopeTree> {
    scope_trees.iter().find(|s| {
        s.kind == ScopeKind::Architecture && position_in_range(*pos, s.range)
    })
}

/// Returns the insertion point: the start of the line after the last declaration
/// in `scope`. Returns `None` if the scope has no declarations.
pub(crate) fn insertion_point(scope: &ScopeTree) -> Option<Position> {
    let last = scope.declarations.iter().max_by_key(|d| d.range.end.line)?;
    Some(Position { line: last.range.end.line + 1, character: 0 })
}

/// Detects the indentation column used by declarations in `scope`.
/// Falls back to 4 spaces if no declarations are present.
pub(crate) fn indent_for_scope(scope: &ScopeTree, rope: &Rope) -> String {
    if let Some(first) = scope.declarations.first() {
        let line_idx = first.range.start.line as usize;
        if let Ok(line_char_idx) = rope.try_line_to_char(line_idx) {
            let chars_on_line: String = rope.chars_at(line_char_idx).take(80).collect();
            let spaces = chars_on_line.chars().take_while(|c| *c == ' ').count();
            return " ".repeat(spaces);
        }
    }
    "    ".to_string()
}

// ── Multi-line selection ──────────────────────────────────────────────────────

fn multi_line_actions(
    params: &CodeActionParams,
    rope: &Rope,
    analysis_map: &AnalysisMap,
    uri: &Url,
    actions: &mut Vec<CodeActionOrCommand>,
) {
    let sel = params.range;
    let Some(analysis) = analysis_map.get(uri) else { return };

    let mut candidates: Vec<(String, Position)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Source A — signal/port declarations whose range falls within the selection
    for root in &analysis.scope_trees {
        collect_decls_in_range(root, sel, &mut candidates, &mut seen);
    }

    // Source B — usages within the selection, resolved to Signal/Port declarations
    for root in &analysis.scope_trees {
        collect_usages_in_range(root, sel, uri, analysis_map, &mut candidates, &mut seen);
    }

    // Filter out already-marked signals
    candidates.retain(|(name, pos)| {
        let scope = find_scope_containing_pos(pos, &analysis.scope_trees);
        !scope.map_or(false, |s| s.is_attr_applied("mark_debug", name))
    });

    if candidates.is_empty() {
        return;
    }

    let count = candidates.len();
    let title = format!("Add mark_debug to all signals in selection ({count} signals)");

    let mut all_edits: Vec<TextEdit> = Vec::new();

    // Arch decl edit (at most once — check all candidates' arch scopes)
    let needs_decl = candidates.iter().any(|(_, pos)| {
        find_arch_scope_for_pos(pos, &analysis.scope_trees).map_or(true, |arch| {
            !arch.declarations.iter().any(|d| {
                matches!(d.decl_type, DeclType::Attribute)
                    && d.name.eq_ignore_ascii_case("mark_debug")
            })
        })
    });

    if needs_decl {
        if let Some((_, pos)) = candidates.first() {
            if let Some(arch) = find_arch_scope_for_pos(pos, &analysis.scope_trees) {
                let decl_indent = indent_for_scope(arch, rope);
                if let Some(decl_pos) = insertion_point(arch) {
                    all_edits.push(TextEdit {
                        range: Range { start: decl_pos, end: decl_pos },
                        new_text: format!("{}attribute mark_debug : string;\n", decl_indent),
                    });
                }
            }
        }
    }

    // Group specs by containing scope (keyed by scope range start line)
    let mut scope_specs: std::collections::HashMap<u32, (Position, String, Vec<String>)> =
        std::collections::HashMap::new();
    for (name, pos) in &candidates {
        if let Some(scope) = find_scope_containing_pos(pos, &analysis.scope_trees) {
            let key = scope.range.start.line;
            let entry = scope_specs.entry(key).or_insert_with(|| {
                let insert_pos = insertion_point(scope)
                    .unwrap_or(Position { line: scope.range.end.line, character: 0 });
                let indent = indent_for_scope(scope, rope);
                (insert_pos, indent, Vec::new())
            });
            entry.2.push(name.clone());
        }
    }

    for (_, (insert_pos, indent, names)) in scope_specs {
        let spec_text: String = names
            .iter()
            .map(|n| format!("{}attribute mark_debug of {} : signal is \"true\";\n", indent, n))
            .collect();
        all_edits.push(TextEdit {
            range: Range { start: insert_pos, end: insert_pos },
            new_text: spec_text,
        });
    }

    if !all_edits.is_empty() {
        push_action(actions, title, uri, all_edits);
    }
}

fn collect_decls_in_range(
    scope: &ScopeTree,
    sel: Range,
    candidates: &mut Vec<(String, Position)>,
    seen: &mut std::collections::HashSet<String>,
) {
    for decl in &scope.declarations {
        if matches!(decl.decl_type, DeclType::Signal | DeclType::Port(_))
            && range_contains_pos(sel, decl.range.start)
            && seen.insert(decl.name.to_lowercase())
        {
            candidates.push((decl.name.clone(), decl.range.start));
        }
    }
    for child in &scope.children {
        collect_decls_in_range(child, sel, candidates, seen);
    }
}

fn collect_usages_in_range(
    scope: &ScopeTree,
    sel: Range,
    uri: &Url,
    analysis_map: &AnalysisMap,
    candidates: &mut Vec<(String, Position)>,
    seen: &mut std::collections::HashSet<String>,
) {
    for usage in &scope.local_usage {
        if !range_contains_pos(sel, usage.range.start) {
            continue;
        }
        let name_lc = usage.name.to_lowercase();
        if seen.contains(&name_lc) {
            continue;
        }
        let results = lookup_symbol(&usage.name, uri, analysis_map, &usage.range.start, false);
        if let Some(result) = results.into_iter().next() {
            if let ResolvedItem::Declaration(decl) = result.item {
                if matches!(decl.decl_type, DeclType::Signal | DeclType::Port(_)) {
                    seen.insert(name_lc);
                    candidates.push((decl.name.clone(), decl.range.start));
                }
            }
        }
    }
    for child in &scope.children {
        collect_usages_in_range(child, sel, uri, analysis_map, candidates, seen);
    }
}

fn range_contains_pos(range: Range, pos: Position) -> bool {
    (pos.line > range.start.line
        || (pos.line == range.start.line && pos.character >= range.start.character))
        && (pos.line < range.end.line
            || (pos.line == range.end.line && pos.character < range.end.character))
}

// ── Port-map LHS detection ────────────────────────────────────────────────────

/// Returns true if the word at `pos` is the formal (LHS) side of a port map association.
///
/// Detection: after the word ends, if the first non-whitespace characters are `=>`,
/// this is a port map formal — do not offer mark_debug for it.
fn is_port_map_lhs(rope: &Rope, pos: Position, word: &str) -> bool {
    let line_idx = pos.line as usize;
    let Ok(line_char_idx) = rope.try_line_to_char(line_idx) else { return false };
    // Walk back from cursor to find the true word start (cursor may be anywhere inside the word).
    let cursor_abs = line_char_idx + pos.character as usize;
    let mut start_abs = cursor_abs;
    while start_abs > line_char_idx {
        let c = rope.char(start_abs - 1);
        if !c.is_alphanumeric() && c != '_' {
            break;
        }
        start_abs -= 1;
    }
    let word_end_abs = start_abs + word.len();
    if word_end_abs > rope.len_chars() {
        return false;
    }
    let rest: String = rope.chars_at(word_end_abs).take(20).collect();
    rest.trim_start().starts_with("=>")
}

// ── Action builder ────────────────────────────────────────────────────────────

pub(crate) fn push_action(
    actions: &mut Vec<CodeActionOrCommand>,
    title: String,
    uri: &Url,
    edits: Vec<TextEdit>,
) {
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);
    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        is_preferred: Some(false),
        ..Default::default()
    }));
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{
        syntax::parser::extract_document_symbols,
        test_utils::parse_text,
    };
    use tower_lsp::lsp_types::{
        CodeActionContext, CodeActionParams, PartialResultParams, TextDocumentIdentifier,
        WorkDoneProgressParams,
    };

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn parse_and_build(code: &str, uri: &Url) -> (Rope, AnalysisMap) {
        let rope = Rope::from_str(code);
        let tree = parse_text(code); // parse_text acquires SHARED_PARSER_LOCK internally
        let root = tree.root_node();
        let analysis = extract_document_symbols(code, root);
        let mut map: AnalysisMap = HashMap::new();
        map.insert(uri.clone(), analysis);
        (rope, map)
    }

    fn make_cursor_params(uri: &Url, line: u32, col: u32) -> CodeActionParams {
        let pos = Position { line, character: col };
        CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range { start: pos, end: pos },
            context: CodeActionContext { diagnostics: vec![], only: None, trigger_kind: None },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        }
    }

    fn make_range_params(uri: &Url, start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> CodeActionParams {
        CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: Position { line: start_line, character: start_col },
                end: Position { line: end_line, character: end_col },
            },
            context: CodeActionContext { diagnostics: vec![], only: None, trigger_kind: None },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        }
    }

    // ── Single cursor tests ───────────────────────────────────────────────────

    #[test]
    fn signal_not_marked_offers_action() {
        let code = "entity foo is end entity;\narchitecture rtl of foo is\n    signal my_sig : std_logic;\nbegin\nend architecture;\n";
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        // "my_sig" is at line 2, starts at col 11
        let params = make_cursor_params(&uri, 2, 11);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert_eq!(actions.len(), 1, "expected 1 action");
        let title = match &actions[0] {
            CodeActionOrCommand::CodeAction(ca) => &ca.title,
            _ => panic!("expected CodeAction"),
        };
        assert!(title.contains("my_sig"), "title should mention signal name: {title}");
        assert!(title.to_lowercase().contains("mark_debug"), "title should mention mark_debug: {title}");
    }

    #[test]
    fn signal_already_marked_no_action() {
        let code = "entity foo is end entity;\narchitecture rtl of foo is\n    signal my_sig : std_logic;\n    attribute mark_debug : string;\n    attribute mark_debug of my_sig : signal is \"true\";\nbegin\nend architecture;\n";
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        let params = make_cursor_params(&uri, 2, 11);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert!(actions.is_empty(), "already-marked signal should yield no action");
    }

    #[test]
    fn constant_no_action() {
        let code = "entity foo is end entity;\narchitecture rtl of foo is\n    constant MY_C : integer := 0;\nbegin\nend architecture;\n";
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        let params = make_cursor_params(&uri, 2, 13);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert!(actions.is_empty(), "constant should yield no action");
    }

    #[test]
    fn arch_decl_present_single_edit() {
        // attribute mark_debug : string; already declared — only one edit (spec)
        let code = "entity foo is end entity;\narchitecture rtl of foo is\n    signal my_sig : std_logic;\n    attribute mark_debug : string;\nbegin\nend architecture;\n";
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        let params = make_cursor_params(&uri, 2, 11);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert_eq!(actions.len(), 1);
        if let CodeActionOrCommand::CodeAction(ca) = &actions[0] {
            let edits = ca.edit.as_ref().unwrap().changes.as_ref().unwrap().get(&uri).unwrap();
            assert_eq!(edits.len(), 1, "only the spec edit expected");
            assert!(edits[0].new_text.contains("of my_sig"), "edit should contain signal name");
        }
    }

    #[test]
    fn arch_decl_absent_two_edits() {
        let code = "entity foo is end entity;\narchitecture rtl of foo is\n    signal my_sig : std_logic;\nbegin\nend architecture;\n";
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        let params = make_cursor_params(&uri, 2, 11);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert_eq!(actions.len(), 1);
        if let CodeActionOrCommand::CodeAction(ca) = &actions[0] {
            let edits = ca.edit.as_ref().unwrap().changes.as_ref().unwrap().get(&uri).unwrap();
            assert_eq!(edits.len(), 2, "expected decl edit + spec edit");
            let combined: String = edits.iter().map(|e| e.new_text.as_str()).collect();
            assert!(combined.contains("attribute mark_debug : string"), "should include declaration");
            assert!(combined.contains("of my_sig"), "should include specification");
        }
    }

    // ── Multi-line tests ──────────────────────────────────────────────────────

    #[test]
    fn multi_line_declaration_block_offers_batch_action() {
        // Code has 2 signals + 1 constant. Select lines 2-4 (0-indexed), expect 1 action mentioning 2 signals.
        // line 0: entity foo is end entity;
        // line 1: architecture rtl of foo is
        // line 2:     signal sig_a : std_logic;
        // line 3:     signal sig_b : std_logic_vector(7 downto 0);
        // line 4:     constant C : integer := 0;
        let code = "entity foo is end entity;\narchitecture rtl of foo is\n    signal sig_a : std_logic;\n    signal sig_b : std_logic_vector(7 downto 0);\n    constant C : integer := 0;\nbegin\nend architecture;\n";
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        // Select lines 2-4 (both signals and the constant line)
        let params = make_range_params(&uri, 2, 0, 4, 0);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert_eq!(actions.len(), 1, "expected 1 batch action");
        let title = match &actions[0] {
            CodeActionOrCommand::CodeAction(ca) => &ca.title,
            _ => panic!("expected CodeAction"),
        };
        assert!(title.contains("2"), "expected 2 signals in title: {title}");
    }

    #[test]
    fn multi_line_process_offers_batch_action_from_usages() {
        // Process uses clk, data, valid — all declared in arch. Select the process body.
        let code = "entity foo is end entity;\narchitecture rtl of foo is\n    signal clk   : std_logic;\n    signal data  : std_logic_vector(7 downto 0);\n    signal valid : std_logic;\nbegin\n    process(clk)\n    begin\n        if clk = '1' then\n            valid <= data(0);\n        end if;\n    end process;\nend architecture;\n";
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        // Select the process: lines 6-12
        let params = make_range_params(&uri, 6, 0, 12, 0);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert_eq!(actions.len(), 1, "expected 1 batch action from usages");
        let title = match &actions[0] {
            CodeActionOrCommand::CodeAction(ca) => &ca.title,
            _ => panic!("expected CodeAction"),
        };
        assert!(title.to_lowercase().contains("mark_debug"), "title: {title}");
    }

    #[test]
    fn multi_line_all_marked_no_action() {
        // Both signals already have mark_debug — no action expected.
        let code = "entity foo is end entity;\narchitecture rtl of foo is\n    signal sig_a : std_logic;\n    signal sig_b : std_logic;\n    attribute mark_debug : string;\n    attribute mark_debug of sig_a, sig_b : signal is \"true\";\nbegin\nend architecture;\n";
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        // Select lines 2-4 (both signal declarations)
        let params = make_range_params(&uri, 2, 0, 4, 0);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert!(actions.is_empty(), "all signals already marked — no action expected");
    }

    #[test]
    fn port_map_lhs_no_action() {
        // "clk" on the LHS of a port map (formal port) should not offer mark_debug.
        // The signal on the RHS (my_clk) is fine but we're not testing that here.
        let code = concat!(
            "entity child is port (clk : in std_logic); end entity;\n",
            "entity foo is end entity;\n",
            "architecture rtl of foo is\n",
            "    signal my_clk : std_logic;\n",
            "    component child is port (clk : in std_logic); end component;\n",
            "begin\n",
            "    U1 : child port map (clk => my_clk);\n",
            "end architecture;\n",
        );
        let uri = Url::parse("file:///test.vhd").unwrap();
        let (rope, map) = parse_and_build(code, &uri);
        // Line 6: "    U1 : child port map (clk => my_clk);"
        // "clk" starts at col 25
        let params = make_cursor_params(&uri, 6, 25);
        let actions = mark_debug_actions(&params, &rope, &map);
        assert!(actions.is_empty(), "LHS of port map should not offer mark_debug, got: {actions:?}");
    }
}
