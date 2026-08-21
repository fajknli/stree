// src/app/event_handler.rs
use crate::app::{Component, Engine, Focus, InternalCommand};
use crate::exec;
use crate::layout::{WindowRect, BorderStyle};
use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind, MouseButton};
use crate::app::data_loader::is_empty_command;

impl Engine {
    /// 处理单个键盘事件，返回 true 表示请求退出程序
    pub fn handle_key_event(
        &mut self,
        key: &KeyEvent,
        all_rects: &[(WindowRect, String, BorderStyle, usize)],
        columns: u16,
        rows: u16,
    ) -> bool {
        if key.kind != crossterm::event::KeyEventKind::Press { return false; }

        if self.drag.active {
            if key.code == KeyCode::Esc {
                self.drag.active = false;
                self.drag.resize_target = None;
                self.drag.start_idx = None;
            }
            return false;
        }

        if self.last_error.is_some() { self.last_error = None; }

        // 1. 构建 Keymap 继承链
        let active_keymaps_owned = self.get_active_keymaps();
        let active_keymaps: Vec<Option<&str>> = active_keymaps_owned.iter().map(|s| s.as_deref()).collect();

        // 2. 查找按键绑定
        let mut binding_opt = self.prepare_key_binding_args_keymap(&active_keymaps, key, columns, rows);

        // 3. 输入框激活时的防穿透逻辑
        if !self.overlay_stack.is_empty() {
            let has_scoped_binding = self.key_bindings.get_keymap_strict(&active_keymaps, key).is_some();
            if !has_scoped_binding {
                binding_opt = None; // 丢弃全局绑定，让它掉进 handle_input_key
            }
        }

        if let Some((full_cmd_args, is_silent)) = binding_opt {
            if let Some(internal_cmd) = InternalCommand::from_args(&full_cmd_args) {
                match internal_cmd {
                    InternalCommand::Exit => return true, // 通知 main_loop 退出
                    InternalCommand::Esc => {
                        self.handle_esc_action(all_rects, columns, rows);
                    }
                    InternalCommand::Tab => self.handle_tab(columns, rows),
                    InternalCommand::Expand => self.toggle_expand(),
                    InternalCommand::Mark => self.toggle_mark(),
                    InternalCommand::Up => self.move_up(),
                    InternalCommand::Down => self.move_down(),
                    InternalCommand::Top => self.jump_to_top(),
                    InternalCommand::Bottom => self.jump_to_bottom(),
                    InternalCommand::Enter => {
                        if let Some(_layer) = self.overlay_stack.last().cloned() {
                            if let Some((input_name, result)) = self.handle_input_key(*key) {
                                // 【重构】调用提取的方法
                                self.handle_input_key_result(&input_name, result, columns, rows);
                            }
                        } else {
                            self.toggle_expand();
                            self.emit("confirm", columns, rows);
                        }
                    }
                    InternalCommand::ActivateInput(name) => {
                        self.activate_input(&name, "");
                    }
                    InternalCommand::ToggleLayout(name) => self.toggle_layout_visible(&name),
                    InternalCommand::ShowLayout(name) => self.set_layout_visible(&name, true),
                    InternalCommand::HideLayout(name) => self.set_layout_visible(&name, false),
                    InternalCommand::ScrollLeft => self.scroll_left(),
                    InternalCommand::ScrollRight => self.scroll_right(),
                    InternalCommand::CycleLayer => self.cycle_layer(all_rects),
                    InternalCommand::FocusLeft => self.focus_direction("left", all_rects),
                    InternalCommand::FocusRight => self.focus_direction("right", all_rects),
                    InternalCommand::FocusUp => self.focus_direction("up", all_rects),
                    InternalCommand::FocusDown => self.focus_direction("down", all_rects),
                    InternalCommand::CloseOverlay(name) => self.close_overlay(&name),
                    InternalCommand::CloseTopOverlay => self.close_top_overlay(),
                    InternalCommand::Noop => { /* 什么都不做，成功吞掉按键！ */ }
                }
            } else {
                crate::runner::execute_binding(self, &full_cmd_args, is_silent, columns, rows);
            }
        } else {
            // 未查到绑定的按键
            if let Some(_layer) = self.overlay_stack.last().cloned() {
                if let Some((input_name, result)) = self.handle_input_key(*key) {
                    // 【重构】调用提取的方法
                    self.handle_input_key_result(&input_name, result, columns, rows);
                }
            }
        }
        false
    }

