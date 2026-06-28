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

/// 预计算的可拖拽物理边界
#[derive(Debug, Clone)]
pub struct DragEdge {
    pub primary_id: String,   // 切割线左侧/上侧的节点 ID (Window name 或 Container id)
    pub neighbor_id: String,  // 切割线右侧/下侧的节点 ID
    pub direction: layout::Direction,
    pub hit_rect: layout::WindowRect, // 鼠标碰撞检测的隐形矩形
    pub z_index: usize,       // 所属图层的 Z 轴
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

#[derive(Debug, Clone, PartialEq)]
pub enum InternalCommand {
    Exit,
    Esc,
    Tab,
    Up,
    Down,
    Expand,
    Mark,
    Top,
    Bottom,
    Enter,
    ActivateSearch,
    ActivateCmd,
    ActivateInput(String),
    ToggleLayout(String),
    ShowLayout(String),
    HideLayout(String),
}

impl InternalCommand {
    pub fn from_args(args: &[String]) -> Option<Self> {
        if args.is_empty() { return None; }
        let cmd = args[0].as_str();
        let val = args.get(1).cloned();
        match (cmd, val) {
            ("__EXIT__", None) => Some(Self::Exit),
            ("__ESC__", None) => Some(Self::Esc),
            ("__TAB__", None) => Some(Self::Tab),
            ("__UP__", None) => Some(Self::Up),
            ("__DOWN__", None) => Some(Self::Down),
            ("__EXPAND__", None) => Some(Self::Expand),
            ("__MARK__", None) => Some(Self::Mark),
            ("__TOP__", None) => Some(Self::Top),
            ("__BOTTOM__", None) => Some(Self::Bottom),
            ("__ENTER__", None) => Some(Self::Enter),
            ("__ACTIVATE_SEARCH__", None) => Some(Self::ActivateSearch),
            ("__ACTIVATE_CMD__", None) => Some(Self::ActivateCmd),
            ("__ACTIVATE_INPUT__", Some(name)) => Some(Self::ActivateInput(name)),
            ("__TOGGLE_LAYOUT__", Some(name)) => Some(Self::ToggleLayout(name)),
            ("__SHOW_LAYOUT__", Some(name)) => Some(Self::ShowLayout(name)),
            ("__HIDE_LAYOUT__", Some(name)) => Some(Self::HideLayout(name)),
            _ => None,
        }
    }
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
    /// 窗口级别的运行时尺寸覆盖（拖拽产生，存储 Absolute 用于当前渲染）
    pub window_rect_overrides: std::collections::HashMap<String, layout::WindowSize>,

    // 信号防抖与状态追踪
    pub last_signal_emit: HashMap<String, Instant>,
    pub last_emitted_select_id: Option<String>,
    /// 【新增】全局可拖拽边界缓存
    pub cached_edges: Vec<DragEdge>,
    pub cached_intersections: Vec<(u16, u16)>,
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
                search_query: None,
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
            cached_edges: Vec::new(),
            cached_intersections: Vec::new(),
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
            // 纯净的 Flexbox 计算，window_rect_overrides 会作为 Absolute 约束传入
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
            if query.is_empty() {
                t.search_query = None;
            } else {
                t.search_query = Some(query.to_string());
            }
            t.rebuild_visible_ids();

