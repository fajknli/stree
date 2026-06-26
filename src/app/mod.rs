// src/app/mod.rs

pub mod view;
pub mod statusbar;
pub mod input;
pub mod tree;
// overlay 模块已彻底删除

pub use view::ViewState;
pub use statusbar::StatusBarState;
pub use input::InputState;
pub use tree::TreeState;

use crate::config::BindConfig;
use crate::exec;
use crate::layout::{self, Layout, WindowRect, BorderStyle};
use crate::protocol::Dataset;
use crate::search;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    None,
    Component(String),
}

#[derive(Debug, Clone)]
pub enum DragTarget {
    /// (主窗口名, 邻居窗口名, 容器方向)
    ResizeEdge(String, String, layout::Direction),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderSide {
    Top, Bottom, Left, Right,
}

#[derive(Debug)]
pub enum Component {
    Tree(TreeState),
    View(ViewState),
    StatusBar(StatusBarState),
    Input(InputState),
    // Overlay 已删除
}

/// 运行时布局层状态
#[derive(Debug, Clone)]
pub struct LayoutLayerState {
    pub z_index: usize,
    pub visible: bool,
    pub layout: Layout,
}

#[derive(Debug)]
pub struct Engine {
    pub drag_start_idx: Option<usize>,
    pub drag_active: bool,
    pub components: HashMap<String, Component>,
    /// 多图层布局状态（按 z_index 升序）
    pub layout_layers: Vec<LayoutLayerState>,
    pub key_bindings: BindConfig,
    pub focused: Focus,
    pub last_error: Option<String>,
    pub last_click_time: Option<Instant>,
    pub last_clicked_id: Option<String>,
    pub global_relations: Vec<crate::protocol::Relation>,
    pub mouse_enabled: bool,
    pub main_tree_name: Option<String>,
    pub border_chars: HashMap<String, String>,
    pub drag_mode: bool,
    pub drag_resize_target: Option<DragTarget>,
    /// 窗口级别的运行时尺寸覆盖（拖拽产生，存储百分比或绝对值）
    pub window_rect_overrides: std::collections::HashMap<String, layout::WindowSize>,


