// src/ui/primitives.rs

use crate::layout::{WindowRect, BorderStyle};
use crossterm::style::Color;
use std::io::Write;
use crossterm::{cursor, style, QueueableCommand};

pub(crate) const BLANK_SPACES: &str = "                                                                                                                                                                                                                                                                ";

pub fn clear_specific_rect<W: Write>(out: &mut W, rect: &WindowRect) -> std::io::Result<()> {
    let w = rect.width as usize;
    let blank_str;
    let blank = if w <= BLANK_SPACES.len() {
        &BLANK_SPACES[..w]
    } else {
        blank_str = " ".repeat(w);
        &blank_str
    };
    for y in 0..rect.height {
        out.queue(cursor::MoveTo(rect.start_col, rect.start_row + y))?;
        out.write_all(blank.as_bytes())?;
    }
    Ok(())
}

/// 【防闪烁核心】只擦除 old 和 new 的差异部分，绝不触碰重叠区域
pub fn clear_rect_diff<W: Write>(out: &mut W, old: &WindowRect, new: &WindowRect) -> std::io::Result<()> {
    let old_x2 = old.start_col.saturating_add(old.width);
    let old_y2 = old.start_row.saturating_add(old.height);
    let new_x2 = new.start_col.saturating_add(new.width);
    let new_y2 = new.start_row.saturating_add(new.height);

    // 1. 顶部条带
    let top_y2 = old_y2.min(new.start_row);
    if top_y2 > old.start_row {
        clear_specific_rect(out, &WindowRect { start_col: old.start_col, start_row: old.start_row, width: old.width, height: top_y2 - old.start_row })?;
    }
    // 2. 底部条带
    let bot_y1 = old.start_row.max(new_y2);
    if bot_y1 < old_y2 {
        clear_specific_rect(out, &WindowRect { start_col: old.start_col, start_row: bot_y1, width: old.width, height: old_y2 - bot_y1 })?;
    }
    // 3. 中间区域的左右条带
    let mid_y1 = old.start_row.max(new.start_row);
    let mid_y2 = old_y2.min(new_y2);
    if mid_y2 > mid_y1 {
        // 左侧条带
        let left_x2 = old_x2.min(new.start_col);
        if left_x2 > old.start_col {
            clear_specific_rect(out, &WindowRect { start_col: old.start_col, start_row: mid_y1, width: left_x2 - old.start_col, height: mid_y2 - mid_y1 })?;
        }
        // 右侧条带
        let right_x1 = old.start_col.max(new_x2);
        if right_x1 < old_x2 {
            clear_specific_rect(out, &WindowRect { start_col: right_x1, start_row: mid_y1, width: old_x2 - right_x1, height: mid_y2 - mid_y1 })?;
        }
    }
    Ok(())
}

pub fn clip_rect_to_term(rect: &WindowRect, term_width: u16, term_height: u16) -> Option<WindowRect> {
    let mut r = *rect;
    if r.start_col >= term_width || r.start_row >= term_height { return None; }
    if r.start_col + r.width > term_width { r.width = term_width - r.start_col; }
    if r.start_row + r.height > term_height { r.height = term_height - r.start_row; }
    if r.width == 0 || r.height == 0 { return None; }
    Some(r)
}

pub fn draw_border<W: Write>(out: &mut W, rect: &WindowRect, title: Option<&str>, border: BorderStyle, border_color: Color, border_chars: Option<&str>) -> std::io::Result<()> {
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
            out.queue(style::SetForegroundColor(border_color))?;
            let mut top_line = String::with_capacity(width);
            for _ in 0..width { top_line.push(horizontal); }
            out.queue(cursor::MoveTo(x, y_top))?;
            out.queue(style::Print(top_line))?;
            out.queue(style::SetForegroundColor(Color::Reset))?;
        }
        BorderStyle::Box => {
            out.queue(style::SetForegroundColor(border_color))?;
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
            out.queue(cursor::MoveTo(x, y_top))?;
            out.queue(style::Print(top_line))?;

            let mut bottom_line = String::with_capacity(width);
            bottom_line.push(bottom_left);
            for _ in 1..width - 1 { bottom_line.push(horizontal); }
            bottom_line.push(bottom_right);
            out.queue(cursor::MoveTo(x, y_bottom))?;
            out.queue(style::Print(bottom_line))?;

            for row in 1..rect.height - 1 {
                out.queue(cursor::MoveTo(x, y_top + row as u16))?;
                out.queue(style::Print(vertical))?;
                out.queue(cursor::MoveTo(x + rect.width - 1, y_top + row as u16))?;
                out.queue(style::Print(vertical))?;
            }
        }
    }
    Ok(())
}
