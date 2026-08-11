// src/app/event_handler.rs
use crate::app::{Component, Engine, Focus, InternalCommand};
use crate::layout::{WindowRect, BorderStyle};
use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind, MouseButton};

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

        let active_scope = self.overlay_stack.last().map(|l| l.source.as_str());
        let binding_opt = self.prepare_key_binding_args(active_scope, key, columns, rows);

        if let Some((full_cmd_args, is_silent)) = binding_opt {
            if let Some(internal_cmd) = InternalCommand::from_args(&full_cmd_args) {
                match internal_cmd {
                    InternalCommand::Exit => return true, // 通知 main_loop 退出
                    InternalCommand::Esc => {
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
                                match result {
                                    crate::app::input::InputKeyResult::Submitted(text) => {
                                        let is_search = self.components.get(&input_name)
                                            .map(|c| matches!(c, Component::Input(i) if i.is_search))
                                            .unwrap_or(false);
                                        if is_search {
                                            self.apply_search(&text, columns, rows);
                                        } else {
                                            self.submit_input(&input_name, &text, columns, rows);
                                        }
                                        self.close_overlay(&input_name);
                                    }
                                    crate::app::input::InputKeyResult::Cancelled => {
                                        self.close_overlay(&input_name);
                                        let is_search = self.components.get(&input_name)
                                            .map(|c| matches!(c, Component::Input(i) if i.is_search))
                                            .unwrap_or(false);
                                        if is_search {
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
                                        let is_search = self.components.get(&input_name)
                                            .map(|c| matches!(c, Component::Input(i) if i.is_search))
                                            .unwrap_or(false);
                                        if is_search {
                                            if let Some(buffer) = self.components.get(&input_name).map(|c| if let Component::Input(i) = c { i.buffer.clone() } else { String::new() }) {
                                                self.apply_search(&buffer, columns, rows);
                                            }
                                        }
                                    }
                                }
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
                }
            } else {
                crate::runner::execute_binding(self, &full_cmd_args, is_silent, columns, rows);
            }
        } else {
            // 未查到绑定的按键
            if let Some(_layer) = self.overlay_stack.last().cloned() {
                if let Some((input_name, result)) = self.handle_input_key(*key) {
                    match result {
                        crate::app::input::InputKeyResult::Cancelled => {
                            self.close_overlay(&input_name);
                            let is_search = self.components.get(&input_name)
                                .map(|c| matches!(c, Component::Input(i) if i.is_search))
                                .unwrap_or(false);
                            if is_search {
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
                        crate::app::input::InputKeyResult::Submitted(text) => {
                            let is_search = self.components.get(&input_name)
                                .map(|c| matches!(c, Component::Input(i) if i.is_search))
                                .unwrap_or(false);
                            if is_search {
                                self.apply_search(&text, columns, rows);
                            } else {
                                self.submit_input(&input_name, &text, columns, rows);
                            }
                            self.close_overlay(&input_name);
                        }
                        crate::app::input::InputKeyResult::Updated => {
                            let is_search = self.components.get(&input_name)
                                .map(|c| matches!(c, Component::Input(i) if i.is_search))
                                .unwrap_or(false);
                            if is_search {
                                if let Some(buffer) = self.components.get(&input_name).map(|c| if let Component::Input(i) = c { i.buffer.clone() } else { String::new() }) {
                                    self.apply_search(&buffer, columns, rows);
                                }
                            }
                        }
                    }
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
            if layout_changed { return; }
            for (rect, name, _, _) in sorted_rects.iter() {
                let in_x = mouse_event.column >= rect.start_col && mouse_event.column < rect.start_col + rect.width;
                let in_y = mouse_event.row >= rect.start_row && mouse_event.row < rect.start_row + rect.height;

                if in_x && in_y {
                    if let Some(Component::StatusBar(_)) = self.components.get(name) { break; }

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
            return;
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

        // 3. 拖拽中实时记录坐标
        if self.drag.active && self.drag.resize_target.is_some() {
            if let MouseEventKind::Drag(MouseButton::Left) = mouse_event.kind {
                self.drag.last_col = mouse_event.column;
                self.drag.last_row = mouse_event.row;
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
                        MouseEventKind::ScrollUp => { self.move_up_n(scroll_step as usize); }
                        MouseEventKind::ScrollDown => { self.move_down_n(scroll_step as usize); }
                        _ => {}
                    }
                }
                _ => {}
            }
            break;
        }
    }
}