    // 信号防抖与状态追踪
    pub last_signal_emit: HashMap<String, Instant>,
    pub last_emitted_select_id: Option<String>,
}

fn parse_component_prefixes(cfg: &str) -> (bool, bool, bool, String) {
    let (click, rest) = if cfg.starts_with("click:") { (true, &cfg[6..]) } else { (false, cfg) };
    let (focus, rest) = if rest.starts_with("focus:") { (true, &rest[6..]) } else { (false, rest) };
    let (nomark, rest) = if rest.starts_with("nomark:") { (true, &rest[7..]) } else { (false, rest) };
    (click, focus, nomark, rest.to_string())
}

impl Engine {
    pub fn new(
        initial_dataset: Dataset,
        layout_strings: Vec<String>,
        key_bindings: BindConfig,
        mouse_enabled: bool,
        border_chars: Vec<String>,
        trees: Vec<String>,
        views: Vec<String>,
        statusbars: Vec<String>,
        inputs: Vec<String>,
        relations_path: Option<String>,
    ) -> Self {
        let mut border_chars_map = HashMap::new();
        for bc in &border_chars {
            let parts: Vec<&str> = bc.splitn(2, ':').collect();
            if parts.len() == 2 {
                border_chars_map.insert(parts[0].to_string(), parts[1].to_string());
            }
        }
        let mut components = HashMap::new();
        let global_relations = if let Some(ref p) = relations_path {
            crate::protocol::parse_relations(Some(p)).unwrap_or_default()
        } else {
            Vec::new()
        };

        // 解析多图层布局
        let mut layout_layers = Vec::new();
        for (i, s) in layout_strings.iter().enumerate() {
            let parsed = layout::parse_layout(s);
            layout_layers.push(LayoutLayerState {
                z_index: i,
                visible: true,
                layout: parsed,
            });
        }
        if layout_layers.is_empty() {
            layout_layers.push(LayoutLayerState {
                z_index: 0,
                visible: true,
                layout: layout::default_layout(),
            });
        }

        let mut init_error = None;
        let mut first_tree_name = None;

        for t_cfg in trees {
            let (click_to_fire, focus_to_fire, markable, rest) = parse_component_prefixes(&t_cfg);
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            let name = parts[0].to_string();
            let source_cmd = parts.get(1).map(|s| s.to_string());

            if first_tree_name.is_none() {
                first_tree_name = Some(name.clone());
            }

            let dataset = if let Some(ref cmd) = source_cmd {
                match crate::exec::execute_reload_hook(Some(cmd)) {
                    Ok(stdout) => {
                        if stdout.trim().is_empty() {
                            init_error = Some(format!("数据源返回为空: {}", cmd));
                            initial_dataset.clone()
                        } else {
                            match crate::protocol::parse_entities(std::io::Cursor::new(stdout)) {
                                Ok(mut ds) => {
                                    ds.relations = global_relations.clone();
                                    ds.child_index = crate::protocol::build_child_index(&ds.relations);
                                    ds
                                }
                                Err(e) => {
                                    init_error = Some(format!("解析数据失败: {}", e));
                                    initial_dataset.clone()
                                }
                            }
                        }
                    }
                    Err(e) => {
                        init_error = Some(format!("执行数据源失败: {} ({})", cmd, e));
                        initial_dataset.clone()
                    }
                }
            } else {
                initial_dataset.clone()
            };

            let root_tree = crate::tree::build_tree(&dataset);
            let mut tree_state = TreeState {
                dataset,
                root_tree,
                selected_id: None,
                expanded_ids: HashSet::new(),
                marked_ids: HashSet::new(),
                visible_ids: Vec::new(),
                visible_depths: Vec::new(),
                selected_idx: 0,
                source_cmd,
                markable,
                relations_path: relations_path.clone(),
                click_to_fire,
                focus_to_fire,
            };
            tree_state.rebuild_visible_ids();
            if let Some(first_id) = tree_state.visible_ids.first().cloned() {
                tree_state.select_id(&first_id);
            }
            components.insert(name, Component::Tree(tree_state));
        }

        for v_cfg in views {
            let parts: Vec<&str> = v_cfg.splitn(2, ':').collect();
            let name = parts[0].to_string();
            let cmd = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
            components.insert(name, Component::View(ViewState {
                cmd_template: cmd,
                scroll_offset: 0,
                content_buffer: String::new(),
                cached_entity_id: None,
                max_offset: 0,
                rect_width: 0,
                rect_height: 0,
            }));
        }

        for s_cfg in statusbars {
            let parts: Vec<&str> = s_cfg.splitn(2, ':').collect();
            let name = parts[0].to_string();
            let fmt = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
            components.insert(name, Component::StatusBar(StatusBarState { format_template: fmt }));
        }

        for i_cfg in inputs {
            let parts: Vec<&str> = i_cfg.splitn(3, ':').collect();
            let name = parts[0].to_string();
            let prefix = parts.get(1).filter(|s| !s.is_empty()).map(|s| s.to_string()).unwrap_or_else(|| ":".to_string());
            let on_submit = parts.get(2).map(|s| s.to_string());

            let mut input_state = InputState::new(&prefix);
            input_state.on_submit = on_submit;
            components.insert(name, Component::Input(input_state));
        }

        // overlays 参数已删除，不再处理

        let focused = first_tree_name.clone()
            .map(|n| Focus::Component(n))
            .unwrap_or(Focus::None);

        let mut engine = Self {
            components,
            layout_layers,
            key_bindings,
            mouse_enabled,
            drag_mode: false,
            focused: focused.clone(),
            last_error: init_error,
            last_click_time: None,
            last_clicked_id: None,
            global_relations,
            main_tree_name: first_tree_name,
            border_chars: border_chars_map,
            drag_start_idx: None,
            drag_active: false,
            drag_resize_target: None,
            window_rect_overrides: std::collections::HashMap::new(),
            last_signal_emit: HashMap::new(),
            last_emitted_select_id: None,
        };

        if let Focus::Component(name) = &focused {
            engine.broadcast_selection_changed(name, 0, 0);
        }

        engine.init_views();

        engine
    }

