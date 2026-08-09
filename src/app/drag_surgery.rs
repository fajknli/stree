// src/app/drag_surgery.rs

use crate::app::{Engine, DragEdge};
use crate::layout::{LayoutNode, WindowRect, BorderStyle, WindowSize, Direction};
use std::collections::HashMap;

impl Engine {
    /// 查找两个相邻窗口在父容器中的“原始百分比总和”
    pub fn get_sibling_percent_sum(&self, name1: &str, name2: &str) -> Option<u16> {
        for layer in &self.layout_layers {
            if let Some(sum) = Self::find_sibling_sum_in_node(&layer.root, name1, name2) {
                return Some(sum);
            }
        }
        None
    }

    fn find_sibling_sum_in_node(node: &LayoutNode, id1: &str, id2: &str) -> Option<u16> {
        if let LayoutNode::Container { children, .. } = node {
            let mut sum = 0;
            let mut found_count = 0;

            for child in children {
                let child_id = match child {
                    LayoutNode::Window { name, .. } => name.as_str(),
                    LayoutNode::Container { id, .. } => id.as_str(),
                };

                if child_id == id1 || child_id == id2 {
                    let size = match child {
                        LayoutNode::Window { size, .. } => *size,
                        LayoutNode::Container { percent, .. } => percent.map(WindowSize::Percent),
                    };

                    if let Some(WindowSize::Percent(p)) = size {
                        sum += p;
                        found_count += 1;
                    } else {
                        return None;
                    }
                }
            }

            if found_count == 2 {
                return Some(sum);
            }

            for child in children {
                if let Some(sum) = Self::find_sibling_sum_in_node(child, id1, id2) {
                    return Some(sum);
                }
            }
        }
        None
    }

    pub fn rebuild_draggable_edges(&mut self, term_width: u16, term_height: u16) {
        self.drag.cached_edges.clear();

        let window_rects = self.calc_all_rects(term_width, term_height);
        let mut w_map: HashMap<String, WindowRect> = HashMap::new();
        for (rect, name, _, _) in &window_rects {
            w_map.insert(name.clone(), *rect);
        }

        for layer in &self.layout_layers {
            if !layer.visible { continue; }
            if !matches!(layer.anchor, crate::layout::Anchor::FullScreen) {
                continue;
            }
            Self::extract_edges_from_node(&layer.root, &w_map, layer.z_index, &mut self.drag.cached_edges);
        }

        self.drag.cached_intersections.clear();
        let v_edges: Vec<_> = self.drag.cached_edges.iter().filter(|e| e.direction == Direction::Horizontal).collect();
        let h_edges: Vec<_> = self.drag.cached_edges.iter().filter(|e| e.direction == Direction::Vertical).collect();
        for v in &v_edges {
            for h in &h_edges {
                let v_x = v.hit_rect.start_col + 1;
                let v_y_start = v.hit_rect.start_row;
                let v_y_end = v.hit_rect.start_row + v.hit_rect.height;

                let h_y = h.hit_rect.start_row + 1;
                let h_x_start = h.hit_rect.start_col;
                let h_x_end = h.hit_rect.start_col + h.hit_rect.width;

                if v_x >= h_x_start && v_x < h_x_end && h_y >= v_y_start && h_y < v_y_end {
                    self.drag.cached_intersections.push((v_x, h_y));
                }
            }
        }
    }

    pub fn get_node_current_bbox(&self, node_id: &str, term_width: u16, term_height: u16) -> Option<(WindowRect, BorderStyle)> {
        let window_rects = self.calc_all_rects(term_width, term_height);
        let mut w_map: HashMap<String, WindowRect> = HashMap::new();
        let mut b_map: HashMap<String, BorderStyle> = HashMap::new();
        for (rect, name, border, _) in &window_rects {
            w_map.insert(name.clone(), *rect);
            b_map.insert(name.clone(), *border);
        }

        for layer in &self.layout_layers {
            if let Some((rect, border)) = Self::search_bbox_in_node(
                &layer.root,
                node_id,
                &w_map,
                &b_map,
            ) {
                return Some((rect, border));
            }
        }
        None
    }

