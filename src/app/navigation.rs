use crate::app::{Engine, Component, Focus};
use crate::layout::{WindowRect, BorderStyle, LayoutNode};
use std::collections::HashSet;

fn collect_visible_leaf_names(node: &LayoutNode, names: &mut HashSet<String>) {
    match node {
        LayoutNode::Window { name, .. } => {
            names.insert(name.clone());
        }
        LayoutNode::Container { children, .. } => {
            for child in children {
                collect_visible_leaf_names(child, names);
            }
        }
    }
}

impl Engine {
    // 统一的焦点设置入口，自动维护历史栈
    pub fn set_focus(&mut self, name: &str) {
        // 【防线1】绝不允许聚焦到 StatusBar
        if let Some(c) = self.components.get(name) {
            if matches!(c, Component::StatusBar(_)) {
                return;
            }
        }

        let old_focus = self.focus.current.clone();
        if let Focus::Component(old) = &old_focus {
            if old != name {
                self.focus_history.retain(|n| n != old);
                self.focus_history.push(old.clone());
                if self.focus_history.len() > 10 {
                    self.focus_history.remove(0);
                }
            }
        }
        self.focus.current = Focus::Component(name.to_string());
        self.mark_dirty(name);
        for (n, c) in &self.components {
            if matches!(c, Component::StatusBar(_)) {
                self.dirty_components.insert(n.clone());
            }
        }
    }

    pub fn handle_tab(&mut self, term_width: u16, term_height: u16) {
        self.last_error = None;

        let mut visible_names = HashSet::new();
        for layer in &self.layout_layers {
            if layer.visible {
                collect_visible_leaf_names(&layer.root, &mut visible_names);
            }
        }

        let mut names: Vec<String> = self.components.iter()
            .filter(|(k, c)| {
                // 【防线2】Tab 循环时过滤掉 StatusBar
                !matches!(c, Component::StatusBar(_)) && visible_names.contains(k.as_str())
            })
            .map(|(k, _)| k.clone())
            .collect();

        names.sort();

        if names.is_empty() { self.focus.current = Focus::None; return; }
        let current_idx = match &self.focus.current {
            Focus::Component(name) => names.iter().position(|n| n == name).unwrap_or(0),
            Focus::None => 0,
        };
        let next_idx = (current_idx + 1) % names.len();
        let next_name = names[next_idx].clone();
        self.set_focus(&next_name);

        let should_emit_focus = if let Some(Component::Tree(t)) = self.components.get(&next_name) {
            t.focus_to_fire
        } else {
            false
        };

        if should_emit_focus {
            self.emit("focus", term_width, term_height);
        }
    }

    // Z 轴图层切换
    pub fn cycle_layer(&mut self, all_rects: &[(WindowRect, String, BorderStyle, usize)]) {
        let visible_layers: Vec<usize> = self.layout_layers.iter()
            .filter(|l| l.visible)
            .map(|l| l.z_index)
            .collect();

        if visible_layers.is_empty() { return; }

        let current_name = match &self.focus.current {
            Focus::Component(n) => n.clone(),
            _ => String::new(),
        };

        let current_z = all_rects.iter().find(|(_, n, _, _)| n == &current_name)
            .map(|(_, _, _, z)| *z)
            .unwrap_or(visible_layers[0]);

        let current_idx = visible_layers.iter().position(|&z| z == current_z).unwrap_or(0);
        let next_idx = (current_idx + 1) % visible_layers.len();
        let target_z = visible_layers[next_idx];

        let mut next_name = all_rects.iter()
            .find(|(_, _, _, z)| *z == target_z)
            .map(|(_, n, _, _)| n.clone());

        if target_z != current_z {
            for hist_name in self.focus_history.iter().rev() {
                if let Some((_, n, _, z)) = all_rects.iter().find(|(_, n, _, _)| n == hist_name) {
                    if *z == target_z {
                        next_name = Some(n.clone());
                        break;
                    }
                }
            }
        }

        if let Some(name) = next_name {
            self.set_focus(&name); // set_focus 内部会拦截 StatusBar
        }
    }

