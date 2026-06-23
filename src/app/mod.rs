// src/app/mod.rs

use crate::config::BindConfig;
use crate::exec;
use crate::layout::Layout;
use crate::protocol::Dataset;
use crate::search;
use crate::tree::TreeNode;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    None,
    Component(String),
}

#[derive(Debug)]
pub struct TreeState {
    pub dataset: Dataset,
    pub root_tree: Vec<TreeNode>,
    pub selected_id: Option<String>,
    pub expanded_ids: HashSet<String>,
    pub marked_ids: HashSet<String>,
    pub visible_ids: Vec<String>,
    pub visible_depths: Vec<usize>,
    pub selected_idx: usize,
    pub search_query: String,
    pub in_search_mode: bool,
    pub matched_ids: HashSet<String>,
    pub ancestors_of_matched: HashSet<String>,
    pub source_cmd: Option<String>,
    pub relations_path: Option<String>,
}

#[derive(Debug)]
pub struct ViewState {
    pub cmd_template: String,
    pub scroll_offset: usize,
    pub content_buffer: String,
    pub cached_entity_id: Option<String>,
    pub max_offset: usize,
    /// 当前格子宽度（由 update_view_rects 写入），用于 {width} 占位符
    pub rect_width: u16,
    /// 当前格子高度（内容区高度），用于 {height} 占位符
    pub rect_height: u16,
}

#[derive(Debug)]
pub struct StatusBarState {
    pub format_template: String,
}

#[derive(Debug)]
pub struct InputState {
    pub buffer: String,
    pub cursor: usize,          // 字符索引（不是字节）
    pub is_active: bool,
    pub prefix: String,         // "/" 或 ":" 等
    pub on_submit: Option<String>, // 提交时执行的命令模板
}