    fn search_bbox_in_node(
        node: &LayoutNode,
        target_id: &str,
        w_map: &HashMap<String, WindowRect>,
        b_map: &HashMap<String, BorderStyle>,
    ) -> Option<(WindowRect, BorderStyle)> {
        match node {
            LayoutNode::Window { name, .. } => {
                if name == target_id {
                    Some((w_map.get(name).copied()?, b_map.get(name).copied().unwrap_or(BorderStyle::Box)))
                } else {
                    None
                }
            }
            LayoutNode::Container { id, children, .. } => {
                if id == target_id {
                    let bbox = Self::get_node_bbox(node, w_map)?;
                    return Some((bbox, BorderStyle::None));
                }
                for child in children {
                    if let Some(res) = Self::search_bbox_in_node(child, target_id, w_map, b_map) {
                        return Some(res);
                    }
                }
                None
            }
        }
    }

    fn get_node_bbox(node: &LayoutNode, w_map: &HashMap<String, WindowRect>) -> Option<WindowRect> {
        match node {
            LayoutNode::Window { name, .. } => w_map.get(name).copied(),
            LayoutNode::Container { children, .. } => {
                let mut min_col = u16::MAX;
                let mut min_row = u16::MAX;
                let mut max_col = 0;
                let mut max_row = 0;
                let mut has_child = false;
                for child in children {
                    if let Some(r) = Self::get_node_bbox(child, w_map) {
                        has_child = true;
                        min_col = min_col.min(r.start_col);
                        min_row = min_row.min(r.start_row);
                        max_col = max_col.max(r.start_col + r.width);
                        max_row = max_row.max(r.start_row + r.height);
                    }
                }
                if has_child {
                    Some(WindowRect {
                        start_col: min_col,
                        start_row: min_row,
                        width: max_col.saturating_sub(min_col),
                        height: max_row.saturating_sub(min_row),
                    })
                } else {
                    None
                }
            }
        }
    }

    fn collect_leaf_rects(
        node: &LayoutNode,
        rects: &HashMap<String, WindowRect>,
        z_index: usize,
        out: &mut Vec<(WindowRect, String, usize)>,
    ) {
        match node {
            LayoutNode::Window { name, .. } => {
                if let Some(rect) = rects.get(name) {
                    out.push((*rect, name.clone(), z_index));
                }
            }
            LayoutNode::Container { children, .. } => {
                for child in children {
                    Self::collect_leaf_rects(child, rects, z_index, out);
                }
            }
        }
    }

    fn extract_edges_from_node(
        node: &LayoutNode,
        rects: &HashMap<String, WindowRect>,
        z_index: usize,
        edges: &mut Vec<DragEdge>
    ) {
        let mut leaf_rects = Vec::new();
        Self::collect_leaf_rects(node, rects, z_index, &mut leaf_rects);

        for i in 0..leaf_rects.len() {
            for j in i + 1..leaf_rects.len() {
                let (r1, name1, _z1) = &leaf_rects[i];
                let (r2, name2, _z2) = &leaf_rects[j];

                let horizontal_adjacent =
                    r1.start_row == r2.start_row
                    && r1.height == r2.height
                    && (r1.start_col + r1.width == r2.start_col
                        || r2.start_col + r2.width == r1.start_col);

                if horizontal_adjacent {
                    let left = if r1.start_col < r2.start_col { r1 } else { r2 };
                    let right = if r1.start_col < r2.start_col { r2 } else { r1 };
                    let left_name = if r1.start_col < r2.start_col { name1 } else { name2 };
                    let right_name = if r1.start_col < r2.start_col { name2 } else { name1 };

                    let x = left.start_col + left.width;
                    edges.push(DragEdge {
                        primary_id: left_name.clone(),
                        neighbor_id: right_name.clone(),
                        direction: Direction::Horizontal,
                        hit_rect: WindowRect {
                            start_col: x.saturating_sub(1),
                            start_row: left.start_row.max(right.start_row),
                            width: 2,
                            height: left.height.min(right.height),
                        },
                        z_index,
                    });
                }

                let vertical_adjacent =
                    r1.start_col == r2.start_col
                    && r1.width == r2.width
                    && (r1.start_row + r1.height == r2.start_row
                        || r2.start_row + r2.height == r1.start_row);

                if vertical_adjacent {
                    let top = if r1.start_row < r2.start_row { r1 } else { r2 };
                    let bottom = if r1.start_row < r2.start_row { r2 } else { r1 };
                    let top_name = if r1.start_row < r2.start_row { name1 } else { name2 };
                    let bottom_name = if r1.start_row < r2.start_row { name2 } else { name1 };

                    let y = top.start_row + top.height;
                    edges.push(DragEdge {
                        primary_id: top_name.clone(),
                        neighbor_id: bottom_name.clone(),
                        direction: Direction::Vertical,
                        hit_rect: WindowRect {
                            start_col: top.start_col.max(bottom.start_col),
                            start_row: y.saturating_sub(1),
                            width: top.width.min(bottom.width),
                            height: 2,
                        },
                        z_index,
                    });
                }
            }
        }
    }

