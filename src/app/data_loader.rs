// src/app/data_loader.rs

use crate::app::{Engine, Component};
use crate::exec;
use std::collections::HashSet;

impl Engine {
    pub fn broadcast_selection_changed(&mut self, tree_name: &str, _term_width: u16, _term_height: u16) {
        let is_focused_tree = match &self.focus.current {
            crate::app::Focus::Component(n) => n == tree_name,
            crate::app::Focus::None => false,
        };
        if !is_focused_tree { return; }

        let selected_entity = if let Some(Component::Tree(t)) = self.components.get(tree_name) {
            t.get_selected_entity().cloned()
        } else { return; };

        let ids_str = selected_entity.as_ref().map(|e| e.id.clone()).unwrap_or_default();
        let paths_str = selected_entity.as_ref().map(|e| {
            if e.path.contains(' ') {
                format!("\"{}\"", e.path)
            } else {
                e.path.clone()
            }
        }).unwrap_or_default();
        let window_name = tree_name.to_string();
        let mut dirty_views = Vec::new();

        for (view_name, comp) in self.components.iter_mut() {
            if let Component::View(v) = comp {
                let new_cached_id = selected_entity.as_ref().map(|e| e.id.clone());
                if v.cached_entity_id == new_cached_id && !v.content_buffer.is_empty() { continue; }
                if v.is_loading { self.pending_view_reload.insert(view_name.clone()); continue; }

                let width_str = v.rect_width.to_string();
                let height_str = v.rect_height.to_string();
                let template_args_vec = crate::config::split_args(&v.cmd_template);

                // 如果命令不依赖选中实体（如静态帮助菜单），且已有内容，则不参与 Tree 选中变化的自动重载！
                let depends_on_selection = template_args_vec.iter().any(|arg|
                    arg.contains("{id}") || arg.contains("{path}") || arg.contains("{display}") ||
                    arg.contains("{tags}") || arg.contains("{ids}") || arg.contains("{paths}")
                );
                if !depends_on_selection && !v.content_buffer.is_empty() {
                    continue;
                }

                v.cached_entity_id = new_cached_id.clone();
                v.is_loading = true;

                // 【新增】切换节点时，重置水平和垂直滚动位置，从头展示新内容
                v.h_scroll = 0;
                v.scroll_offset = 0;

                let ctx = Self::build_exec_context(
                    selected_entity.as_ref(), &ids_str, &paths_str, &window_name,
                    &width_str, &height_str, "", None
                );
                let full_cmd_args = exec::replace_placeholders_in_args(&template_args_vec, &ctx);

                if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) {
                    v.content_buffer = String::new(); v.scroll_offset = 0; v.is_loading = false;
                    dirty_views.push(view_name.clone()); continue;
                }

                let tx = self.async_view_tx.clone();
                let view_name_clone = view_name.clone();
                let target_id_clone = new_cached_id.clone();
                let max_lines = self.max_lines;

                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(|| { crate::exec::execute_command_args(&full_cmd_args, max_lines) });
                    let content = match result {
                        Ok(Ok((code, stdout))) => if code != 0 && stdout.trim().is_empty() { format!("[ERR] Command exited with code {}", code) } else { stdout },
                        Ok(Err(e)) => format!("[ERR] {}", e),
                        Err(_) => "[ERR] Background thread panicked".to_string(),
                    };
                    let _ = tx.send((view_name_clone, target_id_clone, content));
                });
            }
        }
        for name in dirty_views { self.mark_dirty(&name); }
    }

    pub fn init_views(&mut self) {
        self.mark_all_dirty();
        for (view_name, comp) in self.components.iter_mut() {
            if let Component::View(v) = comp {
                let width_str = v.rect_width.to_string();
                let height_str = v.rect_height.to_string();
                let window_name = view_name.clone();
                let template_args_vec = crate::config::split_args(&v.cmd_template);

                // 【优化】如果是依赖选中实体的动态视图，跳过初始化同步执行，
                // 交给 broadcast_selection_changed 进行异步加载，防止闪烁和错误输出。
                let depends_on_selection = template_args_vec.iter().any(|arg|
                    arg.contains("{id}") || arg.contains("{path}") || arg.contains("{display}") ||
                    arg.contains("{tags}") || arg.contains("{ids}") || arg.contains("{paths}")
                );
                if depends_on_selection {
                    continue;
                }

                let ctx = Self::build_exec_context(None, "", "", &window_name, &width_str, &height_str, "", None);
                let full_cmd_args = exec::replace_placeholders_in_args(&template_args_vec, &ctx);

                if full_cmd_args.is_empty() || (full_cmd_args.len() == 1 && full_cmd_args[0].trim().is_empty()) { continue; }

                match exec::execute_command_args(&full_cmd_args, self.max_lines) {
                    Ok((code, stdout)) => {
                        v.content_buffer = if code != 0 && stdout.trim().is_empty() { format!("[ERR] Command exited with code {}", code) } else { stdout };
                        v.scroll_offset = 0;
                    }
                    Err(e) => { v.content_buffer = format!("[ERR] {}", e); }
                }
            }
        }
    }

    pub fn handle_ipc_update(&mut self, target: &str, data: &str, term_width: u16, term_height: u16) {
        if target == "@layout-reset" {
            self.window_rect_overrides.clear();
            // 【新增】从蓝图完全重建 AST，恢复 Auto 和初始 Percent！
            let parsed_layout = crate::layout::parse_layouts(&self.layout_blueprint);
            self.layout_layers = parsed_layout.layers;

            self.mark_all_dirty();
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
                    self.mark_dirty(target);
                }
                Component::View(v) => {
                    v.content_buffer = data.to_string();
                    v.scroll_offset = 0;
                    v.cached_entity_id = None;
                    self.mark_dirty(target);
                }
                Component::StatusBar(s) => {
                    // 【修复】不再永久覆盖模板，改为临时消息，3秒后自动消失
                    s.message = Some(data.to_string());
                    s.message_expire = Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                    self.mark_dirty(target);
                }
                _ => {}
            }
        }
    }

    pub fn trigger_reload(&mut self) {
        self.mark_all_dirty();
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
                let tx = self.async_reload_tx.clone();
                let name_clone = name.clone();
                // 【修复】放入后台线程执行，彻底解除主线程阻塞
                std::thread::spawn(move || {
                    let result = crate::exec::execute_reload_hook(Some(&cmd));
                    let _ = tx.send((name_clone, result));
                });
            }
        }
    }
}