impl InputState {
    pub fn new(prefix: &str) -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            is_active: false,
            prefix: prefix.to_string(),
            on_submit: None,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let char_pos = self.cursor;
        let byte_pos: usize = self.buffer.chars().take(char_pos).map(|ch| ch.len_utf8()).sum();
        self.buffer.insert(byte_pos, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_pos: usize = self.buffer.chars().take(self.cursor).map(|ch| ch.len_utf8()).sum();
            let next_byte_pos = byte_pos + self.buffer[byte_pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            self.buffer.replace_range(byte_pos..next_byte_pos, "");
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 { self.cursor -= 1; }
    }

    pub fn move_right(&mut self) {
        let char_count = self.buffer.chars().count();
        if self.cursor < char_count { self.cursor += 1; }
    }

    pub fn move_home(&mut self) { self.cursor = 0; }

    pub fn move_end(&mut self) {
        self.cursor = self.buffer.chars().count();
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    pub fn activate(&mut self) {
        // 不再覆盖 prefix，保留创建时设定的 prefix
        self.clear();
        self.is_active = true;
    }

    pub fn deactivate(&mut self) {
        self.is_active = false;
        self.clear();
    }
}

#[derive(Debug)]
pub enum Component {
    Tree(TreeState),
    View(ViewState),
    StatusBar(StatusBarState),
    Input(InputState),
}

#[derive(Debug)]
pub struct Engine {
    pub components: HashMap<String, Component>,
    pub layout: Layout,
    pub key_bindings: BindConfig,
    pub focused: Focus,
    pub last_error: Option<String>,
    pub last_click_time: Option<Instant>,
    pub last_clicked_id: Option<String>,
    pub global_relations: Vec<crate::protocol::Relation>,
    pub main_tree_name: Option<String>,
}

impl Engine {
    pub fn new(
        initial_dataset: Dataset,
        layout: Layout,
        key_bindings: BindConfig,
        trees: Vec<String>,
        views: Vec<String>,
        statusbars: Vec<String>,
        inputs: Vec<String>,
        relations_path: Option<String>,
    ) -> Self {
        let mut components = HashMap::new();
        let global_relations = if let Some(ref p) = relations_path {
            crate::protocol::parse_relations(Some(p)).unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut init_error = None;
        let mut first_tree_name = None;

        for t_cfg in trees {
            let parts: Vec<&str> = t_cfg.splitn(2, ':').collect();
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
                search_query: String::new(),
                in_search_mode: false,
                matched_ids: HashSet::new(),
                ancestors_of_matched: HashSet::new(),
                source_cmd,
                relations_path: relations_path.clone(),
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

            let mut input_state = crate::app::InputState::new(&prefix);
            input_state.on_submit = on_submit;
            components.insert(name, Component::Input(input_state));
        }

        let focused = first_tree_name.clone()
            .map(|n| Focus::Component(n))
            .unwrap_or(Focus::None);

        let mut engine = Self {
            components,
            layout,
            key_bindings,
            focused: focused.clone(),
            last_error: init_error,
            last_click_time: None,
            last_clicked_id: None,
            global_relations,
            main_tree_name: first_tree_name,
        };

        if let Focus::Component(name) = &focused {
            engine.broadcast_selection_changed(name);
        }

        // 【新增】启动时主动初始化所有 View，不再依赖 Tree 驱动
        engine.init_views();

        engine
    }

    pub fn handle_tab(&mut self) {
        self.last_error = None;
        // 过滤掉 StatusBar 组件，不参与焦点循环
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
        self.focused = Focus::Component(names[next_idx].clone());
    }

    pub fn move_up(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.move_up();
            self.broadcast_selection_changed(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.scroll_offset.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.move_down();
            self.broadcast_selection_changed(&focused_name);
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

    pub fn jump_to_top(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.jump_to_top();
            self.broadcast_selection_changed(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = 0;
        }
    }

    pub fn jump_to_bottom(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.jump_to_bottom();
            self.broadcast_selection_changed(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.max_offset;
        }
    }

    pub fn jump_to_next_match(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.jump_to_next_match();
            self.broadcast_selection_changed(&focused_name);
        }
    }

    pub fn jump_to_prev_match(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.jump_to_prev_match();
            self.broadcast_selection_changed(&focused_name);
        }
    }

    pub fn update_search_query(&mut self, query: String) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.update_search_query(query);
            self.broadcast_selection_changed(&focused_name);
        }
    }

    pub fn select_id(&mut self, tree_name: &str, id: &str) {
        self.last_error = None;
        if let Some(Component::Tree(t)) = self.components.get_mut(tree_name) {
            t.select_id(id);
        }
        self.broadcast_selection_changed(tree_name);
    }

    /// 只有 tree_name 是当前焦点树时才驱动 View 刷新。
    /// 这样多棵树各自独立，焦点在哪棵树，哪棵树的选中才驱动 View。
    pub fn broadcast_selection_changed(&mut self, tree_name: &str) {
        // 只有焦点树才驱动 View 刷新
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

        // 焦点窗口名，注入给 {window}
        let window_name = tree_name.to_string();

        for (view_name, comp) in self.components.iter_mut() {
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

                let _ = view_name; // 消除未使用警告
            }
        }
    }