    // ================= 多图层布局计算 =================

    pub fn calc_all_rects(&self, term_width: u16, term_height: u16) -> Vec<(WindowRect, String, BorderStyle, usize)> {
        let mut all_rects = Vec::new();
        for layer in &self.layout_layers {
            if !layer.visible { continue; }
            // 将 overrides 传入底层 Flexbox 算法
            let rects = layout::calc_window_rects(&layer.layout, term_width, term_height, &self.window_rect_overrides);
            for (rect, name, border, _z) in rects {
                all_rects.push((rect, name, border, layer.z_index));
            }
        }
        all_rects
    }

    // ================= 布局层显隐控制 =================

    /// 切换指定名称的布局层显隐状态
    /// name 匹配规则：遍历所有图层，找到包含该 name 窗口的图层
    pub fn toggle_layout_visible(&mut self, name: &str) {
        for layer in &mut self.layout_layers {
            // 检查该图层是否包含目标窗口名
            let has_window = Self::layout_contains_window(&layer.layout, name);
            if has_window {
                layer.visible = !layer.visible;
                return;
            }
        }
        // 如果没找到匹配的图层，尝试按图层索引匹配（name 是数字字符串）
        if let Ok(idx) = name.parse::<usize>() {
            if let Some(layer) = self.layout_layers.get_mut(idx) {
                layer.visible = !layer.visible;
            }
        }
    }

    pub fn set_layout_visible(&mut self, name: &str, visible: bool) {
        for layer in &mut self.layout_layers {
            let has_window = Self::layout_contains_window(&layer.layout, name);
            if has_window {
                layer.visible = visible;
                return;
            }
        }
        if let Ok(idx) = name.parse::<usize>() {
            if let Some(layer) = self.layout_layers.get_mut(idx) {
                layer.visible = visible;
            }
        }
    }

    /// 检查一个 Layout 是否包含指定名称的窗口节点
    fn layout_contains_window(layout: &Layout, name: &str) -> bool {
        fn check_node(node: &layout::LayoutNode, name: &str) -> bool {
            match node {
                layout::LayoutNode::Window { name: n, .. } => n == name,
                layout::LayoutNode::Container { children, .. } => {
                    children.iter().any(|c| check_node(c, name))
                }
            }
        }
        for layer in &layout.layers {
            if check_node(&layer.root, name) {
                return true;
            }
        }
        false
    }

    // ================= 核心：统一事件发件箱 =================

    /// 统一的事件发射器 (The Unified Outbox)
    pub fn emit(&mut self, signal: &str, term_width: u16, term_height: u16) {
        // 1. 时间防抖 (针对 select 等高频信号，200ms 内只发一次)
        if signal == "select" {
            let now = Instant::now();
            if let Some(last) = self.last_signal_emit.get(signal) {
                if now.duration_since(*last).as_millis() < 200 {
                    return;
                }
            }
            self.last_signal_emit.insert(signal.to_string(), now);
        }

        // 2. 获取当前焦点窗口名
        let current_window = match &self.focused {
            Focus::Component(n) => Some(n.as_str()),
            Focus::None => None,
        };

        // 3. 查找信号绑定 (局部作用域优先于全局)
        let binding = self.key_bindings.get_signal_binding(current_window, signal);

        if let Some((cmd_template_args, _is_silent)) = binding {
            // 4. 组装上下文
            let tree_name = match &self.focused {
                Focus::Component(n) if matches!(self.components.get(n), Some(Component::Tree(_))) => n.clone(),
                _ => self.main_tree_name.clone().unwrap_or_default(),
            };

            let (selected_entity, ids_str, paths_str) = if let Some(Component::Tree(t)) = self.components.get(&tree_name) {
                let sel = t.get_selected_entity();
                let marked = t.get_marked_entities();
                let entities: Vec<&crate::protocol::Entity> = if !marked.is_empty() {
                    marked.iter().cloned().collect()
                } else {
                    sel.map(|e| vec![e]).unwrap_or_default()
                };
                let ids = entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(" ");
                let paths = entities.iter()
                    .map(|e| format!("\"{}\"", e.path.replace("\"", "\\\"")))
                    .collect::<Vec<_>>()
                    .join(" ");
                (sel.cloned(), ids, paths)
            } else {
                (None, String::new(), String::new())
            };

            let window_name = current_window.unwrap_or("").to_string();

            let full_cmd_args = exec::replace_placeholders_in_args(
                cmd_template_args,
                selected_entity.as_ref(),
                &ids_str,
                &paths_str,
                &window_name,
                &term_width.to_string(),
                &term_height.to_string(),
                signal,
            );

            if !full_cmd_args.is_empty() && !(full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
                // 【绝对防御】：状态信号强制静默执行，绝不交出 TTY！
                let _ = exec::execute_command_silent(&full_cmd_args);
            }
        }
    }

