// src/app/metrics.rs

use crate::app::{Component, Engine, Focus};
use crate::layout::{layout_ast_to_string, BorderStyle, WindowRect};
use std::collections::HashMap;

impl Engine {
    /// 预计算状态栏文本，在渲染前调用
    pub fn update_status_bars(&mut self, term_width: u16, term_height: u16, all_rects: &[(WindowRect, String, BorderStyle, usize)]) {
        let mut m: HashMap<String, String> = HashMap::new();
        self.collect_metrics_into(term_width, term_height, all_rects, &mut m);

        // 将外部注入的变量以 var_ 前缀合并到指标池
        for (k, v) in &self.custom_vars {
            m.insert(format!("var_{}", k), v.clone());
        }

        for (_, comp) in self.components.iter_mut() {
            if let Component::StatusBar(s) = comp {
                let mut status_text = s.format_template.clone();
                let show_msg = if let Some(expire) = s.message_expire {
                    std::time::Instant::now() < expire
                } else { false };
                if show_msg {
                    if let Some(msg) = &s.message { status_text = msg.clone(); }
                }

                for (key, val) in m.iter() {
                    status_text = status_text.replace(&format!("{{stree_{}}}", key), val);
                }
                s.current_text = status_text;
            }
        }
    }

    /// 全息探测器：收集所有引擎内部状态，返回一个平坦的 HashMap
    fn collect_metrics_into(
        &self,
        term_width: u16,
        term_height: u16,
        all_rects: &[(WindowRect, String, BorderStyle, usize)],
        m: &mut HashMap<String, String>,
    ) {
        m.clear();

        // --- 1. 树拓扑与图谱层 ---
        if let Some(t) = self.get_focused_tree_state() {
            let sel = t.get_selected_entity();
            m.insert("visible".into(), t.visible_ids.len().to_string());
            m.insert("total".into(), t.dataset.entities.len().to_string());
            m.insert("marked".into(), t.marked_ids.len().to_string());
            m.insert("expanded".into(), t.expanded_ids.len().to_string());
            m.insert("roots".into(), t.root_tree.len().to_string());
            m.insert("relations".into(), t.dataset.relations.len().to_string());
            m.insert("idx".into(), (t.selected_idx + 1).to_string());
            m.insert("depth".into(), t.visible_depths.get(t.selected_idx).map(|d| d.to_string()).unwrap_or_else(|| "0".into()));
            m.insert("search".into(), t.search_query.clone().unwrap_or_default());
            m.insert("id".into(), t.selected_id.clone().unwrap_or_default());
            m.insert("display".into(), sel.map(|e| e.display.clone()).unwrap_or_default());
            m.insert("path".into(), sel.map(|e| e.path.clone()).unwrap_or_default());
            m.insert("tags".into(), sel.map(|e| e.tags.clone()).unwrap_or_default());
            m.insert("trees".into(), self.components.values().filter(|c| matches!(c, Component::Tree(_))).count().to_string());
            m.insert("markable".into(), if t.markable { "Y" } else { "N" }.into());
            m.insert("scroll_v".into(), t.v_scroll.to_string());
            m.insert("scroll_h".into(), t.h_scroll.to_string());
        }

        // --- 2. 预览窗与 I/O 层 ---
        let focused_name = match &self.focus.current { Focus::Component(n) => Some(n.as_str()), _ => None };
        let mut loading_count = 0u32;
        for (name, comp) in &self.components {
            if let Component::View(v) = comp {
                if v.is_loading { loading_count += 1; }
                if focused_name == Some(name.as_str()) {
                    m.insert("view_v".into(), v.scroll_offset.to_string());
                    m.insert("view_h".into(), v.h_scroll.to_string());
                    m.insert("view_w".into(), v.rect_width.to_string());
                    m.insert("view_h_px".into(), v.rect_height.to_string());

                    // 【修改】通过 match 获取内容长度
                    let buffer_len = match &v.content {
                        crate::app::view::ViewContent::Text(s) => s.len(),
                        crate::app::view::ViewContent::Graphic(b) => b.len(),
                        crate::app::view::ViewContent::Empty => 0,
                    };
                    m.insert("buffer_kb".into(), (buffer_len / 1024).to_string());

                    m.insert("view_max_v".into(), v.max_offset.to_string());
                    m.insert("view_cmd".into(), v.cmd_template.clone());
                    m.insert("cached_id".into(), v.cached_entity_id.clone().unwrap_or_default());
                }
            }
        }
        m.insert("loading".into(), loading_count.to_string());
        m.insert("views".into(), self.components.values().filter(|c| matches!(c, Component::View(_))).count().to_string());

        // --- 3. 布局与渲染引擎层 ---
        m.insert("layers".into(), self.layout_layers.iter().filter(|l| l.visible).count().to_string());
        m.insert("windows".into(), all_rects.len().to_string());
        m.insert("containers".into(), crate::layout::get_container_count().to_string());
        m.insert("overrides".into(), self.window_rect_overrides.len().to_string());
        m.insert("dirty".into(), self.dirty_components.len().to_string());
        m.insert("edges".into(), self.drag.cached_edges.len().to_string());
        m.insert("intersections".into(), self.drag.cached_intersections.len().to_string());
        m.insert("cols".into(), term_width.to_string());
        m.insert("rows".into(), term_height.to_string());
        m.insert("prev_cols".into(), self.prev_term_size.0.to_string());
        m.insert("prev_rows".into(), self.prev_term_size.1.to_string());

        let ast_str = self.layout_layers.iter()
            .find(|l| l.visible)
            .map(|l| {
                let s = layout_ast_to_string(&l.root);
                if s.chars().count() > 60 { format!("{}...", s.chars().take(57).collect::<String>()) } else { s }
            })
            .unwrap_or_default();
        m.insert("ast".into(), ast_str);

        // --- 4. 交互与焦点层 ---
        m.insert("focus".into(), match &self.focus.current { Focus::Component(n) => n.clone(), _ => "None".into() });
        m.insert("history".into(), self.focus_history.len().to_string());
        m.insert("drag".into(), if self.drag.active { "DRAG" } else { "" }.into());
        m.insert("marking".into(), if self.drag.is_marking { "MARK" } else { "" }.into());
        m.insert("input".into(), if self.has_active_input() { "INPUT" } else { "" }.into());
        m.insert("pending".into(), self.pending_view_reload.len().to_string());
        m.insert("mouse".into(), if self.mouse.enabled { "ON" } else { "OFF" }.into());

        // --- 5. 系统与通信层 ---
        m.insert("ipc_sock".into(), std::env::var("STREE_SOCK").unwrap_or_default());
        m.insert("pid".into(), std::process::id().to_string());
        m.insert("init".into(), if self.is_initialized { "INIT" } else { "" }.into());
    }
}
