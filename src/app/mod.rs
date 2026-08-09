// src/app/mod.rs

pub mod view;
pub mod statusbar;
pub mod input;
pub mod tree;
pub mod navigation;
pub mod data_loader;
pub mod drag_surgery;

pub use view::ViewState;
pub use statusbar::StatusBarState;
pub use input::InputState;
pub use tree::TreeState;

use crate::config::BindConfig;
use crate::exec;
use crate::layout::{self, WindowRect, BorderStyle};
use crate::protocol::Dataset;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use std::sync::mpsc::{Sender, Receiver};

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    None,
    Component(String),
}

impl Default for Focus {
    fn default() -> Self {
        Focus::None
    }
}

#[derive(Debug, Clone)]
pub struct DragEdge {
    pub primary_id: String,
    pub neighbor_id: String,
    pub direction: layout::Direction,
    pub hit_rect: layout::WindowRect,
    pub z_index: usize,
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
    ScrollLeft,
    ScrollRight,
    // 【新增】
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    CycleLayer,
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
            ("__SCROLL_LEFT__", None) => Some(Self::ScrollLeft),
            ("__SCROLL_RIGHT__", None) => Some(Self::ScrollRight),
            // 【新增映射】
            ("__FOCUS_LEFT__", None) => Some(Self::FocusLeft),
            ("__FOCUS_RIGHT__", None) => Some(Self::FocusRight),
            ("__FOCUS_UP__", None) => Some(Self::FocusUp),
            ("__FOCUS_DOWN__", None) => Some(Self::FocusDown),
            ("__CYCLE_LAYER__", None) => Some(Self::CycleLayer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DragTarget {
    ResizeEdge(String, String, layout::Direction),
    // 【新增】拖拽浮动窗口边缘 (图层名称, 位掩码: 1=Left, 2=Right, 4=Top, 8=Bottom)
    ResizeFloating(String, u8),
}

#[derive(Debug, Default)]
pub struct DragState {
    pub active: bool,
    pub is_marking: bool,
    pub start_idx: Option<usize>,
    pub resize_target: Option<DragTarget>,
    pub cached_edges: Vec<DragEdge>,
    pub cached_intersections: Vec<(u16, u16)>,
    pub start_col: u16,
    pub start_row: u16,
    pub last_col: u16,
    pub last_row: u16,
    pub initial_t1_rect: WindowRect,
    pub initial_t2_rect: WindowRect,
    pub is_restructured: bool,
    // 【新增】浮动窗口拖拽专用：记录初始状态
    pub initial_width: u16,
    pub initial_height: u16,
    pub initial_anchor_x: u16,
    pub initial_anchor_y: u16,
}

#[derive(Debug, Clone, Default)]
pub struct FocusState {
    pub current: Focus,
    pub main_tree_name: Option<String>,
}

#[derive(Debug, Default)]
pub struct MouseState {
    pub enabled: bool,
    pub last_click_time: Option<Instant>,
    pub last_clicked_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct SignalState {
    pub last_emit: std::collections::HashMap<&'static str, Instant>,
    pub last_emitted_select_id: Option<String>,
}

#[derive(Debug)]
pub struct Engine {
    pub components: HashMap<String, Component>,
    pub layout_layers: Vec<layout::LayoutLayer>,
    pub window_rect_overrides: HashMap<String, layout::WindowSize>,
    pub key_bindings: BindConfig,
    pub border_chars: HashMap<String, String>,
    pub global_relations: Vec<crate::protocol::Relation>,
    pub last_error: Option<String>,

    pub drag: DragState,
    pub focus: FocusState,
    pub mouse: MouseState,
    pub signals: SignalState,
    pub is_initialized: bool,
    pub max_lines: usize,
    pub prev_rects: std::collections::HashMap<String, WindowRect>,
    pub dirty_components: std::collections::HashSet<String>,
    pub pending_selection_changed: Option<String>,
    pub async_view_tx: Sender<(String, Option<String>, String)>,
    pub async_view_rx: Receiver<(String, Option<String>, String)>,
    pub async_reload_tx: Sender<(String, std::io::Result<String>)>,
    pub async_reload_rx: Receiver<(String, std::io::Result<String>)>,
    pub pending_view_reload: std::collections::HashSet<String>,
    pub prev_term_size: (u16, u16),
    pub ui_theme: crate::style::UiTheme,
    pub focus_history: Vec<String>,
}

fn parse_component_prefixes(cfg: &str) -> (bool, bool, bool, &str) {
    let mut click = false;
    let mut focus = false;
    let mut nomark = false;
    let mut rest = cfg.trim();

    loop {
        if let Some(stripped) = rest.strip_prefix("click:") {
            click = true;
            rest = stripped.trim();
        } else if let Some(stripped) = rest.strip_prefix("focus:") {
            focus = true;
            rest = stripped.trim();
        } else if let Some(stripped) = rest.strip_prefix("nomark:") {
            nomark = true;
            rest = stripped.trim();
        } else {
            break;
        }
    }
    (click, focus, nomark, rest)
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
        max_lines: usize,
        ui_colors: &str,
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

        let parsed_layout = layout::parse_layouts(&layout_strings);
        let layout_layers = parsed_layout.layers;

        let mut init_error = None;
        let mut first_tree_name = None;

        for t_cfg in trees {
            let (click_to_fire, focus_to_fire, nomark, rest) = parse_component_prefixes(&t_cfg);
            let markable = !nomark; // 【修复】反转逻辑：没有声明 nomark 的树，默认都是可标记的
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
                h_scroll: 0,
                v_scroll: 0,
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
                h_scroll: 0,
                is_loading: false,
            }));
        }

