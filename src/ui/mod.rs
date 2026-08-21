// src/ui/mod.rs

pub mod primitives;
pub mod renderer;
pub mod tree_view;
pub mod buffer;

use crate::app::{Component, Engine, Focus};
use crate::app::view::ViewContent; // 【新增】引入枚举
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
    if force_full {
        out.queue(crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?;
        for comp in ctx.engine.components.values_mut() {
            if let Component::View(v) = comp {
                // 【修改】如果是 Graphic 且非空，则标记 dirty
                if matches!(v.content, ViewContent::Graphic(_)) {
                    v.graphic_dirty = true;
                }
            }
        }
    }

    CURSOR_POS.with(|cp| cp.replace(None));

    let mut curr_buffer_opt = CURR_BUFFER.with(|c| c.borrow_mut().take());
    let mut curr_buffer = if curr_buffer_opt.as_ref().map_or(true, |p| p.width != term_width || p.height != term_height) {
        Buffer::empty(term_width, term_height)
    } else {
        let mut buf = curr_buffer_opt.take().unwrap();
        buf.clear();
        buf
    };

    // 【修改】pending_graphics 的 content 字段改为 Vec<u8>
    let mut pending_graphics: Vec<(u16, u16, u16, u16, String, Vec<u8>)> = Vec::new();
    let mut pending_clears: Vec<(u16, u16, u16, u16)> = Vec::new();

    let mut visible_rects: Vec<&(WindowRect, String, BorderStyle, usize)> = all_rects.iter().filter(|r| ctx.engine.components.contains_key(&r.1)).collect();
    visible_rects.sort_by_key(|e| e.3);

    for (rect, name, border, _z_index) in visible_rects {
        let safe_rect = match clip_rect_to_term(rect, term_width as u16, term_height as u16) {
            Some(r) => r, None => continue,
        };

        let rendering_name = {
            let mut target_name = name.clone();
            for layer in ctx.engine.overlay_stack.iter().rev() {
                if layer.target == *name {
                    target_name = layer.source.clone();
                    break;
                }
            }
            target_name
        };

        let comp_opt = ctx.engine.components.get_mut(&rendering_name);

        let title_string = comp_opt.as_ref().map(|c| {
            if let Component::Tree(t) = c {
                t.title_override.clone().unwrap_or_else(|| name.clone())
            } else {
                name.clone()
            }
        });
        let title = title_string.as_deref();

        let is_focused = ctx.engine.focus.current == Focus::Component(name.clone());
        let border_chars = ctx.engine.border_chars.get(name).map(|s| s.as_str());
        let border_color = if is_focused { ctx.engine.ui_theme.border_focused } else { ctx.engine.ui_theme.border_unfocused };

        primitives::draw_border(&mut curr_buffer, &safe_rect, title, *border, border_color, border_chars)?;
        let is_floating = *_z_index > 0;
        let mut renderer = WindowRenderer::new(&mut curr_buffer, safe_rect, *border, is_floating);

        if let Some(comp) = comp_opt {
            match comp {
                Component::Tree(t) => {
                    let max_rows = renderer.content_height() as usize;
                    t.v_scroll = calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows, t.v_scroll);
                    let scroll_offset = t.v_scroll;
                    let drawn = draw_tree_window(&mut renderer, t, ctx.style_engine, scroll_offset, is_focused, &ctx.engine.ui_theme)?;
                    for i in drawn as u16..max_rows as u16 { renderer.clear_row(i)?; }
                }
                Component::View(v) => {
                    match &v.content {
                        ViewContent::Graphic(_) => {
                            if v.graphic_dirty {
                                let offset_x = safe_rect.start_col + 1;
                                let offset_y = safe_rect.start_row + 1;
                                let img_w = renderer.content_width();
                                let img_h = renderer.content_height();

                                // 【核心优化】用 mem::take 移走内容，避免 4MB clone！
                                // 渲染完毕后在函数末尾放回去
                                if let ViewContent::Graphic(bytes) = std::mem::take(&mut v.content) {
                                    pending_graphics.push((offset_x, offset_y, img_w, img_h, rendering_name.clone(), bytes));
                                }
                                v.graphic_dirty = false;
                            }
                        }
                        ViewContent::Text(text) => {
                            if v.needs_graphic_clear {
                                let offset_x = safe_rect.start_col + 1;
                                let offset_y = safe_rect.start_row + 1;
                                let img_w = renderer.content_width();
                                let img_h = renderer.content_height();
                                pending_clears.push((offset_x, offset_y, img_w, img_h));
                                v.needs_graphic_clear = false;
                            }

                            let scroll_offset = v.scroll_offset;
                            let h_scroll = v.h_scroll;

                            let lines: Vec<&str> = text.lines().collect();
                            let max_rows = renderer.content_height() as usize;
                            let max_offset = lines.len().saturating_sub(max_rows);
                            let actual_offset = scroll_offset.min(max_offset);
                            let color = if is_focused { ctx.engine.ui_theme.view_focused } else { ctx.engine.ui_theme.view_unfocused };
                            let style = TextStyle { fg: color, ..Default::default() };
                            for i in 0..max_rows {
                                renderer.clear_row(i as u16)?;
                                if let Some(line) = lines.get(i + actual_offset) {
                                    renderer.print(0, i as u16, line, style, h_scroll)?;
                                }
                            }
                        }
                        ViewContent::Empty => {
                            // 空内容，只清理背景
                            let max_rows = renderer.content_height() as usize;
                            for i in 0..max_rows {
                                renderer.clear_row(i as u16)?;
                            }
                        }
                    }
                }
                Component::StatusBar(s) => {
                    let max_rows = renderer.content_height() as u16;
                    for i in 0..max_rows { renderer.clear_row(i)?; }
                    let style = TextStyle { fg: ctx.engine.ui_theme.statusbar_fg, ..Default::default() };
                    renderer.print(0, 0, &s.current_text, style, 0)?;
                }
                Component::Input(i) => {
                    let max_rows = renderer.content_height() as u16;
                    for r in 0..max_rows { renderer.clear_row(r)?; }

                    let prefix_style = TextStyle { fg: ctx.engine.ui_theme.input_prefix, ..Default::default() };
                    renderer.print(0, 0, &i.prefix, prefix_style, 0)?;
                    let prefix_w = UnicodeWidthStr::width(i.prefix.as_str()) as u16;
                    let buffer_style = TextStyle { fg: ctx.engine.ui_theme.input_buffer, ..Default::default() };
                    renderer.print(prefix_w, 0, &i.buffer, buffer_style, 0)?;
                    renderer.show_cursor(prefix_w + i.cursor as u16, 0)?;
                }
            }
        }
    }

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
    // 【新增】预分配可复用的空格缓冲区，消灭每行 b" ".repeat() 分配
    // ==========================================
    let mut space_buf: Vec<u8> = Vec::new();

    // 1. 先强制物理清除旧图片残留区域
    for (offset_x, offset_y, img_w, img_h) in &pending_clears {
        let _ = out.write_all(b"\x1b[0m");
        if space_buf.len() < *img_w as usize {
            space_buf.resize(*img_w as usize, b' ');
        }
        let spaces = &space_buf[..*img_w as usize];
        for i in 0..*img_h {
            let _ = out.queue(crossterm::cursor::MoveTo(*offset_x, *offset_y + i as u16));
            let _ = out.write_all(spaces);
        }
    }

    // 2. 执行文本 Diff 刷新
    PREV_BUFFER.with(|prev_cell| -> std::io::Result<()> {
        let mut prev_opt = prev_cell.borrow_mut();
        if force_full || prev_opt.as_ref().map_or(true, |p| p.width != term_width || p.height != term_height) {
            *prev_opt = Some(Buffer::empty(term_width, term_height));
        }

        let prev = prev_opt.as_ref().unwrap();
        curr_buffer.diff_and_flush(prev, out)?;

        let old_prev = prev_opt.replace(curr_buffer);
        CURR_BUFFER.with(|c| { *c.borrow_mut() = old_prev; });

        Ok(())
    })?;

    // 3. 统一绘制新图片（消费 pending_graphics，避免 clone）
    let mut recovered_content: Vec<(String, Vec<u8>)> = Vec::new();
    for (offset_x, offset_y, img_w, img_h, view_name, content) in pending_graphics {
        let _ = out.write_all(b"\x1b[0m");

        // 【恢复擦除逻辑】画图前必须用空格擦除背景，防止旧图片残影！
        if space_buf.len() < img_w as usize {
            space_buf.resize(img_w as usize, b' ');
        }
        let spaces = &space_buf[..img_w as usize];
        for i in 0..img_h {
            let _ = out.queue(crossterm::cursor::MoveTo(offset_x, offset_y + i as u16));
            let _ = out.write_all(spaces);
        }

        let _ = out.queue(crossterm::cursor::MoveTo(offset_x, offset_y));

        let _ = out.write_all(&content);
        let _ = out.write_all(b"\x1b\\\x1b[0m");

        recovered_content.push((view_name, content));
    }

    // 【新增】将内容放回视图，供下次全屏重绘使用
    for (view_name, content) in recovered_content {
        if let Some(Component::View(v)) = ctx.engine.components.get_mut(&view_name) {
            v.content = ViewContent::Graphic(content);
        }
    }

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
