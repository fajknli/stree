// src/ui/mod.rs

use crate::app::{Component, Engine, Focus};
use crate::layout::{WindowRect, BorderStyle};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use crossterm::style::Color;
use std::io::Write;

const BLANK_SPACES: &str = "                                                                                                                                                                                                                                                                ";


// src/ui/mod.rs 顶部
#[derive(Debug, Clone, Copy)]
pub struct TermSize {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug)]
pub struct RenderCtx<'a> {
    pub engine: &'a mut Engine,
    pub style_engine: &'a crate::style::StyleEngine,
    pub term_size: TermSize, // 改用自定义的
}

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

    /// 核心绘制 API：在局部坐标 (x, y) 处打印文本
    pub fn print(&mut self, x: u16, y: u16, text: &str, style: TextStyle, h_offset: usize) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x + x;
        let real_y = self.rect.start_row + self.offset_y + y;
        let max_w = self.content_w.saturating_sub(x);
        if max_w == 0 { return Ok(()); }

        self.draw_clipped_text(real_x, real_y, text, max_w, None, style, h_offset)
    }

    /// 清空指定行（用空格填充）
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

    /// 光标定位（自动应用坐标变换）
    pub fn show_cursor(&mut self, x: u16, y: u16) -> std::io::Result<()> {
        if y >= self.content_h { return Ok(()); }
        let real_x = self.rect.start_col + self.offset_x + x;
        let real_y = self.rect.start_row + self.offset_y + y;
        self.out.queue(cursor::MoveTo(real_x, real_y))?;
        Ok(())
    }

    /// 内部函数：执行实际的截断与绘制（【极致优化】零内存分配版本）
    fn draw_clipped_text(
        &mut self, start_col: u16, row: u16, text: &str, max_width: u16,
        _highlight: Option<&str>, text_style: TextStyle, h_offset: usize
    ) -> std::io::Result<()> {
        if max_width == 0 { return Ok(()); }
        let max_w = max_width as usize;

        // 清空该行区域
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

        // 应用基础样式
        self.out.queue(style::SetForegroundColor(text_style.fg))?;
        if let Some(bg) = text_style.bg { self.out.queue(style::SetBackgroundColor(bg))?; }
        if text_style.bold { self.out.queue(style::SetAttribute(style::Attribute::Bold))?; }

        let mut chars = text.char_indices().peekable();
        let mut skipped_w = 0;
        let mut current_w = 0;
        let mut plain_start = None;

        while let Some((i, c)) = chars.next() {
            if c == '\x1b' {
                // 遇到控制序列，先把前面累积的普通文本批量输出
                if let Some(start) = plain_start.take() {
                    self.out.write_all(text[start..i].as_bytes())?;
                }

                // 扫描 ANSI 控制序列的结束位置
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
                // 【零分配核心】：直接切片原字符串输出，不分配任何 String
                self.out.write_all(text[i..end_idx].as_bytes())?;
            } else {
                let cw = c.width().unwrap_or(0);

                if skipped_w < h_offset {
                    skipped_w += cw;
                    continue;
                }

                if current_w + cw > max_w {
                    // 【关键修复】截断前先刷掉已累积的 batch，并清空 plain_start
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

        // 退出循环后，如果没有被截断，把剩余的普通文本刷出去
        // （如果被截断了，plain_start 已经在 break 前 take() 掉了，这里是 None）
        if let Some(start) = plain_start {
            self.out.write_all(text[start..].as_bytes())?;
        }

        // 重置样式
        self.out.queue(style::SetForegroundColor(Color::Reset))?;
        if text_style.bg.is_some() { self.out.queue(style::SetBackgroundColor(Color::Reset))?; }
        if text_style.bold { self.out.queue(style::SetAttribute(style::Attribute::NormalIntensity))?; }

        Ok(())
    }
}

// ================= 3. 渲染管线与组件提纯 =================

fn clear_specific_rect<W: Write>(out: &mut W, rect: &WindowRect) -> std::io::Result<()> {
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
fn clear_rect_diff<W: Write>(out: &mut W, old: &WindowRect, new: &WindowRect) -> std::io::Result<()> {
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

fn clip_rect_to_term(rect: &WindowRect, term_width: u16, term_height: u16) -> Option<WindowRect> {
    let mut r = *rect;
    if r.start_col >= term_width || r.start_row >= term_height { return None; }
    if r.start_col + r.width > term_width { r.width = term_width - r.start_col; }
    if r.start_row + r.height > term_height { r.height = term_height - r.start_row; }
    if r.width == 0 || r.height == 0 { return None; }
    Some(r)
}

pub fn draw_border<W: Write>(out: &mut W, rect: &WindowRect, title: Option<&str>, border: BorderStyle, border_color: Color, border_chars: Option<&str>) -> std::io::Result<()> {
    // 保持原样，画外框
    let width = rect.width as usize;
    if width < 2 { return Ok(()); }
    if border == BorderStyle::Box && rect.height < 2 { return Ok(()); }

    let x = rect.start_col;
    let y_top = rect.start_row;
    let y_bottom = rect.start_row + rect.height - 1;

    // 删除了原来根据 is_focused 计算颜色的逻辑，直接使用传入的 border_color
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
    is_focused: bool,
    ui_theme: &crate::style::UiTheme, // 【新增】
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

        let mut display = String::with_capacity(depth * 2 + entity.display.len() + 5);
        display.push(' ');
        for _ in 0..depth * 2 { display.push(' '); }

        let has_children = tree.dataset.child_index.contains_key(id);
        let is_expanded = tree.expanded_ids.contains(id);
        if has_children { display.push(if is_expanded { 'v' } else { '>' }); } else { display.push(' '); }
        display.push(' ');
        display.push_str(&entity.display);

        let mut tags_with_state = String::with_capacity(entity.tags.len() + 24);
        tags_with_state.push_str(&entity.tags);
        if is_selected {
            if !tags_with_state.is_empty() { tags_with_state.push(','); }
            tags_with_state.push_str("__selected__");
        }
        if is_marked {
            if !tags_with_state.is_empty() { tags_with_state.push(','); }
            tags_with_state.push_str("__marked__");
        }
        let (fg_color, is_bold) = style_engine.get_style(&tags_with_state);

        let final_fg = if is_focused {
            fg_color.unwrap_or(ui_theme.view_focused)
        } else {
            fg_color.unwrap_or(ui_theme.view_unfocused)
        };

        let bg_color = if is_selected && is_focused { Some(ui_theme.selected_bg) } else { None };
        let style = TextStyle { fg: final_fg, bg: bg_color, bold: is_bold };

        renderer.print(0, drawn as u16, &display, style, tree.h_scroll)?;
        drawn += 1;
    }

    if drawn == 0 && tree.visible_ids.is_empty() {
        let style = TextStyle { fg: if is_focused { ui_theme.view_focused } else { ui_theme.empty_data_fg }, ..Default::default() };
        renderer.print(0, 0, "No data available", style, 0)?;
        drawn = 1;
    }
    Ok(drawn)
}

pub fn render_all<W: Write>(ctx: &mut RenderCtx, all_rects: &[(WindowRect, String, BorderStyle, usize)], out: &mut W) -> std::io::Result<()> {
    let term_width = ctx.term_size.columns;
    let term_height = ctx.term_size.rows;

    let force_full = ctx.engine.prev_rects.is_empty();
    if force_full {
        out.queue(crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?;
    }

    let mut status_rect_opt: Option<WindowRect> = None;
    for (rect, name, _, _) in all_rects {
        if let Some(comp) = ctx.engine.components.get(name) {
            if matches!(comp, Component::StatusBar(_)) {
                if let Some(safe) = clip_rect_to_term(rect, term_width, term_height) {
                    status_rect_opt = Some(safe);
                }
            }
        }
    }

    let mut current_rects_map = std::collections::HashMap::new();
    for (r, n, _, _) in all_rects {
        current_rects_map.insert(n.clone(), *r);
    }

    // 1. 清除被移除的窗口的旧区域
    if !force_full {
        let current_names: std::collections::HashSet<&String> = all_rects.iter().map(|(_, n, _, _)| n).collect();
        for (name, old_rect) in &ctx.engine.prev_rects {
            if !current_names.contains(name) {
                if let Some(old_safe) = clip_rect_to_term(old_rect, term_width, term_height) {
                    clear_specific_rect(out, &old_safe)?;
                }
            }
        }
    }

    // ==========================================
    // 阶段 1：统一擦除所有脏窗口的旧残影
    // ==========================================
    for (rect, name, _, _) in all_rects {
        let safe_rect = match clip_rect_to_term(rect, term_width, term_height) {
            Some(r) => r,
            None => continue,
        };

        let is_dirty = force_full
            || ctx.engine.dirty_components.contains(name)
            || ctx.engine.prev_rects.get(name).copied() != Some(*rect);

        if !is_dirty { continue; }

        if !force_full {
            if let Some(old) = ctx.engine.prev_rects.get(name) {
                if let Some(old_safe) = clip_rect_to_term(old, term_width, term_height) {
                    // 【修复】恢复使用 clear_rect_diff，不再全量擦除 4 条边！
                    // 这样绝不会擦除没有移动的共享边框（如 A|B），避免边框消失。
                    clear_rect_diff(out, &old_safe, &safe_rect)?;
                }
            }
        } else {
            clear_specific_rect(out, &safe_rect)?;
        }
    }

    // ==========================================
    // 阶段 2：统一绘制所有脏窗口的新内容
    // ==========================================
    for (rect, name, border, _z_index) in all_rects {
        let safe_rect = match clip_rect_to_term(rect, term_width, term_height) {
            Some(r) => r,
            None => continue,
        };

        let is_dirty = force_full
            || ctx.engine.dirty_components.contains(name)
            || ctx.engine.prev_rects.get(name).copied() != Some(*rect);

        if !is_dirty { continue; }

        let comp = ctx.engine.components.get(name);
        let title = comp.map(|_| name.as_str());
        let is_focused = ctx.engine.focus.current == Focus::Component(name.clone());
        let border_chars = ctx.engine.border_chars.get(name).map(|s| s.as_str());

        // 【修改】从主题获取边框颜色
        let border_color = if is_focused { ctx.engine.ui_theme.border_focused } else { ctx.engine.ui_theme.border_unfocused };
        draw_border(out, &safe_rect, title, *border, border_color, border_chars)?;

        let mut renderer = WindowRenderer::new(out, safe_rect, *border);

        // 【关键修复】因为 Tree 组件需要更新 v_scroll 状态，所以必须用 get_mut 获取可变引用
        if let Some(Component::Tree(t)) = ctx.engine.components.get_mut(name) {
            let max_rows = renderer.content_height() as usize;
            t.v_scroll = calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows, t.v_scroll);
            let scroll_offset = t.v_scroll;

            let drawn = draw_tree_window(&mut renderer, t, ctx.style_engine, scroll_offset, is_focused, &ctx.engine.ui_theme)?;
            for i in drawn as u16..max_rows as u16 {
                renderer.clear_row(i)?;
            }
        } else if let Some(comp) = ctx.engine.components.get(name) {
            match comp {
                Component::View(v) => {
                    let content = v.content_buffer.as_str();
                    let lines: Vec<&str> = content.lines().collect();
                    let max_rows = renderer.content_height() as usize;
                    let max_offset = lines.len().saturating_sub(max_rows);
                    let actual_offset = v.scroll_offset.min(max_offset);

                    // 【修改】View 组件颜色从主题获取
                    let color = if is_focused { ctx.engine.ui_theme.view_focused } else { ctx.engine.ui_theme.view_unfocused };
                    let style = TextStyle { fg: color, ..Default::default() };

                    for i in 0..max_rows {
                        if let Some(line) = lines.get(i + actual_offset) {
                            renderer.print(0, i as u16, line, style, v.h_scroll)?;
                        } else {
                            renderer.clear_row(i as u16)?;
                        }
                    }
                }
                Component::StatusBar(s) => {
                    let mut status_text = s.format_template.clone();
                    status_text = status_text.replace("{stree_focus}", match &ctx.engine.focus.current { Focus::Component(n) => n, _ => "None" });
                    if let Some(t) = ctx.engine.get_focused_tree_state() {
                        status_text = status_text.replace("{stree_visible}", &t.visible_ids.len().to_string());
                        status_text = status_text.replace("{stree_total}", &t.dataset.entities.len().to_string());
                        status_text = status_text.replace("{stree_marked}", &t.marked_ids.len().to_string());
                        status_text = status_text.replace("{stree_id}", t.selected_id.as_deref().unwrap_or(""));
                    }

                    let max_rows = renderer.content_height() as u16;
                    for i in 0..max_rows {
                        renderer.clear_row(i)?;
                    }

                    // 【修改】状态栏颜色从主题获取
                    let style = TextStyle { fg: ctx.engine.ui_theme.statusbar_fg, ..Default::default() };
                    renderer.print(0, 0, &status_text, style, 0)?;
                }
                Component::Input(input) => {
                    if input.is_active {
                        renderer.clear_row(0)?;
                        // 【修改】输入框颜色从主题获取
                        let prefix_style = TextStyle { fg: ctx.engine.ui_theme.input_prefix, ..Default::default() };
                        renderer.print(0, 0, &input.prefix, prefix_style, 0)?;

                        let prefix_w = UnicodeWidthStr::width(input.prefix.as_str()) as u16;
                        let buffer_style = TextStyle { fg: ctx.engine.ui_theme.input_buffer, ..Default::default() };
                        renderer.print(prefix_w, 0, &input.buffer, buffer_style, 0)?;

                        renderer.show_cursor(prefix_w + input.cursor as u16, 0)?;
                    } else {
                        let max_rows = renderer.content_height() as u16;
                        for i in 0..max_rows {
                            renderer.clear_row(i)?;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if let Some(err) = &ctx.engine.last_error {
        if let Some(rect) = status_rect_opt {
            let mut err_renderer = WindowRenderer::new(out, rect, BorderStyle::None);
            err_renderer.clear_row(0)?;
            let err_text = format!(" ERR: {} ", err);
            // 【修改】错误提示颜色从主题获取
            let style = TextStyle { fg: ctx.engine.ui_theme.error_fg, bg: Some(ctx.engine.ui_theme.error_bg), ..Default::default() };
            err_renderer.print(0, 0, &err_text, style, 0)?;
        }
    }

    if ctx.engine.has_active_input() {
        out.queue(cursor::Show)?;
    } else {
        out.queue(cursor::Hide)?;
    }

    ctx.engine.prev_rects = current_rects_map;
    ctx.engine.dirty_components.clear();

    out.flush()?;
    Ok(())
}

pub fn calc_scroll_offset(selected_idx: usize, visible_count: usize, max_rows: usize, current_offset: usize) -> usize {
    if visible_count <= max_rows { return 0; }
    let max_offset = visible_count - max_rows;

    if selected_idx < current_offset {
        selected_idx // 光标在可视区上方，把它变成最顶行
    } else if selected_idx >= current_offset + max_rows {
        selected_idx - max_rows + 1 // 光标在可视区下方，把它变成最底行
    } else {
        current_offset // 光标在可视区内，保持原样不动
    }.min(max_offset)
}