    /// 处理单个鼠标事件
    pub fn handle_mouse_event(
        &mut self,
        mouse_event: &MouseEvent,
        all_rects: &[(WindowRect, String, BorderStyle, usize)],
        columns: u16,
        rows: u16,
        scroll_step: u8,
        layout_changed: bool,
    ) {
        if !self.mouse.enabled { return; }

        let mut sorted_rects: Vec<_> = all_rects.iter().collect();
        sorted_rects.sort_by(|a, b| b.3.cmp(&a.3));

        let mut sorted_edges: Vec<_> = self.drag.cached_edges.clone();
        sorted_edges.sort_by(|a, b| b.z_index.cmp(&a.z_index));

        // 1. 悬停获取焦点
        if matches!(mouse_event.kind, MouseEventKind::Moved) && !self.drag.active {
            if self.handle_mouse_hover(mouse_event, &sorted_rects, layout_changed, columns, rows) {
                return;
            }
        }

        // 2. 鼠标释放处理拖拽收尾
        if matches!(mouse_event.kind, MouseEventKind::Up(_)) {
            if self.drag.active {
                if let Some(crate::app::DragTarget::ResizeFloating(_, _)) = &self.drag.resize_target {
                    self.mark_all_dirty();
                } else if self.drag.resize_target.is_some() {
                    let has_dragged = self.drag.last_col != self.drag.start_col
                        || self.drag.last_row != self.drag.start_row;

                    if has_dragged {
                        self.force_recalculate_percentages(all_rects);
                    }
                    self.window_rect_overrides.clear();
                    self.mark_all_dirty();
                }
                self.drag.active = false;
                self.drag.resize_target = None;
                self.drag.start_idx = None;
                return;
            }
        }

        // 3. 拖拽中实时记录坐标并应用尺寸变更
        if self.drag.active && self.drag.resize_target.is_some() {
            if let MouseEventKind::Drag(MouseButton::Left) = mouse_event.kind {
                self.handle_drag_motion(mouse_event, columns, rows);
            }
            return;
        }

        // 4. 碰撞检测
        let mut hit_floating_edge = false;
        let mut hit_floating_inside = false;

        if let MouseEventKind::Down(MouseButton::Left) = mouse_event.kind {
            let (edge_hit, inside) = self.check_floating_edge_hit(mouse_event.column, mouse_event.row, all_rects);
            hit_floating_inside = inside;

            if let Some((win_name, edge_mask, init_x, init_y, w, h)) = edge_hit {
                self.focus.current = Focus::Component(win_name.clone());
                self.mark_dirty(&win_name);

                self.drag.active = true;
                self.drag.resize_target = Some(crate::app::DragTarget::ResizeFloating(win_name.clone(), edge_mask));
                self.drag.start_col = mouse_event.column;
                self.drag.start_row = mouse_event.row;
                self.drag.last_col = mouse_event.column;
                self.drag.last_row = mouse_event.row;
                self.drag.initial_width = w;
                self.drag.initial_height = h;
                self.drag.initial_anchor_x = init_x;
                self.drag.initial_anchor_y = init_y;
                hit_floating_edge = true;
            }
        }

        if hit_floating_edge { return; }

        let mut hit_edge = None;
        if !hit_floating_inside {
            for edge in &sorted_edges {
                let in_x = mouse_event.column >= edge.hit_rect.start_col
                    && mouse_event.column < edge.hit_rect.start_col + edge.hit_rect.width;
                let in_y = mouse_event.row >= edge.hit_rect.start_row
                    && mouse_event.row < edge.hit_rect.start_row + edge.hit_rect.height;

                if in_x && in_y {
                    hit_edge = Some(edge.clone());
                    break;
                }
            }
        }

        let mut in_intersection = false;
        for &(x, y) in &self.drag.cached_intersections {
            if (mouse_event.column == x || mouse_event.column == x - 1) &&
               (mouse_event.row == y || mouse_event.row == y - 1) {
                in_intersection = true;
                break;
            }
        }

        if !in_intersection {
            if let Some(edge) = hit_edge {
                if let MouseEventKind::Down(MouseButton::Left) = mouse_event.kind {
                    let primary_id = edge.primary_id.clone();
                    let neighbor_id = edge.neighbor_id.clone();
                    let dir = edge.direction;

                    self.focus.current = Focus::Component(primary_id.clone());
                    self.mark_dirty(&primary_id);

                    let r1 = self.get_node_current_bbox(&primary_id, columns, rows).map(|(r, _)| r).unwrap_or_default();
                    let r2 = self.get_node_current_bbox(&neighbor_id, columns, rows).map(|(r, _)| r).unwrap_or_default();

                    self.drag.active = true;
                    self.drag.is_restructured = false;
                    self.drag.resize_target = Some(crate::app::DragTarget::ResizeEdge(primary_id, neighbor_id, dir));
                    self.drag.start_col = mouse_event.column;
                    self.drag.start_row = mouse_event.row;
                    self.drag.last_col = mouse_event.column;
                    self.drag.last_row = mouse_event.row;
                    self.drag.initial_t1_rect = r1;
                    self.drag.initial_t2_rect = r2;
                    return;
                }
            }
        }

        // 5. 窗口内容区命中逻辑
        self.handle_content_click(mouse_event, &sorted_rects, columns, rows, scroll_step);
    }
    // 【新增】判断是否为搜索输入框
    fn is_search_input(&self, name: &str) -> bool {
        self.components.get(name)
            .map(|c| matches!(c, Component::Input(i) if i.is_search))
            .unwrap_or(false)
    }

