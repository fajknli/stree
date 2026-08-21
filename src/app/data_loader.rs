// src/app/data_loader.rs

use crate::app::{Engine, Component};
use crate::exec;
use std::collections::HashSet;
use crate::app::quote_if_needed;

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

        let ids_str = selected_entity.as_ref().map(|e| quote_if_needed(&e.id)).unwrap_or_default();
        let paths_str = selected_entity.as_ref().map(|e| quote_if_needed(&e.path)).unwrap_or_default();
        let window_name = tree_name.to_string();
        let mut dirty_views = Vec::new();

        for (view_name, comp) in self.components.iter_mut() {
            if let Component::View(v) = comp {
                let new_cached_id = selected_entity.as_ref().map(|e| e.id.clone());

                let is_empty = matches!(v.content, crate::app::view::ViewContent::Empty);
                if v.cached_entity_id == new_cached_id && !is_empty { continue; }

                if v.is_loading { self.pending_view_reload.insert(view_name.clone()); continue; }

                let width_str = v.rect_width.to_string();
                let height_str = v.rect_height.to_string();
                let template_args_vec = crate::config::split_args(&v.cmd_template);
                let depends_on_selection = cmd_depends_on_selection(&template_args_vec);
                if !depends_on_selection && !is_empty {
                    continue;
                }

                // 【核心魔法】杀掉上一个还在跑的预览进程树！防止孤儿进程吃 CPU
                if let Some(pid) = v.child_pid.lock().unwrap().take() {
                    crate::exec::kill_process_group(pid);
                }

                v.cached_entity_id = new_cached_id.clone();
                v.is_loading = true;
                v.h_scroll = 0;
                v.scroll_offset = 0;

                let ctx = Self::build_exec_context(
                    selected_entity.as_ref(), &ids_str, &paths_str, &window_name,
                    &width_str, &height_str, "", None
                );
                let full_cmd_args = exec::replace_placeholders_in_args(&template_args_vec, &ctx);

                if is_empty_command(&full_cmd_args) {
                    v.content = crate::app::view::ViewContent::Empty;
                    v.scroll_offset = 0;
                    v.is_loading = false;
                    dirty_views.push(view_name.clone()); continue;
                }

                let tx = self.async_view_tx.clone();
                let view_name_clone = view_name.clone();
                let target_id_clone = new_cached_id.clone();
                let max_lines = self.max_lines;

                // 克隆 PID 共享指针传给后台线程
                let child_pid = v.child_pid.clone();

                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(|| {
                        crate::exec::execute_command_args(&full_cmd_args, max_lines, child_pid)
                    });
                    let (content_bytes, is_graphic) = format_command_result(result);
                    let _ = tx.send((view_name_clone, target_id_clone, content_bytes, is_graphic));
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
                let depends_on_selection = cmd_depends_on_selection(&template_args_vec);
                if depends_on_selection {
                    continue;
                }

                let ctx = Self::build_exec_context(None, "", "", &window_name, &width_str, &height_str, "", None);
                let full_cmd_args = exec::replace_placeholders_in_args(&template_args_vec, &ctx);

                if is_empty_command(&full_cmd_args) { continue; }

                // 传入 PID 共享指针
                match exec::execute_command_args(&full_cmd_args, self.max_lines, v.child_pid.clone()) {
                    Ok(res) => {
                        let (content_bytes, is_graphic) = format_command_result(Ok(Ok(res)));
                        v.content = if is_graphic {
                            crate::app::view::ViewContent::Graphic(content_bytes)
                        } else {
                            let text = String::from_utf8_lossy(&content_bytes).to_string();
                            crate::app::view::ViewContent::Text(text)
                        };
                        v.scroll_offset = 0;
                        v.graphic_dirty = true;
                    }
                    Err(e) => {
                        let (content_bytes, _) = format_command_result(Ok(Err(e)));
                        v.content = crate::app::view::ViewContent::Text(String::from_utf8_lossy(&content_bytes).to_string());
                    }
                }
            }
        }
    }

    pub fn handle_ipc_update(&mut self, target: &str, data: &str, term_width: u16, term_height: u16) {
        if self.handle_system_command(target) {
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
                        // 【核心修复】当收到全新的目录数据时，自动清除搜索状态！
                        // 防止切换目录后，旧的搜索词继续过滤新目录的内容。
                        t.search_query = None;
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
                    v.content = crate::app::view::ViewContent::Text(data.to_string());
                    v.scroll_offset = 0;
                    v.cached_entity_id = None;
                    self.mark_dirty(target);
                }
                Component::StatusBar(s) => {
                    s.message = Some(data.to_string());
                    s.message_expire = Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                    self.mark_dirty(target);
                }
                _ => {}
            }
        }
    }

    fn handle_system_command(&mut self, target: &str) -> bool {
        match target {
            "@exit" => {
                crate::signal::request_quit();
                true
            }
            "@layout-reset" => {
                self.window_rect_overrides.clear();
                let parsed_layout = crate::layout::parse_layouts(&self.layout_blueprint);
                self.layout_layers = parsed_layout.layers;
                self.mark_all_dirty();
                true
            }
            "@clear-marks" => {
                let mut cleared = false;
                for comp in self.components.values_mut() {
                    if let crate::app::Component::Tree(t) = comp {
                        if !t.marked_ids.is_empty() {
                            t.marked_ids.clear();
                            cleared = true;
                        }
                    }
                }
                if cleared {
                    self.mark_all_dirty();
                }
                true
            }
            _ => {
                if let Some(layer_name) = target.strip_prefix("@layout-show ") {
                    self.set_layout_visible(layer_name.trim(), true);
                    true
                } else if let Some(layer_name) = target.strip_prefix("@layout-hide ") {
                    self.set_layout_visible(layer_name.trim(), false);
                    true
                } else if let Some(args) = target.strip_prefix("@select ") {
                    let parts: Vec<&str> = args.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        self.select_id(parts[0], parts[1]);
                    }
                    true
                } else if let Some(args) = target.strip_prefix("@title ") {
                    let parts: Vec<&str> = args.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        if let Some(Component::Tree(t)) = self.components.get_mut(parts[0]) {
                            t.title_override = Some(parts[1].to_string());
                            self.mark_dirty(parts[0]);
                        }
                    }
                    true
                } else {
                    false
                }
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
                std::thread::spawn(move || {
                    let result = crate::exec::execute_reload_hook(Some(&cmd));
                    let _ = tx.send((name_clone, result));
                });
            }
        }
    }
}

// ================= 命令解析辅助逻辑 =================

/// 检查命令模板是否依赖选中项
fn cmd_depends_on_selection(template_args: &[String]) -> bool {
    template_args.iter().any(|arg|
        arg.contains("{id}") || arg.contains("{path}") || arg.contains("{display}") ||
        arg.contains("{tags}") || arg.contains("{ids}") || arg.contains("{paths}")
    )
}

/// 检查命令是否为空
pub fn is_empty_command(args: &[String]) -> bool {
    args.is_empty() || (args.len() == 1 && args[0].trim().is_empty())
}

/// 统一处理命令执行结果，收口错误格式化逻辑
fn format_command_result(result: std::thread::Result<std::io::Result<(i32, Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    match result {
        Ok(Ok((code, stdout, is_graphic))) => {
            let is_blank = stdout.iter().all(|&b| b.is_ascii_whitespace());
            if code != 0 && is_blank {
                (format!("[ERR] Command exited with code {}\n", code).into_bytes(), false)
            } else {
                (stdout, is_graphic)
            }
        },
        Ok(Err(e)) => (format!("[ERR] {}\n", e).into_bytes(), false),
        Err(_) => (b"[ERR] Background thread panicked\n".to_vec(), false),
    }
}