    /// 启动时主动初始化所有 View，不依赖 Tree 选中
    /// 这样 View 可以独立渲染，不需要伪造 Tree 来驱动
    pub fn init_views(&mut self) {
        for (view_name, comp) in self.components.iter_mut() {
            if let Component::View(v) = comp {
                let width_str = v.rect_width.to_string();
                let height_str = v.rect_height.to_string();
                let window_name = view_name.clone();

                let template_args_vec = crate::config::split_args(&v.cmd_template);
                let full_cmd_args = exec::replace_placeholders_in_args(
                    &template_args_vec,
                    None, // 没有选中实体，占位符会替换为空
                    "", "", &window_name, &width_str, &height_str,
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

    /// 当前焦点树，用于 statusbar 读取统计信息
    pub fn get_focused_tree_state(&self) -> Option<&TreeState> {
        let name = match &self.focused {
            Focus::Component(n) => n,
            Focus::None => return self.get_main_tree_state(),
        };
        if let Some(Component::Tree(t)) = self.components.get(name) {
            return Some(t);
        }
        // 焦点不在树上时回退到主树
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

    // 【修改】返回值变为 Option<(Vec<String>, bool)>
    pub fn prepare_key_binding_args(&self, key: &crossterm::event::KeyEvent, term_width: u16, term_height: u16) -> Option<(Vec<String>, bool)> {
        let (cmd_template_args, is_silent) = self.key_bindings.get(key)?;

        // 取焦点树（焦点不在树上时取主树）
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
        );

        if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
            None
        } else {
            // 【修改】返回命令和静默标志
            Some((full_cmd_args, *is_silent))
        }
    }

    pub fn handle_ipc_update(&mut self, target: &str, data: &str) {
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
                        // IPC 更新树后，若它是焦点树则广播
                        let target_owned = target.to_string();
                        self.broadcast_selection_changed(&target_owned);
                    }
                }
                Component::View(v) => {
                    v.content_buffer = data.to_string();
                    v.scroll_offset = 0;
                    // IPC 推进 View 时清除缓存 id，防止下次同实体选中时被跳过
                    v.cached_entity_id = None;
                }
                Component::StatusBar(s) => {
                    s.format_template = data.to_string();
                }
                _ => {}
            }
        }
    }

    pub fn trigger_reload(&mut self) {
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
                            self.handle_ipc_update(&name, &stdout);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    }

    /// 更新各 View 的滚动上限和格子尺寸
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

    pub fn move_up_n(&mut self, n: usize) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            for _ in 0..n {
                if t.selected_idx > 0 {
                    t.selected_idx -= 1;
                    t.selected_id = Some(t.visible_ids[t.selected_idx].clone());
                } else { break; }
            }
            self.broadcast_selection_changed(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.scroll_offset.saturating_sub(n);
        }
    }

    pub fn move_down_n(&mut self, n: usize) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focused { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            for _ in 0..n {
                if t.selected_idx < t.visible_ids.len().saturating_sub(1) {
                    t.selected_idx += 1;
                    t.selected_id = Some(t.visible_ids[t.selected_idx].clone());
                } else { break; }
            }
            self.broadcast_selection_changed(&focused_name);
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

    pub fn activate_input(&mut self, name: &str) {
        if let Some(Component::Input(input)) = self.components.get_mut(name) {
            input.activate();
        }
    }
}

impl TreeState {
    pub fn rebuild_visible_ids(&mut self) {
        self.visible_ids.clear();
        self.visible_depths.clear();
        let is_filtering = !self.search_query.trim().is_empty();

        for root in &self.root_tree {
            Self::collect_visible_recursive(
                root,
                is_filtering,
                &self.matched_ids,
                &self.ancestors_of_matched,
                &self.expanded_ids,
                &mut self.visible_ids,
                &mut self.visible_depths,
            );
        }
    }

    fn collect_visible_recursive(
        node: &TreeNode,
        is_filtering: bool,
        matched_ids: &HashSet<String>,
        ancestors_of_matched: &HashSet<String>,
        expanded_ids: &HashSet<String>,
        visible_ids: &mut Vec<String>,
        visible_depths: &mut Vec<usize>,
    ) {
        let is_match = matched_ids.contains(&node.entity.id);
        let is_ancestor = ancestors_of_matched.contains(&node.entity.id);
        if is_filtering && !is_match && !is_ancestor { return; }

        visible_ids.push(node.entity.id.clone());
        visible_depths.push(node.depth);

        if expanded_ids.contains(&node.entity.id) {
            for child in &node.children {
                Self::collect_visible_recursive(
                    child, is_filtering, matched_ids, ancestors_of_matched,
                    expanded_ids, visible_ids, visible_depths,
                );
            }
        }
    }