    // 【新增】提取输入结果处理逻辑
    fn handle_input_key_result(&mut self, input_name: &str, result: crate::app::input::InputKeyResult, columns: u16, rows: u16) {
        match result {
            crate::app::input::InputKeyResult::Submitted(text) => {
                // 移除多余的 & 符号
                if self.is_search_input(input_name) {
                    self.apply_search(&text, columns, rows);
                } else {
                    self.submit_input(input_name, &text, columns, rows);
                }
                self.close_overlay(input_name);
            }
            crate::app::input::InputKeyResult::Cancelled => {
                self.close_overlay(input_name);
                if self.is_search_input(input_name) {
                    if let Focus::Component(focused_name) = self.focus.current.clone() {
                        if let Some(Component::Tree(t)) = self.components.get_mut(&focused_name) {
                            if t.search_query.take().is_some() {
                                t.rebuild_visible_ids();
                                if !t.visible_ids.is_empty() {
                                    t.selected_idx = 0;
                                    t.selected_id = Some(t.visible_ids[0].clone());
                                }
                            }
                        }
                        self.pending_selection_changed = Some(focused_name);
                    }
                    self.mark_all_dirty();
                }
            }
            crate::app::input::InputKeyResult::Updated => {
                if self.is_search_input(input_name) {
                    // 移除多余的 & 符号
                    if let Some(buffer) = self.components.get(input_name).map(|c| if let Component::Input(i) = c { i.buffer.clone() } else { String::new() }) {
                        self.apply_search(&buffer, columns, rows);
                    }
                }
            }
        }
    }
    pub fn prepare_key_binding_args_keymap(&self, keymaps: &[Option<&str>], key: &crossterm::event::KeyEvent, term_width: u16, term_height: u16) -> Option<(Vec<String>, bool)> {
        let (cmd_template_args, is_silent) = self.key_bindings.get_keymap(keymaps, key)?;
        let tree_name = self.get_active_tree_name()?;
        let tree_state = if let Some(Component::Tree(t)) = self.components.get(&tree_name) { t } else { return None; };
        let selected_entity = tree_state.get_selected_entity();
        let (ids_str, paths_str) = self.get_target_strings(&tree_name);
        let window_name = match &self.focus.current { Focus::Component(n) => n.clone(), Focus::None => String::new() };
        let ctx = Self::build_exec_context(selected_entity, &ids_str, &paths_str, &window_name, &term_width.to_string(), &term_height.to_string(), "", None);
        let full_cmd_args = exec::replace_placeholders_in_args(cmd_template_args, &ctx);
        if is_empty_command(&full_cmd_args) { None } else { Some((full_cmd_args, *is_silent)) }
    }
    /// 提取 Esc 键的处理逻辑，降低 handle_key_event 圈复杂度
    fn handle_esc_action(
        &mut self,
        all_rects: &[(WindowRect, String, BorderStyle, usize)],
        columns: u16,
        rows: u16,
    ) {
        if !self.overlay_stack.is_empty() {
            self.close_top_overlay();
        } else {
            let mut hid_layer = false;
            if let Focus::Component(name) = self.focus.current.clone() {
                if let Some((_, _, _, z)) = all_rects.iter().find(|(_, n, _, _)| n == &name) {
                    if *z > 0 {
                        self.set_layout_visible(&name, false);
                        hid_layer = true;
                    }
                }
            }
            if !hid_layer {
                let mut cleared_search = false;
                for (name, comp) in self.components.iter_mut() {
                    if let Component::Tree(t) = comp {
                        if t.search_query.take().is_some() {
                            t.rebuild_visible_ids();
                            if !t.visible_ids.is_empty() {
                                t.selected_idx = 0;
                                t.selected_id = Some(t.visible_ids[0].clone());
                            }
                            self.pending_selection_changed = Some(name.clone());
                            cleared_search = true;
                        }
                    }
                }
                if cleared_search {
                    self.mark_all_dirty();
                } else {
                    self.emit("quit_request", columns, rows);
                }
            }
        }
    }
    /// 提取拖拽中实时尺寸计算的逻辑，降低 handle_mouse_event 圈复杂度
    fn handle_drag_motion(&mut self, mouse_event: &MouseEvent, columns: u16, rows: u16) {
        self.drag.last_col = mouse_event.column;
        self.drag.last_row = mouse_event.row;

        // 【新增】如果是浮动窗口拖拽，直接在此处计算并应用尺寸
        if let Some(crate::app::DragTarget::ResizeFloating(name, mask)) = &self.drag.resize_target.clone() {
            // 必须使用 i32 计算 delta，因为可以向左/向上拖动
            let delta_x = self.drag.last_col as i32 - self.drag.start_col as i32;
            let delta_y = self.drag.last_row as i32 - self.drag.start_row as i32;

            let mut new_x = self.drag.initial_anchor_x as i32;
            let mut new_y = self.drag.initial_anchor_y as i32;
            let mut new_w = self.drag.initial_width as i32;
            let mut new_h = self.drag.initial_height as i32;

            if mask & 1 != 0 { // Left
                new_x += delta_x;
                new_w -= delta_x;
            }
            if mask & 2 != 0 { // Right
                new_w += delta_x;
            }
            if mask & 4 != 0 { // Top
                new_y += delta_y;
                new_h -= delta_y;
            }
            if mask & 8 != 0 { // Bottom
                new_h += delta_y;
            }

            // 碰撞检测与最小尺寸限制
            let term_w = columns as i32;
            let term_h = rows as i32;
            const MIN_W: i32 = 2;
            const MIN_H: i32 = 2;

            if new_w < MIN_W { new_w = MIN_W; }
            if new_h < MIN_H { new_h = MIN_H; }
            if new_x < 0 { new_x = 0; }
            if new_y < 0 { new_y = 0; }
            if new_x + new_w > term_w { new_w = term_w - new_x; }
            if new_y + new_h > term_h { new_h = term_h - new_y; }

            let final_x = new_x as u16;
            let final_y = new_y as u16;
            let final_w = new_w as u16;
            let final_h = new_h as u16;

            // 覆盖窗口本身的尺寸声明
            self.window_rect_overrides.insert(
                name.clone(),
                crate::layout::WindowSize::Absolute2D(final_w, final_h)
            );

            // 覆盖画布尺寸（维持锚点）
            let name_clone = name.clone();
            for layer in &mut self.layout_layers {
                if crate::app::Engine::layout_contains_window(layer, &name_clone) {
                    layer.runtime_rect_override = Some(crate::layout::WindowRect {
                        start_col: final_x,
                        start_row: final_y,
                        width: final_w,
                        height: final_h,
                    });
                }
            }
        }
    }