        for s_cfg in statusbars {
            let parts: Vec<&str> = s_cfg.splitn(2, ':').collect();
            let name = parts[0].to_string();
            let fmt = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
            components.insert(name, Component::StatusBar(StatusBarState {
                format_template: fmt,
                message: None,
                message_expire: None,
            }));
        }

        for i_cfg in inputs {
            let parts: Vec<&str> = i_cfg.splitn(3, ':').collect();
            let name = parts[0].to_string();
            let prefix = parts.get(1).filter(|s| !s.is_empty()).map(|s| s.to_string()).unwrap_or_else(|| ":".to_string());
            let on_submit_raw = parts.get(2).map(|s| s.to_string());

            // 【修复】统一返回 Option<String>
            let (is_silent, on_submit) = if let Some(ref cmd) = on_submit_raw {
                if cmd.starts_with('@') {
                    (true, Some(cmd[1..].to_string()))
                } else {
                    (false, Some(cmd.clone()))
                }
            } else {
                (false, None)
            };

            let mut input_state = InputState::new(&prefix);
            input_state.on_submit = on_submit;
            input_state.on_submit_is_silent = is_silent;
            components.insert(name, Component::Input(input_state));
        }

        let focused = first_tree_name.clone()
            .map(|n| Focus::Component(n))
            .unwrap_or(Focus::None);

        let (tx, rx) = std::sync::mpsc::channel();
        let (rtx, rrx) = std::sync::mpsc::channel();

        let mut engine = Self {
            components,
            layout_layers,
            key_bindings,
            last_error: init_error,
            global_relations,
            border_chars: border_chars_map,
            window_rect_overrides: std::collections::HashMap::new(),
            drag: DragState::default(),
            focus: FocusState {
                current: focused.clone(),
                main_tree_name: first_tree_name,
            },
            mouse: MouseState {
                enabled: mouse_enabled,
                ..Default::default()
            },
            signals: SignalState::default(),
            is_initialized: false,
            max_lines,
            prev_rects: std::collections::HashMap::new(),
            dirty_components: std::collections::HashSet::new(),
            pending_selection_changed: None,
            async_view_tx: tx,
            async_view_rx: rx,
            async_reload_tx: rtx,
            async_reload_rx: rrx,
            pending_view_reload: std::collections::HashSet::new(),
            prev_term_size: (0, 0),
            ui_theme: crate::style::UiTheme::parse(ui_colors),
            focus_history: Vec::new(),
        };

