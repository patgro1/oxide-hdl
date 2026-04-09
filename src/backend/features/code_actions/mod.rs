//! Code actions for VHDL process sensitivity lists.
//!
//! Provides quick-fix actions for diagnostics emitted by the sensitivity checker:
//! - **Missing signal**: add one or all missing signals, creating the list if absent.
//! - **Unnecessary signal**: remove one or all unnecessary signals.
//!
//! Each sensitivity diagnostic carries a [`MissingSensitivityData`] or
//! [`UnnecessarySensitivityData`] value in its `data` field (serialized as JSON).
//! The LSP client echoes those diagnostics back on `textDocument/codeAction`, so
//! the handler can reconstruct the full context without re-parsing.
//!
//! The edit strategy is **full-replace**: the entire `sensitivity_specification`
//! node `(a, b, c)` is replaced with a freshly formatted version.  Signals are
//! word-wrapped at [`MAX_LINE_LEN`] characters, aligning continuation lines with
//! the first signal in the list.

pub mod mark_debug;
pub use mark_debug::mark_debug_actions;

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, Position, Range, TextEdit,
    Url, WorkspaceEdit,
};

/// Target line length for the word-wrap formatter.
const MAX_LINE_LEN: usize = 120;

// ── Data types (serialized into Diagnostic.data) ──────────────────────────────

/// Payload stored in a **missing-signal** diagnostic's `data` field.
///
/// Carries everything the code-action handler needs to build the fix without
/// re-parsing the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingSensitivityData {
    /// The signal that triggered this individual diagnostic.
    pub signal: String,

    /// Range of the `sensitivity_specification` node `(a, b, c)`, or `None`
    /// when the process has no sensitivity list at all.
    pub sensitivity_spec_range: Option<Range>,

    /// Position immediately after the `process` keyword — used to insert a
    /// brand-new `(signal)` list when none exists.
    pub process_kw_end: Position,

    /// Signals already present in the sensitivity list (original casing).
    pub existing_signals: Vec<String>,

    /// All signals missing for this process, for the "fix all" action.
    pub all_missing: Vec<String>,
}

/// Payload stored in an **unnecessary-signal** diagnostic's `data` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnnecessarySensitivityData {
    /// The signal that triggered this individual diagnostic.
    pub signal: String,

    /// Range of the `sensitivity_specification` node `(a, b, c)`.
    pub sensitivity_spec_range: Range,

    /// Signals already present in the sensitivity list (original casing).
    pub existing_signals: Vec<String>,

    /// All signals that are unnecessary for this process, for the "fix all" action.
    pub all_unnecessary: Vec<String>,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Produces code actions for sensitivity-list diagnostics at the cursor.
