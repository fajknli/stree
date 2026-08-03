// src/ui/renderer.rs

use crate::layout::{WindowRect, BorderStyle};
use super::TextStyle;
use super::primitives::BLANK_SPACES;
use crossterm::style::Color;
use std::io::Write;
use crossterm::{cursor, style, QueueableCommand};
use unicode_width::UnicodeWidthChar;

/// 局部渲染上下文：封装坐标变换与边界裁剪
pub struct WindowRenderer<'a, W: Write> {
    out: &'a mut W,
    rect: WindowRect,
    offset_x: u16,
    offset_y: u16,
    content_w: u16,
    content_h: u16,
}

impl<'a, W: Write> WindowRenderer<'a, W> {
    pub fn new(out: &'a mut W, rect: WindowRect, border: BorderStyle) -> Self {
        let (offset_x, offset_y, oh_x, oh_y) = match border {
            BorderStyle::Box => (1, 1, 2, 2),
            BorderStyle::Line => (0, 1, 0, 1),
            BorderStyle::None => (0, 0, 0, 0),
        };
        Self {
            out,
            rect,
            offset_x,
            offset_y,
            content_w: rect.width.saturating_sub(oh_x),
            content_h: rect.height.saturating_sub(oh_y),
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

        self.draw_clipped_text(real_x, real_y, text, max_w, None, style, h_offset)
    }

    pub fn clear_row(&mut self, y: u16) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x;
        let real_y = self.rect.start_row + self.offset_y + y;
        self.out.queue(cursor::MoveTo(real_x, real_y))?;

        let w = self.content_w as usize;
        let fallback;
        let blank = if w <= BLANK_SPACES.len() {
            &BLANK_SPACES[..w]
        } else {
            fallback = " ".repeat(w);
            &fallback
        };

        self.out.queue(style::Print(blank))?;
        Ok(())
    }

    pub fn show_cursor(&mut self, x: u16, y: u16) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x + x;
        let real_y = self.rect.start_row + self.offset_y + y;
        self.out.queue(cursor::MoveTo(real_x, real_y))?;
        Ok(())
    }

    fn draw_clipped_text(
        &mut self, start_col: u16, row: u16, text: &str, max_width: u16,
        _highlight: Option<&str>, text_style: TextStyle, h_offset: usize
    ) -> std::io::Result<()> {
        if max_width == 0 { return Ok(()); }
        let max_w = max_width as usize;

        self.out.queue(cursor::MoveTo(start_col, row))?;
        let blank_str;
        let blank = if max_w <= BLANK_SPACES.len() {
            &BLANK_SPACES[..max_w]
        } else {
            blank_str = " ".repeat(max_w);
            &blank_str
        };
        self.out.write_all(blank.as_bytes())?;
        self.out.queue(cursor::MoveTo(start_col, row))?;

        self.out.queue(style::SetForegroundColor(text_style.fg))?;
        if let Some(bg) = text_style.bg { self.out.queue(style::SetBackgroundColor(bg))?; }
        if text_style.bold { self.out.queue(style::SetAttribute(style::Attribute::Bold))?; }

        let mut chars = text.char_indices().peekable();
        let mut skipped_w = 0;
        let mut current_w = 0;
        let mut plain_start = None;

        while let Some((i, c)) = chars.next() {
            if c == '\x1b' {
                if let Some(start) = plain_start.take() {
                    self.out.write_all(text[start..i].as_bytes())?;
                }

                let mut end_idx = i + c.len_utf8();
                if let Some(&(_, next_c)) = chars.peek() {
                    if next_c == '[' {
                        end_idx += next_c.len_utf8();
                        chars.next();
                        while let Some(&(idx, c_inner)) = chars.peek() {
                            end_idx = idx + c_inner.len_utf8();
                            chars.next();
                            if (0x40..=0x7E).contains(&(c_inner as u32)) { break; }
                        }
                    } else if [']', '_', 'P', '^', 'X'].contains(&next_c) {
                        end_idx += next_c.len_utf8();
                        chars.next();
                        let mut prev_was_esc = false;
                        while let Some(&(idx, c_inner)) = chars.peek() {
                            end_idx = idx + c_inner.len_utf8();
                            chars.next();
                            if c_inner == '\x07' { break; }
                            if c_inner == '\\' && prev_was_esc { break; }
                            prev_was_esc = c_inner == '\x1b';
                        }
                    } else {
                        end_idx += next_c.len_utf8();
                        chars.next();
                    }
                }
                self.out.write_all(text[i..end_idx].as_bytes())?;
            } else {
                let cw = c.width().unwrap_or(0);

                if skipped_w < h_offset {
                    skipped_w += cw;
                    continue;
                }

                if current_w + cw > max_w {
                    if let Some(start) = plain_start.take() {
                        self.out.write_all(text[start..i].as_bytes())?;
                    }
                    if current_w < max_w {
                        self.out.write_all(b"~")?;
                    }
                    break;
                }

                if plain_start.is_none() {
                    plain_start = Some(i);
                }
                current_w += cw;
            }
        }

        if let Some(start) = plain_start {
            self.out.write_all(text[start..].as_bytes())?;
        }

        self.out.queue(style::SetForegroundColor(Color::Reset))?;
        if text_style.bg.is_some() { self.out.queue(style::SetBackgroundColor(Color::Reset))?; }
        if text_style.bold { self.out.queue(style::SetAttribute(style::Attribute::NormalIntensity))?; }

        Ok(())
    }
}