    // 【优化】返回 &str，避免不必要的 String 分配
    fn get_node_id(node: &LayoutNode) -> &str {
        match node {
            LayoutNode::Window { name, .. } => name.as_str(),
            LayoutNode::Container { id, .. } => id.as_str(),
        }
    }

    pub fn find_parent_container(&self, node_id: &str) -> Option<String> {
        for layer in &self.layout_layers {
            if let Some(parent) = Self::search_parent_container(&layer.root, node_id) {
                return Some(parent);
            }
        }
        None
    }

    pub fn find_resize_targets(&self, leaf1: &str, leaf2: &str, drag_dir: Direction) -> Option<(String, String)> {
        for layer in &self.layout_layers {
            if let Some((t1, t2)) = Self::find_resize_targets_in_node(&layer.root, leaf1, leaf2, drag_dir) {
                return Some((t1, t2));
            }
        }
        None
    }

    fn find_resize_targets_in_node(node: &LayoutNode, leaf1: &str, leaf2: &str, drag_dir: Direction) -> Option<(String, String)> {
        if let LayoutNode::Container { direction, children, .. } = node {
            let mut c1_idx = None;
            let mut c2_idx = None;
            for (i, child) in children.iter().enumerate() {
                if Self::contains_node(child, leaf1) { c1_idx = Some(i); }
                if Self::contains_node(child, leaf2) { c2_idx = Some(i); }
            }

            match (c1_idx, c2_idx) {
                (Some(idx1), Some(idx2)) if idx1 != idx2 => {
                    if *direction == drag_dir {
                        let t1 = match &children[idx1] {
                            LayoutNode::Window { name, .. } => name.clone(),
                            LayoutNode::Container { id, .. } => id.clone(),
                        };
                        let t2 = match &children[idx2] {
                            LayoutNode::Window { name, .. } => name.clone(),
                            LayoutNode::Container { id, .. } => id.clone(),
                        };
                        return Some((t1, t2));
                    }
                    return None;
                }
                (Some(idx), Some(_)) => {
                    return Self::find_resize_targets_in_node(&children[idx], leaf1, leaf2, drag_dir);
                }
                _ => return None
            }
        }
        None
    }

    // ================= 终极魔法：运行时树重组 =================

    pub fn force_recalculate_percentages(
        &mut self,
        all_rects: &[(WindowRect, String, BorderStyle, usize)]
    ) {
        let mut rect_map: HashMap<String, WindowRect> = HashMap::new();
        for (rect, name, _, _) in all_rects {
            rect_map.insert(name.clone(), *rect);
        }

        for layer in &mut self.layout_layers {
            Self::recalc_node_percentages_recursive(&mut layer.root, &rect_map);
        }
    }

    fn recalc_node_percentages_recursive(
        node: &mut LayoutNode,
        rect_map: &HashMap<String, WindowRect>
    ) {
        if let LayoutNode::Container { direction, children, .. } = node {
            let dir_val = *direction;

            for child in children.iter_mut() {
                Self::recalc_node_percentages_recursive(child, rect_map);
            }

            if children.len() < 2 { return; }

            let mut total_flex_content = 0u16;
            let mut child_contents: Vec<u16> = Vec::with_capacity(children.len());
            let mut is_absolute_flags: Vec<bool> = Vec::with_capacity(children.len());

            for child in children.iter() {
                let phys = Self::get_node_physical_rect(child, rect_map).unwrap_or_default();

                let overhead = match child {
                    LayoutNode::Window { border, .. } => {
                        let (ox, oy) = border.overhead();
                        if dir_val == Direction::Horizontal { ox } else { oy }
                    },
                    LayoutNode::Container { .. } => 0,
                };

                let size = match dir_val {
                    Direction::Horizontal => phys.width.saturating_sub(overhead),
                    Direction::Vertical => phys.height.saturating_sub(overhead),
                };

                let is_abs = match child {
                    LayoutNode::Window { size: Some(WindowSize::Absolute(_)), .. } => true,
                    LayoutNode::Window { size: Some(WindowSize::Absolute2D(_, _)), .. } => true,
                    LayoutNode::Window { size: Some(WindowSize::Percent2D(_, _)), .. } => true,
                    LayoutNode::Window { size: Some(WindowSize::Auto(_)), .. } => true, // 【必须保留】
                    _ => false,
                };
                is_absolute_flags.push(is_abs);

                if !is_abs {
                    total_flex_content = total_flex_content.saturating_add(size);
                }
                child_contents.push(size);
            }

            for i in 0..children.len() {
                if is_absolute_flags[i] {
                    Self::set_node_percent(&mut children[i], Some(WindowSize::Absolute(child_contents[i])));
                }
            }

            if total_flex_content == 0 { return; }

            let mut sum_pct = 0u16;
            let mut flex_indices: Vec<usize> = Vec::new();
            for (i, _) in children.iter().enumerate() {
                if !is_absolute_flags[i] {
                    flex_indices.push(i);
                }
            }

            for &i in &flex_indices[..flex_indices.len().saturating_sub(1)] {
                let pct = ((child_contents[i] as u32 * 10000 + total_flex_content as u32 / 2) / total_flex_content as u32) as u16;
                Self::set_node_percent(&mut children[i], Some(WindowSize::Percent(pct)));
                sum_pct += pct;
            }

            if let Some(&last_idx) = flex_indices.last() {
                let last_pct = 10000u16.saturating_sub(sum_pct);
                Self::set_node_percent(&mut children[last_idx], Some(WindowSize::Percent(last_pct)));
            }
        }
    }

