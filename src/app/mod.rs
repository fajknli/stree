// src/app/mod.rs

pub mod view;
pub mod statusbar;
pub mod input;
pub mod tree;
pub mod overlay;

pub use overlay::OverlayState;
pub use view::ViewState;
pub use statusbar::StatusBarState;
pub use input::InputState;
pub use tree::TreeState;

use crate::config::BindConfig;
use crate::exec;
use crate::layout::Layout;
use crate::protocol::Dataset;
use crate::search;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    None,
    Component(String),
}

#[derive(Debug)]
pub enum Component {
    Tree(TreeState),
    View(ViewState),
    StatusBar(StatusBarState),
    Input(InputState),
    Overlay(OverlayState),
}

#[derive(Debug)]
pub struct Engine {
    pub drag_start_idx: Option<usize>,
    pub drag_active: bool,
    pub components: HashMap<String, Component>,
    pub layout: Layout,
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
        layout: Layout,
        key_bindings: BindConfig,
        mouse_enabled: bool,
        border_chars: Vec<String>,
        trees: Vec<String>,
        views: Vec<String>,
        statusbars: Vec<String>,
        inputs: Vec<String>,
        relations_path: Option<String>,
        overlays: Vec<String>,
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

        for o_cfg in overlays {
            let parts: Vec<&str> = o_cfg.splitn(5, ':').collect();
            // 格式: Name:Position:Width:Height:Text
            // Position: center | top-left | bottom-right | x,y
            let name = parts[0].to_string();
            let _position = parts.get(1).map(|s| s.to_string()).unwrap_or_else(|| "center".to_string());
            let width: u16 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(40);
            let height: u16 = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
            let text = parts.get(4).map(|s| s.to_string()).unwrap_or_default();

            // 位置先占位，渲染时计算
            components.insert(name, Component::Overlay(OverlayState {
                visible: false,
                text,
                x: 0,
                y: 0,
                width,
                height,
            }));
        }

        let focused = first_tree_name.clone()
            .map(|n| Focus::Component(n))
            .unwrap_or(Focus::None);

        let mut engine = Self {
            components,
            layout,
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
        };

        if let Focus::Component(name) = &focused {
            engine.broadcast_selection_changed(name);
        }

        engine.init_views();

        engine
    }

    pub fn handle_tab(&mut self) {
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

    pub fn select_id(&mut self, tree_name: &str, id: &str) {
        self.last_error = None;
        if let Some(Component::Tree(t)) = self.components.get_mut(tree_name) {
            t.select_id(id);
        }
        self.broadcast_selection_changed(tree_name);
    }

    pub fn broadcast_selection_changed(&mut self, tree_name: &str) {
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

                let _ = view_name;
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
        );

        if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
            None
        } else {
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
                        let target_owned = target.to_string();
                        self.broadcast_selection_changed(&target_owned);
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

    pub fn activate_input(&mut self, name: &str, prefix: &str) {
        if let Some(Component::Input(input)) = self.components.get_mut(name) {
            input.prefix = prefix.to_string();
            input.activate();
        }
    }

    pub fn apply_search(&mut self, query: &str) {
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
}
