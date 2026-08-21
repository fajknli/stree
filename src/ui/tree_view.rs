// src/ui/tree_view.rs

use super::renderer::WindowRenderer;
use super::TextStyle;
use crate::app::TreeState;
use crate::style::{StyleEngine, UiTheme};

/// 【提纯】Tree 组件不再关心绝对坐标和边框开销
pub fn draw_tree_window(
    renderer: &mut WindowRenderer,
    tree: &TreeState,
    style_engine: &StyleEngine,
    scroll_offset: usize,
    is_focused: bool,
    ui_theme: &UiTheme,
) -> std::io::Result<usize> {
    let max_rows = renderer.content_height() as usize;
    if max_rows == 0 { return Ok(0); }

    let start = scroll_offset;
    let end = (start + max_rows).min(tree.visible_ids.len());
    let mut drawn = 0;

    // 获取搜索词（如果有）
    let query_opt = tree.search_query.as_deref().filter(|q| !q.is_empty());

    let _highlight_start = "\x1b[38;2;201;59;59m";
    let _highlight_end = "\x1b[39m";

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

        // 【新增】高亮匹配的子串
        if let Some(query) = query_opt {
            // 【修复】使用 to_ascii_lowercase 保证字节长度不变，防止多字节切片 Panic
            let lower_query = query.to_ascii_lowercase();
            let lower_display = entity.display.to_ascii_lowercase();

            let mut last_idx = 0;
            for (idx, _) in lower_display.match_indices(&lower_query) {
                display.push_str(&entity.display[last_idx..idx]);
                let match_end = idx + query.len();
                display.push_str(&entity.display[idx..match_end]);
                last_idx = match_end;
            }
            display.push_str(&entity.display[last_idx..]);
        } else {
            display.push_str(&entity.display);
        }

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

        let bg_color = if is_selected { Some(ui_theme.selected_bg) } else { None };
        let style = TextStyle { fg: final_fg, bg: bg_color, bold: is_bold };

        // 【终极修复】画字前先清空该行，彻底消灭旧字符属性（bold/color）残留！
        renderer.clear_row(drawn as u16)?;
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