///
/// Called from the LSP `code_action` handler. `params.context.diagnostics`
/// contains the diagnostics that overlap the requested range; the client echoes
/// them back including their `data` field.
///
/// # Arguments
///
/// * `params` - The LSP code-action request parameters
/// * `text` - Full source text of the document (unused for now, reserved for
///   future context-aware formatting)
pub fn sensitivity_actions(params: &CodeActionParams, _text: &str) -> Vec<CodeActionOrCommand> {
    let uri = &params.text_document.uri;
    let mut seen: HashSet<String> = HashSet::new();

    // Buckets — filled below, then concatenated in priority order at the end:
    //   1. combined fix  2. remove-all  3. add-all  4. individual fixes
    let mut combined: Vec<CodeActionOrCommand> = Vec::new();
    let mut remove_all: Vec<CodeActionOrCommand> = Vec::new();
    let mut add_all: Vec<CodeActionOrCommand> = Vec::new();
    let mut individual: Vec<CodeActionOrCommand> = Vec::new();

    // First pass: bucket diagnostics by kind.
    let mut missing_data: Vec<MissingSensitivityData> = Vec::new();
    let mut unnecessary_data: Vec<UnnecessarySensitivityData> = Vec::new();

    for diag in &params.context.diagnostics {
        if diag.source.as_deref() != Some("oxide-hdl-sensitivity") {
            continue;
        }
        let Some(raw) = diag.data.as_ref() else {
            continue;
        };
        if let Ok(data) = serde_json::from_value::<MissingSensitivityData>(raw.clone()) {
            missing_data.push(data);
        } else if let Ok(data) = serde_json::from_value::<UnnecessarySensitivityData>(raw.clone()) {
            unnecessary_data.push(data);
        }
    }

    // 1. Combined fix — only when both kinds are present.
    if !missing_data.is_empty()
        && !unnecessary_data.is_empty()
        && let Some(edit) = compute_fix_all(&missing_data[0], &unnecessary_data[0])
    {
        push_action(
            &mut combined,
            &mut seen,
            "Fix sensitivity list".to_string(),
            uri,
            edit,
        );
    }

    // 2. Remove-all unnecessary (when more than one unnecessary signal).
    for data in &unnecessary_data {
        if data.all_unnecessary.len() > 1
            && let Some(edit) = compute_remove_all_edit(data)
        {
            let title = "Remove all unnecessary signals from sensitivity list".to_string();
            push_action(&mut remove_all, &mut seen, title, uri, edit);
        }
    }

    // 3. Add-all missing (when more than one missing signal).
    for data in &missing_data {
        if data.all_missing.len() > 1
            && let Some(edit) = compute_add_all_edit(data)
        {
            let title = "Add all missing signals to sensitivity list".to_string();
            push_action(&mut add_all, &mut seen, title, uri, edit);
        }
    }

    // 4. Individual fixes.
    for data in &unnecessary_data {
        if let Some(edit) = compute_remove_signal_edit(&data.signal, data) {
            let title = format!("Remove '{}' from sensitivity list", data.signal);
            push_action(&mut individual, &mut seen, title, uri, edit);
        }
    }
    for data in &missing_data {
        if let Some(edit) = compute_add_signal_edit(&data.signal, data) {
            let title = format!("Add '{}' to sensitivity list", data.signal);
            push_action(&mut individual, &mut seen, title, uri, edit);
        }
    }

    combined
        .into_iter()
        .chain(remove_all)
        .chain(add_all)
        .chain(individual)
        .collect()
}

// ── Edit computation ──────────────────────────────────────────────────────────

/// Returns a [`TextEdit`] that adds `signal` to the sensitivity list.
///
/// If no list exists yet, inserts `(signal)` at `data.process_kw_end`.
fn compute_add_signal_edit(signal: &str, data: &MissingSensitivityData) -> Option<TextEdit> {
    let mut new_signals = data.existing_signals.clone();
    new_signals.push(signal.to_string());
    build_add_edit(data, new_signals)
}

/// Returns a [`TextEdit`] that adds all signals in `data.all_missing`.
fn compute_add_all_edit(data: &MissingSensitivityData) -> Option<TextEdit> {
    let mut new_signals = data.existing_signals.clone();
    for s in &data.all_missing {
        if !new_signals
            .iter()
            .any(|e| e.to_lowercase() == s.to_lowercase())
        {
            new_signals.push(s.clone());
        }
    }
    build_add_edit(data, new_signals)
}

/// Returns a [`TextEdit`] that removes `signal` from the sensitivity list.
fn compute_remove_signal_edit(signal: &str, data: &UnnecessarySensitivityData) -> Option<TextEdit> {
    let new_signals: Vec<String> = data
        .existing_signals
        .iter()
        .filter(|s| s.to_lowercase() != signal.to_lowercase())
        .cloned()
        .collect();
    Some(build_sensitivity_spec(
        data.sensitivity_spec_range,
        &new_signals,
    ))
}

/// Returns a [`TextEdit`] that removes all unnecessary signals and adds all
/// missing signals in one edit — the combined "Fix sensitivity list" action.
fn compute_fix_all(
    missing: &MissingSensitivityData,
    unnecessary: &UnnecessarySensitivityData,
) -> Option<TextEdit> {
    let unnecessary_lc: HashSet<String> = unnecessary
        .all_unnecessary
        .iter()
        .map(|s| s.to_lowercase())
        .collect();

    let mut new_signals: Vec<String> = unnecessary
        .existing_signals
        .iter()
        .filter(|s| !unnecessary_lc.contains(&s.to_lowercase()))
        .cloned()
        .collect();

    for s in &missing.all_missing {
        if !new_signals
            .iter()
            .any(|e| e.to_lowercase() == s.to_lowercase())
        {
            new_signals.push(s.clone());
        }
    }

    let range = unnecessary.sensitivity_spec_range;
    Some(build_sensitivity_spec(range, &new_signals))
}

