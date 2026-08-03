// src/ui/mod.rs

pub mod primitives;
pub mod renderer;
pub mod tree_view;

use crate::app::{Component, Engine, Focus};
use crate::layout::{WindowRect, BorderStyle};
use crossterm::style::Color;
use std::io::Write;
use crossterm::{cursor, QueueableCommand};
use unicode_width::UnicodeWidthStr;

use renderer::WindowRenderer;
use primitives::{clear_specific_rect, clear_rect_diff, clip_rect_to_term, draw_border};
use tree_view::draw_tree_window;

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

// ================= 2. 渲染管线 =================

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

    // 【浮动窗口保护】如果底层(Z=0)有窗口刷新(如按j/k滚动)，上层(Z>0)的浮动窗口像素会被底层擦除。
    // 必须强制所有可见的浮窗也标脏，以便在底层重绘后再次重绘自己，恢复被破坏的像素。
    let has_dirty_base = all_rects.iter().any(|(_, n, _, z)| *z == 0 && ctx.engine.dirty_components.contains(n));
    if has_dirty_base {
        for (_, n, _, z) in all_rects.iter() {
            if *z > 0 {
                ctx.engine.dirty_components.insert(n.clone());
            }
        }
    }

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

        let border_color = if is_focused { ctx.engine.ui_theme.border_focused } else { ctx.engine.ui_theme.border_unfocused };
        draw_border(out, &safe_rect, title, *border, border_color, border_chars)?;

        let mut renderer = WindowRenderer::new(out, safe_rect, *border);

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

                    let style = TextStyle { fg: ctx.engine.ui_theme.statusbar_fg, ..Default::default() };
                    renderer.print(0, 0, &status_text, style, 0)?;
                }
                Component::Input(input) => {
                    if input.is_active {
                        renderer.clear_row(0)?;
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
