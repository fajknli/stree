// src/app/overlay.rs
use crate::app::{Component, Engine, OverlayLayer};
use crossterm::event::KeyModifiers;
use crate::app::input::InputKeyResult;

impl Engine {
    pub fn has_active_input(&self) -> bool {
        !self.overlay_stack.is_empty()
    }

    pub fn activate_input(&mut self, name: &str, initial_text: &str) {
        // 1. 前置处理：如果是配置文件中的静态 prompt，执行变量替换
        if initial_text.is_empty() {
            let computed_prefix = if let Some(Component::Input(input)) = self.components.get(name) {
                let mut p = input.prompt_template.clone();
                if let Some(tree) = self.get_focused_tree_state() {
                    // 计算真实目标数量：优先标记，回退选中
                    let target_count = if !tree.marked_ids.is_empty() {
                        tree.marked_ids.len()
                    } else {
                        if tree.selected_id.is_some() { 1 } else { 0 }
                    };
                    p = p.replace("{stree_targets}", &target_count.to_string());
                    p = p.replace("{stree_marked}", &tree.marked_ids.len().to_string());
                    p = p.replace("{stree_idx}", &(tree.selected_idx + 1).to_string());
                    p = p.replace("{stree_total}", &tree.dataset.entities.len().to_string());
                    p = p.replace("{stree_visible}", &tree.visible_ids.len().to_string());
                } else {
                    p = p.replace("{stree_targets}", "0");
                    p = p.replace("{stree_marked}", "0");
                    p = p.replace("{stree_idx}", "0");
                    p = p.replace("{stree_total}", "0");
                    p = p.replace("{stree_visible}", "0");
                }
                Some(p)
            } else {
                None
            };

            if let Some(p) = computed_prefix {
                if let Some(Component::Input(input)) = self.components.get_mut(name) {
                    input.prefix = p;
                }
            }
        }

        // 2. 激活并提取 target_override，避免双重借用
        let target_override_opt = if let Some(Component::Input(input)) = self.components.get_mut(name) {
            if !initial_text.is_empty() {
                input.prefix = initial_text.to_string();
            }
            input.activate();
            input.target_override.clone()
        } else {
            None
        };

        // 3. 标记脏并压栈
        if target_override_opt.is_some() {
            self.mark_all_dirty();
            if let Some(target) = target_override_opt {
                self.overlay_stack.push(OverlayLayer {
                    source: name.to_string(),
                    target: target,
                });
            }
        }
    }

    pub fn cancel_input(&mut self) {
        if let Some(layer) = self.overlay_stack.last() {
            let name = layer.source.clone();
            self.close_overlay(&name);
        }
    }

    // 显式关闭指定覆盖者
    pub fn close_overlay(&mut self, name: &str) {
        self.overlay_stack.retain(|layer| layer.source != name);
        if let Some(comp) = self.components.get_mut(name) {
            if let Component::Input(i) = comp {
                i.deactivate();
                self.mark_all_dirty();
            }
        }
    }

    // 关闭栈顶覆盖者
    pub fn close_top_overlay(&mut self) {
        if let Some(layer) = self.overlay_stack.pop() {
            if let Some(comp) = self.components.get_mut(&layer.source) {
                if let Component::Input(i) = comp {
                    i.deactivate();
                    self.mark_all_dirty();
                }
            }
        }
    }

    pub fn handle_input_key(&mut self, key: crossterm::event::KeyEvent) -> Option<(String, InputKeyResult)> {
        if let Some(layer) = self.overlay_stack.last().cloned() {
            let input_name = layer.source;

            if let Some(Component::Input(input)) = self.components.get_mut(&input_name) {
                // 瞬时模式：按下任意字符键立即提交，其他键取消
                if input.is_instant {
                    if let crossterm::event::KeyCode::Char(c) = key.code {
                        return Some((input_name, InputKeyResult::Submitted(c.to_string())));
                    } else {
                        return Some((input_name, InputKeyResult::Cancelled));
                    }
                }

                // 普通输入模式逻辑
                match key.code {
                    crossterm::event::KeyCode::Esc => {
                        return Some((input_name, InputKeyResult::Cancelled));
                    }
                    crossterm::event::KeyCode::Enter => {
                        let submitted = input.buffer.clone();
                        return Some((input_name, InputKeyResult::Submitted(submitted)));
                    }
                    crossterm::event::KeyCode::Backspace => {
                        if input.buffer.is_empty() {
                            return Some((input_name, InputKeyResult::Cancelled));
                        } else {
                            input.backspace();
                        }
                    }
                    crossterm::event::KeyCode::Left => input.move_left(),
                    crossterm::event::KeyCode::Right => input.move_right(),
                    crossterm::event::KeyCode::Home => input.move_home(),
                    crossterm::event::KeyCode::End => input.move_end(),
                    crossterm::event::KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'u' { input.clear(); }
                        else if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'a' { input.move_home(); }
                        else if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'e' { input.move_end(); }
                        else { input.insert_char(c); }
                    }
                    _ => {}
                }
                // 普通按键操作后，标记 dirty 以刷新光标位置
                self.mark_all_dirty();
                return Some((input_name, InputKeyResult::Updated));
            }
        }
        None
    }

    // 统一的输入提交执行器
    pub fn submit_input(&mut self, input_name: &str, text: &str, term_width: u16, term_height: u16) {
        let (template_opt, is_silent) = if let Some(Component::Input(input)) = self.components.get(input_name) {
            (input.on_submit.clone(), input.on_submit_is_silent)
        } else { return };

        if let Some(template) = template_opt {
            let args = crate::config::split_args(&template);

            let tree_name = match &self.focus.current {
                crate::app::Focus::Component(n) if matches!(self.components.get(n), Some(Component::Tree(_))) => n.clone(),
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

            let window_name = match &self.focus.current { crate::app::Focus::Component(n) => n.clone(), _ => String::new() };

            let ctx = Self::build_exec_context(
                selected_entity.as_ref(), &ids_str, &paths_str, &window_name,
                &term_width.to_string(), &term_height.to_string(), "",
                Some(&[("input", text)])
            );

            let full_cmd_args = crate::exec::replace_placeholders_in_args(&args, &ctx);
            if !full_cmd_args.is_empty() {
                crate::runner::execute_binding(self, &full_cmd_args, is_silent, term_width, term_height);
            }
        }
    }
}