    pub fn update_search_query(&mut self, query: String) {
        self.search_query = query;
        self.matched_ids = search::match_entities(&self.dataset.entities, &self.search_query);
        self.ancestors_of_matched.clear();
        if !self.matched_ids.is_empty() {
            let mut temp_ancestors = HashSet::new();
            for root in &self.root_tree {
                Self::collect_ancestors_inner(root, &self.matched_ids, &mut temp_ancestors, &mut Vec::new());
            }
            self.ancestors_of_matched = temp_ancestors;
        }
        for ancestor_id in &self.ancestors_of_matched {
            self.expanded_ids.insert(ancestor_id.clone());
        }
        self.rebuild_visible_ids();
        if !self.matched_ids.is_empty() {
            if let Some(first_match) = self.visible_ids.iter().find(|id| self.matched_ids.contains(*id)).cloned() {
                self.select_id(&first_match);
            }
        } else {
            if let Some(first_id) = self.visible_ids.first().cloned() {
                self.select_id(&first_id);
            }
        }
    }

    fn collect_ancestors_inner(node: &TreeNode, matched_ids: &HashSet<String>, ancestors: &mut HashSet<String>, path: &mut Vec<String>) {
        let is_match = matched_ids.contains(&node.entity.id);
        let has_matched_descendant = Self::has_matched_descendant_inner(node, matched_ids);
        if has_matched_descendant {
            for id in path.iter() { ancestors.insert(id.clone()); }
            if !is_match { ancestors.insert(node.entity.id.clone()); }
        }
        path.push(node.entity.id.clone());
        for child in &node.children {
            Self::collect_ancestors_inner(child, matched_ids, ancestors, path);
        }
        path.pop();
    }

    fn has_matched_descendant_inner(node: &TreeNode, matched_ids: &HashSet<String>) -> bool {
        if matched_ids.contains(&node.entity.id) { return true; }
        for child in &node.children {
            if Self::has_matched_descendant_inner(child, matched_ids) { return true; }
        }
        false
    }

    pub fn get_selected_entity(&self) -> Option<&crate::protocol::Entity> {
        let id = self.selected_id.as_ref()?;
        self.dataset.entity_map.get(id)
    }

    pub fn get_marked_entities(&self) -> Vec<&crate::protocol::Entity> {
        self.dataset.entities.iter()
            .filter(|e| self.marked_ids.contains(&e.id) && !e.id.is_empty())
            .collect()
    }

    pub fn move_up(&mut self) {
        if self.visible_ids.is_empty() { return; }
        if self.selected_idx > 0 {
            self.selected_idx -= 1;
            self.selected_id = Some(self.visible_ids[self.selected_idx].clone());
        }
    }

    pub fn move_down(&mut self) {
        if self.visible_ids.is_empty() { return; }
        if self.selected_idx < self.visible_ids.len().saturating_sub(1) {
            self.selected_idx += 1;
            self.selected_id = Some(self.visible_ids[self.selected_idx].clone());
        }
    }

    pub fn toggle_expand(&mut self) {
        let target_id = self.selected_id.clone();
        if let Some(id) = target_id {
            let has_children = crate::tree::find_node_in_roots(&self.root_tree, &id)
                .map(|n| !n.children.is_empty())
                .unwrap_or(false);
            if has_children {
                if self.expanded_ids.contains(&id) {
                    self.expanded_ids.remove(&id);
                } else {
                    self.expanded_ids.insert(id.clone());
                }
                self.rebuild_visible_ids();
                self.select_id(&id);
            }
        }
    }

    pub fn toggle_mark(&mut self) {
        if let Some(id) = self.selected_id.clone() {
            if self.marked_ids.contains(&id) {
                self.marked_ids.remove(&id);
            } else {
                self.marked_ids.insert(id);
            }
            if self.selected_idx < self.visible_ids.len().saturating_sub(1) {
                self.selected_idx += 1;
                self.selected_id = Some(self.visible_ids[self.selected_idx].clone());
            }
        }
    }