    // 带空间记忆的方向切换
    pub fn focus_direction(&mut self, dir: &str, all_rects: &[(WindowRect, String, BorderStyle, usize)]) {
        let current_name = match &self.focus.current {
            Focus::Component(n) => n.clone(),
            _ => {
                self.recover_focus(all_rects);
                return;
            }
        };

        let current_rect_opt = all_rects.iter().find(|(_, n, _, _)| n == &current_name)
            .map(|(r, _, _, _)| *r);

        let current_rect = match current_rect_opt {
            Some(r) => r,
            None => {
                self.recover_focus(all_rects);
                return;
            }
        };

        let tolerance: i32 = 2;
        let mut candidates: Vec<(String, u32, usize)> = Vec::new();

        for (i, (rect, name, _, z)) in all_rects.iter().enumerate() {
            if name == &current_name { continue; }

            // 【防线3】方向切换时，绝对过滤掉 StatusBar！防止误触 Ctrl-J 跳进去
            if let Some(comp) = self.components.get(name) {
                if matches!(comp, Component::StatusBar(_)) { continue; }
            }

            let layer = self.layout_layers.get(*z);
            if let Some(layer) = layer {
                if !layer.visible { continue; }
            } else { continue; }

            let cur_x2 = current_rect.start_col + current_rect.width;
            let cur_y2 = current_rect.start_row + current_rect.height;
            let tgt_x2 = rect.start_col + rect.width;
            let tgt_y2 = rect.start_row + rect.height;

            let is_valid = match dir {
                "left" => (tgt_x2 as i32 - current_rect.start_col as i32) <= tolerance,
                "right" => (rect.start_col as i32 - cur_x2 as i32) >= -tolerance,
                "up" => (tgt_y2 as i32 - current_rect.start_row as i32) <= tolerance,
                "down" => (rect.start_row as i32 - cur_y2 as i32) >= -tolerance,
                _ => false,
            };

            if is_valid {
                let cur_cx = current_rect.start_col + current_rect.width / 2;
                let cur_cy = current_rect.start_row + current_rect.height / 2;
                let tgt_cx = rect.start_col + rect.width / 2;
                let tgt_cy = rect.start_row + rect.height / 2;

                let dx = cur_cx.abs_diff(tgt_cx);
                let dy = cur_cy.abs_diff(tgt_cy);
                let dist_sq = (dx as u32).pow(2) + (dy as u32).pow(2);

                candidates.push((name.clone(), dist_sq, i));
            }
        }

        if candidates.is_empty() { return; }

        candidates.sort_by(|a, b| {
            if a.1 != b.1 {
                return a.1.cmp(&b.1);
            }
            a.2.cmp(&b.2)
        });

        let next_name = candidates[0].0.clone();
        self.set_focus(&next_name);
    }

    fn recover_focus(&mut self, all_rects: &[(WindowRect, String, BorderStyle, usize)]) {
        let mut next_name = None;
        for hist_name in self.focus_history.iter().rev() {
            // 恢复焦点时也要排除 StatusBar
            if all_rects.iter().any(|(_, n, _, _)| n == hist_name) {
                if let Some(comp) = self.components.get(hist_name) {
                    if !matches!(comp, Component::StatusBar(_)) {
                        next_name = Some(hist_name.clone());
                        break;
                    }
                }
            }
        }
        if next_name.is_none() {
            // 找第一个不是 StatusBar 的组件
            for (_, n, _, _) in all_rects {
                if let Some(comp) = self.components.get(n) {
                    if !matches!(comp, Component::StatusBar(_)) {
                        next_name = Some(n.clone());
                        break;
                    }
                }
            }
        }
        if let Some(name) = next_name {
            self.set_focus(&name);
        }
    }