    /// 状态防抖：只有当 selected_id 真正改变时，才发射 select 信号
    pub fn emit_select_if_changed(&mut self, term_width: u16, term_height: u16) {
        let current_id = self.get_selected_entity().map(|e| e.id.clone());
        if current_id != self.last_emitted_select_id {
            self.last_emitted_select_id = current_id;
            self.emit("select", term_width, term_height);
        }
    }

    // ================= UI 导航与状态变迁 =================

    pub fn handle_tab(&mut self, term_width: u16, term_height: u16) {
        self.last_error = None;
        let mut names: Vec<String> = self.components.iter()
            .filter(|(_, c)| !matches!(c, Component::StatusBar(_)))
            .map(|(k, _)| k.clone())
            .collect();

        names.sort();

        if names.is_empty() { self.focused = Focus::None; return; }
        let current_idx = match &self.focused {
            Focus::Component(name) => names.iter().position(|n| n == name).unwrap_or(0),
            Focus::None => 0,
        };
        let next_idx = (current_idx + 1) % names.len();
        let next_name = names[next_idx].clone();
        self.focused = Focus::Component(next_name.clone());

        // 【门禁检查】：新焦点的窗口是否签署了 focus: 协议？
        let should_emit_focus = if let Some(Component::Tree(t)) = self.components.get(&next_name) {
            t.focus_to_fire
        } else {
            false
        };

        if should_emit_focus {
            self.emit("focus", term_width, term_height);
        }
    }

