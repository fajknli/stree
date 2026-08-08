// src/app/input.rs

#[derive(Debug)]
pub struct InputState {
    pub buffer: String,
    pub cursor: usize,          // 字符索引（不是字节）
    pub is_active: bool,
    pub prefix: String,         // "/" 或 ":" 等
    pub last_rendered: String,
    pub on_submit: Option<String>, // 提交时执行的命令模板
    pub on_submit_is_silent: bool,
}

impl InputState {
    pub fn new(prefix: &str) -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            is_active: false,
            prefix: prefix.to_string(),
            on_submit: None,
            on_submit_is_silent: false,
            last_rendered: String::new(),
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let char_pos = self.cursor;
        let byte_pos: usize = self.buffer.chars().take(char_pos).map(|ch| ch.len_utf8()).sum();
        self.buffer.insert(byte_pos, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_pos: usize = self.buffer.chars().take(self.cursor).map(|ch| ch.len_utf8()).sum();
            let next_byte_pos = byte_pos + self.buffer[byte_pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            self.buffer.replace_range(byte_pos..next_byte_pos, "");
        }
    }

    pub fn move_left(&mut self) { if self.cursor > 0 { self.cursor -= 1; } }
    pub fn move_right(&mut self) {
        let char_count = self.buffer.chars().count();
        if self.cursor < char_count { self.cursor += 1; }
    }
    pub fn move_home(&mut self) { self.cursor = 0; }
    pub fn move_end(&mut self) { self.cursor = self.buffer.chars().count(); }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    pub fn activate(&mut self) {
        self.clear();
        self.is_active = true;
    }

    pub fn deactivate(&mut self) {
        self.is_active = false;
        self.clear();
    }
}
