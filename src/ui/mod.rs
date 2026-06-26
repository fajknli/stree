// src/ui/mod.rs

use crate::app::{Component, Engine, Focus};
use crate::layout::{WindowRect, BorderStyle};
use crossterm::terminal::WindowSize;
use unicode_width::UnicodeWidthChar;
use crossterm::style::Color;

#[derive(Debug)]
pub struct RenderCtx<'a> {
    pub engine: &'a Engine,
    pub style_engine: &'a crate::style::StyleEngine,
    pub term_size: WindowSize,
}

use std::io::Write;
use crossterm::{cursor, style, QueueableCommand};

/// 渲染层防弹衣：将 Rect 严格裁剪在终端边界内，防止 ANSI 越界触发终端换行
fn clip_rect_to_term(rect: &WindowRect, term_width: u16, term_height: u16) -> Option<WindowRect> {
    let mut r = *rect;
    if r.start_col >= term_width || r.start_row >= term_height {
        return None; // 完全在屏幕外
    }
    if r.start_col + r.width > term_width {
        r.width = term_width - r.start_col;
    }
    if r.start_row + r.height > term_height {
        r.height = term_height - r.start_row;
    }
    if r.width == 0 || r.height == 0 {
        return None; // 尺寸为 0，不渲染
    }
    Some(r)
}

pub fn draw_border<W: Write>(out: &mut W, rect: &WindowRect, title: Option<&str>, border: BorderStyle, is_focused: bool, border_chars: Option<&str>) -> std::io::Result<()> {
    let width = rect.width as usize;
    if width < 2 { return Ok(()); }
    if border == BorderStyle::Box && rect.height < 2 { return Ok(()); }

    let x = rect.start_col;
    let y_top = rect.start_row;
    let y_bottom = rect.start_row + rect.height - 1;

    let border_color = if is_focused { Color::Green } else { Color::DarkGrey };
    let (top_left, top_right, bottom_left, bottom_right, vertical, horizontal) = if let Some(chars) = border_chars {
        let chars: Vec<char> = chars.chars().collect();
        (
            chars.get(0).copied().unwrap_or(' '),
            chars.get(1).copied().unwrap_or(' '),
            chars.get(2).copied().unwrap_or(' '),
            chars.get(3).copied().unwrap_or(' '),
            chars.get(4).copied().unwrap_or('│'),
            chars.get(5).copied().unwrap_or('─'),
        )
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
                out.queue(style::Print(vertical.to_string()))?;
                out.queue(cursor::MoveTo(x + rect.width - 1, y_top + row as u16))?;
                out.queue(style::Print(vertical.to_string()))?;
            }
        }
    }
    Ok(())
}

enum Segment { Text(String), Ansi(String), Osc(String) }

