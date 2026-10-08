use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

/// A small editable text buffer (single or multi-line). `cursor` is a byte index.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub text: String,
    pub cursor: usize,
    pub multiline: bool,
}

impl TextInput {
    pub fn new(multiline: bool) -> Self {
        Self {
            multiline,
            ..Default::default()
        }
    }

    pub fn with_text(text: &str, multiline: bool) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
            multiline,
        }
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
    }

    pub fn insert_str(&mut self, s: &str) {
        let s = if self.multiline {
            s.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            s.replace(['\n', '\r'], " ")
        };
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i]
            .char_indices()
            .next_back()
            .map(|(j, _)| j)
            .unwrap_or(0)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..]
            .chars()
            .next()
            .map(|c| i + c.len_utf8())
            .unwrap_or(i)
    }

    fn word_start(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(|c: char| c.is_whitespace());
        trimmed
            .rfind(|c: char| c.is_whitespace())
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len())
    }

    /// Returns true if the key was consumed.
    pub fn handle_key(&mut self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Enter
                if self.multiline && (alt || k.modifiers.contains(KeyModifiers::SHIFT)) =>
            {
                self.insert_str("\n")
            }
            KeyCode::Char('j') if ctrl && self.multiline => self.insert_str("\n"),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace if alt || ctrl => {
                let s = self.word_start();
                self.text.replace_range(s..self.cursor, "");
                self.cursor = s;
            }
            KeyCode::Char('w') if ctrl => {
                let s = self.word_start();
                self.text.replace_range(s..self.cursor, "");
                self.cursor = s;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let p = self.prev(self.cursor);
                    self.text.replace_range(p..self.cursor, "");
                    self.cursor = p;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.text.len() {
                    let n = self.next(self.cursor);
                    self.text.replace_range(self.cursor..n, "");
                }
            }
            KeyCode::Char('u') if ctrl => {
                let s = self.line_start();
                self.text.replace_range(s..self.cursor, "");
                self.cursor = s;
            }
            KeyCode::Char('k') if ctrl => {
                let e = self.line_end();
                self.text.replace_range(self.cursor..e, "");
            }
            KeyCode::Left if alt || ctrl => self.cursor = self.word_start(),
            KeyCode::Char('b') if alt => self.cursor = self.word_start(),
            KeyCode::Right if alt || ctrl => {
                let rest = &self.text[self.cursor..];
                let skip_ws = rest.len() - rest.trim_start().len();
                let word = rest[skip_ws..]
                    .find(char::is_whitespace)
                    .unwrap_or(rest.len() - skip_ws);
                self.cursor += skip_ws + word;
            }
            KeyCode::Char('f') if alt => {
                let rest = &self.text[self.cursor..];
                let skip_ws = rest.len() - rest.trim_start().len();
                let word = rest[skip_ws..]
                    .find(char::is_whitespace)
                    .unwrap_or(rest.len() - skip_ws);
                self.cursor += skip_ws + word;
            }
            KeyCode::Left => self.cursor = self.prev(self.cursor),
            KeyCode::Right => self.cursor = self.next(self.cursor),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Up
                if self.multiline && self.cursor > 0 && self.text[..self.cursor].contains('\n') =>
            {
                let col = self.cursor - self.line_start();
                let ls = self.line_start();
                let prev_start = self.text[..ls - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let prev_len = ls - 1 - prev_start;
                self.cursor = self.snap(prev_start + col.min(prev_len));
            }
            KeyCode::Down if self.multiline && self.text[self.cursor..].contains('\n') => {
                let col = self.cursor - self.line_start();
                let next_start = self.line_end() + 1;
                let next_end = self.text[next_start..]
                    .find('\n')
                    .map(|i| next_start + i)
                    .unwrap_or(self.text.len());
                self.cursor = self.snap(next_start + col.min(next_end - next_start));
            }
            _ => return false,
        }
        true
    }

    fn snap(&self, mut i: usize) -> usize {
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// Wrap into display lines of `width`, returning lines and the cursor (row, col).
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut lines = vec![String::new()];
        let mut col = 0;
        let mut cur = (0, 0);
        for (i, ch) in self.text.char_indices() {
            if ch == '\n' {
                if i == self.cursor {
                    cur = (lines.len() - 1, col);
                }
                lines.push(String::new());
                col = 0;
                continue;
            }
            let w = ch.width().unwrap_or(0);
            if col + w > width {
                // Carry the partial word after the line's last space onto the next line;
                // a word longer than the whole line is still split mid-word.
                let row = lines.len() - 1;
                let line = &mut lines[row];
                let tail = match line.rfind(' ') {
                    Some(p) if ch != ' ' => line.split_off(p + 1),
                    _ => String::new(),
                };
                let tail_w: usize = tail.chars().map(|c| c.width().unwrap_or(0)).sum();
                if self.cursor < i && cur.0 == row && cur.1 >= col - tail_w {
                    cur = (row + 1, cur.1 - (col - tail_w));
                }
                lines.push(tail);
                col = tail_w;
                // A space that lands on the break is swallowed by it.
                if ch == ' ' {
                    if i == self.cursor {
                        cur = (row + 1, 0);
                    }
                    continue;
                }
            }
            if i == self.cursor {
                cur = (lines.len() - 1, col);
            }
            lines.last_mut().unwrap().push(ch);
            col += w;
        }
        if self.cursor >= self.text.len() {
            if col >= width {
                lines.push(String::new());
                col = 0;
            }
            cur = (lines.len() - 1, col);
        }
        (lines, cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: &str, cursor: usize) -> TextInput {
        TextInput {
            text: text.into(),
            cursor,
            multiline: true,
        }
    }

    #[test]
    fn layout_wraps_at_word_boundaries() {
        let (lines, _) = input("hello brave world", 0).layout(10);
        assert_eq!(lines, ["hello ", "brave ", "world"]);
    }

    #[test]
    fn layout_splits_words_longer_than_the_line() {
        let (lines, _) = input("abcdefghijkl", 0).layout(5);
        assert_eq!(lines, ["abcde", "fghij", "kl"]);
    }

    #[test]
    fn layout_follows_the_cursor_onto_the_wrapped_word() {
        // Cursor on the "w" of "world".
        let (_, cur) = input("hello world", 6).layout(8);
        assert_eq!(cur, (1, 0));
        // Cursor at the end.
        let (_, cur) = input("hello world", 11).layout(8);
        assert_eq!(cur, (1, 5));
    }

    #[test]
    fn layout_swallows_a_space_at_the_break() {
        let (lines, cur) = input("hello world", 5).layout(5);
        assert_eq!(lines, ["hello", "world"]);
        assert_eq!(cur, (1, 0));
    }
}
