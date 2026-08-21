use crate::app::{Engine, DragEdge};
use std::collections::HashMap;
use crate::layout::{LayoutNode, LayoutLayer, WindowRect, BorderStyle, WindowSize, Direction};

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

    // 【修改】收集叶子节点时，同时记录 draggable 标志
    fn collect_leaf_rects(
        node: &LayoutNode,
        rects: &HashMap<String, WindowRect>,
        z_index: usize,
        out: &mut Vec<(WindowRect, String, usize, bool)>,
    ) {
        match node {
            LayoutNode::Window { name, draggable, .. } => {
                if let Some(rect) = rects.get(name) {
                    out.push((*rect, name.clone(), z_index, *draggable));
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
                let (r1, name1, _z1, drag1) = &leaf_rects[i];
                let (r2, name2, _z2, drag2) = &leaf_rects[j];

                // 【新增绝对防线】只有双方都显式声明了 [drag]，它们之间的边框才可拖拽！
                if !(*drag1 && *drag2) { continue; }

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
    // 【新增辅助函数】收集节点下的所有窗口矩形，用于遮挡判定
    fn collect_node_window_rects(
        node: &LayoutNode,
        all_rects: &[(WindowRect, String, BorderStyle, usize)]
    ) -> Vec<WindowRect> {
        let mut rects = Vec::new();
        match node {
            LayoutNode::Window { name, .. } => {
                if let Some((r, _, _, _)) = all_rects.iter().find(|(_, n, _, _)| n == name) {
                    rects.push(*r);
                }
            }
            LayoutNode::Container { children, .. } => {
                for child in children {
                    rects.extend(Self::collect_node_window_rects(child, all_rects));
                }
            }
        }
        rects
    }

    // 【重写】统一的浮动窗口边缘检测辅助函数，加入遮挡剔除
    pub fn check_floating_edge_hit(
        &self,
        mouse_col: u16,
        mouse_row: u16,
        all_rects: &[(WindowRect, String, BorderStyle, usize)]
    ) -> (Option<(String, u8, u16, u16, u16, u16)>, bool) {
        let mut hit_inside_any = false;

        // 1. 提取可见的浮动图层
        let mut visible_float_layers: Vec<&LayoutLayer> = self.layout_layers.iter()
            .filter(|l| l.visible && matches!(l.anchor, crate::layout::Anchor::ScreenAbsolute {..}))
            .collect();

        // 2. 按 z_index 倒序排序，模拟渲染覆盖关系（高层在前）
        visible_float_layers.sort_by(|a, b| b.z_index.cmp(&a.z_index));

        let mut covered_rects: Vec<WindowRect> = Vec::new();

        for layer in visible_float_layers {
            if let Some(res) = Self::check_node_edge_recursive(&layer.root, mouse_col, mouse_row, all_rects) {
                if res.1 {
                    hit_inside_any = true;

                    // 【核心修复】碰撞检测：判断鼠标点是否被更高层级的窗口覆盖
                    let is_covered = covered_rects.iter().any(|r| {
                        mouse_col >= r.start_col && mouse_col < r.start_col + r.width &&
                        mouse_row >= r.start_row && mouse_row < r.start_row + r.height
                    });

                    if !is_covered {
                        // 只有未被遮挡的边缘，才允许触发边缘拖拽
                        if res.0.is_some() {
                            return (res.0, true);
                        }
                    }
                }
                // 收集当前层级的所有窗口矩形，作为更低层级判定遮挡的参考
                covered_rects.extend(Self::collect_node_window_rects(&layer.root, all_rects));
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
            LayoutNode::Window { name, draggable, .. } => {
                let rect = all_rects.iter().find(|(_, n, _, _)| n == name)?.0;
                let in_x = col >= rect.start_col && col < rect.start_col + rect.width;
                let in_y = row >= rect.start_row && row < rect.start_row + rect.height;
                if in_x && in_y {
                    let mut mask = 0u8;

                    // 【新增绝对防线】只有声明了 [drag] 的浮动窗口，才计算边缘拉伸掩码
                    if *draggable {
                        if col == rect.start_col { mask |= 1; }
                        // 【修复】防止 width/height 为 0 时减法下溢导致引擎崩溃
                        if rect.width > 0 && col == rect.start_col + rect.width - 1 { mask |= 2; }
                        if row == rect.start_row { mask |= 4; }
                        if rect.height > 0 && row == rect.start_row + rect.height - 1 { mask |= 8; }
                    }

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
    /// 处理当前帧的拖拽逻辑，注入 Absolute 覆盖，让布局引擎自然计算
    /// 【重构提取】将 main.rs 中的物理篡改与 AST 重组逻辑收束回引擎内部
    pub fn process_drag_frame(
        &mut self,
        all_rects: &mut Vec<(WindowRect, String, BorderStyle, usize)>,
        columns: u16,
        rows: u16,
    ) {
        if self.drag.active {
            // 【终极架构】拖拽时注入 Absolute 覆盖，让布局引擎自然计算，彻底消灭 AST 突变与布局偏移
            if let Some(crate::app::DragTarget::ResizeFloating(layer_name, edge_mask)) = self.drag.resize_target.clone() {
                let dx = self.drag.last_col as i32 - self.drag.start_col as i32;
                let dy = self.drag.last_row as i32 - self.drag.start_row as i32;

                let mut new_x = self.drag.initial_anchor_x as i32;
                let mut new_y = self.drag.initial_anchor_y as i32;
                let mut new_w = self.drag.initial_width as i32;
                let mut new_h = self.drag.initial_height as i32;

                // 1. 根据掩码应用原始位移
                if edge_mask & 1 != 0 { // Left
                    new_x += dx;
                    new_w -= dx;
                }
                if edge_mask & 2 != 0 { // Right
                    new_w += dx;
                }
                if edge_mask & 4 != 0 { // Top
                    new_y += dy;
                    new_h -= dy;
                }
                if edge_mask & 8 != 0 { // Bottom
                    new_h += dy;
                }

                // 2. 最小尺寸限制（保证对侧边缘绝对不动！）
                const MIN_W: i32 = 2;
                const MIN_H: i32 = 2;
                if new_w < MIN_W {
                    // 如果是左侧收缩到极限，必须反向修正 x，保持右边缘不动
                    if edge_mask & 1 != 0 {
                        new_x = self.drag.initial_anchor_x as i32 + (self.drag.initial_width as i32 - MIN_W);
                    }
                    new_w = MIN_W;
                }
                if new_h < MIN_H {
                    // 如果是顶部收缩到极限，必须反向修正 y，保持下边缘不动
                    if edge_mask & 4 != 0 {
                        new_y = self.drag.initial_anchor_y as i32 + (self.drag.initial_height as i32 - MIN_H);
                    }
                    new_h = MIN_H;
                }

                // 3. 屏幕边界限制（同样保证对侧边缘不动！）
                let term_w = columns as i32;
                let term_h = rows as i32;
                if new_x < 0 {
                    // 如果左侧碰到了左边界，必须加宽，保持右边缘不动
                    if edge_mask & 1 != 0 {
                        new_w += new_x; // new_x 是负数，相当于加宽
                    }
                    new_x = 0;
                }
                if new_y < 0 {
                    // 如果顶部碰到了上边界，必须加高，保持下边缘不动
                    if edge_mask & 4 != 0 {
                        new_h += new_y;
                    }
                    new_y = 0;
                }
                if new_x + new_w > term_w {
                    // 右侧超出屏幕，直接截断宽度
                    new_w = term_w - new_x;
                }
                if new_y + new_h > term_h {
                    // 底部超出屏幕，直接截断高度
                    new_h = term_h - new_y;
                }

                let final_x = new_x as u16;
                let final_y = new_y as u16;
                let final_w = new_w as u16;
                let final_h = new_h as u16;

                // 【关键修复】必须同时注入 window_rect_overrides！
                self.window_rect_overrides.insert(
                    layer_name.clone(),
                    crate::layout::WindowSize::Absolute2D(final_w, final_h)
                );

                // 更新画布尺寸（维持锚点位置）
                for layer in &mut self.layout_layers {
                    if !matches!(layer.anchor, crate::layout::Anchor::ScreenAbsolute {..}) { continue; }
                    if Self::layout_contains_window(layer, &layer_name) {
                        layer.runtime_rect_override = Some(crate::layout::WindowRect {
                            start_col: final_x,
                            start_row: final_y,
                            width: final_w,
                            height: final_h,
                        });
                        break;
                    }
                }

                *all_rects = self.calc_all_rects(columns, rows);
                self.mark_all_dirty();

            } else if let Some(crate::app::DragTarget::ResizeEdge(primary, neighbor, dir)) = self.drag.resize_target.clone() {
                let has_moved = self.drag.last_col != self.drag.start_col
                    || self.drag.last_row != self.drag.start_row;

                // 【核心修复】只在首次真正拖拽时重组 AST，并立刻冻结物理像素
                if !self.drag.is_restructured && has_moved {
                    // 1. 重组 AST（改变拓扑结构，将叶子拉平为兄弟）
                    self.restructure_tree_after_drag(&primary, &neighbor, dir, all_rects);
                    // 2. 立刻用旧物理坐标反算新 AST 百分比，杜绝拓扑突变带来的视觉跳跃
                    self.force_recalculate_percentages(all_rects);
                    self.drag.is_restructured = true;
                    // 3. AST 变了，必须重算物理真相
                    *all_rects = self.calc_all_rects(columns, rows);
                }

                // 只有重组完成后，才进行物理坐标篡改
                if self.drag.is_restructured {
                    let r1 = self.drag.initial_t1_rect;
                    let r2 = self.drag.initial_t2_rect;

                    let oh1 = all_rects.iter().find(|(_, n, _, _)| n == &primary)
                        .map(|(_, _, b, _)| {
                            let (ox, oy) = b.overhead();
                            if dir == crate::layout::Direction::Horizontal { ox } else { oy }
                        }).unwrap_or(0);

                    let oh2 = all_rects.iter().find(|(_, n, _, _)| n == &neighbor)
                        .map(|(_, _, b, _)| {
                            let (ox, oy) = b.overhead();
                            if dir == crate::layout::Direction::Horizontal { ox } else { oy }
                        }).unwrap_or(0);

                    match dir {
                        crate::layout::Direction::Horizontal => {
                            let min_split = r1.start_col.saturating_add(oh1.max(1));
                            let max_split = r2.start_col.saturating_add(r2.width).saturating_sub(oh2.max(1));
                            if min_split < max_split {
                                let split = self.drag.last_col.clamp(min_split, max_split);
                                let new_w1 = split - r1.start_col;
                                let new_w2 = (r2.start_col + r2.width) - split;

                                self.window_rect_overrides.insert(primary.clone(), crate::layout::WindowSize::Absolute(new_w1.saturating_sub(oh1)));
                                self.window_rect_overrides.insert(neighbor.clone(), crate::layout::WindowSize::Absolute(new_w2.saturating_sub(oh2)));
                            }
                        }
                        crate::layout::Direction::Vertical => {
                            let min_split = r1.start_row.saturating_add(oh1.max(1));
                            let max_split = r2.start_row.saturating_add(r2.height).saturating_sub(oh2.max(1));
                            if min_split < max_split {
                                let split = self.drag.last_row.clamp(min_split, max_split);
                                let new_h1 = split - r1.start_row;
                                let new_h2 = (r2.start_row + r2.height) - split;

                                self.window_rect_overrides.insert(primary.clone(), crate::layout::WindowSize::Absolute(new_h1.saturating_sub(oh1)));
                                self.window_rect_overrides.insert(neighbor.clone(), crate::layout::WindowSize::Absolute(new_h2.saturating_sub(oh2)));
                            }
                        }
                    }
                    // 覆盖注入后，必须重算 all_rects 才能拿到正确的物理坐标供渲染使用
                    *all_rects = self.calc_all_rects(columns, rows);
                }
            }
        }
    }
}