    pub fn jump_to_top(&mut self) {
        if self.visible_ids.is_empty() { return; }
        self.selected_idx = 0;
        self.selected_id = Some(self.visible_ids[0].clone());
    }

    pub fn jump_to_bottom(&mut self) {
        if self.visible_ids.is_empty() { return; }
        self.selected_idx = self.visible_ids.len().saturating_sub(1);
        self.selected_id = Some(self.visible_ids.last().unwrap().clone());
    }

    pub fn jump_to_next_match(&mut self) {
        if self.visible_ids.is_empty() || self.matched_ids.is_empty() { return; }
        let start = self.selected_idx + 1;
        let len = self.visible_ids.len();
        for i in 0..len {
            let idx = (start + i) % len;
            if self.matched_ids.contains(&self.visible_ids[idx]) {
                self.selected_idx = idx;
                self.selected_id = Some(self.visible_ids[idx].clone());
                return;
            }
        }
    }

    pub fn jump_to_prev_match(&mut self) {
        if self.visible_ids.is_empty() || self.matched_ids.is_empty() { return; }
        let start = if self.selected_idx == 0 { self.visible_ids.len().saturating_sub(1) } else { self.selected_idx - 1 };
        let len = self.visible_ids.len();
        for i in 0..len {
            let idx = (start + len - i) % len;
            if self.matched_ids.contains(&self.visible_ids[idx]) {
                self.selected_idx = idx;
                self.selected_id = Some(self.visible_ids[idx].clone());
                return;
            }
        }
    }

    pub fn select_id(&mut self, id: &str) {
        if self.dataset.entity_map.contains_key(id) {
            if let Some(idx) = self.visible_ids.iter().position(|v| v == id) {
                self.selected_idx = idx;
                self.selected_id = Some(id.to_string());
            } else {
                // 【核心修复】如果节点存在但不可见，强制重置到第一个可见项，防止 selected_idx 越界
                self.selected_idx = 0;
                self.selected_id = self.visible_ids.first().cloned();
            }
        } else {
            self.selected_idx = 0;
            self.selected_id = self.visible_ids.first().cloned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Dataset, Entity};

    fn create_test_tree() -> TreeState {
        let mut dataset = Dataset::new();
        dataset.entities.push(Entity {
            id: "U-01".into(),
            display: "Root".into(),
            path: "/root.md".into(),
            tags: "live".into(),
        });
        dataset.entity_map.insert("U-01".into(), dataset.entities[0].clone());

        let root_tree = vec![TreeNode::new(dataset.entities[0].clone(), 0)];

        let mut state = TreeState {
            dataset,
            root_tree,
            selected_id: None,
            expanded_ids: HashSet::new(),
            marked_ids: HashSet::new(),
            visible_ids: vec!["U-01".into()],
            visible_depths: vec![0],
            selected_idx: 0,
            search_query: String::new(),
            in_search_mode: false,
            matched_ids: HashSet::new(),
            ancestors_of_matched: HashSet::new(),
            source_cmd: None,
            relations_path: None,
        };
        state.select_id("U-01");
        state
    }

    #[test]
    fn test_select_id_fallback_when_invisible() {
        let mut state = create_test_tree();

        // 清空 visible_ids，模拟节点被过滤
        state.visible_ids.clear();
        state.visible_ids.push("U-02".into()); // 不存在的 ID

        // 选择存在的 ID，但不可见
        state.select_id("U-01");

        // 应该 fallback 到第一个可见项
        assert_eq!(state.selected_idx, 0);
        assert_eq!(state.selected_id, Some("U-02".into()));
    }

    #[test]
    fn test_select_id_nonexistent() {
        let mut state = create_test_tree();

        // 选择不存在的 ID
        state.select_id("NONEXISTENT");

        // 应该 fallback 到第一个可见项
        assert_eq!(state.selected_idx, 0);
        assert_eq!(state.selected_id, Some("U-01".into()));
    }
}
