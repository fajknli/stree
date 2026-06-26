// src/ui/mod.rs

use crate::app::{Component, Engine, Focus};
use crate::layout::{WindowRect, BorderStyle};
use crossterm::terminal::WindowSize;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use crossterm::style::Color;

#[derive(Debug)]
pub struct RenderCtx<'a> {
    pub engine: &'a Engine,
    pub style_engine: &'a crate::style::StyleEngine,
    pub term_size: WindowSize,
}

use std::io::Write;
use crossterm::{cursor, style, QueueableCommand};

// ================= 1. 样式抽象 =================

/// 统一的文本样式，消灭组件里零散的 SetForegroundColor / SetBackgroundColor
#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub fg: Color,
    pub bg: Option<Color>,
    pub bold: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self { fg: Color::Reset, bg: None, bold: false }
    }
}

// ================= 2. 核心：WindowRenderer =================

/// 局部渲染上下文：封装坐标变换与边界裁剪
pub struct WindowRenderer<'a, W: Write> {
    out: &'a mut W,
    rect: WindowRect,
    offset_x: u16, // 内容区相对于 rect.start_col 的偏移
    offset_y: u16, // 内容区相对于 rect.start_row 的偏移
    content_w: u16,
    content_h: u16,
}

impl<'a, W: Write> WindowRenderer<'a, W> {
    pub fn new(out: &'a mut W, rect: WindowRect, border: BorderStyle) -> Self {
        let (offset_x, offset_y, overhead_x, overhead_y) = match border {
            BorderStyle::Box => (1, 1, 2, 2),
            BorderStyle::Line => (0, 1, 0, 1), // Line 顶部有一根线，Y偏移1，高度减1
            BorderStyle::None => (0, 0, 0, 0),
        };
        Self {
            out,
            rect,
            offset_x,
            offset_y,
            content_w: rect.width.saturating_sub(overhead_x),
            content_h: rect.height.saturating_sub(overhead_y),
        }
    }

    pub fn content_width(&self) -> u16 { self.content_w }
    pub fn content_height(&self) -> u16 { self.content_h }

    /// 核心绘制 API：在局部坐标 (x, y) 处打印文本
    pub fn print(&mut self, x: u16, y: u16, text: &str, style: TextStyle) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x + x;
        let real_y = self.rect.start_row + self.offset_y + y;
        let max_w = self.content_w.saturating_sub(x);
        if max_w == 0 { return Ok(()); }

        self.draw_clipped_text(real_x, real_y, text, max_w, None, style)
    }

    /// 清空指定行（用空格填充）
    pub fn clear_row(&mut self, y: u16) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x;
        let real_y = self.rect.start_row + self.offset_y + y;
        self.out.queue(cursor::MoveTo(real_x, real_y))?;
        self.out.queue(style::Print(" ".repeat(self.content_w as usize)))?;
        Ok(())
    }

    /// 光标定位（自动应用坐标变换）
    pub fn show_cursor(&mut self, x: u16, y: u16) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x + x;
        let real_y = self.rect.start_row + self.offset_y + y;
        self.out.queue(cursor::MoveTo(real_x, real_y))?;
        Ok(())
    }

    /// 内部函数：执行实际的截断与绘制（修复了 ~ 越界 Bug）
    fn draw_clipped_text(
        &mut self, start_col: u16, row: u16, text: &str, max_width: u16,
        highlight: Option<&str>, text_style: TextStyle
    ) -> std::io::Result<()> {
        if max_width == 0 { return Ok(()); }
        let max_w = max_width as usize;

        // 清空该行区域
        self.out.queue(cursor::MoveTo(start_col, row))?;
        self.out.queue(style::Print(" ".repeat(max_w)))?;
        self.out.queue(cursor::MoveTo(start_col, row))?;

        // 应用基础样式
        self.out.queue(style::SetForegroundColor(text_style.fg))?;
        if let Some(bg) = text_style.bg { self.out.queue(style::SetBackgroundColor(bg))?; }
        if text_style.bold { self.out.queue(style::SetAttribute(style::Attribute::Bold))?; }

        // 解析 ANSI 和文本段 (省略详细解析代码，复用原逻辑)
        let mut segments = Vec::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                if chars.peek() == Some(&'[') {
                    let mut ansi = String::from("\x1b[");
                    chars.next();
                    while let Some(&next_c) = chars.peek() {
                        ansi.push(next_c); chars.next();
                        if next_c.is_ascii_alphabetic() { break; }
                    }
                    segments.push(Segment::Ansi(ansi));
                } else {
                    segments.push(Segment::Text(String::from(c)));
                }
            } else {
                let mut text_seg = String::new();
                text_seg.push(c);
                while let Some(&next_c) = chars.peek() {
                    if next_c == '\x1b' { break; }
                    text_seg.push(next_c); chars.next();
                }
                segments.push(Segment::Text(text_seg));
            }
        }

        // 计算纯文本字符及其宽度
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

        // 【核心修复】：为 ~ 预留 1 格宽度，防止总宽度越界触发终端换行
        let mut keep_count = plain_chars.len();
        let mut truncated = false;
        if total_w > max_w {
            truncated = true;
            let target_w = max_w.saturating_sub(1);
            while total_w > target_w && keep_count > 0 {
                let (_, cw) = plain_chars[keep_count - 1];
                total_w -= cw;
                keep_count -= 1;
            }
        }

        // 高亮处理
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

        // 实际绘制
        let mut current_plain_count = 0;
        for seg in &segments {
            match seg {
                Segment::Ansi(s) => { self.out.queue(style::Print(s))?; }
                Segment::Text(s) => {
                    for c in s.chars() {
                        if current_plain_count < keep_count {
                            if let Some((start, end)) = highlight_indices {
                                if current_plain_count == start {
                                    self.out.queue(style::SetForegroundColor(Color::Yellow))?;
                                    self.out.queue(style::SetAttribute(style::Attribute::Bold))?;
                                }
                                self.out.queue(style::Print(c.to_string()))?;
                                if current_plain_count == end - 1 {
                                    self.out.queue(style::SetAttribute(style::Attribute::NormalIntensity))?;
                                    self.out.queue(style::SetForegroundColor(text_style.fg))?;
                                }
                            } else {
                                self.out.queue(style::Print(c.to_string()))?;
                            }
                            current_plain_count += 1;
                        }
                    }
                }
            }
        }

        if truncated { self.out.queue(style::Print("~"))?; }

        // 重置样式
        self.out.queue(style::SetForegroundColor(Color::Reset))?;
        if text_style.bg.is_some() { self.out.queue(style::SetBackgroundColor(Color::Reset))?; }
        if text_style.bold { self.out.queue(style::SetAttribute(style::Attribute::NormalIntensity))?; }

        Ok(())
    }
}

