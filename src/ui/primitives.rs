// src/ui/primitives.rs

use crate::layout::{WindowRect, BorderStyle};
use crossterm::style::Color;
use super::buffer::Buffer;

pub fn clip_rect_to_term(rect: &WindowRect, term_width: u16, term_height: u16) -> Option<WindowRect> {
    let mut r = *rect;
    if r.start_col >= term_width || r.start_row >= term_height { return None; }
    if r.start_col + r.width > term_width { r.width = term_width - r.start_col; }
    if r.start_row + r.height > term_height { r.height = term_height - r.start_row; }
    if r.width == 0 || r.height == 0 { return None; }
    Some(r)
}

pub fn draw_border(buf: &mut Buffer, rect: &WindowRect, title: Option<&str>, border: BorderStyle, border_color: Color, border_chars: Option<&str>) -> std::io::Result<()> {
    let width = rect.width as usize;
    if width < 2 { return Ok(()); }
    if border == BorderStyle::Box && rect.height < 2 { return Ok(()); }

    let x = rect.start_col;
    let y_top = rect.start_row;
    let y_bottom = rect.start_row + rect.height - 1;

    let (top_left, top_right, bottom_left, bottom_right, vertical, horizontal) = if let Some(chars) = border_chars {
        let chars: Vec<char> = chars.chars().collect();
        (chars.get(0).copied().unwrap_or(' '), chars.get(1).copied().unwrap_or(' '),
         chars.get(2).copied().unwrap_or(' '), chars.get(3).copied().unwrap_or(' '),
         chars.get(4).copied().unwrap_or('│'), chars.get(5).copied().unwrap_or('─'))
    } else {
        ('┌', '┐', '└', '┘', '│', '─')
    };

    match border {
        BorderStyle::None => {}
        BorderStyle::Line => {
            for i in 0..width {
                buf.set_cell(x as usize + i, y_top as usize, horizontal, border_color, None, false);
            }
        }
        BorderStyle::Box => {
            // Top line
            let mut top_line = String::with_capacity(width);
            top_line.push(top_left);
            if let Some(t) = title {
                if !t.is_empty() {
                    let title_str = format!(" {} ", t);
                    if width >= title_str.chars().count() + 2 { top_line.push_str(&title_str); }
                }
            }
            let current_len = top_line.chars().count();
            for _ in current_len..width - 1 { top_line.push(horizontal); }
            top_line.push(top_right);

            for (i, c) in top_line.chars().enumerate() {
                buf.set_cell(x as usize + i, y_top as usize, c, border_color, None, false);
            }

            // Bottom line
            let mut bottom_line = String::with_capacity(width);
            bottom_line.push(bottom_left);
            for _ in 1..width - 1 { bottom_line.push(horizontal); }
            bottom_line.push(bottom_right);

            for (i, c) in bottom_line.chars().enumerate() {
                buf.set_cell(x as usize + i, y_bottom as usize, c, border_color, None, false);
            }

            // Left and right lines
            for row in 1..rect.height - 1 {
                buf.set_cell(x as usize, (y_top + row) as usize, vertical, border_color, None, false);
                buf.set_cell((x + rect.width - 1) as usize, (y_top + row) as usize, vertical, border_color, None, false);
            }
        }
    }
    Ok(())
}
