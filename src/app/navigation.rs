// src/app/navigation.rs

use crate::app::{Engine, Component, Focus};

impl Engine {
    pub fn handle_tab(&mut self, term_width: u16, term_height: u16) {
        self.last_error = None;
        let old_focus = self.focus.current.clone();
        let mut names: Vec<String> = self.components.iter()
            .filter(|(_, c)| !matches!(c, Component::StatusBar(_)))
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
        self.focus.current = Focus::Component(next_name.clone());

        if let Focus::Component(old) = &old_focus {
            self.mark_dirty(old);
        }
        self.mark_dirty(&next_name);

        for (n, c) in &self.components {
            if matches!(c, Component::StatusBar(_)) {
                self.dirty_components.insert(n.clone());
            }
        }

        let should_emit_focus = if let Some(Component::Tree(t)) = self.components.get(&next_name) {
            t.focus_to_fire
        } else {
            false
        };

        if should_emit_focus {
            self.emit("focus", term_width, term_height);
        }
    }

    pub fn toggle_expand(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.toggle_expand();
            self.mark_dirty(&focused_name);
        }
    }

    pub fn toggle_mark(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.toggle_mark();
            self.mark_dirty(&focused_name);
        }
    }

    pub fn move_up(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.move_up();
            self.pending_selection_changed = Some(focused_name.clone());
            self.mark_dirty(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.scroll_offset.saturating_sub(1);
            self.mark_dirty(&focused_name);
        }
    }

    pub fn move_down(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.move_down();
            self.pending_selection_changed = Some(focused_name.clone());
            self.mark_dirty(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = (v.scroll_offset + 1).min(v.max_offset);
            self.mark_dirty(&focused_name);
        }
    }

    pub fn jump_to_top(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.jump_to_top();
            self.pending_selection_changed = Some(focused_name.clone());
            self.mark_dirty(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = 0;
            self.mark_dirty(&focused_name);
        }
    }

    pub fn jump_to_bottom(&mut self) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            t.jump_to_bottom();
            self.pending_selection_changed = Some(focused_name.clone());
            self.mark_dirty(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.max_offset;
            self.mark_dirty(&focused_name);
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
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            for _ in 0..n {
                if t.selected_idx > 0 {
                    t.selected_idx -= 1;
                    t.selected_id = Some(t.visible_ids[t.selected_idx].clone());
                } else { break; }
            }
            self.pending_selection_changed = Some(focused_name.clone());
            self.mark_dirty(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = v.scroll_offset.saturating_sub(n);
            self.mark_dirty(&focused_name);
        }
    }

    pub fn move_down_n(&mut self, n: usize) {
        self.last_error = None;
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return; };
        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
            for _ in 0..n {
                if t.selected_idx < t.visible_ids.len().saturating_sub(1) {
                    t.selected_idx += 1;
                    t.selected_id = Some(t.visible_ids[t.selected_idx].clone());
                } else { break; }
            }
            self.pending_selection_changed = Some(focused_name.clone());
            self.mark_dirty(&focused_name);
        } else if let Some(Component::View(v)) = self.components.get_mut(&focused_name) {
            v.scroll_offset = (v.scroll_offset + n).min(v.max_offset);
            self.mark_dirty(&focused_name);
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
        let focused_name = if let Focus::Component(name) = &self.focus.current { name.clone() } else { return };
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
            self.mark_dirty(&focused_name);
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