enum Segment { Text(String), Ansi(String) }

// ================= 3. 渲染管线与组件提纯 =================

fn clip_rect_to_term(rect: &WindowRect, term_width: u16, term_height: u16) -> Option<WindowRect> {
    let mut r = *rect;
    if r.start_col >= term_width || r.start_row >= term_height { return None; }
    if r.start_col + r.width > term_width { r.width = term_width - r.start_col; }
    if r.start_row + r.height > term_height { r.height = term_height - r.start_row; }
    if r.width == 0 || r.height == 0 { return None; }
    Some(r)
}

pub fn draw_border<W: Write>(out: &mut W, rect: &WindowRect, title: Option<&str>, border: BorderStyle, is_focused: bool, border_chars: Option<&str>) -> std::io::Result<()> {
    // 保持原样，画外框
    let width = rect.width as usize;
    if width < 2 { return Ok(()); }
    if border == BorderStyle::Box && rect.height < 2 { return Ok(()); }

    let x = rect.start_col;
    let y_top = rect.start_row;
    let y_bottom = rect.start_row + rect.height - 1;

    let border_color = if is_focused { Color::Green } else { Color::DarkGrey };
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
                out.queue(style::Print(vertical.to_string()))?;
                out.queue(cursor::MoveTo(x + rect.width - 1, y_top + row as u16))?;
                out.queue(style::Print(vertical.to_string()))?;
            }
        }
    }
    Ok(())
}

/// 【提纯】Tree 组件不再关心绝对坐标和边框开销
pub fn draw_tree_window<'a, W: Write>(
    renderer: &mut WindowRenderer<'a, W>,
    tree: &crate::app::TreeState,
    style_engine: &crate::style::StyleEngine,
    scroll_offset: usize,
    is_focused: bool
) -> std::io::Result<usize> {
    let max_rows = renderer.content_height() as usize;
    if max_rows == 0 { return Ok(0); }

    let start = scroll_offset;
    let end = (start + max_rows).min(tree.visible_ids.len());
    let mut drawn = 0;

    for i in start..end {
        let id = &tree.visible_ids[i];
        let depth = tree.visible_depths[i];
        let entity = &tree.dataset.entity_map[id];
        let is_selected = tree.selected_id.as_deref() == Some(id.as_str());
        let is_marked = tree.marked_ids.contains(id);

        let mut display = String::from(" ");
        for _ in 0..depth * 2 { display.push(' '); }

        let has_children = tree.dataset.child_index.contains_key(id);
        let is_expanded = tree.expanded_ids.contains(id);
        if has_children { display.push(if is_expanded { 'v' } else { '>' }); } else { display.push(' '); }
        display.push(' ');
        display.push_str(&entity.display);

        let mut tags_with_state = entity.tags.clone();
        if is_selected {
            if !tags_with_state.is_empty() { tags_with_state.push_str(","); }
            tags_with_state.push_str("__selected__");
        }
        if is_marked {
            if !tags_with_state.is_empty() { tags_with_state.push_str(","); }
            tags_with_state.push_str("__marked__");
        }
        let (fg_color, is_bold) = style_engine.get_style(&tags_with_state);
        let final_fg = if is_focused { fg_color.unwrap_or(Color::White) } else { fg_color.unwrap_or(Color::DarkGrey) };

        let bg_color = if is_selected && is_focused { Some(Color::DarkGrey) } else { None };

        let style = TextStyle { fg: final_fg, bg: bg_color, bold: is_bold };

        // 组件只管输出，不管坐标偏移，不管截断
        renderer.print(0, drawn as u16, &display, style)?;
        drawn += 1;
    }

    if drawn == 0 && tree.visible_ids.is_empty() {
        let style = TextStyle { fg: if is_focused { Color::White } else { Color::DarkGrey }, ..Default::default() };
        renderer.print(0, 0, "No data available", style)?;
        drawn = 1;
    }
    Ok(drawn)
}