    pub fn restructure_tree_after_drag(
        &mut self,
        primary: &str,
        neighbor: &str,
        drag_dir: Direction,
        all_rects: &[(WindowRect, String, BorderStyle, usize)]
    ) -> bool {
        let mut rect_map: HashMap<String, WindowRect> = HashMap::new();
        for (rect, name, _, _) in all_rects {
            rect_map.insert(name.clone(), *rect);
        }

        for layer in &mut self.layout_layers {
            let root = &mut layer.root;
            if Self::surgery_tree_node(root, primary, neighbor, drag_dir, &rect_map) {
                return true;
            }
        }
        false
    }

    fn split_node_by_child_containing(
        node: &LayoutNode,
        target_id: &str,
    ) -> (Vec<LayoutNode>, Option<LayoutNode>, Vec<LayoutNode>) {
        if Self::is_node_id_match(node, target_id) {
            return (Vec::new(), Some(node.clone()), Vec::new());
        }
        if let LayoutNode::Container { children, .. } = node {
            if let Some(idx) = children.iter().position(|c| Self::contains_node(c, target_id)) {
                let before = children[..idx].to_vec();
                let middle = children[idx].clone();
                let after = children[idx+1..].to_vec();
                return (before, Some(middle), after);
            }
        }
        (Vec::new(), None, Vec::new())
    }