    pub fn toggle_expand(&mut self) {
        self.last_error = None;
        if let Focus::Component(focused_name) = self.focus.current.clone() {
            if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                t.toggle_expand();
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn toggle_mark(&mut self) {
        self.last_error = None;
        if let Focus::Component(focused_name) = self.focus.current.clone() {
            if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                t.toggle_mark();
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn move_up(&mut self) {
        self.last_error = None;
        if let Focus::Component(focused_name) = self.focus.current.clone() {
            if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                t.move_up();
                self.mark_dirty(&focused_name);
                // move 转移所有权，消灭第二次 clone
                self.pending_selection_changed = Some(focused_name);
            } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
                v.scroll_offset = v.scroll_offset.saturating_sub(1);
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn move_down(&mut self) {
        self.last_error = None;
        if let Focus::Component(focused_name) = self.focus.current.clone() {
            if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                t.move_down();
                self.mark_dirty(&focused_name);
                self.pending_selection_changed = Some(focused_name);
            } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
                v.scroll_offset = (v.scroll_offset + 1).min(v.max_offset);
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn jump_to_top(&mut self) {
        self.last_error = None;
        if let Focus::Component(focused_name) = self.focus.current.clone() {
            if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                t.jump_to_top();
                self.mark_dirty(&focused_name);
                self.pending_selection_changed = Some(focused_name);
            } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
                v.scroll_offset = 0;
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn jump_to_bottom(&mut self) {
        self.last_error = None;
        if let Focus::Component(focused_name) = self.focus.current.clone() {
            if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                t.jump_to_bottom();
                self.mark_dirty(&focused_name);
                self.pending_selection_changed = Some(focused_name);
            } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
                v.scroll_offset = v.max_offset;
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn select_id(&mut self, tree_name: &str, id: &str) {
        self.last_error = None;
        if let Some(Component::Tree(t)) = self.components.get_mut(tree_name) {
            t.select_id(id);
        }
        self.pending_selection_changed = Some(tree_name.to_string());
        self.mark_dirty(tree_name);
    }

    pub fn move_up_n(&mut self, n: usize) {
        self.last_error = None;
        if let Focus::Component(focused_name) = self.focus.current.clone() {
            if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                for _ in 0..n {
                    if t.selected_idx > 0 {
                        t.selected_idx -= 1;
                        t.selected_id = Some(t.visible_ids[t.selected_idx].clone());
                    } else { break; }
                }
                self.mark_dirty(&focused_name);
                self.pending_selection_changed = Some(focused_name);
            } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
                v.scroll_offset = v.scroll_offset.saturating_sub(n);
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn move_down_n(&mut self, n: usize) {
        self.last_error = None;
        if let Focus::Component(focused_name) = self.focus.current.clone() {
            if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                for _ in 0..n {
                    if t.selected_idx < t.visible_ids.len().saturating_sub(1) {
                        t.selected_idx += 1;
                        t.selected_id = Some(t.visible_ids[t.selected_idx].clone());
                    } else { break; }
                }
                self.mark_dirty(&focused_name);
                self.pending_selection_changed = Some(focused_name);
            } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
                v.scroll_offset = (v.scroll_offset + n).min(v.max_offset);
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn has_active_input(&self) -> bool {
        self.components.values().any(|c| matches!(c, Component::Input(i) if i.is_active))
    }

    pub fn handle_input_key(&mut self, key: crossterm::event::KeyEvent) -> Option<(String, String)> {
        use crossterm::event::{KeyCode, KeyModifiers};

        let input_name = self.components.iter()
            .find(|(_, c)| matches!(c, Component::Input(i) if i.is_active))
            .map(|(n, _)| n.clone())?;

        if let Some(Component::Input(input)) = self.components.get_mut(&input_name) {
            match key.code {
                KeyCode::Esc => {
                    input.deactivate();
                    self.mark_all_dirty();
                    return Some((input_name, "__CANCEL__".to_string()));
                }
                KeyCode::Enter => {
                    let submitted = input.buffer.clone();
                    input.deactivate();
                    self.mark_all_dirty();
                    return Some((input_name, submitted));
                }
                KeyCode::Backspace => input.backspace(),
                KeyCode::Left => input.move_left(),
                KeyCode::Right => input.move_right(),
                KeyCode::Home => input.move_home(),
                KeyCode::End => input.move_end(),
                KeyCode::Char(c) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'u' { input.clear(); }
                    else if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'a' { input.move_home(); }
                    else if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'e' { input.move_end(); }
                    else { input.insert_char(c); }
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
            self.mark_all_dirty(); // 触发重绘以显示 Input
        }
    }

    pub fn apply_search(&mut self, query: &str, term_width: u16, term_height: u16) {
        if let Focus::Component(focused_name) = self.focus.current.clone() {
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

                // NLL 会在 t 不再使用后自动释放 &mut self.components 的借用
                self.broadcast_selection_changed(&focused_name, term_width, term_height);
                self.emit_select_if_changed(term_width, term_height);
                self.mark_dirty(&focused_name);
            }
        }
    }

    pub fn cancel_input(&mut self) {
        if let Some((name, _)) = self.components.iter().find(|(_, c)| matches!(c, Component::Input(i) if i.is_active)) {
            let name = name.clone();
            if let Some(Component::Input(input)) = self.components.get_mut(&name) {
                input.deactivate();
            }
            self.mark_all_dirty(); // 触发重绘以隐藏 Input
        }
    }
}