/// 【提纯】多图层统一渲染入口，消灭所有 match border
pub fn render_all<W: Write>(ctx: &RenderCtx, out: &mut W) -> std::io::Result<()> {
    out.queue(crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?;
    let term_width = ctx.term_size.columns;
    let term_height = ctx.term_size.rows;

    let all_rects = ctx.engine.calc_all_rects(term_width, term_height);
    if all_rects.is_empty() { return Ok(()); }

    let mut status_rect_opt: Option<WindowRect> = None;

    for (rect, name, border, _z_index) in &all_rects {
        let safe_rect = match clip_rect_to_term(rect, term_width, term_height) {
            Some(r) => r,
            None => continue,
        };

        let comp = ctx.engine.components.get(name);
        let title = comp.map(|_| name.as_str());
        let is_focused = ctx.engine.focused == Focus::Component(name.clone());
        let border_chars = ctx.engine.border_chars.get(name).map(|s| s.as_str());

        // 1. 画物理边界
        draw_border(out, &safe_rect, title, *border, is_focused, border_chars)?;

        // 2. 实例化局部画笔
        let mut renderer = WindowRenderer::new(out, safe_rect, *border);

        // 3. 分发渲染（组件彻底失忆）
        match comp {
            Some(Component::Tree(t)) => {
                let max_rows = renderer.content_height() as usize;
                let scroll_offset = calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows);
                draw_tree_window(&mut renderer, t, ctx.style_engine, scroll_offset, is_focused)?;
            }
            Some(Component::View(v)) => {
                let content = v.content_buffer.as_str();
                let lines: Vec<&str> = content.lines().collect();
                let max_rows = renderer.content_height() as usize;
                let max_offset = lines.len().saturating_sub(max_rows);
                let actual_offset = v.scroll_offset.min(max_offset);
                let color = if is_focused { Color::White } else { Color::DarkGrey };
                let style = TextStyle { fg: color, ..Default::default() };

                for i in 0..max_rows {
                    if let Some(line) = lines.get(i + actual_offset) {
                        renderer.print(0, i as u16, line, style)?;
                    }
                }
            }
            Some(Component::StatusBar(s)) => {
                status_rect_opt = Some(safe_rect);
                let mut status_text = s.format_template.clone();
                status_text = status_text.replace("{stree_focus}", match &ctx.engine.focused { Focus::Component(n) => n, _ => "None" });
                if let Some(t) = ctx.engine.get_focused_tree_state() {
                    status_text = status_text.replace("{stree_visible}", &t.visible_ids.len().to_string());
                    status_text = status_text.replace("{stree_total}", &t.dataset.entities.len().to_string());
                    status_text = status_text.replace("{stree_marked}", &t.marked_ids.len().to_string());
                    status_text = status_text.replace("{stree_id}", t.selected_id.as_deref().unwrap_or(""));
                }

                renderer.clear_row(0)?;
                let style = TextStyle { fg: Color::White, ..Default::default() };
                renderer.print(0, 0, &status_text, style)?;
            }
            Some(Component::Input(input)) => {
                if !input.is_active { continue; }

                renderer.clear_row(0)?;
                let prefix_style = TextStyle { fg: Color::Yellow, ..Default::default() };
                renderer.print(0, 0, &input.prefix, prefix_style)?;

                let prefix_w = UnicodeWidthStr::width(input.prefix.as_str()) as u16;
                let buffer_style = TextStyle { fg: Color::White, ..Default::default() };
                renderer.print(prefix_w, 0, &input.buffer, buffer_style)?;

                renderer.show_cursor(prefix_w + input.cursor as u16, 0)?;
            }
            None => {}
        }
    }

    // 全局错误提示
    if let Some(err) = &ctx.engine.last_error {
        if let Some(rect) = status_rect_opt {
            let mut err_renderer = WindowRenderer::new(out, rect, BorderStyle::None);
            err_renderer.clear_row(0)?;
            let err_text = format!(" ERR: {} ", err);
            let style = TextStyle { fg: Color::White, bg: Some(Color::Red), ..Default::default() };
            err_renderer.print(0, 0, &err_text, style)?;
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
