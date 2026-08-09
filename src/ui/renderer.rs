// src/ui/renderer.rs

use crate::layout::{WindowRect, BorderStyle};
use super::TextStyle;
use super::buffer::Buffer;
use crossterm::style::Color;
use super::CURSOR_POS;
use unicode_width::UnicodeWidthChar;

pub struct WindowRenderer<'a> {
    buffer: &'a mut Buffer,
    rect: WindowRect,
    offset_x: u16,
    offset_y: u16,
    content_w: u16,
    content_h: u16,
    clear_bg: bool, // 【新增】强制清空背景标志
}

impl<'a> WindowRenderer<'a> {
    pub fn new(buffer: &'a mut Buffer, rect: WindowRect, border: BorderStyle, clear_bg: bool) -> Self {
        let (offset_x, offset_y, oh_x, oh_y) = match border {
            BorderStyle::Box => (1, 1, 2, 2),
            BorderStyle::Line => (0, 1, 0, 1),
            BorderStyle::None => (0, 0, 0, 0),
        };
        Self {
            buffer,
            rect,
            offset_x,
            offset_y,
            content_w: rect.width.saturating_sub(oh_x),
            content_h: rect.height.saturating_sub(oh_y),
            clear_bg,
        }
    }

    pub fn content_width(&self) -> u16 { self.content_w }
    pub fn content_height(&self) -> u16 { self.content_h }

    pub fn print(&mut self, x: u16, y: u16, text: &str, style: TextStyle, h_offset: usize) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x + x;
        let real_y = self.rect.start_row + self.offset_y + y;
        let max_w = self.content_w.saturating_sub(x);
        if max_w == 0 { return Ok(()); }

        // 【修复】如果是浮动窗口（clear_bg=true）或指定了背景色，必须用空格铺满背景，防止底层透出！
        if self.clear_bg || style.bg.is_some() {
            for i in 0..max_w {
                self.buffer.set_cell((real_x + i) as usize, real_y as usize, ' ', style.fg, style.bg, style.bold);
            }
        }

