use crate::backend::Position;
use ropey::Rope;

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