pub fn draw_text<W: Write>(out: &mut W, start_col: u16, row: u16, text: &str, max_width: u16, highlight: Option<&str>, default_color: Color) -> std::io::Result<()> {
    if max_width == 0 { return Ok(()); }
    let max_w = max_width as usize;
    out.queue(cursor::MoveTo(start_col, row))?;
    out.queue(style::Print(" ".repeat(max_w)))?;
    out.queue(cursor::MoveTo(start_col, row))?;

    let mut segments = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                let mut ansi = String::from("\x1b[");
                chars.next();
                while let Some(&next_c) = chars.peek() {
                    ansi.push(next_c);
                    chars.next();
                    if next_c.is_ascii_alphabetic() { break; }
                }
                segments.push(Segment::Ansi(ansi));
            } else if chars.peek() == Some(&'_') || chars.peek() == Some(&']') {
                let mut osc = String::from("\x1b");
                let osc_type = chars.next().unwrap();
                osc.push(osc_type);

                while let Some(&next_c) = chars.peek() {
                    osc.push(next_c);
                    chars.next();
                    if next_c == '\\' && osc.ends_with("\x1b\\") {
                        break;
                    }
                }
                segments.push(Segment::Osc(osc));
            } else {
                segments.push(Segment::Text(String::from(c)));
            }
        } else {
            let mut text_seg = String::new();
            text_seg.push(c);
            while let Some(&next_c) = chars.peek() {
                if next_c == '\x1b' { break; }
                text_seg.push(next_c);
                chars.next();
            }
            segments.push(Segment::Text(text_seg));
        }
    }

    let mut total_w = 0;
    let mut plain_chars: Vec<(char, usize)> = Vec::new();
    for seg in &segments {
        if let Segment::Text(s) = seg {
            for c in s.chars() {
                let cw = c.width().unwrap_or(0);
                plain_chars.push((c, cw));
                total_w += cw;
            }
        }
    }

    let mut keep_count = plain_chars.len();
    let mut truncated = false;
    if total_w > max_w {
        truncated = true;
        while total_w > max_w && keep_count > 0 {
            let (_, cw) = plain_chars[keep_count - 1];
            total_w -= cw;
            keep_count -= 1;
        }
    }

    let lower_highlight = highlight.unwrap_or("").to_lowercase();
    let plain_str: String = plain_chars.iter().take(keep_count).map(|(c, _)| *c).collect();
    let lower_plain = plain_str.to_lowercase();
    let mut highlight_indices = None;
    if !lower_highlight.is_empty() {
        if let Some(start) = lower_plain.find(&lower_highlight) {
            let start_idx = lower_plain[..start].chars().count();
            let end_idx = start_idx + lower_highlight.chars().count();
            highlight_indices = Some((start_idx, end_idx));
        }
    }

    let mut current_plain_count = 0;
    for seg in &segments {
        match seg {
            Segment::Ansi(s) => { out.queue(style::Print(s))?; }
            Segment::Osc(s) => { out.queue(style::Print(s))?; }
            Segment::Text(s) => {
                for c in s.chars() {
                    if current_plain_count < keep_count {
                        if let Some((start, end)) = highlight_indices {
                            if current_plain_count == start {
                                out.queue(style::SetForegroundColor(Color::Yellow))?;
                                out.queue(style::SetAttribute(style::Attribute::Bold))?;
                            }
                            out.queue(style::Print(c.to_string()))?;
                            if current_plain_count == end - 1 {
                                out.queue(style::SetAttribute(style::Attribute::NormalIntensity))?;
                                out.queue(style::SetForegroundColor(default_color))?;
                            }
                        } else {
                            out.queue(style::Print(c.to_string()))?;
                        }
                        current_plain_count += 1;
                    }
                }
            }
        }
    }
    if truncated { out.queue(style::Print("~"))?; }
    Ok(())
}

pub fn draw_tree_window<W: Write>(out: &mut W, rect: &WindowRect, tree: &crate::app::TreeState, style_engine: &crate::style::StyleEngine, scroll_offset: usize, is_focused: bool, border: BorderStyle) -> std::io::Result<usize> {
    let (min_w, min_h) = match border {
        BorderStyle::Box => (4, 3),
        BorderStyle::Line => (1, 2),
        BorderStyle::None => (1, 1),
    };
    if rect.width < min_w || rect.height < min_h { return Ok(0); }
    let border_overhead = match border {
        BorderStyle::Box => 2,
        BorderStyle::Line => 1,
        BorderStyle::None => 0,
    };
    let max_rows = (rect.height as usize).saturating_sub(border_overhead);
    let start = scroll_offset;
    let end = (start + max_rows).min(tree.visible_ids.len());
    let is_filtering = false;
    let mut drawn = 0;

    for i in start..end {
        let id = &tree.visible_ids[i];
        let depth = tree.visible_depths[i];
        let entity = &tree.dataset.entity_map[id];
        let is_selected = tree.selected_id.as_deref() == Some(id.as_str());
        let is_marked = tree.marked_ids.contains(id);

        let mut display = String::new();

        display.push(' ');

        for _ in 0..depth * 2 { display.push(' '); }

        let has_children = tree.dataset.child_index.contains_key(id);
        let is_expanded = tree.expanded_ids.contains(id);
        if has_children { display.push(if is_expanded { 'v' } else { '>' }); } else { display.push(' '); }
        display.push(' '); display.push_str(&entity.display);

        let border_offset = if border == BorderStyle::None { 0 } else { 1 };
        let screen_row = rect.start_row + border_offset + drawn as u16;
        let start_col = rect.start_col + border_offset;
        let (final_color, is_bold) = {
            let mut tags_with_state = entity.tags.clone();
            if is_selected {
                if !tags_with_state.is_empty() { tags_with_state.push_str(","); }
                tags_with_state.push_str("__selected__");
            }
            if is_marked {
                if !tags_with_state.is_empty() { tags_with_state.push_str(","); }
                tags_with_state.push_str("__marked__");
            }
            let (c, b) = style_engine.get_style(&tags_with_state);
            let c = if is_focused {
                c.unwrap_or(Color::White)
            } else {
                c.unwrap_or(Color::DarkGrey)
            };
            (c, b)
        };

        if is_selected && is_focused { out.queue(crossterm::style::SetBackgroundColor(crossterm::style::Color::DarkGrey))?; }
        if is_bold { out.queue(crossterm::style::SetAttribute(crossterm::style::Attribute::Bold))?; }
        out.queue(crossterm::style::SetForegroundColor(final_color))?;

        let highlight = None;

        draw_text(out, start_col, screen_row, &display, rect.width.saturating_sub(2), highlight, final_color)?;

        out.queue(crossterm::style::SetForegroundColor(crossterm::style::Color::Reset))?;
        if is_bold { out.queue(crossterm::style::SetAttribute(crossterm::style::Attribute::NormalIntensity))?; }
        if is_selected && is_focused { out.queue(crossterm::style::SetBackgroundColor(crossterm::style::Color::Reset))?; }
        drawn += 1;
    }

    if drawn == 0 && tree.visible_ids.is_empty() {
        draw_text(out, rect.start_col + 1, rect.start_row + 1, "No data available", rect.width.saturating_sub(2), None, if is_focused { Color::White } else { Color::DarkGrey })?;
        drawn = 1;
    } else if drawn == 0 && is_filtering {
        draw_text(out, rect.start_col + 1, rect.start_row + 1, "No matches found", rect.width.saturating_sub(2), None, if is_focused { Color::White } else { Color::DarkGrey })?;
        drawn = 1;
    }
    Ok(drawn)
}