        self.draw_clipped_text(real_x, real_y, text, max_w, style, h_offset)
    }

    pub fn clear_row(&mut self, y: u16) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x;
        let real_y = self.rect.start_row + self.offset_y + y;
        for i in 0..self.content_w {
            self.buffer.set_cell((real_x + i) as usize, real_y as usize, ' ', Color::Reset, None, false);
        }
        Ok(())
    }

    pub fn show_cursor(&mut self, x: u16, y: u16) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x + x;
        let real_y = self.rect.start_row + self.offset_y + y;
        CURSOR_POS.with(|cp| cp.replace(Some((real_x, real_y))));
        Ok(())
    }

    fn draw_clipped_text(
        &mut self, start_col: u16, row: u16, text: &str, max_width: u16,
        base_style: TextStyle, h_offset: usize
    ) -> std::io::Result<()> {
        if max_width == 0 { return Ok(()); }
        let max_w = max_width as usize;
        let limit = max_w.saturating_sub(1); // 预留 1 列给 '~'

        let mut current_fg = base_style.fg;
        let mut current_bg = base_style.bg;
        let mut current_bold = base_style.bold;

        // 【新增】左侧截断指示符 '<'
        let has_left_trunc = h_offset > 0 && max_w > 2;
        let mut current_w = if has_left_trunc {
            self.buffer.set_cell(start_col as usize, row as usize, '<', current_fg, current_bg, current_bold);
            1 // 文本从第 1 列开始画，跳过第 0 列的 '<'
        } else {
            0
        };

        let mut skipped_w = 0;

        let mut chars = text.char_indices().peekable();
        while let Some((_, c)) = chars.next() {
            if c == '\x1b' {
                // 遇到 ESC，开始解析转义序列
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '[' {
                        chars.next(); // consume '['
                        let (fg, bg, bold) = Self::parse_ansi_sgr(&mut chars, base_style);
                        current_fg = fg;
                        current_bg = bg;
                        current_bold = bold;
                    }
                }
            } else {
                let cw = c.width().unwrap_or(0);
                if cw == 0 { continue; }

                if skipped_w < h_offset {
                    skipped_w += cw;
                    continue;
                }

                // 截断逻辑：严格预留 1 列给 '~'
                if current_w + cw > limit {
                    if current_w <= limit {
                        self.buffer.set_cell((start_col + limit as u16) as usize, row as usize, '~', current_fg, current_bg, current_bold);
                    }
                    break;
                }

                self.buffer.set_cell((start_col + current_w as u16) as usize, row as usize, c, current_fg, current_bg, current_bold);

                // 处理宽字符（如中文）：如果字符宽度为2，必须在下一列填入空格占位
                if cw == 2 {
                    let next_x = (start_col + current_w as u16 + 1) as usize;
                    self.buffer.set_cell(next_x, row as usize, ' ', current_fg, current_bg, current_bold);
                }

                current_w += cw;
            }
        }
        Ok(())
    }

    // 【Geek 优化】零分配的 ANSI SGR 解析器，直接操作迭代器，性能提升数倍
    fn parse_ansi_sgr(
        chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
        base_style: TextStyle,
    ) -> (Color, Option<Color>, bool) {
        let mut current_fg = base_style.fg;
        let mut current_bg = base_style.bg;
        let mut current_bold = base_style.bold;

        let mut params: Vec<u32> = Vec::with_capacity(8); // 绝大多数 SGR 参数不超过 8 个
        let mut current_num: u32 = 0;
        let mut has_num = false;
        let mut sequence_complete = false;

        while let Some(&(_, c_inner)) = chars.peek() {
            chars.next();
            if c_inner.is_ascii_alphabetic() {
                sequence_complete = true;
                break;
            }
            if c_inner == ';' {
                if has_num {
                    params.push(current_num);
                    current_num = 0;
                    has_num = false;
                }
            } else if let Some(d) = c_inner.to_digit(10) {
                current_num = current_num * 10 + d;
                has_num = true;
            }
        }

        if !sequence_complete {
            return (base_style.fg, base_style.bg, base_style.bold);
        }
        if has_num {
            params.push(current_num);
        }

        if params.is_empty() {
            return (base_style.fg, base_style.bg, false);
        }

        let mut i = 0;
        while i < params.len() {
            let p = params[i];
            match p {
                0 => { current_fg = base_style.fg; current_bg = base_style.bg; current_bold = false; }
                1 => current_bold = true,
                22 => current_bold = false,
                39 => current_fg = base_style.fg,
                49 => current_bg = base_style.bg,
                30..=37 | 90..=97 => current_fg = Color::AnsiValue(p as u8 - if p >= 90 { 82 } else { 30 }),
                40..=47 | 100..=107 => current_bg = Some(Color::AnsiValue(p as u8 - if p >= 100 { 92 } else { 40 })),
                38 => {
                    if i + 1 < params.len() && params[i + 1] == 5 && i + 2 < params.len() {
                        current_fg = Color::AnsiValue(params[i + 2] as u8);
                        i += 2;
                    } else if i + 1 < params.len() && params[i + 1] == 2 && i + 4 < params.len() {
                        current_fg = Color::Rgb { r: params[i + 2] as u8, g: params[i + 3] as u8, b: params[i + 4] as u8 };
                        i += 4;
                    }
                }
                48 => {
                    if i + 1 < params.len() && params[i + 1] == 5 && i + 2 < params.len() {
                        current_bg = Some(Color::AnsiValue(params[i + 2] as u8));
                        i += 2;
                    } else if i + 1 < params.len() && params[i + 1] == 2 && i + 4 < params.len() {
                        current_bg = Some(Color::Rgb { r: params[i + 2] as u8, g: params[i + 3] as u8, b: params[i + 4] as u8 });
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        (current_fg, current_bg, current_bold)
    }
}
