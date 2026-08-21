// src/app/mod.rs

pub mod view;
pub mod statusbar;
pub mod input;
pub mod tree;
pub mod navigation;
pub mod data_loader;
pub mod drag_surgery;
pub mod metrics;
pub mod overlay;
pub mod event_handler;

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

#[derive(Debug)]
pub enum Component {
    Tree(TreeState),
    View(ViewState),
    StatusBar(StatusBarState),
    Input(InputState),
}

#[derive(Debug, Clone)]
pub struct OverlayLayer {
    pub source: String,
    pub target: String,
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
    ActivateInput(String),
    ToggleLayout(String),
    ShowLayout(String),
    HideLayout(String),
    ScrollLeft,
    ScrollRight,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    CycleLayer,
    CloseOverlay(String),  // 新增
    CloseTopOverlay,       // 新增
    Noop,
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
            ("__NOOP__", None) => Some(Self::Noop),
            ("__ACTIVATE_INPUT__", Some(name)) => Some(Self::ActivateInput(name)),
            ("__TOGGLE_LAYOUT__", Some(name)) => Some(Self::ToggleLayout(name)),
            ("__SHOW_LAYOUT__", Some(name)) => Some(Self::ShowLayout(name)),
            ("__HIDE_LAYOUT__", Some(name)) => Some(Self::HideLayout(name)),
            ("__SCROLL_LEFT__", None) => Some(Self::ScrollLeft),
            ("__SCROLL_RIGHT__", None) => Some(Self::ScrollRight),
            ("__FOCUS_LEFT__", None) => Some(Self::FocusLeft),
            ("__FOCUS_RIGHT__", None) => Some(Self::FocusRight),
            ("__FOCUS_UP__", None) => Some(Self::FocusUp),
            ("__FOCUS_DOWN__", None) => Some(Self::FocusDown),
            ("__CYCLE_LAYER__", None) => Some(Self::CycleLayer),
            ("__CLOSE_OVERLAY__", Some(name)) => Some(Self::CloseOverlay(name)),
            ("__CLOSE_TOP_OVERLAY__", None) => Some(Self::CloseTopOverlay),
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

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub initial_dataset: Dataset,
    pub layout_strings: Vec<String>,
    pub key_bindings: BindConfig,
    pub mouse_enabled: bool,
    pub border_chars: Vec<String>,
    pub trees: Vec<String>,
    pub views: Vec<String>,
    pub statusbars: Vec<String>,
    pub inputs: Vec<String>,
    pub relations_path: Option<String>,
    pub max_lines: usize,
    pub ui_colors: String,
}


#[derive(Debug)]
pub struct Engine {
    pub components: HashMap<String, Component>,
    pub layout_layers: Vec<layout::LayoutLayer>,
    pub window_rect_overrides: HashMap<String, layout::WindowSize>,
    pub auto_overrides: HashMap<String, layout::WindowSize>,
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
    pub pending_blur: Option<String>,
    pub async_view_tx: Sender<(String, Option<String>, Vec<u8>, bool)>,
    pub async_view_rx: Receiver<(String, Option<String>, Vec<u8>, bool)>,
    pub async_reload_tx: Sender<(String, std::io::Result<String>)>,
    pub async_reload_rx: Receiver<(String, std::io::Result<String>)>,
    pub async_exec_tx: Sender<()>,       // 【新增】异步脚本执行完成通知
    pub async_exec_rx: Receiver<()>,     // 【新增】异步脚本执行完成接收
    pub pending_view_reload: std::collections::HashSet<String>,
    pub prev_term_size: (u16, u16),
    pub ui_theme: crate::style::UiTheme,
    pub focus_history: Vec<String>,
    pub layout_blueprint: Vec<String>,
    pub overlay_stack: Vec<OverlayLayer>, // 【新增】统一覆盖栈
}

