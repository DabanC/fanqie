//! Unicode-aware command-line editing, independent of application state.
use crossterm::event::KeyCode;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct Editor {
    pub text: String,
    pub cursor: usize,
}
impl Editor {
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }
    pub fn insert(&mut self, text: &str) {
        let text: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(4096)
            .collect();
        if self.text.len() + text.len() > 16384 {
            return;
        }
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }
    pub fn key(&mut self, code: KeyCode) {
        let prev = || {
            self.text[..self.cursor]
                .grapheme_indices(true)
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        let next = || {
            self.cursor
                + self.text[self.cursor..]
                    .graphemes(true)
                    .next()
                    .map(str::len)
                    .unwrap_or(0)
        };
        match code {
            KeyCode::Left => self.cursor = prev(),
            KeyCode::Right => self.cursor = next(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                let start = prev();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                let end = next();
                self.text.replace_range(self.cursor..end, "");
            }
            KeyCode::Char(c) if !c.is_control() => self.insert(&c.to_string()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_handles_chinese_and_combining_characters() {
        let mut editor = Editor::default();
        editor.insert("你好e\u{301}");
        editor.key(KeyCode::Backspace);
        assert_eq!(editor.text, "你好");
        editor.key(KeyCode::Left);
        editor.insert("，");
        assert_eq!(editor.text, "你，好");
        editor.key(KeyCode::Delete);
        assert_eq!(editor.text, "你，");
        editor.insert("\x1b\n");
        assert!(!editor.text.contains('\x1b'));
    }
}