    fn surgery_tree_node(
        node: &mut LayoutNode,
        primary: &str,
        neighbor: &str,
        drag_dir: Direction,
        rect_map: &HashMap<String, WindowRect>
    ) -> bool {
        if let LayoutNode::Container { direction, children, .. } = node {
            let dir_val = *direction;

            let mut p_child_idx = None;
            let mut n_child_idx = None;
            for (i, child) in children.iter().enumerate() {
                if Self::contains_node(child, primary) { p_child_idx = Some(i); }
                if Self::contains_node(child, neighbor) { n_child_idx = Some(i); }
            }

            if let (Some(idx_p), Some(idx_n)) = (p_child_idx, n_child_idx) {
                if idx_p != idx_n {
                    let c1 = &children[idx_p];
                    let c2 = &children[idx_n];

                    if dir_val == drag_dir {
                        let p_is_direct = Self::is_node_id_match(c1, primary);
                        let n_is_direct = Self::is_node_id_match(c2, neighbor);
                        if p_is_direct && n_is_direct {
                            return false;
                        }
                    }

                    let c1_dir = match c1 {
                        LayoutNode::Container { direction, .. } => Some(*direction),
                        _ => None,
                    };
                    let c2_dir = match c2 {
                        LayoutNode::Container { direction, .. } => Some(*direction),
                        _ => None,
                    };

                    if let (Some(d1), Some(d2)) = (c1_dir, c2_dir) {
                        if d1 != d2 { return false; }
                    }

                    let (before_p, p_node_opt, after_p) = Self::split_node_by_child_containing(c1, primary);
                    let (before_n, n_node_opt, after_n) = Self::split_node_by_child_containing(c2, neighbor);

                    if let (Some(p_node), Some(n_node)) = (p_node_opt, n_node_opt) {

                        let p_pos = Self::get_node_physical_rect(&p_node, rect_map).unwrap_or_default();
                        let n_pos = Self::get_node_physical_rect(&n_node, rect_map).unwrap_or_default();

                        let before_packed = Self::project_and_pack(before_p, before_n, drag_dir, rect_map);
                        let after_packed = Self::project_and_pack(after_p, after_n, drag_dir, rect_map);

                        let is_p_first = match drag_dir {
                            Direction::Horizontal => p_pos.start_col <= n_pos.start_col,
                            Direction::Vertical => p_pos.start_row <= n_pos.start_row,
                        };
                        let new_core = LayoutNode::Container {
                            id: crate::layout::generate_container_id_pub(),
                            direction: drag_dir,
                            percent: None,
                            children: if is_p_first { vec![p_node, n_node] } else { vec![n_node, p_node] },
                        };

                        let mut sequence = Vec::new();
                        sequence.extend(before_packed);
                        sequence.push(new_core);
                        sequence.extend(after_packed);

                        let wrap_dir = c1_dir.or(c2_dir).unwrap_or(dir_val);
                        let c1_pct = match c1 {
                            LayoutNode::Container { percent, .. } => (*percent).unwrap_or(0),
                            LayoutNode::Window { size: Some(WindowSize::Percent(p)), .. } => *p,
                            _ => 0,
                        };
                        let c2_pct = match c2 {
                            LayoutNode::Container { percent, .. } => (*percent).unwrap_or(0),
                            LayoutNode::Window { size: Some(WindowSize::Percent(p)), .. } => *p,
                            _ => 0,
                        };
                        let total_pct = c1_pct.saturating_add(c2_pct);

                        let replacement_node = if sequence.len() == 1 {
                            sequence.remove(0)
                        } else {
                            LayoutNode::Container {
                                id: crate::layout::generate_container_id_pub(),
                                direction: wrap_dir,
                                percent: if total_pct > 0 { Some(total_pct) } else { None },
                                children: sequence,
                            }
                        };

                        let mut new_children: Vec<LayoutNode> = Vec::with_capacity(children.len() - 1);
                        let insert_idx = idx_p.min(idx_n);

                        // 【优化】使用 Option 包裹，用 take() 完美实现单次所有权转移
                        let mut replacement_opt = Some(replacement_node);

                        for (i, child) in children.drain(..).enumerate() {
                            if i == insert_idx {
                                if let Some(rep) = replacement_opt.take() {
                                    new_children.push(rep);
                                }
                            }
                            if i != idx_p && i != idx_n {
                                new_children.push(child);
                            }
                        }

                        *children = new_children;
                        return true;
                    }
                }
            }

            for child in children.iter_mut() {
                if Self::surgery_tree_node(child, primary, neighbor, drag_dir, rect_map) {
                    return true;
                }
            }
        }
        false
    }

    fn project_and_pack(
        list_p: Vec<LayoutNode>,
        list_n: Vec<LayoutNode>,
        drag_dir: Direction,
        _rect_map: &HashMap<String, WindowRect>
    ) -> Vec<LayoutNode> {
        let mut result_containers: Vec<LayoutNode> = Vec::new();
        if list_p.is_empty() && list_n.is_empty() { return result_containers; }

        let max_len = list_p.len().max(list_n.len());
        for i in 0..max_len {
            let p_clone = list_p.get(i).cloned();
            let n_clone = list_n.get(i).cloned();

            if let Some(c) = Self::combine_packed(p_clone, n_clone, drag_dir) {
                result_containers.push(c);
            }
        }
        result_containers
    }

    fn combine_packed(
        p: Option<LayoutNode>,
        n: Option<LayoutNode>,
        dir: Direction
    ) -> Option<LayoutNode> {
        match (p, n) {
            (None, None) => None,
            (Some(x), None) => Some(x),
            (None, Some(y)) => Some(y),
            (Some(x), Some(y)) => Some(LayoutNode::Container {
                id: crate::layout::generate_container_id_pub(),
                direction: dir,
                percent: None,
                children: vec![x, y],
            }),
        }
    }

    fn is_node_id_match(node: &LayoutNode, id: &str) -> bool {
        match node {
            LayoutNode::Window { name, .. } => name == id,
            LayoutNode::Container { id: node_id, .. } => node_id == id,
        }
    }