        engine.is_initialized = false;
        engine
    }

    pub fn mark_dirty(&mut self, name: &str) {
        self.dirty_components.insert(name.to_string());
        if matches!(self.components.get(name), Some(Component::Tree(_))) {
            for (n, c) in &self.components {
                if matches!(c, Component::StatusBar(_)) {
                    self.dirty_components.insert(n.clone());
                }
            }
        }
    }

    pub fn mark_all_dirty(&mut self) {
        for name in self.components.keys() {
            self.dirty_components.insert(name.clone());
        }
    }

    pub fn flush_pending_updates(&mut self, term_width: u16, term_height: u16) {
        if let Some(tree_name) = self.pending_selection_changed.take() {
            self.broadcast_selection_changed(&tree_name, term_width, term_height);
            self.emit_select_if_changed(term_width, term_height);
        }
    }

    pub fn scroll_left(&mut self) {
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.h_scroll = v.h_scroll.saturating_sub(5);
        } else if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.h_scroll = t.h_scroll.saturating_sub(5);
        }
    }

    pub fn scroll_right(&mut self) {
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.h_scroll = v.h_scroll.saturating_add(5);
        } else if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.h_scroll = t.h_scroll.saturating_add(5);
        }
    }

    pub fn calc_all_rects(&self, term_width: u16, term_height: u16) -> Vec<(WindowRect, String, BorderStyle, usize)> {
        layout::calc_window_rects(&self.layout_layers, term_width, term_height, &self.window_rect_overrides)
    }

    pub fn toggle_layout_visible(&mut self, name: &str) {
        for layer in &mut self.layout_layers {
            let has_window = Self::layout_contains_window(layer, name);
            if has_window {
                layer.visible = !layer.visible;

                if layer.visible {
                    // 打开图层：获取焦点
                    if let Some(win_name) = Self::get_first_window_name(&layer.root) {
                        self.set_focus(&win_name);
                    }
                } else {
                    // 关闭图层：如果关的是当前焦点，从历史栈找上一个可见的
                    if let Focus::Component(curr_name) = &self.focus.current {
                        if Self::layout_contains_window(layer, curr_name) {
                            let mut restored = false;
                            while let Some(prev_name) = self.focus_history.pop() {
                                let is_visible = self.layout_layers.iter().any(|l| l.visible && Self::layout_contains_window(l, &prev_name));
                                if is_visible {
                                    self.focus.current = Focus::Component(prev_name.clone());
                                    self.mark_dirty(&prev_name);
                                    restored = true;
                                    break;
                                }
                            }
                            if !restored {
                                if let Some(main_tree) = self.focus.main_tree_name.clone() {
                                    self.set_focus(&main_tree);
                                }
                            }
                        }
                    }
                }

                self.mark_all_dirty();
                self.prev_rects.clear();
                return;
            }
        }
        if let Ok(idx) = name.parse::<usize>() {
            if let Some(layer) = self.layout_layers.get_mut(idx) {
                layer.visible = !layer.visible;

                if layer.visible {
                    if let Some(win_name) = Self::get_first_window_name(&layer.root) {
                        self.set_focus(&win_name);
                    }
                } else {
                    if let Focus::Component(curr_name) = &self.focus.current {
                        if Self::layout_contains_window(layer, curr_name) {
                            if let Some(main_tree) = self.focus.main_tree_name.clone() {
                                self.set_focus(&main_tree);
                            }
                        }
                    }
                }

                self.mark_all_dirty();
                self.prev_rects.clear();
            }
        }
    }

    pub fn set_layout_visible(&mut self, name: &str, visible: bool) {
        for layer in &mut self.layout_layers {
            let has_window = Self::layout_contains_window(layer, name);
            if has_window {
                if layer.visible != visible {
                    layer.visible = visible;

                    if visible {
                        if let Some(win_name) = Self::get_first_window_name(&layer.root) {
                            self.set_focus(&win_name);
                        }
                    } else {
                        if let Focus::Component(curr_name) = &self.focus.current {
                            if Self::layout_contains_window(layer, curr_name) {
                                let mut restored = false;
                                while let Some(prev_name) = self.focus_history.pop() {
                                    let is_visible = self.layout_layers.iter().any(|l| l.visible && Self::layout_contains_window(l, &prev_name));
                                    if is_visible {
                                        self.focus.current = Focus::Component(prev_name.clone());
                                        self.mark_dirty(&prev_name);
                                        restored = true;
                                        break;
                                    }
                                }
                                if !restored {
                                    if let Some(main_tree) = self.focus.main_tree_name.clone() {
                                        self.set_focus(&main_tree);
                                    }
                                }
                            }
                        }
                    }

                    self.mark_all_dirty();
                    self.prev_rects.clear();
                }
                return;
            }
        }
        if let Ok(idx) = name.parse::<usize>() {
            if let Some(layer) = self.layout_layers.get_mut(idx) {
                if layer.visible != visible {
                    layer.visible = visible;

                    if visible {
                        if let Some(win_name) = Self::get_first_window_name(&layer.root) {
                            self.set_focus(&win_name);
                        }
                    } else {
                        if let Focus::Component(curr_name) = &self.focus.current {
                            if Self::layout_contains_window(layer, curr_name) {
                                if let Some(main_tree) = self.focus.main_tree_name.clone() {
                                    self.set_focus(&main_tree);
                                }
                            }
                        }
                    }

                    self.mark_all_dirty();
                    self.prev_rects.clear();
                }
            }
        }
    }

    fn layout_contains_window(layer: &layout::LayoutLayer, name: &str) -> bool {
        fn check_node(node: &layout::LayoutNode, name: &str) -> bool {
            match node {
                layout::LayoutNode::Window { name: n, .. } => n == name,
                layout::LayoutNode::Container { children, .. } => {
                    children.iter().any(|c| check_node(c, name))
                }
            }
        }
        check_node(&layer.root, name)
    }

    // 【新增】获取图层中的第一个窗口名称，用于自动获取焦点
    fn get_first_window_name(node: &layout::LayoutNode) -> Option<String> {
        match node {
            layout::LayoutNode::Window { name, .. } => Some(name.clone()),
            layout::LayoutNode::Container { children, .. } => {
                for c in children {
                    if let Some(n) = Self::get_first_window_name(c) {
                        return Some(n);
                    }
                }
                None
            }
        }
    }

    pub fn build_exec_context(
        selected_entity: Option<&crate::protocol::Entity>,
        ids_str: &str,
        paths_str: &str,
        window_name: &str,
        width: &str,
        height: &str,
        event: &str,
        extra: Option<&[(&str, &str)]>,
    ) -> HashMap<String, String> {
        let mut ctx = HashMap::new();

        if let Some(entity) = selected_entity {
            ctx.insert("id".to_string(), entity.id.clone());
            ctx.insert("path".to_string(), entity.path.clone());
            ctx.insert("display".to_string(), entity.display.clone());
            ctx.insert("tags".to_string(), entity.tags.clone());
        } else {
            ctx.insert("id".to_string(), String::new());
            ctx.insert("path".to_string(), String::new());
            ctx.insert("display".to_string(), String::new());
            ctx.insert("tags".to_string(), String::new());
        }

        ctx.insert("ids".to_string(), ids_str.to_string());
        ctx.insert("paths".to_string(), paths_str.to_string());
        ctx.insert("window".to_string(), window_name.to_string());
        ctx.insert("width".to_string(), width.to_string());
        ctx.insert("height".to_string(), height.to_string());
        ctx.insert("event".to_string(), event.to_string());

        if let Some(extra_pairs) = extra {
            for (k, v) in extra_pairs {
                ctx.insert(k.to_string(), v.to_string());
            }
        }
        ctx
    }

    // 【新增】统一的输入提交执行器
    pub fn submit_input(&mut self, input_name: &str, text: &str, term_width: u16, term_height: u16) {
        let (template_opt, is_silent) = if let Some(Component::Input(input)) = self.components.get(input_name) {
            (input.on_submit.clone(), input.on_submit_is_silent)
        } else { return };

        if let Some(template) = template_opt {
            let args = crate::config::split_args(&template);

            let tree_name = match &self.focus.current {
                Focus::Component(n) if matches!(self.components.get(n), Some(Component::Tree(_))) => n.clone(),
                _ => self.focus.main_tree_name.clone().unwrap_or_default(),
            };

            let (selected_entity, ids_str, paths_str) = if let Some(Component::Tree(t)) = self.components.get(&tree_name) {
                let sel = t.get_selected_entity().cloned();
                let marked = t.get_marked_entities();
                let entities: Vec<&crate::protocol::Entity> = if !marked.is_empty() {
                    marked.iter().cloned().collect()
                } else {
                    sel.as_ref().map(|e| vec![e]).unwrap_or_default()
                };
                let ids = entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(" ");
                let paths = entities.iter()
                    .map(|e| {
                        if e.path.contains(' ') {
                            format!("\"{}\"", e.path)
                        } else {
                            e.path.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                (sel, ids, paths)
            } else {
                (None, String::new(), String::new())
            };

            let window_name = match &self.focus.current { Focus::Component(n) => n.clone(), _ => String::new() };

            let ctx = Self::build_exec_context(
                selected_entity.as_ref(), &ids_str, &paths_str, &window_name,
                &term_width.to_string(), &term_height.to_string(), "",
                Some(&[("input", text)])
            );

            let full_cmd_args = exec::replace_placeholders_in_args(&args, &ctx);
            if !full_cmd_args.is_empty() {
                crate::runner::execute_binding(self, &full_cmd_args, is_silent, term_width, term_height);
            }
        }
    }

    pub fn emit(&mut self, signal: &'static str, term_width: u16, term_height: u16) -> bool {
        // ... (保留防抖与实体获取逻辑)
        if signal == "select" {
            let now = Instant::now();
            if let Some(last) = self.signals.last_emit.get(signal) {
                if now.duration_since(*last).as_millis() < 200 { return false; }
            }
            self.signals.last_emit.insert(signal, now);
        }

        let current_window = match &self.focus.current { Focus::Component(n) => Some(n.as_str()), Focus::None => None };
        let binding = self.key_bindings.get_signal_binding(current_window, signal);

        if let Some((cmd_template_args, _is_silent)) = binding {
            let tree_name = match &self.focus.current {
                Focus::Component(n) if matches!(self.components.get(n), Some(Component::Tree(_))) => n.clone(),
                _ => self.focus.main_tree_name.clone().unwrap_or_default(),
            };

            let (selected_entity, ids_str, paths_str) = if let Some(Component::Tree(t)) = self.components.get(&tree_name) {
                let sel = t.get_selected_entity();
                let marked = t.get_marked_entities();
                let entities: Vec<&crate::protocol::Entity> = if !marked.is_empty() {
                    marked.iter().cloned().collect()
                } else { sel.map(|e| vec![e]).unwrap_or_default() };
                let ids = entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(" ");
                let paths = entities.iter()
                    .map(|e| {
                        if e.path.contains(' ') {
                            format!("\"{}\"", e.path)
                        } else {
                            e.path.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                (sel.cloned(), ids, paths)
            } else { (None, String::new(), String::new()) };

            let window_name = current_window.unwrap_or("").to_string();
            let ctx = Self::build_exec_context(
                selected_entity.as_ref(), &ids_str, &paths_str, &window_name,
                &term_width.to_string(), &term_height.to_string(), signal, None
            );

            let full_cmd_args = exec::replace_placeholders_in_args(cmd_template_args, &ctx);
            if !full_cmd_args.is_empty() && !(full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
                let _ = exec::execute_command_silent(&full_cmd_args);
            }
        }
        true
    }

    pub fn emit_select_if_changed(&mut self, term_width: u16, term_height: u16) {
        let current_id = self.get_selected_entity().map(|e| e.id.clone());
        if current_id != self.signals.last_emitted_select_id {
            if self.emit("select", term_width, term_height) {
                self.signals.last_emitted_select_id = current_id;
            }
        }
    }

    pub fn get_focused_tree_state(&self) -> Option<&TreeState> {
        let name = match &self.focus.current {
            Focus::Component(n) => n,
            Focus::None => return self.get_main_tree_state(),
        };
        if let Some(Component::Tree(t)) = self.components.get(name) {
            return Some(t);
        }
        self.get_main_tree_state()
    }

    pub fn get_selected_entity(&self) -> Option<&crate::protocol::Entity> {
        let name = match &self.focus.current {
            Focus::Component(n) => n,
            Focus::None => self.focus.main_tree_name.as_ref()?,
        };
        if let Some(Component::Tree(t)) = self.components.get(name) {
            return t.get_selected_entity();
        }
        None
    }

    pub fn prepare_key_binding_args(&self, key: &crossterm::event::KeyEvent, term_width: u16, term_height: u16) -> Option<(Vec<String>, bool)> {
        let (cmd_template_args, is_silent) = self.key_bindings.get(key)?;

        let tree_name = match &self.focus.current {
            Focus::Component(n) if matches!(self.components.get(n), Some(Component::Tree(_))) => n.clone(),
            Focus::None => self.focus.main_tree_name.as_ref()?.clone(),
            _ => self.focus.main_tree_name.as_ref()?.clone(),
        };

        let tree_state = if let Some(Component::Tree(t)) = self.components.get(&tree_name) { t } else { return None; };
        let selected_entity = tree_state.get_selected_entity();
        let marked_entities = tree_state.get_marked_entities();

        let entities: Vec<&crate::protocol::Entity> = if !marked_entities.is_empty() {
            marked_entities.iter().cloned().collect()
        } else { selected_entity.map(|e| vec![e]).unwrap_or_default() };

        let ids_str = entities.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(" ");
        let paths_str = entities.iter()
            .map(|e| {
                if e.path.contains(' ') {
                    format!("\"{}\"", e.path)
                } else {
                    e.path.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        let window_name = match &self.focus.current { Focus::Component(n) => n.clone(), Focus::None => String::new() };
        let ctx = Self::build_exec_context(
            selected_entity, &ids_str, &paths_str, &window_name,
            &term_width.to_string(), &term_height.to_string(), "", None
        );

        let full_cmd_args = exec::replace_placeholders_in_args(cmd_template_args, &ctx);
        if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
            None
        } else { Some((full_cmd_args, *is_silent)) }
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
        let name = self.focus.main_tree_name.as_ref()?;
        if let Some(Component::Tree(t)) = self.components.get(name) { Some(t) } else { None }
    }
}