/// Returns a [`TextEdit`] that removes all signals in `data.all_unnecessary`.
fn compute_remove_all_edit(data: &UnnecessarySensitivityData) -> Option<TextEdit> {
    let unnecessary_lc: HashSet<String> = data
        .all_unnecessary
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    let new_signals: Vec<String> = data
        .existing_signals
        .iter()
        .filter(|s| !unnecessary_lc.contains(&s.to_lowercase()))
        .cloned()
        .collect();
    Some(build_sensitivity_spec(
        data.sensitivity_spec_range,
        &new_signals,
    ))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Routes between "update existing list" and "create new list" for add edits.
fn build_add_edit(data: &MissingSensitivityData, new_signals: Vec<String>) -> Option<TextEdit> {
    Some(match data.sensitivity_spec_range {
        Some(range) => build_sensitivity_spec(range, &new_signals),
        None => {
            // No list exists yet: insert right after the `process` keyword.
            let col = data.process_kw_end.character as usize;
            TextEdit {
                range: Range {
                    start: data.process_kw_end,
                    end: data.process_kw_end,
                },
                new_text: word_wrap_signals(&new_signals, col),
            }
        }
    })
}

/// Builds a [`TextEdit`] that replaces `range` with a formatted sensitivity list.
///
/// The list is word-wrapped at [`MAX_LINE_LEN`], with continuation lines aligned
/// to the column immediately after the opening `(`.
fn build_sensitivity_spec(range: Range, signals: &[String]) -> TextEdit {
    let col = range.start.character as usize;
    TextEdit {
        range,
        new_text: word_wrap_signals(signals, col),
    }
}

/// Formats `signals` as a sensitivity specification, word-wrapping at [`MAX_LINE_LEN`].
///
/// Signals are appended to the current line until the next one would exceed the
/// threshold, at which point a newline is inserted and the continuation is
/// indented to align with the first signal (one column past the opening `(`).
///
/// A single signal is always placed on the current line even if it would exceed
/// the threshold, preventing infinite wrapping on very long names.
///
/// # Arguments
///
/// * `signals` - Ordered list of signal names
/// * `col` - Column of the opening `(` in the document
///
/// # Examples
///
/// ```text
/// // col = 7, signals fit on one line:
/// // "(clk, rst)"
///
/// // col = 7, signals overflow:
/// // "(sig_a, sig_b, sig_c,\n        sig_d, sig_e)"
/// ```
fn word_wrap_signals(signals: &[String], col: usize) -> String {
    if signals.is_empty() {
        return "()".to_string();
    }

    // Continuation lines align one column past the `(`.
    let indent = " ".repeat(col + 1);

    let mut result = String::from("(");
    // Tracks the length of the line currently being built, starting after `(`.
    let mut line_len = col + 1;

    for (i, signal) in signals.iter().enumerate() {
        if i == 0 {
            result.push_str(signal);
            line_len += signal.len();
        } else {
            // ", signal" would add sep_len + signal.len() characters.
            let sep = ", ";
            let addition = sep.len() + signal.len();
            if line_len + addition > MAX_LINE_LEN {
                // Wrap: close current line with comma, start new one.
                result.push_str(",\n");
                result.push_str(&indent);
                result.push_str(signal);
                line_len = col + 1 + signal.len();
            } else {
                result.push_str(sep);
                result.push_str(signal);
                line_len += addition;
            }
        }
    }

    result.push(')');
    result
}

/// Adds a code action to `actions` unless its `title` is already in `seen`.
fn push_action(
    actions: &mut Vec<CodeActionOrCommand>,
    seen: &mut HashSet<String>,
    title: String,
    uri: &Url,
    edit: TextEdit,
) {
    if !seen.insert(title.clone()) {
        return;
    }
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        is_preferred: Some(true),
        ..Default::default()
    }));
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

    fn pos(line: u32, ch: u32) -> Position {
        Position {
            line,
            character: ch,
        }
    }

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
        Range {
            start: pos(sl, sc),
            end: pos(el, ec),
        }
    }

    fn missing_data(
        signal: &str,
        existing: &[&str],
        all_missing: &[&str],
    ) -> MissingSensitivityData {
        MissingSensitivityData {
            signal: signal.to_string(),
            // `(` at col 11 — short lists fit on one line.
            sensitivity_spec_range: Some(range(3, 11, 3, 20)),
            process_kw_end: pos(3, 11),
            existing_signals: existing.iter().map(|s| s.to_string()).collect(),
            all_missing: all_missing.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn unnecessary_data(
        signal: &str,
        existing: &[&str],
        all_unnecessary: &[&str],
    ) -> UnnecessarySensitivityData {
        UnnecessarySensitivityData {
            signal: signal.to_string(),
            // `(` at col 11 — short lists fit on one line.
            sensitivity_spec_range: range(3, 11, 3, 25),
            existing_signals: existing.iter().map(|s| s.to_string()).collect(),
            all_unnecessary: all_unnecessary.iter().map(|s| s.to_string()).collect(),
        }
    }

    // ── word_wrap_signals ─────────────────────────────────────────────────────

    #[test]
    fn short_list_stays_on_one_line() {
        let signals = ["a", "b", "sel"].map(str::to_string).to_vec();
        assert_eq!(word_wrap_signals(&signals, 11), "(a, b, sel)");
    }

    #[test]
    fn empty_list_produces_empty_parens() {
        assert_eq!(word_wrap_signals(&[], 11), "()");
    }

    #[test]
    fn long_list_wraps_at_threshold() {
        // col 7 — `process(` at the start of an indented line (4 spaces + `process`).
        // Build a list that overflows 80 chars on the first line.
        let signals: Vec<String> = (0..10).map(|i| format!("signal_{i:02}")).collect();
        let result = word_wrap_signals(&signals, 7);
        // Every line should be at most MAX_LINE_LEN characters.
        for line in result.lines() {
            assert!(
                line.len() <= MAX_LINE_LEN,
                "line too long ({} chars): {line:?}",
                line.len()
            );
        }
        // All signals must appear in the output.
        for s in &signals {
            assert!(result.contains(s.as_str()));
        }
    }

    #[test]
    fn wrap_aligns_continuation_to_col_after_paren() {
        // col = 4: continuation indent must be 5 spaces.
        let signals = vec![
            "signal_aaaa".to_string(),
            "signal_bbbb".to_string(),
            "signal_cccc".to_string(),
            "signal_dddd".to_string(),
            "signal_eeee".to_string(),
            "signal_ffff".to_string(),
            "signal_gggg".to_string(),
        ];
        let result = word_wrap_signals(&signals, 4);
        let lines: Vec<&str> = result.lines().collect();
        // All continuation lines must start with exactly 5 spaces.
        for line in &lines[1..] {
            assert!(
                line.starts_with("     "),
                "continuation line has wrong indent: {line:?}"
            );
        }
    }

    #[test]
    fn single_very_long_signal_is_never_skipped() {
        let signals = vec!["a_signal_name_so_long_it_exceeds_the_entire_line_by_itself_and_then_some_more_characters_added".to_string()];
        let result = word_wrap_signals(&signals, 0);
        assert!(result.contains("a_signal_name_so_long"));
    }

    // ── add signal ────────────────────────────────────────────────────────────

    #[test]
    fn add_one_signal_to_existing_list() {
        let data = missing_data("sel", &["a", "b"], &["sel"]);
        let edit = compute_add_signal_edit("sel", &data).unwrap();
        assert_eq!(edit.new_text, "(a, b, sel)");
        assert_eq!(edit.range, data.sensitivity_spec_range.unwrap());
    }

    #[test]
    fn add_signal_creates_list_when_none_exists() {
        let data = MissingSensitivityData {
            signal: "clk".to_string(),
            sensitivity_spec_range: None,
            process_kw_end: pos(4, 11),
            existing_signals: vec![],
            all_missing: vec!["clk".to_string()],
        };
        let edit = compute_add_signal_edit("clk", &data).unwrap();
        assert_eq!(edit.new_text, "(clk)");
        assert_eq!(edit.range.start, pos(4, 11));
        assert_eq!(edit.range.end, pos(4, 11));
    }

    #[test]
    fn add_all_missing_signals() {
        let data = missing_data("b", &["a"], &["b", "c", "sel"]);
        let edit = compute_add_all_edit(&data).unwrap();
        assert_eq!(edit.new_text, "(a, b, c, sel)");
    }

    #[test]
    fn add_all_does_not_duplicate_existing() {
        let data = missing_data("b", &["a"], &["a", "b"]);
        let edit = compute_add_all_edit(&data).unwrap();
        assert_eq!(edit.new_text, "(a, b)");
    }

    // ── remove signal ─────────────────────────────────────────────────────────

    #[test]
    fn remove_one_signal_from_list() {
        let data = unnecessary_data("unused", &["a", "b", "unused"], &["unused"]);
        let edit = compute_remove_signal_edit("unused", &data).unwrap();
        assert_eq!(edit.new_text, "(a, b)");
        assert_eq!(edit.range, data.sensitivity_spec_range);
    }

    #[test]
    fn remove_only_signal_leaves_empty_list() {
        let data = unnecessary_data("dead", &["dead"], &["dead"]);
        let edit = compute_remove_signal_edit("dead", &data).unwrap();
        assert_eq!(edit.new_text, "()");
    }

    #[test]
    fn remove_all_unnecessary_signals() {
        let data = unnecessary_data("extra1", &["a", "extra1", "extra2"], &["extra1", "extra2"]);
        let edit = compute_remove_all_edit(&data).unwrap();
        assert_eq!(edit.new_text, "(a)");
    }

    // ── combined fix ─────────────────────────────────────────────────────────

    #[test]
    fn fix_all_removes_unnecessary_and_adds_missing() {
        let missing = MissingSensitivityData {
            signal: "needed".to_string(),
            sensitivity_spec_range: Some(range(3, 11, 3, 30)),
            process_kw_end: pos(3, 11),
            existing_signals: vec!["unnecessary".to_string(), "kept".to_string()],
            all_missing: vec!["needed".to_string()],
        };
        let unnecessary = UnnecessarySensitivityData {
            signal: "unnecessary".to_string(),
            sensitivity_spec_range: range(3, 11, 3, 30),
            existing_signals: vec!["unnecessary".to_string(), "kept".to_string()],
            all_unnecessary: vec!["unnecessary".to_string()],
        };
        let edit = compute_fix_all(&missing, &unnecessary).unwrap();
        assert_eq!(edit.new_text, "(kept, needed)");
    }

    // ── deduplication ─────────────────────────────────────────────────────────

    #[test]
    fn fix_all_action_appears_only_once_for_multiple_missing_diags() {
        use tower_lsp::lsp_types::{
            CodeActionContext, CodeActionParams, PartialResultParams, TextDocumentIdentifier,
            WorkDoneProgressParams,
        };

        let uri = Url::parse("file:///test.vhd").unwrap();

        let make_diag = |signal: &str| {
            let data = MissingSensitivityData {
                signal: signal.to_string(),
                sensitivity_spec_range: Some(range(3, 11, 3, 14)),
                process_kw_end: pos(3, 11),
                existing_signals: vec![],
                all_missing: vec!["b".to_string(), "c".to_string()],
            };
            tower_lsp::lsp_types::Diagnostic {
                range: range(3, 0, 6, 0),
                source: Some("oxide-hdl-sensitivity".to_string()),
                data: Some(serde_json::to_value(data).unwrap()),
                ..Default::default()
            }
        };

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: range(3, 0, 6, 0),
            context: CodeActionContext {
                diagnostics: vec![make_diag("b"), make_diag("c")],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let actions = sensitivity_actions(&params, "");
        let fix_all_count = actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    CodeActionOrCommand::CodeAction(ca)
                    if ca.title.contains("all missing")
                )
            })
            .count();

        assert_eq!(fix_all_count, 1, "\"fix all\" should appear exactly once");
    }
}