    fn get_node_physical_rect(
        node: &LayoutNode,
        rect_map: &HashMap<String, WindowRect>
    ) -> Option<WindowRect> {
        match node {
            LayoutNode::Window { name, .. } => rect_map.get(name).copied(),
            LayoutNode::Container { children, .. } => {
                let mut min_col = u16::MAX; let mut min_row = u16::MAX;
                let mut max_col = 0u16; let mut max_row = 0u16;
                let mut found = false;
                for child in children {
                    if let Some(r) = Self::get_node_physical_rect(child, rect_map) {
                        found = true;
                        min_col = min_col.min(r.start_col);
                        min_row = min_row.min(r.start_row);
                        max_col = max_col.max(r.start_col + r.width);
                        max_row = max_row.max(r.start_row + r.height);
                    }
                }
                if found {
                    Some(WindowRect { start_col: min_col, start_row: min_row, width: max_col - min_col, height: max_row - min_row })
                } else { None }
            }
        }
    }

    fn set_node_percent(node: &mut LayoutNode, pct: Option<WindowSize>) {
        match node {
            LayoutNode::Window { size, .. } => *size = pct,
            LayoutNode::Container { percent, .. } => {
                *percent = match pct {
                    Some(WindowSize::Percent(p)) => Some(p),
                    _ => None,
                };
            }
        }
    }

    fn contains_node(node: &LayoutNode, target_id: &str) -> bool {
        match node {
            LayoutNode::Window { name, .. } => name == target_id,
            LayoutNode::Container { children, .. } => children.iter().any(|c| Self::contains_node(c, target_id)),
        }
    }

    fn search_parent_container(node: &LayoutNode, target_id: &str) -> Option<String> {
        match node {
            LayoutNode::Container { id, children, .. } => {
                for child in children {
                    let child_id = match child {
                        LayoutNode::Window { name, .. } => name.as_str(),
                        LayoutNode::Container { id, .. } => id.as_str(),
                    };
                    if child_id == target_id {
                        return Some(id.clone());
                    }
                }
                for child in children {
                    if let Some(parent) = Self::search_parent_container(child, target_id) {
                        return Some(parent);
                    }
                }
                None
            }
            _ => None,
        }
    }
    // 【新增】统一的浮动窗口边缘检测辅助函数
    pub fn check_floating_edge_hit(
        &self,
        mouse_col: u16,
        mouse_row: u16,
        all_rects: &[(WindowRect, String, BorderStyle, usize)]
    ) -> (Option<(String, u8, u16, u16, u16, u16)>, bool) {
        let mut hit_inside_any = false;
        for layer in &self.layout_layers {
            if !layer.visible || !matches!(layer.anchor, crate::layout::Anchor::ScreenAbsolute {..}) { continue; }

            if let Some(res) = Self::check_node_edge_recursive(&layer.root, mouse_col, mouse_row, all_rects) {
                // 如果点中了边缘，直接返回
                if res.0.is_some() {
                    return (res.0, true);
                }
                // 如果只是点在了内部，记录下来
                if res.1 {
                    hit_inside_any = true;
                }
            }
        }
        (None, hit_inside_any)
    }

    fn check_node_edge_recursive(
        node: &LayoutNode,
        col: u16, row: u16,
        all_rects: &[(WindowRect, String, BorderStyle, usize)]
    ) -> Option<(Option<(String, u8, u16, u16, u16, u16)>, bool)> {
        match node {
            LayoutNode::Window { name, .. } => {
                let rect = all_rects.iter().find(|(_, n, _, _)| n == name)?.0;
                let in_x = col >= rect.start_col && col < rect.start_col + rect.width;
                let in_y = row >= rect.start_row && row < rect.start_row + rect.height;
                if in_x && in_y {
                    let mut mask = 0u8;
                    if col == rect.start_col { mask |= 1; }
                    // 【修复】防止 width/height 为 0 时减法下溢导致引擎崩溃
                    if rect.width > 0 && col == rect.start_col + rect.width - 1 { mask |= 2; }
                    if row == rect.start_row { mask |= 4; }
                    if rect.height > 0 && row == rect.start_row + rect.height - 1 { mask |= 8; }
                    let edge_data = if mask != 0 {
                        Some((name.clone(), mask, rect.start_col, rect.start_row, rect.width, rect.height))
                    } else { None };
                    return Some((edge_data, true));
                }
                None
            }
            LayoutNode::Container { children, .. } => {
                let mut found_inside = false;
                for c in children {
                    if let Some(res) = Self::check_node_edge_recursive(c, col, row, all_rects) {
                        if res.0.is_some() {
                            return Some(res);
                        }
                        if res.1 { found_inside = true; }
                    }
                }
                if found_inside { return Some((None, true)); }
                None
            }
        }
    }
}