fn parse_view_tree_prefixes(cfg_str: &str) -> (bool, bool, String, String, &str) {
    let mut no_hover = false;
    let mut no_focus = false;
    let mut search_scope = "all".to_string();
    let mut keymap = "default".to_string(); // 默认是 default
    let mut rest = cfg_str.trim();

    loop {
        if let Some(stripped) = rest.strip_prefix("nohover:") {
            no_hover = true; rest = stripped.trim();
        } else if let Some(stripped) = rest.strip_prefix("nofocus:") {
            no_focus = true; rest = stripped.trim();
        } else if let Some(stripped) = rest.strip_prefix("search-scope:") {
            if let Some(end_idx) = stripped.find(':') {
                search_scope = stripped[..end_idx].to_string();
                rest = stripped[end_idx + 1..].trim();
            } else { break; }
        } else if let Some(stripped) = rest.strip_prefix("keymap:") {
            // 解析语法: keymap:default+bookmarks
            if let Some(end_idx) = stripped.find(':') {
                keymap = stripped[..end_idx].to_string();
                rest = stripped[end_idx + 1..].trim();
            } else { break; }
        } else {
            break;
        }
    }
    (no_hover, no_focus, search_scope, keymap, rest)
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
    pub fn new(config: EngineConfig) -> Self {
        let EngineConfig {
            initial_dataset,
            layout_strings,
            key_bindings,
            mouse_enabled,
            border_chars,
            trees,
            views,
            statusbars,
            inputs,
            relations_path,
            max_lines,
            ui_colors,
        } = config;

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
            // 【修复】解构出 5 个返回值
            let (no_hover, no_focus, search_scope, keymap, rest_cfg) = parse_view_tree_prefixes(&t_cfg);
            let (click_to_fire, focus_to_fire, nomark, rest) = parse_component_prefixes(rest_cfg);
            let markable = !nomark;
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
                let mut ds = initial_dataset.clone();
                ds.relations = global_relations.clone();
                ds.child_index = crate::protocol::build_child_index(&ds.relations);
                ds
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
                title_override: None,
                no_hover,
                no_focus,
                search_scope,
                keymap, // 【修复】注入 keymap
            };
            tree_state.rebuild_visible_ids();
            if let Some(first_id) = tree_state.visible_ids.first().cloned() {
                tree_state.select_id(&first_id);
            }
            components.insert(name, Component::Tree(tree_state));
        }

        for v_cfg in views {
            // 【修复】解构出 5 个返回值
            let (no_hover, no_focus, _search_scope, keymap, rest_cfg) = parse_view_tree_prefixes(&v_cfg);
            let parts: Vec<&str> = rest_cfg.splitn(2, ':').collect();
            let name = parts[0].to_string();
            let cmd = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
            components.insert(name, Component::View(ViewState {
                cmd_template: cmd,
                scroll_offset: 0,
                content: crate::app::view::ViewContent::Empty,
                cached_entity_id: None,
                max_offset: 0,
                rect_width: 0,
                rect_height: 0,
                h_scroll: 0,
                is_loading: false,
                no_hover,
                no_focus,
                graphic_dirty: false,
                needs_graphic_clear: false,
                child_pid: std::sync::Arc::new(std::sync::Mutex::new(None)),
                keymap, // 【修复】注入 keymap
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
                current_text: String::new(),
            }));
        }

        for i_cfg in inputs {
            let mut is_instant = false;
            let mut is_search = false; // 新增
            let mut cfg_str = i_cfg.trim();

            if let Some(stripped) = cfg_str.strip_prefix("instant:") {
                is_instant = true;
                cfg_str = stripped.trim();
            }

            // 【新增】解析 search: 前缀
            if let Some(stripped) = cfg_str.strip_prefix("search:") {
                is_search = true;
                cfg_str = stripped.trim();
            }

            let parts: Vec<&str> = cfg_str.splitn(3, ':').collect();
            let name_part = parts[0].to_string();

            // 解析 Name[Target] 语法
            let (name, target_override) = if let Some(open_bracket) = name_part.find('[') {
                if let Some(close_bracket) = name_part.find(']') {
                    if close_bracket > open_bracket {
                        let n = name_part[..open_bracket].to_string();
                        let t = name_part[open_bracket+1..close_bracket].to_string();
                        (n, Some(t))
                    } else {
                        (name_part, None)
                    }
                } else {
                    (name_part, None)
                }
            } else {
                (name_part, None)
            };

            let prefix = parts.get(1).filter(|s| !s.is_empty()).map(|s| s.to_string()).unwrap_or_else(|| ":".to_string());
            let on_submit_raw = parts.get(2).map(|s| s.to_string());

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
            input_state.is_instant = is_instant;
            input_state.target_override = target_override;
            input_state.is_search = is_search; // 注入标志
            components.insert(name, Component::Input(input_state));
        }

        let focused = first_tree_name.clone()
            .map(|n| Focus::Component(n))
            .unwrap_or(Focus::None);

        let (tx, rx) = std::sync::mpsc::channel();
        let (rtx, rrx) = std::sync::mpsc::channel();
        let (etx, erx) = std::sync::mpsc::channel(); // 【新增】


        let mut engine = Self {
            components,
            layout_layers,
            key_bindings,
            last_error: init_error,
            global_relations,
            border_chars: border_chars_map,
            window_rect_overrides: std::collections::HashMap::new(),
            auto_overrides: std::collections::HashMap::new(),
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
            pending_blur: None,
            async_view_tx: tx,
            async_view_rx: rx,
            async_reload_tx: rtx,
            async_reload_rx: rrx,
            async_exec_tx: etx,   // 【新增】
            async_exec_rx: erx,   // 【新增】
            pending_view_reload: std::collections::HashSet::new(),
            prev_term_size: (0, 0),
            ui_theme: crate::style::UiTheme::parse(&ui_colors),
            focus_history: Vec::new(),
            layout_blueprint: layout_strings,
            overlay_stack: Vec::new(), // 【新增】初始化
        };

        engine.is_initialized = false;
        engine
    }

    /// 【新增】判断组件是否绝对不可获取焦点（StatusBar 和 声明了 nofocus: 的组件）
    pub fn is_unfocusable(&self, name: &str) -> bool {
        match self.components.get(name) {
            Some(Component::StatusBar(_)) => true,
            Some(Component::View(v)) => v.no_focus,
            Some(Component::Tree(t)) => t.no_focus,
            _ => false,
        }
    }

    /// 【新增】判断组件是否免疫鼠标悬停
    pub fn is_hover_immune(&self, name: &str) -> bool {
        match self.components.get(name) {
            Some(Component::StatusBar(_)) => true,
            Some(Component::View(v)) => v.no_hover,
            Some(Component::Tree(t)) => t.no_hover,
            _ => false,
        }
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

        // 【新增】处理失去焦点信号
        if let Some(blur_name) = self.pending_blur.take() {
            // 【修复】使用 keymap 查找 blur 信号，保持一致性
            let blur_keymaps_owned = self.get_keymaps_for(&blur_name);
            let blur_keymaps: Vec<Option<&str>> = blur_keymaps_owned.iter().map(|s| s.as_deref()).collect();
            let binding = self.key_bindings.get_signal_binding_keymap(&blur_keymaps, "blur");

            if let Some((cmd_template_args, _is_silent)) = binding {
                let ctx = Self::build_exec_context(
                    None, "", "", &blur_name,
                    &term_width.to_string(), &term_height.to_string(), "blur", None
                );
                let full_cmd_args = crate::exec::replace_placeholders_in_args(cmd_template_args, &ctx);

                // 复用内部指令直连魔法！零延迟同步执行 UI 状态变更
                if let Some(internal_cmd) = InternalCommand::from_args(&full_cmd_args) {
                    match internal_cmd {
                        InternalCommand::ToggleLayout(name) => self.toggle_layout_visible(&name),
                        InternalCommand::ShowLayout(name) => self.set_layout_visible(&name, true),
                        InternalCommand::HideLayout(name) => self.set_layout_visible(&name, false),
                        _ => {}
                    }
                    let _ = crate::exec::execute_command_silent(&full_cmd_args);
                }
            }
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
        layout::calc_window_rects(
            &self.layout_layers,
            term_width,
            term_height,
            &self.window_rect_overrides,
            &self.auto_overrides // 【新增】
        )
    }

    // 【新增】提取的统一图层应用逻辑
    fn apply_layer_visibility(&mut self, layer_idx: usize, visible: bool) {
        let layer = &mut self.layout_layers[layer_idx];
        if layer.visible == visible { return; }
        layer.visible = visible;

        if visible {
            if let Some(win_name) = Self::get_first_window_name(&layer.root) {
                self.set_focus(&win_name);
            }
        } else {
            if let Focus::Component(curr_name) = &self.focus.current.clone() {
                let layer_contains_curr = Self::layout_contains_window(&self.layout_layers[layer_idx], curr_name);
                if layer_contains_curr {
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

    pub fn toggle_layout_visible(&mut self, name: &str) {
        let mut found_idx = None;
        for (i, layer) in self.layout_layers.iter().enumerate() {
            if Self::layout_contains_window(layer, name) {
                found_idx = Some(i);
                break;
            }
        }
        if found_idx.is_none() {
            if let Ok(idx) = name.parse::<usize>() {
                if idx < self.layout_layers.len() { found_idx = Some(idx); }
            }
        }
        if let Some(idx) = found_idx {
            let visible = !self.layout_layers[idx].visible;
            self.apply_layer_visibility(idx, visible);
        }
    }

    pub fn set_layout_visible(&mut self, name: &str, visible: bool) {
        let mut found_idx = None;
        for (i, layer) in self.layout_layers.iter().enumerate() {
            if Self::layout_contains_window(layer, name) {
                found_idx = Some(i);
                break;
            }
        }
        if found_idx.is_none() {
            if let Ok(idx) = name.parse::<usize>() {
                if idx < self.layout_layers.len() { found_idx = Some(idx); }
            }
        }
        if let Some(idx) = found_idx {
            self.apply_layer_visibility(idx, visible);
        }
    }

    pub fn layout_contains_window(layer: &layout::LayoutLayer, name: &str) -> bool {
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

    pub fn update_view_rects(&mut self, view_rects: HashMap<String, (usize, u16, u16)>) {
        for (name, (max_rows, width, height)) in view_rects {
            if let Some(Component::View(v)) = self.components.get_mut(&name) {
                // 【终极修复】只有 Text 才计算行数，Graphic 直接跳过！
                let total_lines = match &v.content {
                    crate::app::view::ViewContent::Text(text) => text.lines().count(),
                    _ => 1, // 图片或空内容不需要计算
                };

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

    // 【新增】预计算自适应高度，将 Auto 转化为临时的 Absolute 覆盖
    pub fn precalculate_auto_sizes(&mut self, term_height: u16) {
        // 拖拽期间绝对禁止预计算，防止与物理像素冻结冲突
        if self.drag.active {
            return;
        }

        let mut next_overrides = std::collections::HashMap::new();

        // 1. 扫描布局树，为非加载中的节点计算最新高度
        for layer in &self.layout_layers {
            Self::scan_auto_nodes(&layer.root, &self.components, term_height, &mut next_overrides);
        }

        // 2. 【冻结魔法】对于正在加载中的视图，如果上一帧有高度，直接继承过来！
        // 这样在异步加载的零点几秒内，窗口尺寸纹丝不动，消灭闪烁。
        for (name, size) in &self.auto_overrides {
            if !next_overrides.contains_key(name) {
                if let Some(Component::View(v)) = self.components.get(name) {
                    if v.is_loading {
                        // 【修复】继承高度时，必须强制 clamp，防止终端缩小导致高度溢出
                        let clamped_size = match size {
                            layout::WindowSize::Absolute(h) => {
                                layout::WindowSize::Absolute((*h).min(term_height).max(1))
                            }
                            _ => *size,
                        };
                        next_overrides.insert(name.clone(), clamped_size);
                    }
                }
            }
        }

        self.auto_overrides = next_overrides;
    }

    fn scan_auto_nodes(
        node: &layout::LayoutNode,
        components: &std::collections::HashMap<String, Component>,
        term_height: u16,
        overrides: &mut std::collections::HashMap<String, layout::WindowSize>
    ) {
        match node {
            layout::LayoutNode::Window { name, size, .. } => {
                if let Some(layout::WindowSize::Auto(fallback)) = size {

                    // 【核心防御】如果视图正在加载，直接跳过，不生成新高度
                    // 让外层逻辑把上一帧的高度继承过来
                    let is_loading = if let Some(Component::View(v)) = components.get(name) {
                        v.is_loading
                    } else {
                        false
                    };

                    if !is_loading {
                        let content_lines = match components.get(name) {
                            Some(Component::View(v)) => {
                                match &v.content {
                                    crate::app::view::ViewContent::Text(text) => {
                                        if text.is_empty() { *fallback as usize } else { text.lines().count() }
                                    }
                                    _ => *fallback as usize,
                                }
                            }
                            Some(Component::Tree(t)) => {
                                if t.visible_ids.is_empty() {
                                    *fallback as usize
                                } else {
                                    t.visible_ids.len()
                                }
                            }
                            _ => *fallback as usize
                        };

                        // 限制不超过终端总高度，且至少为 1
                        let clamped = content_lines.min(term_height as usize).max(1) as u16;
                        overrides.insert(name.clone(), layout::WindowSize::Absolute(clamped));
                    }
                }
            }
            layout::LayoutNode::Container { children, .. } => {
                for child in children {
                    Self::scan_auto_nodes(child, components, term_height, overrides);
                }
            }
        }
    }
    // 【新增】提取：获取当前活动的 Tree 名称
    fn get_active_tree_name(&self) -> Option<String> {
        match &self.focus.current {
            Focus::Component(n) if matches!(self.components.get(n), Some(Component::Tree(_))) => Some(n.clone()),
            _ => self.focus.main_tree_name.clone(),
        }
    }

    // 【新增】提取：获取目标的 ids 和 paths 字符串
    fn get_target_strings(&self, tree_name: &str) -> (String, String) {
        if let Some(Component::Tree(t)) = self.components.get(tree_name) {
            let sel = t.get_selected_entity();
            let marked = t.get_marked_entities();
            let entities: Vec<&crate::protocol::Entity> = if !marked.is_empty() {
                marked.iter().cloned().collect()
            } else {
                sel.map(|e| vec![e]).unwrap_or_default()
            };

    // 【修改】给 ids_str 也加上引号处理，防止带有空格的 ID 传参断裂
            let ids_str = entities
                .iter()
                .map(|e| {
                    if e.id.contains(' ') {
                        format!("\"{}\"", e.id)
                    } else {
                        e.id.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            let paths_str = entities
                .iter()
                .map(|e| {
                    if e.path.contains(' ') {
                        format!("\"{}\"", e.path)
                    } else {
                        e.path.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");

            (ids_str, paths_str)
        } else {
            (String::new(), String::new())
        }
    }

    pub fn prepare_key_binding_args(&self, scope: Option<&str>, key: &crossterm::event::KeyEvent, term_width: u16, term_height: u16) -> Option<(Vec<String>, bool)> {
        let (cmd_template_args, is_silent) = self.key_bindings.get_scoped(scope, key)?;

        let tree_name = self.get_active_tree_name()?;
        let tree_state = if let Some(Component::Tree(t)) = self.components.get(&tree_name) { t } else { return None; };
        let selected_entity = tree_state.get_selected_entity();

        // 【重构】调用提取的方法
        let (ids_str, paths_str) = self.get_target_strings(&tree_name);

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

    pub fn emit(&mut self, signal: &'static str, term_width: u16, term_height: u16) -> bool {
        if signal == "select" {
            let now = Instant::now();
            if let Some(last) = self.signals.last_emit.get(signal) {
                if now.duration_since(*last).as_millis() < 200 { return false; }
            }
            self.signals.last_emit.insert(signal, now);
        }

        let active_keymaps_owned = self.get_active_keymaps();
        let active_keymaps: Vec<Option<&str>> = active_keymaps_owned.iter().map(|s| s.as_deref()).collect();

        // 【修复】使用 get_signal_binding_keymap 替代原来的 get_signal_binding
        if let Some((cmd_template_args, _is_silent)) = self.key_bindings.get_signal_binding_keymap(&active_keymaps, signal) {
            let tree_name = self.get_active_tree_name().unwrap_or_default();

            let (selected_entity, ids_str, paths_str) = if !tree_name.is_empty() {
                if let Some(Component::Tree(t)) = self.components.get(&tree_name) {
                    let sel = t.get_selected_entity().cloned();
                    let (ids, paths) = self.get_target_strings(&tree_name);
                    (sel, ids, paths)
                } else {
                    (None, String::new(), String::new())
                }
            } else {
                (None, String::new(), String::new())
            };

            let current_window = match &self.focus.current { Focus::Component(n) => Some(n.as_str()), Focus::None => None };
            let window_name = current_window.unwrap_or("").to_string();
            let ctx = Self::build_exec_context(
                selected_entity.as_ref(), &ids_str, &paths_str, &window_name,
                &term_width.to_string(), &term_height.to_string(), signal, None
            );

            let full_cmd_args = exec::replace_placeholders_in_args(cmd_template_args, &ctx);

            if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
                return true;
            }

            if let Some(internal_cmd) = InternalCommand::from_args(&full_cmd_args) {
                match internal_cmd {
                    InternalCommand::ToggleLayout(name) => self.toggle_layout_visible(&name),
                    InternalCommand::ShowLayout(name) => self.set_layout_visible(&name, true),
                    InternalCommand::HideLayout(name) => self.set_layout_visible(&name, false),
                    InternalCommand::ActivateInput(name) => self.activate_input(&name, ""),
                    _ => {}
                }
                return true;
            }

            let tx = self.async_exec_tx.clone();
            std::thread::spawn(move || {
                let _ = exec::execute_command_silent(&full_cmd_args);
                let _ = tx.send(());
            });
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
    // 【新增】通用的 Keymap 解析方法，供 active 和 blur 复用
    pub fn get_keymaps_for(&self, comp_name: &str) -> Vec<Option<String>> {
        let mut keymaps: Vec<Option<String>> = Vec::new();

        let keymap_str = self.components.get(comp_name).and_then(|c| match c {
            Component::Tree(t) => Some(t.keymap.as_str()),
            Component::View(v) => Some(v.keymap.as_str()),
            Component::Input(i) => Some(i.keymap.as_str()),
            _ => None,
        }).unwrap_or("default");

        if keymap_str == "default" {
            // 向后兼容：如果默认，先查组件名作用域，再查全局 None
            keymaps.push(Some(comp_name.to_string()));
            keymaps.push(None);
        } else {
            // 新的 keymap 逻辑：按 + 号拆分，从右向左压栈
            let parts: Vec<&str> = keymap_str.split('+').collect();
            for part in parts.iter().rev() {
                if part.eq_ignore_ascii_case("default") || part.is_empty() {
                    keymaps.push(None); // None 代表全局默认包
                } else {
                    keymaps.push(Some(part.to_string()));
                }
            }
        }
        keymaps
    }

    pub fn get_active_keymaps(&self) -> Vec<Option<String>> {
        let active_comp_name = if let Some(layer) = self.overlay_stack.last() {
            Some(layer.source.clone())
        } else {
            if let Focus::Component(n) = &self.focus.current { Some(n.clone()) } else { None }
        };

        if let Some(comp_name) = active_comp_name {
            self.get_keymaps_for(&comp_name)
        } else {
            vec![None]
        }
    }
}
