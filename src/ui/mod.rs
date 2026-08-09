// src/ui/mod.rs

pub mod primitives;
pub mod renderer;
pub mod tree_view;
pub mod buffer;

use crate::app::{Component, Engine, Focus};
use crate::layout::{WindowRect, BorderStyle};
use crossterm::style::Color;
use std::io::Write;
use crossterm::{cursor, QueueableCommand};
use unicode_width::UnicodeWidthStr;

use renderer::WindowRenderer;
use primitives::clip_rect_to_term;
use tree_view::draw_tree_window;
use buffer::Buffer;

use std::cell::RefCell;

thread_local! {
    static PREV_BUFFER: RefCell<Option<Buffer>> = RefCell::new(None);
    // 【优化1】新增 CURR_BUFFER 用于复用
    static CURR_BUFFER: RefCell<Option<Buffer>> = RefCell::new(None);
    static CURSOR_POS: RefCell<Option<(u16, u16)>> = RefCell::new(None);
}

#[derive(Debug, Clone, Copy)]
pub struct TermSize {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug)]
pub struct RenderCtx<'a> {
    pub engine: &'a mut Engine,
    pub style_engine: &'a crate::style::StyleEngine,
    pub term_size: TermSize,
}

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

pub fn render_all<W: Write>(ctx: &mut RenderCtx, all_rects: &[(WindowRect, String, BorderStyle, usize)], out: &mut W) -> std::io::Result<()> {
    let term_width = ctx.term_size.columns as usize;
    let term_height = ctx.term_size.rows as usize;

    let force_full = ctx.engine.prev_rects.is_empty();
    if force_full { out.queue(crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?; }

    CURSOR_POS.with(|cp| cp.replace(None));

    // 【优化1】获取或分配 CURR_BUFFER
    let mut curr_buffer_opt = CURR_BUFFER.with(|c| c.borrow_mut().take());
    let mut curr_buffer = if curr_buffer_opt.as_ref().map_or(true, |p| p.width != term_width || p.height != term_height) {
        Buffer::empty(term_width, term_height)
    } else {
        let mut buf = curr_buffer_opt.take().unwrap();
        buf.clear(); // 复用内存
        buf
    };

    let mut visible_rects: Vec<&(WindowRect, String, BorderStyle, usize)> = all_rects.iter().filter(|r| ctx.engine.components.contains_key(&r.1)).collect();
    visible_rects.sort_by_key(|e| e.3);

    // 【修复借用冲突】提取所需数据并克隆，立即释放不可变借用
    let active_input_data = ctx.engine.components.values()
        .find_map(|c| if let Component::Input(i) = c {
            if i.is_active {
                Some((i.prefix.clone(), i.buffer.clone(), i.cursor as u16))
            } else { None }
        } else { None });

    for (rect, name, border, _z_index) in visible_rects {
        let safe_rect = match clip_rect_to_term(rect, term_width as u16, term_height as u16) {
            Some(r) => r, None => continue,
        };

        let comp = ctx.engine.components.get(name);
        let title = comp.map(|_| name.as_str());
        let is_focused = ctx.engine.focus.current == Focus::Component(name.clone());
        let border_chars = ctx.engine.border_chars.get(name).map(|s| s.as_str());
        let border_color = if is_focused { ctx.engine.ui_theme.border_focused } else { ctx.engine.ui_theme.border_unfocused };

        primitives::draw_border(&mut curr_buffer, &safe_rect, title, *border, border_color, border_chars)?;
        // 【修复】浮动窗口必须强制清空背景，防止底层内容透出
        let is_floating = *_z_index > 0;
        let mut renderer = WindowRenderer::new(&mut curr_buffer, safe_rect, *border, is_floating);

        if let Some(Component::Tree(t)) = ctx.engine.components.get_mut(name) {
            // ... (保留 Tree 渲染逻辑不变)
            let max_rows = renderer.content_height() as usize;
            t.v_scroll = calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows, t.v_scroll);
            let scroll_offset = t.v_scroll;
            let drawn = draw_tree_window(&mut renderer, t, ctx.style_engine, scroll_offset, is_focused, &ctx.engine.ui_theme)?;
            for i in drawn as u16..max_rows as u16 { renderer.clear_row(i)?; }
        } else if let Some(comp) = ctx.engine.components.get(name) {
            match comp {
                Component::View(v) => {
                    // ... (保留 View 渲染逻辑不变)
                    let content = v.content_buffer.as_str();
                    let lines: Vec<&str> = content.lines().collect();
                    let max_rows = renderer.content_height() as usize;
                    let max_offset = lines.len().saturating_sub(max_rows);
                    let actual_offset = v.scroll_offset.min(max_offset);
                    let color = if is_focused { ctx.engine.ui_theme.view_focused } else { ctx.engine.ui_theme.view_unfocused };
                    let style = TextStyle { fg: color, ..Default::default() };
                    for i in 0..max_rows {
                        if let Some(line) = lines.get(i + actual_offset) { renderer.print(0, i as u16, line, style, v.h_scroll)?; }
                        else { renderer.clear_row(i as u16)?; }
                    }
                }
                Component::StatusBar(s) => {
                    if let Some((prefix, buffer, cursor)) = &active_input_data {
                        // 有 Input 激活：劫持此 StatusBar 的物理区域渲染输入框
                        renderer.clear_row(0)?;
                        let prefix_style = TextStyle { fg: ctx.engine.ui_theme.input_prefix, ..Default::default() };
                        renderer.print(0, 0, prefix, prefix_style, 0)?;
                        let prefix_w = UnicodeWidthStr::width(prefix.as_str()) as u16;
                        let buffer_style = TextStyle { fg: ctx.engine.ui_theme.input_buffer, ..Default::default() };
                        renderer.print(prefix_w, 0, buffer, buffer_style, 0)?;
                        renderer.show_cursor(prefix_w + *cursor, 0)?;
                    } else {
                        // 无 Input 激活：正常渲染状态栏
                        let mut status_text = s.format_template.clone();

                        // 【修复】优先渲染临时消息，如果未过期
                        let show_msg = if let Some(expire) = s.message_expire {
                            std::time::Instant::now() < expire
                        } else {
                            false
                        };
                        if show_msg {
                            if let Some(msg) = &s.message {
                                status_text = msg.clone();
                            }
                        }

                        status_text = status_text.replace("{stree_focus}", match &ctx.engine.focus.current { Focus::Component(n) => n, _ => "None" });
                        if let Some(t) = ctx.engine.get_focused_tree_state() {
                            status_text = status_text.replace("{stree_visible}", &t.visible_ids.len().to_string());
                            status_text = status_text.replace("{stree_total}", &t.dataset.entities.len().to_string());
                            status_text = status_text.replace("{stree_marked}", &t.marked_ids.len().to_string());
                            status_text = status_text.replace("{stree_id}", t.selected_id.as_deref().unwrap_or(""));
                        }
                        let max_rows = renderer.content_height() as u16;
                        for i in 0..max_rows { renderer.clear_row(i)?; }
                        let style = TextStyle { fg: ctx.engine.ui_theme.statusbar_fg, ..Default::default() };
                        renderer.print(0, 0, &status_text, style, 0)?;
                    }
                }
                _ => {}
            }
        }
    }

    // 【修改】兜底机制也使用克隆出的数据
    if active_input_data.is_some() {
        let has_statusbar = ctx.engine.components.values().any(|c| matches!(c, Component::StatusBar(_)));
        if !has_statusbar {
            let bottom_y = (term_height as u16).saturating_sub(1);
            let fake_rect = WindowRect { start_col: 0, start_row: bottom_y, width: term_width as u16, height: 1 };
            let mut fake_renderer = WindowRenderer::new(&mut curr_buffer, fake_rect, BorderStyle::None, true);

            if let Some((prefix, buffer, cursor)) = &active_input_data {
                fake_renderer.clear_row(0)?;
                let prefix_style = TextStyle { fg: ctx.engine.ui_theme.input_prefix, ..Default::default() };
                fake_renderer.print(0, 0, prefix, prefix_style, 0)?;
                let prefix_w = UnicodeWidthStr::width(prefix.as_str()) as u16;
                let buffer_style = TextStyle { fg: ctx.engine.ui_theme.input_buffer, ..Default::default() };
                fake_renderer.print(prefix_w, 0, buffer, buffer_style, 0)?;
                fake_renderer.show_cursor(prefix_w + *cursor, 0)?;
            }
        }
    }

    // 3. 全局错误提示直接画到 Buffer
    if let Some(err) = &ctx.engine.last_error {
        let status_rect_opt = all_rects.iter().find(|(_, n, _, _)| matches!(ctx.engine.components.get(n), Some(Component::StatusBar(_)))).map(|(r, _, _, _)| *r);
        if let Some(rect) = status_rect_opt {
            let safe_rect = clip_rect_to_term(&rect, term_width as u16, term_height as u16).unwrap_or(rect);
            let mut err_renderer = WindowRenderer::new(&mut curr_buffer, safe_rect, BorderStyle::None, true);
            err_renderer.clear_row(0)?;
            let err_text = format!(" ERR: {} ", err);
            let style = TextStyle { fg: ctx.engine.ui_theme.error_fg, bg: Some(ctx.engine.ui_theme.error_bg), ..Default::default() };
            err_renderer.print(0, 0, &err_text, style, 0)?;
        }
    }

    // ==========================================
    // 【优化1】Diff 并交换缓冲区
    // ==========================================
    PREV_BUFFER.with(|prev_cell| -> std::io::Result<()> {
        let mut prev_opt = prev_cell.borrow_mut();
        if force_full || prev_opt.as_ref().map_or(true, |p| p.width != term_width || p.height != term_height) {
            *prev_opt = Some(Buffer::empty(term_width, term_height));
        }

        let prev = prev_opt.as_ref().unwrap();
        curr_buffer.diff_and_flush(prev, out)?;

        // 将当前帧所有权移入 PREV_BUFFER，把旧的 prev 移回 CURR_BUFFER 供下一帧复用
        let old_prev = prev_opt.replace(curr_buffer);
        CURR_BUFFER.with(|c| { *c.borrow_mut() = old_prev; });

        Ok(())
    })?;

    // 6. 处理光标显示
    if ctx.engine.has_active_input() {
        out.queue(cursor::Show)?;
        CURSOR_POS.with(|cp| {
            if let Some((x, y)) = *cp.borrow() {
                let _ = out.queue(cursor::MoveTo(x, y));
            }
        });
    } else {
        out.queue(cursor::Hide)?;
    }

    // 7. 更新业务层的快照和脏标记
    // 【优化4】直接 clear 并复用已有 HashMap，避免每帧重新分配内存
    ctx.engine.prev_rects.clear();
    ctx.engine.prev_rects.reserve(all_rects.len());
    for (r, n, _, _) in all_rects {
        ctx.engine.prev_rects.insert(n.clone(), *r);
    }
    ctx.engine.dirty_components.clear();

    out.flush()?;
    Ok(())
}

pub fn calc_scroll_offset(selected_idx: usize, visible_count: usize, max_rows: usize, current_offset: usize) -> usize {
    if visible_count <= max_rows { return 0; }
    let max_offset = visible_count - max_rows;
    if selected_idx < current_offset {
        selected_idx
    } else if selected_idx >= current_offset + max_rows {
        selected_idx - max_rows + 1
    } else {
        current_offset
    }.min(max_offset)
}