/// 【重构】多图层统一渲染入口
/// 按 Z 轴升序遍历所有可见图层的窗口，统一绘制
pub fn render_all<W: Write>(ctx: &RenderCtx, out: &mut W) -> std::io::Result<()> {
    out.queue(crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?;
    let term_width = ctx.term_size.columns;
    let term_height = ctx.term_size.rows;

    let all_rects = ctx.engine.calc_all_rects(term_width, term_height);
    if all_rects.is_empty() { return Ok(()); }

    let mut status_rect_opt: Option<WindowRect> = None;

    for (rect, name, border, _z_index) in &all_rects {
        // 【核心防御】：裁剪越界 Rect，彻底消灭右边字符跑到左边的 Bug
        let safe_rect = match clip_rect_to_term(rect, term_width, term_height) {
            Some(r) => r,
            None => continue, // 完全在屏幕外或尺寸为 0，跳过渲染
        };

        let comp = ctx.engine.components.get(name);
        let title = comp.map(|_| name.as_str());
        let is_focused = ctx.engine.focused == Focus::Component(name.clone());

        let border_chars = ctx.engine.border_chars.get(name).map(|s| s.as_str());
        draw_border(out, &safe_rect, title, *border, is_focused, border_chars)?;

        match comp {
            Some(Component::Tree(t)) => {
                let border_overhead = match border {
                    BorderStyle::Box => 2,
                    _ => 0,
                };
                let max_rows = (safe_rect.height as usize).saturating_sub(border_overhead);
                let scroll_offset = calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows);
                draw_tree_window(out, &safe_rect, t, ctx.style_engine, scroll_offset, is_focused, *border)?;
            }
            Some(Component::View(v)) => {
                let content = v.content_buffer.as_str();
                let lines: Vec<&str> = content.lines().collect();
                let (inner_col, inner_row, inner_w, inner_h) = match border {
                    BorderStyle::Box => (safe_rect.start_col + 1, safe_rect.start_row + 1, safe_rect.width.saturating_sub(2), safe_rect.height.saturating_sub(2)),
                    _ => (safe_rect.start_col, safe_rect.start_row, safe_rect.width, safe_rect.height),
                };
                let max_rows = inner_h as usize;
                let max_offset = lines.len().saturating_sub(max_rows);
                let actual_offset = v.scroll_offset.min(max_offset);
                let color = if is_focused { Color::White } else { Color::DarkGrey };
                for i in 0..max_rows {
                    if let Some(line) = lines.get(i + actual_offset) {
                        draw_text(out, inner_col, inner_row + i as u16, line, inner_w, None, color)?;
                    }
                }
            }
            Some(Component::StatusBar(s)) => {
                status_rect_opt = Some(safe_rect);
                let row = safe_rect.start_row;
                let col = safe_rect.start_col;
                let cover_width = safe_rect.width as usize;

                let mut status_text = s.format_template.clone();
                status_text = status_text.replace("{stree_focus}", match &ctx.engine.focused { Focus::Component(n) => n, _ => "None" });

                if let Some(t) = ctx.engine.get_focused_tree_state() {
                    status_text = status_text.replace("{stree_visible}", &t.visible_ids.len().to_string());
                    status_text = status_text.replace("{stree_total}", &t.dataset.entities.len().to_string());
                    status_text = status_text.replace("{stree_marked}", &t.marked_ids.len().to_string());
                    status_text = status_text.replace("{stree_id}", t.selected_id.as_deref().unwrap_or(""));
                }

                out.queue(cursor::MoveTo(col, row))?;
                out.queue(style::Print(" ".repeat(cover_width)))?;
                out.queue(cursor::MoveTo(col, row))?;
                let display_text: String = status_text.chars().take(cover_width).collect();
                out.queue(style::Print(&display_text))?;
            }

            Some(Component::Input(input)) => {
                if !input.is_active {
                    continue;
                }

                let (inner_col, inner_row, inner_w) = match border {
                    BorderStyle::Box => (safe_rect.start_col + 1, safe_rect.start_row + 1, safe_rect.width.saturating_sub(2)),
                    BorderStyle::Line => (safe_rect.start_col, safe_rect.start_row + 1, safe_rect.width),
                    BorderStyle::None => (safe_rect.start_col, safe_rect.start_row, safe_rect.width),
                };

                let width = inner_w as usize;
                let prefix_len = input.prefix.chars().count();
                let content_width = width.saturating_sub(prefix_len);

                let render_key = format!("{}{}{}", input.prefix, input.buffer, input.cursor);
                let same = input.last_rendered == render_key;
                if same {
                    let cursor_col = inner_col + prefix_len as u16 + input.cursor as u16;
                    out.queue(cursor::MoveTo(cursor_col, inner_row))?;
                    continue;
                }

                out.queue(cursor::MoveTo(inner_col, inner_row))?;
                out.queue(style::Print(" ".repeat(width)))?;
                out.queue(cursor::MoveTo(inner_col, inner_row))?;

                out.queue(style::SetForegroundColor(Color::Yellow))?;
                out.queue(style::Print(&input.prefix))?;
                out.queue(style::SetForegroundColor(Color::Reset))?;

                let display: String = input.buffer.chars().take(content_width).collect();
                out.queue(style::Print(&display))?;

                let cursor_col = inner_col + prefix_len as u16 + input.cursor as u16;
                out.queue(cursor::MoveTo(cursor_col, inner_row))?;
            }
            None => {}
        }
    }

    // 全局错误提示直接覆盖在状态栏上
    if let Some(err) = &ctx.engine.last_error {
        if let Some(rect) = status_rect_opt {
            out.queue(cursor::MoveTo(rect.start_col, rect.start_row))?;
            let cover_width = rect.width as usize;
            out.queue(style::Print(" ".repeat(cover_width)))?;
            out.queue(cursor::MoveTo(rect.start_col, rect.start_row))?;
            out.queue(style::SetBackgroundColor(Color::Red))?;
            out.queue(style::SetForegroundColor(Color::White))?;
            let err_text = format!(" ERR: {} ", err);
            let display_err: String = err_text.chars().take(cover_width).collect();
            out.queue(style::Print(&display_err))?;
            out.queue(style::SetAttribute(style::Attribute::Reset))?;
        }
    }

    if ctx.engine.has_active_input() {
        out.queue(cursor::Show)?;
    } else {
        out.queue(cursor::Hide)?;
    }

    out.flush()?;
    Ok(())
}

pub fn calc_scroll_offset(selected_idx: usize, visible_count: usize, max_rows: usize) -> usize {
    if visible_count <= max_rows { return 0; }
    let max_offset = visible_count - max_rows;
    if selected_idx >= max_rows {
        selected_idx - max_rows + 1
    } else {
        0
    }.min(max_offset)
}