            if !t.visible_ids.is_empty() {
                t.selected_idx = 0;
                t.selected_id = Some(t.visible_ids[0].clone());
            } else {
                t.selected_idx = 0;
                t.selected_id = None;
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
    /// 查找两个相邻窗口在父容器中的“原始百分比总和”
    pub fn get_sibling_percent_sum(&self, name1: &str, name2: &str) -> Option<u16> {
        for layer in &self.layout_layers {
            if let Some(sum) = Self::find_sibling_sum_in_node(&layer.layout.layers[0].root, name1, name2) {
                return Some(sum);
            }
        }
        None
    }

    fn find_sibling_sum_in_node(node: &crate::layout::LayoutNode, id1: &str, id2: &str) -> Option<u16> {
        if let crate::layout::LayoutNode::Container { children, .. } = node {
            let mut sum = 0;
            let mut found_count = 0;

            for child in children {
                let child_id = match child {
                    crate::layout::LayoutNode::Window { name, .. } => name.as_str(),
                    crate::layout::LayoutNode::Container { id, .. } => id.as_str(),
                };

                if child_id == id1 || child_id == id2 {
                    let size = match child {
                        crate::layout::LayoutNode::Window { size, .. } => *size,
                        crate::layout::LayoutNode::Container { percent, .. } => percent.map(layout::WindowSize::Percent),
                    };

                    if let Some(layout::WindowSize::Percent(p)) = size {
                        sum += p;
                        found_count += 1;
                    } else {
                        return None;
                    }
                }
            }

            if found_count == 2 {
                return Some(sum);
            }

            for child in children {
                if let Some(sum) = Self::find_sibling_sum_in_node(child, id1, id2) {
                    return Some(sum);
                }
            }
        }
        None
    }

    pub fn rebuild_draggable_edges(&mut self, term_width: u16, term_height: u16) {
        self.cached_edges.clear();

        // 获取所有叶子 Window 的物理 Rect
        let window_rects = self.calc_all_rects(term_width, term_height);
        let mut w_map: std::collections::HashMap<String, layout::WindowRect> = std::collections::HashMap::new();
        for (rect, name, _, _) in &window_rects {
            w_map.insert(name.clone(), *rect);
        }

        // 遍历所有图层，提取叶子之间的边
        for layer in &self.layout_layers {
            if !layer.visible { continue; }
            Self::extract_edges_from_node(&layer.layout.layers[0].root, &w_map, layer.z_index, &mut self.cached_edges);
        }

        // 【新增】计算横竖线的交点，用于命中剔除
        self.cached_intersections.clear();
        let v_edges: Vec<_> = self.cached_edges.iter().filter(|e| e.direction == layout::Direction::Vertical).collect();
        let h_edges: Vec<_> = self.cached_edges.iter().filter(|e| e.direction == layout::Direction::Horizontal).collect();
        for v in &v_edges {
            for h in &h_edges {
                let v_x = v.hit_rect.start_col + 1; // 竖线所在列
                let v_y_start = v.hit_rect.start_row;
                let v_y_end = v.hit_rect.start_row + v.hit_rect.height;

                let h_y = h.hit_rect.start_row + 1; // 横线所在行
                let h_x_start = h.hit_rect.start_col;
                let h_x_end = h.hit_rect.start_col + h.hit_rect.width;

                if v_x >= h_x_start && v_x < h_x_end && h_y >= v_y_start && h_y < v_y_end {
                    self.cached_intersections.push((v_x, h_y));
                }
            }
        }
    }

    /// 【新增】获取任意节点（Window 或 Container）的当前物理外包络矩形
    /// 这是整个拖拽系统的核心：让交互层完全无视树结构
    pub fn get_node_current_bbox(&self, node_id: &str, term_width: u16, term_height: u16) -> Option<(layout::WindowRect, layout::BorderStyle)> {
        // 1. 获取所有叶子 Window 的 Rect
        let window_rects = self.calc_all_rects(term_width, term_height);
        let mut w_map: std::collections::HashMap<String, layout::WindowRect> = std::collections::HashMap::new();
        let mut b_map: std::collections::HashMap<String, layout::BorderStyle> = std::collections::HashMap::new();
        for (rect, name, border, _) in &window_rects {
            w_map.insert(name.clone(), *rect);
            b_map.insert(name.clone(), *border);
        }

        // 2. 遍历所有图层，搜索目标节点
        for layer in &self.layout_layers {
            if let Some((rect, border)) = Self::search_bbox_in_node(
                &layer.layout.layers[0].root,
                node_id,
                &w_map,
                &b_map,
            ) {
                return Some((rect, border));
            }
        }
        None
    }

    /// 在树中递归搜索节点，返回它的外包络矩形
    fn search_bbox_in_node(
        node: &layout::LayoutNode,
        target_id: &str,
        w_map: &std::collections::HashMap<String, layout::WindowRect>,
        b_map: &std::collections::HashMap<String, layout::BorderStyle>,
    ) -> Option<(layout::WindowRect, layout::BorderStyle)> {
        match node {
            layout::LayoutNode::Window { name, .. } => {
                if name == target_id {
                    // Window 直接返回它的 Rect 和边框样式
                    Some((w_map.get(name).copied()?, b_map.get(name).copied().unwrap_or(layout::BorderStyle::Box)))
                } else {
                    None
                }
            }
            layout::LayoutNode::Container { id, children, .. } => {
                if id == target_id {
                    // Container 计算 BBox，边框为 None
                    let bbox = Self::get_node_bbox(node, w_map)?;
                    return Some((bbox, layout::BorderStyle::None));
                }
                // 递归搜索子节点
                for child in children {
                    if let Some(res) = Self::search_bbox_in_node(child, target_id, w_map, b_map) {
                        return Some(res);
                    }
                }
                None
            }
        }
    }

    /// 辅助函数：获取节点（Window 或 Container）的外包络矩形 (Bounding Box)
    fn get_node_bbox(node: &layout::LayoutNode, w_map: &std::collections::HashMap<String, layout::WindowRect>) -> Option<layout::WindowRect> {
        match node {
            layout::LayoutNode::Window { name, .. } => w_map.get(name).copied(),
            layout::LayoutNode::Container { children, .. } => {
                let mut min_col = u16::MAX;
                let mut min_row = u16::MAX;
                let mut max_col = 0;
                let mut max_row = 0;
                let mut has_child = false;
                for child in children {
                    if let Some(r) = Self::get_node_bbox(child, w_map) {
                        has_child = true;
                        min_col = min_col.min(r.start_col);
                        min_row = min_row.min(r.start_row);
                        max_col = max_col.max(r.start_col + r.width);
                        max_row = max_row.max(r.start_row + r.height);
                    }
                }
                if has_child {
                    Some(layout::WindowRect {
                        start_col: min_col,
                        start_row: min_row,
                        width: max_col.saturating_sub(min_col),
                        height: max_row.saturating_sub(min_row),
                    })
                } else {
                    None
                }
            }
        }
    }

    /// 收集叶子 Window 的 Rect
    fn collect_leaf_rects(
        node: &layout::LayoutNode,
        rects: &std::collections::HashMap<String, layout::WindowRect>,
        z_index: usize,
        out: &mut Vec<(layout::WindowRect, String, usize)>,
    ) {
        match node {
            layout::LayoutNode::Window { name, .. } => {
                if let Some(rect) = rects.get(name) {
                    out.push((*rect, name.clone(), z_index));
                }
            }
            layout::LayoutNode::Container { children, .. } => {
                for child in children {
                    Self::collect_leaf_rects(child, rects, z_index, out);
                }
            }
        }
    }

    /// 纯物理线段提取：只提取叶子 Window 之间的相邻边（竖线 + 横线）
    /// 完全无视树结构，不提取任何 Container 之间的边
    fn extract_edges_from_node(
        node: &layout::LayoutNode,
        rects: &std::collections::HashMap<String, layout::WindowRect>,
        z_index: usize,
        edges: &mut Vec<DragEdge>
    ) {
        // 1. 收集所有叶子 Window 的 Rect
        let mut leaf_rects = Vec::new();
        Self::collect_leaf_rects(node, rects, z_index, &mut leaf_rects);

        // 2. 提取所有相邻叶子之间的边
        for i in 0..leaf_rects.len() {
            for j in i + 1..leaf_rects.len() {
                let (r1, name1, _z1) = &leaf_rects[i];
                let (r2, name2, _z2) = &leaf_rects[j];

                // 检查左右相邻（竖线）
                let horizontal_adjacent =
                    r1.start_row == r2.start_row
                    && r1.height == r2.height
                    && (r1.start_col + r1.width == r2.start_col
                        || r2.start_col + r2.width == r1.start_col);

                if horizontal_adjacent {
                    let left = if r1.start_col < r2.start_col { r1 } else { r2 };
                    let right = if r1.start_col < r2.start_col { r2 } else { r1 };
                    let left_name = if r1.start_col < r2.start_col { name1 } else { name2 };
                    let right_name = if r1.start_col < r2.start_col { name2 } else { name1 };

                    let x = left.start_col + left.width;
                    edges.push(DragEdge {
                        primary_id: left_name.clone(),
                        neighbor_id: right_name.clone(),
                        direction: layout::Direction::Horizontal,
                        hit_rect: layout::WindowRect {
                            start_col: x.saturating_sub(1),
                            start_row: left.start_row.max(right.start_row),
                            width: 2,
                            height: left.height.min(right.height),
                        },
                        z_index,
                    });
                }

                // 检查上下相邻（横线）
                let vertical_adjacent =
                    r1.start_col == r2.start_col
                    && r1.width == r2.width
                    && (r1.start_row + r1.height == r2.start_row
                        || r2.start_row + r2.height == r1.start_row);

                if vertical_adjacent {
                    let top = if r1.start_row < r2.start_row { r1 } else { r2 };
                    let bottom = if r1.start_row < r2.start_row { r2 } else { r1 };
                    let top_name = if r1.start_row < r2.start_row { name1 } else { name2 };
                    let bottom_name = if r1.start_row < r2.start_row { name2 } else { name1 };

                    let y = top.start_row + top.height;
                    edges.push(DragEdge {
                        primary_id: top_name.clone(),
                        neighbor_id: bottom_name.clone(),
                        direction: layout::Direction::Vertical,
                        hit_rect: layout::WindowRect {
                            start_col: top.start_col.max(bottom.start_col),
                            start_row: y.saturating_sub(1),
                            width: top.width.min(bottom.width),
                            height: 2,
                        },
                        z_index,
                    });
                }
            }
        }
    }

    fn get_node_id(node: &layout::LayoutNode) -> String {
        match node {
            layout::LayoutNode::Window { name, .. } => name.clone(),
            layout::LayoutNode::Container { id, .. } => id.clone(),
        }
    }

    /// 查找一个节点（Window 或 Container）的父容器 ID
    pub fn find_parent_container(&self, node_id: &str) -> Option<String> {
        for layer in &self.layout_layers {
            if let Some(parent) = Self::search_parent_container(&layer.layout.layers[0].root, node_id) {
                return Some(parent);
            }
        }
        None
    }

    // ================= 终极魔法：运行时树重组 =================

    /// 强制根据物理真相重算所有百分比，防止松手回弹
    pub fn force_recalculate_percentages(
        &mut self,
        all_rects: &[(layout::WindowRect, String, layout::BorderStyle, usize)]
    ) {
        let mut rect_map: std::collections::HashMap<String, layout::WindowRect> = std::collections::HashMap::new();
        for (rect, name, _, _) in all_rects {
            rect_map.insert(name.clone(), *rect);
        }

        for layer in &mut self.layout_layers {
            let root = &mut layer.layout.layers[0].root;
            Self::recalc_node_percentages_recursive(root, &rect_map);
        }
    }

    /// 拖拽松手后，重组 Flexbox 树
    pub fn restructure_tree_after_drag(
        &mut self,
        primary: &str,
        neighbor: &str,
        drag_dir: layout::Direction,
        all_rects: &[(layout::WindowRect, String, layout::BorderStyle, usize)]
    ) -> bool {
        // 构建物理坐标索引
        let mut rect_map: std::collections::HashMap<String, layout::WindowRect> = std::collections::HashMap::new();
        for (rect, name, _, _) in all_rects {
            rect_map.insert(name.clone(), *rect);
        }

        for layer in &mut self.layout_layers {
            let root = &mut layer.layout.layers[0].root;
            // 将 rect_map 传入手术刀函数
            if Self::surgery_tree_node(root, primary, neighbor, drag_dir, &rect_map) {
                return true;
            }
        }
        false
    }

    fn surgery_tree_node(
        node: &mut layout::LayoutNode,
        primary: &str,
        neighbor: &str,
        drag_dir: layout::Direction,
        rect_map: &std::collections::HashMap<String, layout::WindowRect>
    ) -> bool {
        if let layout::LayoutNode::Container { direction, children, .. } = node {
            let mut p_child_idx = None;
            let mut n_child_idx = None;
            for (i, child) in children.iter().enumerate() {
                if Self::contains_node(child, primary) { p_child_idx = Some(i); }
                if Self::contains_node(child, neighbor) { n_child_idx = Some(i); }
            }

            if let (Some(idx_p), Some(idx_n)) = (p_child_idx, n_child_idx) {
                if idx_p != idx_n {
                    let p_is_direct = Self::is_node_id_match(&children[idx_p], primary);
                    let n_is_direct = Self::is_node_id_match(&children[idx_n], neighbor);

                    if p_is_direct && n_is_direct && *direction == drag_dir {
                        return false;
                    }

                    let opposite_dir = match drag_dir {
                        layout::Direction::Horizontal => layout::Direction::Vertical,
                        layout::Direction::Vertical => layout::Direction::Horizontal,
                    };

                    let mut temp_children = std::mem::take(children);
                    let mut p_child = temp_children.remove(idx_p);
                    let n_idx_adjusted = if idx_n > idx_p { idx_n - 1 } else { idx_n };
                    let mut n_child = temp_children.remove(n_idx_adjusted);

                    let p_node = Self::extract_node_by_id(&p_child, primary).unwrap();
                    let n_node = Self::extract_node_by_id(&n_child, neighbor).unwrap();

                    let mut remaining_nodes = temp_children;
                    if let Some(remaining) = Self::remove_node_and_collapse(&mut p_child, primary) {
                        remaining_nodes.push(remaining);
                    }
                    if let Some(remaining) = Self::remove_node_and_collapse(&mut n_child, neighbor) {
                        remaining_nodes.push(remaining);
                    }

                    let new_main_container = layout::LayoutNode::Container {
                        id: crate::layout::generate_container_id_pub(),
                        direction: drag_dir,
                        percent: None,
                        children: vec![p_node, n_node],
                    };

                    let new_remaining_container = if remaining_nodes.is_empty() {
                        None
                    } else if remaining_nodes.len() == 1 {
                        Some(remaining_nodes.remove(0))
                    } else {
                        Some(layout::LayoutNode::Container {
                            id: crate::layout::generate_container_id_pub(),
                            direction: drag_dir,
                            percent: None,
                            children: remaining_nodes,
                        })
                    };

                    // 【核心架构修复】：在构建 children 数组时，根据物理坐标决定先后顺序！
                    // 保证树结构拓扑与屏幕物理位置绝对一致，彻底消灭 badc 现象！
                    if let Some(rem_container) = new_remaining_container {
                        let main_pos = Self::get_node_physical_rect(&new_main_container, rect_map).unwrap_or_default();
                        let rem_pos = Self::get_node_physical_rect(&rem_container, rect_map).unwrap_or_default();

                        let is_main_first = match opposite_dir {
                            layout::Direction::Horizontal => main_pos.start_col <= rem_pos.start_col,
                            layout::Direction::Vertical => main_pos.start_row <= rem_pos.start_row,
                        };

                        if is_main_first {
                            *children = vec![new_main_container, rem_container];
                        } else {
                            *children = vec![rem_container, new_main_container];
                        }
                    } else {
                        *children = vec![new_main_container];
                    }

                    *direction = opposite_dir;
                    return true;
                }
            }

            for child in children.iter_mut() {
                if Self::surgery_tree_node(child, primary, neighbor, drag_dir, rect_map) {
                    return true;
                }
            }
        }
        false
    }

    fn remove_node_and_collapse(node: &mut layout::LayoutNode, target_id: &str) -> Option<layout::LayoutNode> {
        match node {
            layout::LayoutNode::Window { name, .. } => {
                if name == target_id { None } else { Some(node.clone()) }
            }
            layout::LayoutNode::Container { children, .. } => {
                let mut found = false;
                if let Some(pos) = children.iter().position(|c| Self::is_node_id_match(c, target_id)) {
                    children.remove(pos);
                    found = true;
                }

                if !found {
                    for child in children.iter_mut() {
                        if let Some(modified) = Self::remove_node_and_collapse(child, target_id) {
                            *child = modified;
                            found = true;
                            break;
                        }
                    }
                }

                if found {
                    if children.is_empty() {
                        None
                    } else if children.len() == 1 {
                        Some(children.remove(0)) // 容器只剩一个子节点，解包
                    } else {
                        Some(node.clone())
                    }
                } else {
                    Some(node.clone())
                }
            }
        }
    }

    fn is_node_id_match(node: &layout::LayoutNode, id: &str) -> bool {
        match node {
            layout::LayoutNode::Window { name, .. } => name == id,
            layout::LayoutNode::Container { id: node_id, .. } => node_id == id,
        }
    }

    fn recalc_node_percentages_recursive(
        node: &mut layout::LayoutNode,
        rect_map: &std::collections::HashMap<String, layout::WindowRect>
    ) {
        if let layout::LayoutNode::Container { direction, children, .. } = node {
            let dir_val = *direction; // 【修复】：解引用拷贝，避免 move 错误

            for child in children.iter_mut() {
                Self::recalc_node_percentages_recursive(child, rect_map);
            }

            if children.len() < 2 { return; }

            let mut total_content = 0u16;
            let mut child_contents: Vec<u16> = Vec::with_capacity(children.len());

            for child in children.iter() {
                let phys = Self::get_node_physical_rect(child, rect_map).unwrap_or_default();

                let overhead = match child {
                    layout::LayoutNode::Window { border, .. } => {
                        let (ox, oy) = border.overhead();
                        if dir_val == layout::Direction::Horizontal { ox } else { oy }
                    },
                    layout::LayoutNode::Container { .. } => 0,
                };

                let size = match dir_val { // 这里用 dir_val
                    layout::Direction::Horizontal => phys.width.saturating_sub(overhead),
                    layout::Direction::Vertical => phys.height.saturating_sub(overhead),
                };

                child_contents.push(size);
                total_content = total_content.saturating_add(size);
            }

            if total_content == 0 { return; }

            // 【终极修复】：最后一个子节点补齐 100%，彻底消灭取整误差导致的抖动！
            let mut sum_pct = 0u16;
            for i in 0..children.len().saturating_sub(1) {
                let pct = (child_contents[i] as u32 * 100 / total_content as u32) as u16;
                Self::set_node_percent(&mut children[i], Some(pct));
                sum_pct += pct;
            }

            if !children.is_empty() {
                let last_pct = 100u16.saturating_sub(sum_pct);
                if let Some(last_child) = children.last_mut() {
                    Self::set_node_percent(last_child, Some(last_pct));
                }
            }
        }
    }

    fn get_node_physical_rect(
        node: &layout::LayoutNode,
        rect_map: &std::collections::HashMap<String, layout::WindowRect>
    ) -> Option<layout::WindowRect> {
        match node {
            layout::LayoutNode::Window { name, .. } => rect_map.get(name).copied(),
            layout::LayoutNode::Container { children, .. } => {
                let mut min_col = u16::MAX; let mut min_row = u16::MAX;
                let mut max_col = 0u16; let mut max_row = 0u16;
                let mut found = false;
                for child in children {
                    if let Some(r) = Self::get_node_physical_rect(child, rect_map) {
                        found = true;
                        min_col = min_col.min(r.start_col);
                        min_row = min_row.min(r.start_row);
                        max_col = max_col.max(r.start_col + r.width);
                        max_row = max_row.max(r.start_row + r.height);
                    }
                }
                if found {
                    Some(layout::WindowRect { start_col: min_col, start_row: min_row, width: max_col - min_col, height: max_row - min_row })
                } else { None }
            }
        }
    }

    fn set_node_percent(node: &mut layout::LayoutNode, pct: Option<u16>) {
        match node {
            layout::LayoutNode::Window { size, .. } => *size = pct.map(layout::WindowSize::Percent),
            layout::LayoutNode::Container { percent, .. } => *percent = pct,
        }
    }

    fn contains_node(node: &layout::LayoutNode, target_id: &str) -> bool {
        match node {
            layout::LayoutNode::Window { name, .. } => name == target_id,
            layout::LayoutNode::Container { children, .. } => children.iter().any(|c| Self::contains_node(c, target_id)),
        }
    }

    fn extract_node_by_id(node: &layout::LayoutNode, target_id: &str) -> Option<layout::LayoutNode> {
        match node {
            layout::LayoutNode::Window { name, .. } => {
                if name == target_id { Some(node.clone()) } else { None }
            }
            layout::LayoutNode::Container { children, .. } => {
                for child in children {
                    if let Some(found) = Self::extract_node_by_id(child, target_id) {
                        return Some(found);
                    }
                }
                None
            }
        }
    }

    fn search_parent_container(node: &layout::LayoutNode, target_id: &str) -> Option<String> {
        match node {
            layout::LayoutNode::Container { id, children, .. } => {
                // 检查目标是否在直接子节点中
                for child in children {
                    let child_id = match child {
                        layout::LayoutNode::Window { name, .. } => name.as_str(),
                        layout::LayoutNode::Container { id, .. } => id.as_str(),
                    };
                    if child_id == target_id {
                        return Some(id.clone());
                    }
                }
                // 递归搜索
                for child in children {
                    if let Some(parent) = Self::search_parent_container(child, target_id) {
                        return Some(parent);
                    }
                }
                None
            }
            _ => None,
        }
    }
}
