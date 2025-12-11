use crate::backend::Position;
use ropey::Rope;

/// Extracts the VHDL identifier at the given cursor position.
///
/// This function scans backwards and forwards from the provided `position` to find
/// the boundaries of the word. It considers alphanumeric characters and underscores
/// (`_`) as part of a valid identifier.
///
/// # Arguments
///
/// * `rope` - The immutable reference to the `Rope` holding the document text.
/// * `position` - The LSP `Position` (line and character index) where the cursor is located.
///
/// # Returns
///
/// * `Some(String)` - The extracted identifier string if one is found at the cursor.
/// * `None` - If the position is out of bounds or if the cursor is not on a valid identifier character.
///
/// # Examples
///
/// ```
/// use oxide_hdl::backend::syntax::utils::get_word_at_pos;
/// use ropey::Rope;
/// use tower_lsp::lsp_types::Position;
///
/// let text = Rope::from_str("signal my_sig : std_logic;");
///
/// // Cursor is on the 's' in 'sig' (index 11)
/// let pos = Position { line: 0, character: 11 };
///
/// assert_eq!(get_word_at_pos(&text, pos), Some("my_sig".to_string()));
/// ```
pub fn get_word_at_pos(rope: &Rope, position: Position) -> Option<String> {
    let line_idx = rope.try_line_to_char(position.line as usize).ok()?;
    let char_idx = line_idx + position.character as usize;

    if char_idx >= rope.len_chars() {
        return None;
    }

    // Find the start of the word
    let mut start = char_idx;
    while start > 0 {
        let c = rope.char(start - 1);
        if !c.is_alphanumeric() && c != '_' {
            break;
        }
        start -= 1;
    }

    let mut end = char_idx;
    while end < rope.len_chars() {
        let c = rope.char(end);
        if !c.is_alphanumeric() && c != '_' {
            break;
        }
        end += 1;
    }

    if start < end {
        Some(rope.slice(start..end).to_string())
    } else {
        None
    }
}