    /// 处理窗口内容区的点击、滚动与标记逻辑
    fn handle_content_click(
        &mut self,
        mouse_event: &MouseEvent,
        sorted_rects: &[&(WindowRect, String, BorderStyle, usize)],
        columns: u16,
        rows: u16,
        scroll_step: u8,
    ) {
        for (rect, name, _border, _z) in sorted_rects.iter() {
            let in_x = mouse_event.column >= rect.start_col && mouse_event.column < rect.start_col + rect.width;
            let in_y = mouse_event.row >= rect.start_row && mouse_event.row < rect.start_row + rect.height;
            if !in_x || !in_y { continue; }

            if let Some(Component::StatusBar(_)) = self.components.get(name) {
                continue;
            }

            let is_press = matches!(mouse_event.kind,
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right)
            );

            if is_press {
                // 【防线】如果点击的是 nofocus 组件，不夺焦
                if !self.is_unfocusable(name) {
                    let old_focus = self.focus.current.clone();
                    self.focus.current = Focus::Component(name.to_string());

                    if old_focus != self.focus.current {
                        if let Focus::Component(old_name) = &old_focus {
                            self.mark_dirty(old_name);
                        }
                        self.mark_dirty(name);
                        for (n, c) in &self.components {
                            if matches!(c, Component::StatusBar(_)) {
                                self.dirty_components.insert(n.clone());
                            }
                        }
                        if let Some(Component::Tree(t)) = self.components.get(name) {
                            if t.focus_to_fire {
                                self.emit("focus", columns, rows);
                            }
                        }
                    }
                }

                if self.has_active_input() {
                    self.cancel_input();
                }
            }

            match self.components.get(name) {
                Some(Component::Tree(_)) => {
                    let click_to_fire = if let Some(Component::Tree(t)) = self.components.get(name) { t.click_to_fire } else { false };
                    let (target_idx, clicked_id, visible_len, tree_name) =
                    if let Some(Component::Tree(t)) = self.components.get(name) {
                        let max_rows = (rect.height as usize).saturating_sub(2);
                        let scroll_offset = crate::ui::calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows, t.v_scroll);
                        let target_idx = scroll_offset + mouse_event.row.saturating_sub(rect.start_row).saturating_sub(1) as usize;
                        let is_valid_click = target_idx < t.visible_ids.len();
                        let clicked_id = if is_valid_click { Some(t.visible_ids[target_idx].clone()) } else { None };
                        (target_idx, clicked_id, t.visible_ids.len(), name.to_string())
                    } else {
                        (0, None, 0, String::new())
                    };

                    match mouse_event.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if let Some(ref cid) = clicked_id {
                                let now = std::time::Instant::now();
                                let is_double_click = self.mouse.last_click_time
                                    .map_or(false, |t| now.duration_since(t).as_millis() < 300)
                                    && self.mouse.last_clicked_id.as_deref() == Some(cid.as_str());

                                if is_double_click {
                                    self.select_id(&tree_name, cid);
                                    self.toggle_expand();
                                    self.emit("confirm", columns, rows);
                                    self.mouse.last_click_time = None;
                                } else {
                                    self.select_id(&tree_name, cid);
                                    self.mouse.last_click_time = Some(now);
                                    self.mouse.last_clicked_id = Some(cid.clone());
                                    if click_to_fire {
                                        self.emit("click", columns, rows);
                                    }
                                }
                            }
                        }
                        MouseEventKind::Down(MouseButton::Right) => {
                            if let Some(ref cid) = clicked_id {
                                if let Some(Component::Tree(t)) = self.components.get_mut(&tree_name) {
                                    let was_marked = t.marked_ids.contains(cid);
                                    if was_marked { t.marked_ids.remove(cid); } else { t.marked_ids.insert(cid.clone()); }
                                    self.drag.is_marking = !was_marked;
                                }
                                self.drag.start_idx = Some(target_idx);
                                self.drag.active = true;
                                self.mark_dirty(&tree_name);
                            }
                        }
                        MouseEventKind::ScrollUp => { self.move_up_n(scroll_step as usize); }
                        MouseEventKind::ScrollDown => { self.move_down_n(scroll_step as usize); }
                        MouseEventKind::Up(_) => {
                            self.drag.active = false;
                            self.drag.start_idx = None;
                            self.drag.resize_target = None;
                            self.mark_dirty(&tree_name);
                        }
                        MouseEventKind::Drag(MouseButton::Right) => {
                            if self.drag.active {
                                if let Some(start_idx) = self.drag.start_idx {
                                    let clamped_target = target_idx.min(visible_len.saturating_sub(1));
                                    let range = if clamped_target >= start_idx {
                                        start_idx..=clamped_target
                                    } else {
                                        clamped_target..=start_idx
                                    };
                                    if let Some(Component::Tree(t)) = self.components.get_mut(&tree_name) {
                                        for i in range {
                                            if let Some(id) = t.visible_ids.get(i) {
                                                if self.drag.is_marking { t.marked_ids.insert(id.clone()); } else { t.marked_ids.remove(id); }
                                            }
                                        }
                                        self.mark_dirty(&tree_name);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some(Component::View(_)) => {
                    match mouse_event.kind {
                        // 【修改】直接滚动鼠标当前悬停的组件，而不是键盘焦点组件
                        MouseEventKind::ScrollUp => { self.scroll_target_up(name, scroll_step as usize); }
                        MouseEventKind::ScrollDown => { self.scroll_target_down(name, scroll_step as usize); }
                        _ => {}
                    }
                }
                _ => {}
            }
            break;
        }
    }

    /// 处理鼠标悬停获取焦点的逻辑
    fn handle_mouse_hover(
        &mut self,
        mouse_event: &MouseEvent,
        sorted_rects: &[&(WindowRect, String, BorderStyle, usize)],
        layout_changed: bool,
        columns: u16,
        rows: u16,
    ) -> bool {
        if layout_changed { return true; }
        for (rect, name, _, _) in sorted_rects.iter() {
            let in_x = mouse_event.column >= rect.start_col && mouse_event.column < rect.start_col + rect.width;
            let in_y = mouse_event.row >= rect.start_row && mouse_event.row < rect.start_row + rect.height;

            if in_x && in_y {
                // 【修改】扩展免疫防线：StatusBar 和 开启了 no_hover 的组件均不抢夺焦点
                if self.is_hover_immune(name) || self.is_unfocusable(name) { break; }

                if self.focus.current != Focus::Component(name.to_string()) {

                    let old_focus = self.focus.current.clone();
                    self.focus.current = Focus::Component(name.to_string());

                    if let Focus::Component(old_name) = &old_focus {
                        self.mark_dirty(old_name);
                    }
                    self.mark_dirty(name);

                    for (n, c) in &self.components {
                        if matches!(c, Component::StatusBar(_)) {
                            self.dirty_components.insert(n.clone());
                        }
                    }

                    if let Some(Component::Tree(t)) = self.components.get(name) {
                        if t.focus_to_fire {
                            self.emit("focus", columns, rows);
                        }
                    }
                }
                break;
            }
        }
        true
    }
}
