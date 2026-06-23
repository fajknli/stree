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

pub fn draw_border<W: Write>(out: &mut W, rect: &WindowRect, title: Option<&str>, border: BorderStyle, is_focused: bool) -> std::io::Result<()> {
    let width = rect.width as usize;
    if width < 2 { return Ok(()); }
    if border == BorderStyle::Box && rect.height < 2 { return Ok(()); }

    let x = rect.start_col;
    let y_top = rect.start_row;
    let y_bottom = rect.start_row + rect.height - 1;

    let border_color = if is_focused { Color::Green } else { Color::DarkGrey };

    match border {
        BorderStyle::None => {}
        BorderStyle::Line => {
            out.queue(style::SetForegroundColor(border_color))?;
            let mut top_line = String::with_capacity(width);
            for _ in 0..width { top_line.push('─'); }
            out.queue(cursor::MoveTo(x, y_top))?;
            out.queue(style::Print(top_line))?;
            out.queue(style::SetForegroundColor(Color::Reset))?;
        }
        BorderStyle::Box => {
            out.queue(style::SetForegroundColor(border_color))?;
            let mut top_line = String::with_capacity(width);
            top_line.push('┌');
            if let Some(t) = title {
                if !t.is_empty() {
                    let title_str = format!(" {} ", t);
                    if width >= title_str.chars().count() + 2 { top_line.push_str(&title_str); }
                }
            }
            let current_len = top_line.chars().count();
            for _ in current_len..width - 1 { top_line.push('─'); }
            top_line.push('┐');
            out.queue(cursor::MoveTo(x, y_top))?;
            out.queue(style::Print(top_line))?;

            let mut bottom_line = String::with_capacity(width);
            bottom_line.push('└');
            for _ in 1..width - 1 { bottom_line.push('─'); }
            bottom_line.push('┘');
            out.queue(cursor::MoveTo(x, y_bottom))?;
            out.queue(style::Print(bottom_line))?;

            for row in 1..rect.height - 1 {
                out.queue(cursor::MoveTo(x, y_top + row as u16))?;
                out.queue(style::Print("│"))?;
                out.queue(cursor::MoveTo(x + rect.width - 1, y_top + row as u16))?;
                out.queue(style::Print("│"))?;
            }
            out.queue(style::SetForegroundColor(Color::Reset))?;
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
                // CSI 序列：\x1b[...X（X 是任意字母，不只是 'm'）
                let mut ansi = String::from("\x1b[");
                chars.next();
                while let Some(&next_c) = chars.peek() {
                    ansi.push(next_c);
                    chars.next();
                    if next_c.is_ascii_alphabetic() { break; }
                }
                segments.push(Segment::Ansi(ansi));
            } else if chars.peek() == Some(&'_') || chars.peek() == Some(&']') {
                // 【新增】OSC 序列：\x1b_...\x1b\\ 或 \x1b]...\x1b\\
                // 用于 Kitty Graphics Protocol、iTerm2 Inline Images 等
                let mut osc = String::from("\x1b");
                let osc_type = chars.next().unwrap();
                osc.push(osc_type);

                while let Some(&next_c) = chars.peek() {
                    osc.push(next_c);
                    chars.next();
                    // 检测 String Terminator (ST)：\x1b\\
                    if next_c == '\\' && osc.ends_with("\x1b\\") {
                        break;
                    }
                }
                segments.push(Segment::Osc(osc));
            } else {
                // 未知转义序列，当普通文本处理
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
        while total_w >= max_w && keep_count > 0 {
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
            Segment::Osc(s) => { out.queue(style::Print(s))?; } // 【新增】透传 OSC 序列
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
    let is_filtering = !tree.search_query.trim().is_empty();
    let mut drawn = 0;

    for i in start..end {
        let id = &tree.visible_ids[i];
        let depth = tree.visible_depths[i];
        let entity = &tree.dataset.entity_map[id];
        let is_selected = tree.selected_id.as_deref() == Some(id.as_str());
        let is_marked = tree.marked_ids.contains(id);

        let mut display = String::new();

        // 标记 * 放在最左侧
        display.push(if is_marked { '*' } else { ' ' });
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
            let (c, b) = style_engine.get_style(&entity.tags);
            let c = if is_focused { c.unwrap_or(Color::White) } else { Color::DarkGrey };
            (c, b)
        };

        if is_selected && is_focused { out.queue(crossterm::style::SetBackgroundColor(crossterm::style::Color::DarkGrey))?; }
        if is_bold { out.queue(crossterm::style::SetAttribute(crossterm::style::Attribute::Bold))?; }
        out.queue(crossterm::style::SetForegroundColor(final_color))?;

        let highlight = if is_filtering && tree.matched_ids.contains(id) { Some(tree.search_query.as_str()) } else { None };

        draw_text(out, start_col, screen_row, &display, rect.width - 2, highlight, final_color)?;

        out.queue(crossterm::style::SetForegroundColor(crossterm::style::Color::Reset))?;
        if is_bold { out.queue(crossterm::style::SetAttribute(crossterm::style::Attribute::NormalIntensity))?; }
        if is_selected && is_focused { out.queue(crossterm::style::SetBackgroundColor(crossterm::style::Color::Reset))?; }
        drawn += 1;
    }

    if drawn == 0 && tree.visible_ids.is_empty() {
        draw_text(out, rect.start_col + 1, rect.start_row + 1, "No data available", rect.width - 2, None, if is_focused { Color::White } else { Color::DarkGrey })?;
        drawn = 1;
    } else if drawn == 0 && is_filtering {
        draw_text(out, rect.start_col + 1, rect.start_row + 1, "No matches found", rect.width - 2, None, if is_focused { Color::White } else { Color::DarkGrey })?;
        drawn = 1;
    }
    Ok(drawn)
}

pub fn render_all<W: Write>(ctx: &RenderCtx, out: &mut W) -> std::io::Result<()> {
    out.queue(crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?;
    let term_width = ctx.term_size.columns;
    let term_height = ctx.term_size.rows;
    let rects = crate::layout::calc_window_rects(&ctx.engine.layout, term_width, term_height);
    if rects.is_empty() { return Ok(()); }

    // 收集搜索状态，用于覆盖状态栏
    let mut search_info: Option<(bool, String)> = None;
    if let Focus::Component(name) = &ctx.engine.focused {
        if let Some(Component::Tree(t)) = ctx.engine.components.get(name) {
            if t.in_search_mode || !t.search_query.is_empty() {
                search_info = Some((t.in_search_mode, t.search_query.clone()));
            }
        }
    }

    // 找到状态栏的 rect，用于后续错误覆盖
    let mut status_rect_opt: Option<WindowRect> = None;

    for (rect, name, border) in rects.iter() {
        let comp = ctx.engine.components.get(name);
        let title = comp.map(|_| name.as_str());
        let is_focused = ctx.engine.focused == Focus::Component(name.clone());

        draw_border(out, rect, title, *border, is_focused)?;

        match comp {
            Some(Component::Tree(t)) => {
                let border_overhead = match border {
                    BorderStyle::Box => 2,
                    _ => 0,
                };
                let max_rows = (rect.height as usize).saturating_sub(border_overhead);
                let scroll_offset = calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows);
                draw_tree_window(out, rect, t, ctx.style_engine, scroll_offset, is_focused, *border)?;
            }
            Some(Component::View(v)) => {
                let content = v.content_buffer.as_str();
                let lines: Vec<&str> = content.lines().collect();
                let (inner_col, inner_row, inner_w, inner_h) = match border {
                    BorderStyle::Box => (rect.start_col + 1, rect.start_row + 1, rect.width.saturating_sub(2), rect.height.saturating_sub(2)),
                    _ => (rect.start_col, rect.start_row, rect.width, rect.height),
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
                status_rect_opt = Some(*rect);
                let row = rect.start_row;
                let col = rect.start_col;
                let cover_width = rect.width as usize;

                if let Some((in_search, query)) = &search_info {
                    let _ = in_search;
                    let status_text = format!("/{}", query);
                    out.queue(cursor::MoveTo(col, row))?;
                    out.queue(style::Print(" ".repeat(cover_width)))?;
                    out.queue(cursor::MoveTo(col, row))?;
                    out.queue(style::SetAttribute(style::Attribute::Reverse))?;
                    let display_text: String = status_text.chars().take(cover_width).collect();
                    out.queue(style::Print(&display_text))?;
                    out.queue(style::SetAttribute(style::Attribute::Reset))?;
                } else {
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
            }

            Some(Component::Input(input)) => {
                // 【修复】只在 active 状态下渲染
                if !input.is_active {
                    continue; // 跳过这个组件的渲染
                }

                // 【核心修复】根据 border 类型计算内容区位置，避免覆盖边框
                let (inner_col, inner_row, inner_w) = match border {
                    BorderStyle::Box => (rect.start_col + 1, rect.start_row + 1, rect.width.saturating_sub(2)),
                    BorderStyle::Line => (rect.start_col, rect.start_row + 1, rect.width), // Line 边框占 1 行顶边
                    BorderStyle::None => (rect.start_col, rect.start_row, rect.width),
                };

                let width = inner_w as usize;

                // 清空行
                out.queue(cursor::MoveTo(inner_col, inner_row))?;
                out.queue(style::Print(" ".repeat(width)))?;
                out.queue(cursor::MoveTo(inner_col, inner_row))?;

                // 显示前缀
                let prefix_display = &input.prefix;
                out.queue(style::SetForegroundColor(Color::Yellow))?;
                out.queue(style::SetAttribute(style::Attribute::Bold))?;
                out.queue(style::Print(prefix_display))?;
                out.queue(style::SetAttribute(style::Attribute::NormalIntensity))?;
                out.queue(style::SetForegroundColor(Color::Reset))?;

                let prefix_len = prefix_display.chars().count();
                let content_width = width.saturating_sub(prefix_len);

                // 显示输入内容
                let display: String = input.buffer.chars().take(content_width).collect();
                out.queue(style::Print(&display))?;

                // 光标
                if input.cursor <= content_width {
                    let cursor_col = inner_col + prefix_len as u16 + input.cursor as u16;
                    out.queue(cursor::MoveTo(cursor_col, inner_row))?;
                    out.queue(style::SetAttribute(style::Attribute::Reverse))?;
                    let cursor_char = input.buffer.chars().nth(input.cursor).unwrap_or(' ');
                    out.queue(style::Print(cursor_char.to_string()))?;
                    out.queue(style::SetAttribute(style::Attribute::Reset))?;
                }
            }
            None => {}
        }
    }

    // 全局错误提示直接覆盖在状态栏上，不再多占一行
    if let Some(err) = &ctx.engine.last_error {
        if let Some(rect) = status_rect_opt {
            out.queue(cursor::MoveTo(rect.start_col, rect.start_row))?;
            let cover_width = rect.width as usize;
            out.queue(style::Print(" ".repeat(cover_width)))?;
            out.queue(cursor::MoveTo(rect.start_col, rect.start_row))?;
            out.queue(style::SetBackgroundColor(Color::Red))?;
            out.queue(style::SetForegroundColor(Color::White))?;
            // 【修复】截断错误信息，防止溢出导致换行破坏布局
            let err_text = format!(" ERR: {} ", err);
            let display_err: String = err_text.chars().take(cover_width).collect();
            out.queue(style::Print(&display_err))?;
            out.queue(style::SetAttribute(style::Attribute::Reset))?;
        }
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