    pub fn move_up(&mut self, term_width: u16, term_height: u16) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.move_up();
            self.broadcast_selection_changed(&focused_name, term_width, term_height);
            self.emit_select_if_changed(term_width, term_height);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.scroll_offset.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self, term_width: u16, term_height: u16) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.move_down();
            self.broadcast_selection_changed(&focused_name, term_width, term_height);
            self.emit_select_if_changed(term_width, term_height);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = (v.scroll_offset + 1).min(v.max_offset);
        }
    }

    pub fn toggle_expand(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.toggle_expand();
        }
    }

    pub fn toggle_mark(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.toggle_mark();
        }
    }

    pub fn jump_to_top(&mut self, term_width: u16, term_height: u16) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.jump_to_top();
            self.broadcast_selection_changed(&focused_name, term_width, term_height);
            self.emit_select_if_changed(term_width, term_height);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = 0;
        }
    }

    pub fn jump_to_bottom(&mut self, term_width: u16, term_height: u16) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.jump_to_bottom();
            self.broadcast_selection_changed(&focused_name, term_width, term_height);
            self.emit_select_if_changed(term_width, term_height);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.max_offset;
        }
    }

    pub fn select_id(&mut self, tree_name: &str, id: &str, term_width: u16, term_height: u16) {
        self.last_error = None;
        if let Some(Component::Tree(t)) = self.components.get_mut(tree_name) {
            t.select_id(id);
        }
        self.broadcast_selection_changed(tree_name, term_width, term_height);
        self.emit_select_if_changed(term_width, term_height);
    }

    pub fn broadcast_selection_changed(&mut self, tree_name: &str, _term_width: u16, _term_height: u16) {
        let is_focused_tree = match &self.focused {
            Focus::Component(n) => n == tree_name,
            Focus::None => false,
        };
        if !is_focused_tree {
            return;
        }

        let selected_entity = if let Some(Component::Tree(t)) = self.components.get(tree_name) {
            t.get_selected_entity().cloned()
        } else {
            return;
        };

        let ids_str = selected_entity.as_ref().map(|e| e.id.clone()).unwrap_or_default();
        let paths_str = selected_entity.as_ref()
            .map(|e| format!("\"{}\"", e.path.replace("\"", "\\\"")))
            .unwrap_or_default();

        let window_name = tree_name.to_string();

        for (_view_name, comp) in self.components.iter_mut() {
            if let Component::View(v) = comp {
                let new_cached_id = selected_entity.as_ref().map(|e| e.id.clone());
                if v.cached_entity_id == new_cached_id && !v.content_buffer.is_empty() {
                    continue;
                }
                v.cached_entity_id = new_cached_id;

                let width_str = v.rect_width.to_string();
                let height_str = v.rect_height.to_string();

                let template_args_vec = crate::config::split_args(&v.cmd_template);
                let full_cmd_args = exec::replace_placeholders_in_args(
                    &template_args_vec,
                    selected_entity.as_ref(),
                    &ids_str,
                    &paths_str,
                    &window_name,
                    &width_str,
                    &height_str,
                    "",
                );

                if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
                    v.content_buffer = String::new();
                    v.scroll_offset = 0;
                    continue;
                }

                match exec::execute_command_args(&full_cmd_args) {
                    Ok((code, stdout)) => {
                        if code != 0 && stdout.trim().is_empty() {
                            v.content_buffer = format!("[ERR] Command exited with code {}", code);
                        } else {
                            v.content_buffer = stdout;
                        }
                        v.scroll_offset = 0;
                    }
                    Err(e) => {
                        v.content_buffer = format!("[ERR] {}", e);
                    }
                }
            }
        }
    }

    pub fn init_views(&mut self) {
        for (view_name, comp) in self.components.iter_mut() {
            if let Component::View(v) = comp {
                let width_str = v.rect_width.to_string();
                let height_str = v.rect_height.to_string();
                let window_name = view_name.clone();

                let template_args_vec = crate::config::split_args(&v.cmd_template);
                let full_cmd_args = exec::replace_placeholders_in_args(
                    &template_args_vec,
                    None,
                    "", "", &window_name, &width_str, &height_str, "",
                );

                if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
                    continue;
                }

                match exec::execute_command_args(&full_cmd_args) {
                    Ok((code, stdout)) => {
                        v.content_buffer = if code != 0 && stdout.trim().is_empty() {
                            format!("[ERR] Command exited with code {}", code)
                        } else {
                            stdout
                        };
                        v.scroll_offset = 0;
                    }
                    Err(e) => {
                        v.content_buffer = format!("[ERR] {}", e);
                    }
                }
            }
        }
    }

    pub fn get_focused_tree_state(&self) -> Option<&TreeState> {
        let name = match &self.focused {
            Focus::Component(n) => n,
            Focus::None => return self.get_main_tree_state(),
        };
        if let Some(Component::Tree(t)) = self.components.get(name) {
            return Some(t);
        }
        self.get_main_tree_state()
    }

    pub fn get_selected_entity(&self) -> Option<&crate::protocol::Entity> {
        let name = match &self.focused {
            Focus::Component(n) => n,
            Focus::None => self.main_tree_name.as_ref()?,
        };
        if let Some(Component::Tree(t)) = self.components.get(name) {
            return t.get_selected_entity();
        }
        None
    }

    pub fn prepare_key_binding_args(&self, key: &crossterm::event::KeyEvent, term_width: u16, term_height: u16) -> Option<(Vec<String>, bool)> {
        let (cmd_template_args, is_silent) = self.key_bindings.get(key)?;

        let tree_name = match &self.focused {
            Focus::Component(n) => {
                if matches!(self.components.get(n), Some(Component::Tree(_))) {
                    n.clone()
                } else {
                    self.main_tree_name.as_ref()?.clone()
                }
            }
            Focus::None => self.main_tree_name.as_ref()?.clone(),
        };

        let tree_state = if let Some(Component::Tree(t)) = self.components.get(&tree_name) {
            t
        } else {
            return None;
        };

        let selected_entity = tree_state.get_selected_entity();
        let marked_entities = tree_state.get_marked_entities();

        let entities: Vec<&crate::protocol::Entity> = if !marked_entities.is_empty() {
            marked_entities.iter().cloned().collect()
        } else {
            selected_entity.map(|e| vec![e]).unwrap_or_default()
        };

        let ids_str = entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(" ");
        let paths_str = entities.iter()
            .map(|e| format!("\"{}\"", e.path.replace("\"", "\\\"")))
            .collect::<Vec<_>>()
            .join(" ");

        let window_name = match &self.focused {
            Focus::Component(n) => n.clone(),
            Focus::None => String::new(),
        };

        let full_cmd_args = exec::replace_placeholders_in_args(
            cmd_template_args,
            selected_entity,
            &ids_str,
            &paths_str,
            &window_name,
            &term_width.to_string(),
            &term_height.to_string(),
            "",
        );

        if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
            None
        } else {
            Some((full_cmd_args, *is_silent))
        }
    }

    pub fn handle_ipc_update(&mut self, target: &str, data: &str, term_width: u16, term_height: u16) {
        if target == "@layout-reset" {
            self.window_rect_overrides.clear();
            return;
        }
        if let Some(layer_name) = target.strip_prefix("@layout-reset ") {
            self.window_rect_overrides.remove(layer_name.trim());
            return;
        }
        if let Some(comp) = self.components.get_mut(target) {
            match comp {
                Component::Tree(t) => {
                    let cursor = std::io::Cursor::new(data.to_string());
                    if let Ok(mut new_dataset) = crate::protocol::parse_entities(cursor) {
                        new_dataset.relations = if let Some(p) = &t.relations_path {
                            crate::protocol::parse_relations(Some(p)).unwrap_or_default()
                        } else {
                            t.dataset.relations.clone()
                        };
                        new_dataset.child_index = crate::protocol::build_child_index(&new_dataset.relations);
                        let old_selected_id = t.selected_id.clone();
                        t.dataset = new_dataset;
                        t.root_tree = crate::tree::build_tree(&t.dataset);
                        let valid_ids: HashSet<_> = t.dataset.entity_map.keys().cloned().collect();
                        t.expanded_ids.retain(|id| valid_ids.contains(id));
                        t.rebuild_visible_ids();
                        if let Some(id) = old_selected_id {
                            t.select_id(&id);
                        } else if let Some(first_id) = t.visible_ids.first().cloned() {
                            t.select_id(&first_id);
                        }
                        let target_owned = target.to_string();
                        self.broadcast_selection_changed(&target_owned, term_width, term_height);
                        self.emit_select_if_changed(term_width, term_height);
                        self.emit("load", term_width, term_height);
                    }
                }
                Component::View(v) => {
                    v.content_buffer = data.to_string();
                    v.scroll_offset = 0;
                    v.cached_entity_id = None;
                }
                Component::StatusBar(s) => {
                    s.format_template = data.to_string();
                }
                _ => {}
            }
        }
    }

    pub fn trigger_reload(&mut self, term_width: u16, term_height: u16) {
        let tree_names: Vec<String> = self.components.iter()
            .filter(|(_, c)| matches!(c, Component::Tree(_)))
            .map(|(k, _)| k.clone())
            .collect();

        for name in tree_names {
            let source_cmd = if let Some(Component::Tree(t)) = self.components.get(&name) {
                t.source_cmd.clone()
            } else {
                None
            };
            if let Some(cmd) = source_cmd {
                match crate::exec::execute_reload_hook(Some(&cmd)) {
                    Ok(stdout) => {
                        if !stdout.trim().is_empty() {
                            self.handle_ipc_update(&name, &stdout, term_width, term_height);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    }

    pub fn update_view_rects(&mut self, view_rects: HashMap<String, (usize, u16, u16)>) {
        for (name, (max_rows, width, height)) in view_rects {
            if let Some(Component::View(v)) = self.components.get_mut(&name) {
                let total_lines = v.content_buffer.lines().count();
                v.max_offset = total_lines.saturating_sub(max_rows);
                if v.scroll_offset > v.max_offset {
                    v.scroll_offset = v.max_offset;
                }
                v.rect_width = width;
                v.rect_height = height;
            }
        }
    }

    pub fn get_main_tree_state(&self) -> Option<&TreeState> {
        let name = self.main_tree_name.as_ref()?;
        if let Some(Component::Tree(t)) = self.components.get(name) { Some(t) } else { None }
    }

    pub fn move_up_n(&mut self, n: usize, term_width: u16, term_height: u16) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            for _ in 0..n {
                if t.selected_idx > 0 {
                    t.selected_idx -= 1;
                    t.selected_id = Some(t.visible_ids[t.selected_idx].clone());
                } else { break; }
            }
            self.broadcast_selection_changed(&focused_name, term_width, term_height);
            self.emit_select_if_changed(term_width, term_height);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.scroll_offset.saturating_sub(n);
        }
    }

    pub fn move_down_n(&mut self, n: usize, term_width: u16, term_height: u16) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            for _ in 0..n {
                if t.selected_idx < t.visible_ids.len().saturating_sub(1) {
                    t.selected_idx += 1;
                    t.selected_id = Some(t.visible_ids[t.selected_idx].clone());
                } else { break; }
            }
            self.broadcast_selection_changed(&focused_name, term_width, term_height);
            self.emit_select_if_changed(term_width, term_height);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = (v.scroll_offset + n).min(v.max_offset);
        }
    }

    pub fn has_active_input(&self) -> bool {
        self.components.values().any(|c| matches!(c, Component::Input(i) if i.is_active))
    }

    pub fn handle_input_key(&mut self, key: crossterm::event::KeyEvent) -> Option<(String, String)> {
        use crossterm::event::{KeyCode, KeyModifiers};

        let input_name = self.components.iter()
            .find(|(_, c)| matches!(c, Component::Input(i) if i.is_active))
            .map(|(n, _)| n.clone());

        let input_name = input_name?;

        if let Some(Component::Input(input)) = self.components.get_mut(&input_name) {
            match key.code {
                KeyCode::Esc => {
                    input.deactivate();
                    return Some((input_name, "__CANCEL__".to_string()));
                }
                KeyCode::Enter => {
                    let submitted = input.buffer.clone();
                    input.deactivate();
                    return Some((input_name, submitted));
                }
                KeyCode::Backspace => input.backspace(),
                KeyCode::Left => input.move_left(),
                KeyCode::Right => input.move_right(),
                KeyCode::Home => input.move_home(),
                KeyCode::End => input.move_end(),
                KeyCode::Char(c) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'u' {
                        input.clear();
                    } else if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'a' {
                        input.move_home();
                    } else if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'e' {
                        input.move_end();
                    } else {
                        input.insert_char(c);
                    }
                }
                _ => {}
            }
        }
        None
    }

    pub fn activate_input(&mut self, name: &str, prefix: &str) {
        if let Some(Component::Input(input)) = self.components.get_mut(name) {
            input.prefix = prefix.to_string();
            input.activate();
        }
    }

    pub fn apply_search(&mut self, query: &str, term_width: u16, term_height: u16) {
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            let matched = search::match_entities(&t.dataset.entities, query);
            t.rebuild_visible_ids();
            if !matched.is_empty() {
                t.visible_ids.retain(|id| matched.contains(id));
                if !t.visible_ids.is_empty() {
                    t.selected_idx = 0;
                    t.selected_id = Some(t.visible_ids[0].clone());
                }
            }
            self.broadcast_selection_changed(&focused_name, term_width, term_height);
            self.emit_select_if_changed(term_width, term_height);
        }
    }

    pub fn cancel_input(&mut self) {
        if let Some((name, _)) = self.components.iter().find(|(_, c)| matches!(c, Component::Input(i) if i.is_active)) {
            let name = name.clone();
            if let Some(Component::Input(input)) = self.components.get_mut(&name) {
                input.deactivate();
            }
        }
    }
    // ================= 窗口边框拖拽：合法性判定 =================

    /// 步骤1：纯树递归找候选邻居
    pub fn find_drag_neighbor(&self, window_name: &str, side: BorderSide) -> Option<(String, layout::Direction)> {
        for layer in &self.layout_layers {
            if let Some(result) = Self::search_drag_neighbor_in_node(&layer.layout.layers[0].root, window_name, side) {
                return Some(result);
            }
        }
        None
    }

    fn search_drag_neighbor_in_node(node: &layout::LayoutNode, target: &str, side: BorderSide) -> Option<(String, layout::Direction)> {
        match node {
            layout::LayoutNode::Container { direction, children, .. } => {
                // 检查目标是否在当前容器的直接子节点中
                let my_idx = children.iter().position(|c| {
                    matches!(c, layout::LayoutNode::Window { name, .. } if name == target)
                });

                if let Some(idx) = my_idx {
                    // 判断拖拽边方向是否与容器方向匹配
                    let is_matching = match (direction, side) {
                        (layout::Direction::Horizontal, BorderSide::Left | BorderSide::Right) => true,
                        (layout::Direction::Vertical, BorderSide::Top | BorderSide::Bottom) => true,
                        _ => false,
                    };

                    if is_matching {
                        let neighbor_idx = match side {
                            BorderSide::Right | BorderSide::Bottom => idx + 1,
                            BorderSide::Left | BorderSide::Top => idx.checked_sub(1)?,
                        };
                        // 邻居可以是 Window 或 Container，取其名字
                        let neighbor_name = match children.get(neighbor_idx) {
                            Some(layout::LayoutNode::Window { name, .. }) => Some(name.clone()),
                            Some(layout::LayoutNode::Container { .. }) => {
                                // 容器没有名字，用第一个子窗口的名字作为标识
                                // 实际上我们需要容器的"名字"，但容器没有
                                // 这里返回 None，因为容器级别的拖拽需要特殊处理
                                // 暂时只支持叶子窗口之间的拖拽
                                None
                            }
                            None => None,
                        };
                        if let Some(n) = neighbor_name {
                            return Some((n, *direction));
                        }
                    }
                }

                // 目标不在直接子节点中，或方向不匹配 → 递归进子容器
                for child in children {
                    if let Some(result) = Self::search_drag_neighbor_in_node(child, target, side) {
                        return Some(result);
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// 步骤2：纯几何边对齐校验
    pub fn is_edge_aligned(&self, my: &layout::WindowRect, neighbor: &layout::WindowRect, side: BorderSide) -> bool {
        match side {
            BorderSide::Right => {
                my.start_col + my.width == neighbor.start_col
                    && my.start_row == neighbor.start_row
                    && my.height == neighbor.height
            }
            BorderSide::Left => {
                my.start_col == neighbor.start_col + neighbor.width
                    && my.start_row == neighbor.start_row
                    && my.height == neighbor.height
            }
            BorderSide::Bottom => {
                my.start_row + my.height == neighbor.start_row
                    && my.start_col == neighbor.start_col
                    && my.width == neighbor.width
            }
            BorderSide::Top => {
                my.start_row == neighbor.start_row + neighbor.height
                    && my.start_col == neighbor.start_col
                    && my.width == neighbor.width
            }
        }
    }
}
