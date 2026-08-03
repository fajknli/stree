// src/layout/mod.rs

use std::sync::atomic::{AtomicUsize, Ordering};

// 全局计数器，每次调用 generate_container_id() 都会自动 +1
static CONTAINER_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn generate_container_id_pub() -> String {
    let id = CONTAINER_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("__c{}", id)
}

/// 坐标类型：区分绝对像素和百分比
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Coord {
    Pixels(u16),
    Percent(u16),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderStyle {
    Box,
    Line,
    None,
}

impl BorderStyle {
    /// 返回 (x_overhead, y_overhead)
    pub fn overhead(&self) -> (u16, u16) {
        match self {
            BorderStyle::Box => (2, 2),
            BorderStyle::Line => (0, 1),
            BorderStyle::None => (0, 0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowSize {
    Percent(u16), // 【升级】万分比精度：0 = 0.00%, 10000 = 100.00%
    Absolute(u16),
}

#[derive(Debug, Clone)]
pub enum LayoutNode {
    Window {
        name: String,
        size: Option<WindowSize>,
        border: BorderStyle,
        border_chars: Option<String>,
        draggable: bool,
    },
    Container {
        id: String,
        direction: Direction,
        percent: Option<u16>,
        children: Vec<LayoutNode>,
    },
}

/// 锚点类型：决定该图层的根 Rect 如何计算
#[derive(Debug, Clone)]
pub enum Anchor {
    FullScreen,
    ScreenAbsolute { x: Coord, y: Coord },
}

#[derive(Debug, Clone)]
pub struct LayoutLayer {
    pub name: Option<String>,
    pub z_index: usize,
    pub anchor: Anchor,
    pub root: LayoutNode,
    pub visible: bool,
    pub runtime_rect_override: Option<WindowRect>,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub layers: Vec<LayoutLayer>,
}

/// 解析百分比字符串为万分比 u16 (0-10000)
fn parse_percent_to_mille(s: &str) -> Option<u16> {
    if let Some(dot) = s.find('.') {
        let int_part = s[..dot].parse::<u16>().unwrap_or(0);
        let frac_part = &s[dot+1..];
        // 取前两位小数，不足补0
        let mut chars = frac_part.chars();
        let c1 = chars.next().unwrap_or('0');
        let c2 = chars.next().unwrap_or('0');
        let frac_str = format!("{}{}", c1, c2);
        let frac = frac_str.parse::<u16>().unwrap_or(0);
        Some((int_part * 100 + frac).min(10000))
    } else {
        s.parse::<u16>().ok().map(|v| (v * 100).min(10000))
    }
}

impl Layout {
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() {
            return default_layout();
        }
        if let Some(layer) = parse_anchored_layer(s, 0) {
            return Layout { layers: vec![layer] };
        }
        match parse_node(s) {
            Some(node) => Layout {
                layers: vec![LayoutLayer {
                    name: None,
                    z_index: 0,
                    anchor: Anchor::FullScreen,
                    root: node,
                    visible: true,
                    runtime_rect_override: None,
                }],
            },
            None => {
                eprintln!("[WARN] 布局字符串无效，使用默认布局: {}", s);
                default_layout()
            }
        }
    }
}

/// 解析多个 --layout 参数，合并为多图层
pub fn parse_layouts(layout_args: &[String]) -> Layout {
    if layout_args.is_empty() {
        return default_layout();
    }
    let mut layers = Vec::new();
    for (i, arg) in layout_args.iter().enumerate() {
        let s = arg.trim();
        if s.is_empty() { continue; }
        if let Some(mut layer) = parse_anchored_layer(s, i) {
            layer.z_index = i;
            layers.push(layer);
        } else if let Some(node) = parse_node(s) {
            layers.push(LayoutLayer {
                name: None,
                z_index: i,
                anchor: Anchor::FullScreen,
                root: node,
                visible: true,
                runtime_rect_override: None,
            });
        } else {
            eprintln!("[WARN] 布局字符串无效，已跳过: {}", s);
        }
    }
    if layers.is_empty() {
        default_layout()
    } else {
        Layout { layers }
    }
}

/// 解析 @(x,y) area(...):Name 格式的浮动布局
fn parse_anchored_layer(s: &str, z_index: usize) -> Option<LayoutLayer> {
    let s = s.trim();
    if !s.starts_with("@(") {
        return None;
    }
    let close_paren = s.find(')')?;
    let coords_str = &s[2..close_paren];
    let rest = s[close_paren + 1..].trim();

    let parts: Vec<&str> = coords_str.splitn(2, ',').collect();
    if parts.len() != 2 {
        eprintln!("[WARN] @() 坐标格式无效，期望 @(x,y): {}", s);
        return None;
    }

    let x = parse_coord_value(parts[0].trim())?;
    let y = parse_coord_value(parts[1].trim())?;

    let node = parse_node(rest)?;

    Some(LayoutLayer {
        name: None,
        z_index,
        anchor: Anchor::ScreenAbsolute { x, y },
        root: node,
        visible: true,
        runtime_rect_override: None,
    })
}

fn parse_coord_value(s: &str) -> Option<Coord> {
    if let Some(pct_str) = s.strip_suffix('%') {
        pct_str.parse::<u16>().ok().map(|v| Coord::Percent(v.min(100)))
    } else {
        s.parse::<u16>().ok().map(Coord::Pixels)
    }
}

fn has_top_level_comma(s: &str) -> bool {
    let mut depth_paren = 0;
    let mut depth_bracket = 0;
    for c in s.chars() {
        match c {
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            ',' if depth_paren == 0 && depth_bracket == 0 => return true,
            _ => {}
        }
    }
    false
}

fn parse_node(s: &str) -> Option<LayoutNode> {
    let s = s.trim();
    if s.starts_with("horizontal(") {
        parse_container(s, Direction::Horizontal)
    } else if s.starts_with("vertical(") {
        parse_container(s, Direction::Vertical)
    } else if has_top_level_comma(s) {
        let children = parse_children(s)?;
        if children.len() > 1 {
            Some(LayoutNode::Container {
                id: generate_container_id_pub(),
                direction: Direction::Horizontal,
                percent: None,
                children,
            })
        } else {
            parse_window(s)
        }
    } else {
        parse_window(s)
    }
}

fn parse_container(s: &str, dir: Direction) -> Option<LayoutNode> {
    let open_paren = s.find('(')?;
    let close_paren = s.rfind(')')?;
    if close_paren <= open_paren {
        return None;
    }

    let inside = &s[open_paren + 1..close_paren].trim();

    let (percent, children_str): (Option<u16>, &str) = {
        let mut depth_paren = 0;
        let mut depth_bracket = 0;
        let mut first_comma = None;
        for (i, c) in inside.char_indices() {
            match c {
                '(' => depth_paren += 1,
                ')' => depth_paren -= 1,
                '[' => depth_bracket += 1,
                ']' => depth_bracket -= 1,
                ',' if depth_paren == 0 && depth_bracket == 0 => {
                    first_comma = Some(i);
                    break;
                }
                _ => {}
            }
        }

        if let Some(comma_pos) = first_comma {
            let potential_pct = inside[..comma_pos].trim();
            if let Some(p_str) = potential_pct.strip_suffix('%') {
                // 【升级】支持万分比
                let p = parse_percent_to_mille(p_str);
                (p, &inside[comma_pos + 1..])
            } else {
                (None, inside)
            }
        } else {
            (None, inside)
        }
    };

    let children = parse_children(children_str)?;

    if children.is_empty() {
        return None;
    }

    Some(LayoutNode::Container {
        id: generate_container_id_pub(),
        direction: dir,
        percent,
        children,
    })
}

fn parse_children(s: &str) -> Option<Vec<LayoutNode>> {
    let mut children = Vec::new();
    let mut depth_paren = 0;
    let mut depth_bracket = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            ',' if depth_paren == 0 && depth_bracket == 0 => {
                if let Some(child) = parse_node(&s[start..i]) {
                    children.push(child);
                }
                start = i + 1;
            }
            _ => {}
        }
    }

    if start < s.len() {
        if let Some(child) = parse_node(&s[start..]) {
            children.push(child);
        }
    }

    if children.is_empty() {
        None
    } else {
        Some(children)
    }
}

fn parse_window(s: &str) -> Option<LayoutNode> {
    let s = s.trim();

    if !s.starts_with("area") {
        eprintln!("[WARN] 未知窗口类型，期望 'area': {}", s);
        return None;
    }

    let rest = &s[4..];

    let (rest_after_size, size) = if rest.starts_with('(') {
        let close = rest.find(')')?;
        let size_str = rest[1..close].trim();
        let sz = if let Some(p_str) = size_str.strip_suffix('%') {
            // 【升级】支持万分比
            Some(WindowSize::Percent(parse_percent_to_mille(p_str)?))
        } else if size_str.is_empty() {
            None
        } else {
            Some(WindowSize::Absolute(size_str.parse::<u16>().ok()?))
        };
        (&rest[close + 1..], sz)
    } else {
        (rest, None)
    };

    let (rest_after_border, border, draggable) = if rest_after_size.starts_with('[') {
        let close = rest_after_size.find(']')?;
        let b_str = rest_after_size[1..close].trim();
        let parts: Vec<&str> = b_str.split(',').map(|s| s.trim()).collect();
        let drag = parts.contains(&"drag");
        let b = if parts.contains(&"line") {
            BorderStyle::Line
        } else if parts.contains(&"none") {
            BorderStyle::None
        } else {
            BorderStyle::Box
        };
        (&rest_after_size[close + 1..], b, drag)
    } else {
        (rest_after_size, BorderStyle::Box, false)
    };

    let name = if let Some(colon_pos) = rest_after_border.find(':') {
        rest_after_border[colon_pos + 1..].trim().to_string()
    } else {
        format!("area_{}", match &size {
            Some(WindowSize::Percent(p)) => format!("{}pct", p),
            Some(WindowSize::Absolute(n)) => format!("{}abs", n),
            None => "unnamed".to_string(),
        })
    };

    if name.is_empty() {
        eprintln!("[WARN] 窗口名称为空: {}", s);
        return None;
    }

    Some(LayoutNode::Window { name, size, border, border_chars: None, draggable })
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowRect {
    pub start_col: u16,
    pub start_row: u16,
    pub width: u16,
    pub height: u16,
}

pub fn calc_window_rects(
    layers: &[LayoutLayer],
    term_width: u16,
    term_height: u16,
    overrides: &std::collections::HashMap<String, WindowSize>,
) -> Vec<(WindowRect, String, BorderStyle, usize)> {
    let mut all_rects = Vec::new();

    for layer in layers {
        if !layer.visible { continue; }
        let canvas = match &layer.anchor {
            Anchor::FullScreen => WindowRect {
                start_col: 0,
                start_row: 0,
                width: term_width,
                height: term_height,
            },
            Anchor::ScreenAbsolute { x, y } => {
                let actual_x = match x {
                    Coord::Pixels(p) => *p,
                    Coord::Percent(p) => (*p as u32 * term_width as u32 / 100) as u16,
                };
                let actual_y = match y {
                    Coord::Pixels(p) => *p,
                    Coord::Percent(p) => (*p as u32 * term_height as u32 / 100) as u16,
                };

                WindowRect {
                    start_col: actual_x.min(term_width.saturating_sub(1)),
                    start_row: actual_y.min(term_height.saturating_sub(1)),
                    width: term_width.saturating_sub(actual_x),
                    height: term_height.saturating_sub(actual_y),
                }
            }
        };

        let mut layer_rects = Vec::new();
        compute_rects(&layer.root, canvas, &mut layer_rects, overrides);

        for (rect, name, border) in layer_rects {
            all_rects.push((rect, name, border, layer.z_index));
        }
    }

    all_rects
}

// 纯粹的 Flexbox 空间分配算法（最大余数法）
fn compute_rects(
    node: &LayoutNode,
    rect: WindowRect,
    rects: &mut Vec<(WindowRect, String, BorderStyle)>,
    overrides: &std::collections::HashMap<String, WindowSize>,
) {
    match node {
        LayoutNode::Window { name, border, .. } => {
            rects.push((rect, name.clone(), *border));
        }
        LayoutNode::Container { direction, children, .. } => {
            // 【视觉扁平化】：单子节点直接穿透，无视方向差异！
            if children.len() == 1 {
                if let LayoutNode::Container { .. } = &children[0] {
                    compute_rects(&children[0], rect, rects, overrides);
                    return;
                }
            }

            let total_len = match direction {
                Direction::Horizontal => rect.width,
                Direction::Vertical => rect.height,
            };

            let mut total_border_overhead: u16 = 0;
            let mut absolute_content_len: u16 = 0;
            let mut declared_pct_sum: u16 = 0;
            let mut undeclared_count: u16 = 0;

            for child in children.iter() {
                let border_extra = match child {
                    LayoutNode::Window { border: BorderStyle::Box, .. } => 2,
                    LayoutNode::Window { border: BorderStyle::Line, .. } => 1,
                    _ => 0,
                };
                total_border_overhead = total_border_overhead.saturating_add(border_extra);

                let child_size = match child {
                    LayoutNode::Window { name, size, .. } => overrides.get(name).copied().or(*size),
                    LayoutNode::Container { id, percent, .. } => overrides.get(id).copied().or(percent.map(WindowSize::Percent)),
                };

                match child_size {
                    Some(WindowSize::Absolute(n)) => {
                        absolute_content_len = absolute_content_len.saturating_add(n);
                    }
                    Some(WindowSize::Percent(p)) => {
                        declared_pct_sum = declared_pct_sum.saturating_add(p);
                    }
                    None => {
                        undeclared_count += 1;
                    }
                }
            }

            let available_for_content = total_len.saturating_sub(total_border_overhead);
            let flex_len = available_for_content.saturating_sub(absolute_content_len);

            let mut content_sizes: Vec<u16> = Vec::with_capacity(children.len());

            // 【修复】万分比基数：10000
            let pct_base = if undeclared_count > 0 && declared_pct_sum <= 10000 {
                10000
            } else {
                declared_pct_sum.max(1)
            };

            // ================ Phase 1: 基础整数分配 ================
            for child in children.iter() {
                let child_size = match child {
                    LayoutNode::Window { name, size, .. } => overrides.get(name).copied().or(*size),
                    LayoutNode::Container { id, percent, .. } => overrides.get(id).copied().or(percent.map(WindowSize::Percent)),
                };

                let s = match child_size {
                    Some(WindowSize::Absolute(n)) => n,
                    Some(WindowSize::Percent(p)) => {
                        if pct_base == 0 { 0 } else {
                            (flex_len as u32 * p as u32 / pct_base as u32) as u16
                        }
                    }
                    None => 0,
                };
                content_sizes.push(s);
            }

            // ================ Phase 2: 为 undeclared 节点分配公平份额 ================
            if undeclared_count > 0 {
                let declared_allocated: u16 = children.iter().enumerate()
                    .filter_map(|(i, child)| {
                        let child_size = match child {
                            LayoutNode::Window { name, size, .. } => overrides.get(name).copied().or(*size),
                            LayoutNode::Container { id, percent, .. } => overrides.get(id).copied().or_else(|| percent.map(WindowSize::Percent)),
                        };
                        if matches!(child_size, Some(WindowSize::Percent(_))) { Some(content_sizes[i]) } else { None }
                    })
                    .sum();

                let undeclared_total = flex_len.saturating_sub(declared_allocated);
                let undeclared_share = undeclared_total / undeclared_count;
                let mut undeclared_rem = undeclared_total % undeclared_count;

                for (i, child) in children.iter().enumerate() {
                    let child_size = match child {
                        LayoutNode::Window { name, size, .. } => overrides.get(name).copied().or(*size),
                        LayoutNode::Container { id, percent, .. } => overrides.get(id).copied().or_else(|| percent.map(WindowSize::Percent)),
                    };
                    if child_size.is_none() {
                        content_sizes[i] = undeclared_share;
                        if undeclared_rem > 0 {
                            content_sizes[i] += 1;
                            undeclared_rem -= 1;
                        }
                    }
                }
            }

            // ================ Phase 3: 最大余数法分配全局余数 ================
            let non_absolute_sum: u16 = children.iter().enumerate()
                .filter_map(|(i, child)| {
                    let child_size = match child {
                        LayoutNode::Window { name, size, .. } => overrides.get(name).copied().or(*size),
                        LayoutNode::Container { id, percent, .. } => overrides.get(id).copied().or_else(|| percent.map(WindowSize::Percent)),
                    };
                    if !matches!(child_size, Some(WindowSize::Absolute(_))) { Some(content_sizes[i]) } else { None }
                })
                .sum();
            let remainder = flex_len.saturating_sub(non_absolute_sum) as usize;

            if remainder > 0 {
                let undeclared_each_pct: u32 = if undeclared_count > 0 && pct_base > 0 {
                    (pct_base as u32).saturating_sub(declared_pct_sum as u32) / undeclared_count as u32
                } else {
                    0
                };

                let mut fractional_parts: Vec<(usize, u64)> = Vec::new();
                for (i, child) in children.iter().enumerate() {
                    let child_size = match child {
                        LayoutNode::Window { name, size, .. } => overrides.get(name).copied().or(*size),
                        LayoutNode::Container { id, percent, .. } => overrides.get(id).copied().or_else(|| percent.map(WindowSize::Percent)),
                    };
                    if !matches!(child_size, Some(WindowSize::Absolute(_))) {
                        let exact_x10000: u64 = match child_size {
                            Some(WindowSize::Percent(p)) => {
                                if pct_base == 0 { 0 } else {
                                    flex_len as u64 * p as u64 * 10000 / pct_base as u64
                                }
                            }
                            None => {
                                if pct_base == 0 { 0 } else {
                                    flex_len as u64 * undeclared_each_pct as u64 * 10000 / pct_base as u64
                                }
                            }
                            _ => 0,
                        };
                        let frac = exact_x10000 % 10000;
                        fractional_parts.push((i, frac));
                    }
                }

                fractional_parts.sort_by(|a, b| b.1.cmp(&a.1));

                for (idx, _) in fractional_parts.iter().take(remainder) {
                    content_sizes[*idx] += 1;
                }

                let distributed = fractional_parts.len().min(remainder);
                let leftover = remainder - distributed;
                if leftover > 0 {
                    if let Some(last) = content_sizes.last_mut() { *last += leftover as u16; }
                }
            }

            let mut physical_sizes: Vec<u16> = Vec::with_capacity(children.len());
            for (i, child) in children.iter().enumerate() {
                let border_extra = match child {
                    LayoutNode::Window { border: BorderStyle::Box, .. } => 2,
                    LayoutNode::Window { border: BorderStyle::Line, .. } => 1,
                    _ => 0,
                };
                physical_sizes.push(content_sizes[i].saturating_add(border_extra));
            }

            let mut current_pos = match direction {
                Direction::Horizontal => rect.start_col,
                Direction::Vertical => rect.start_row,
            };

            for (i, child) in children.iter().enumerate() {
                let child_len = physical_sizes[i];
                let child_rect = match direction {
                    Direction::Horizontal => WindowRect {
                        start_col: current_pos,
                        start_row: rect.start_row,
                        width: child_len,
                        height: rect.height,
                    },
                    Direction::Vertical => WindowRect {
                        start_col: rect.start_col,
                        start_row: current_pos,
                        width: rect.width,
                        height: child_len,
                    },
                };
                current_pos = current_pos.saturating_add(child_len);
                compute_rects(child, child_rect, rects, overrides);
            }
        }
    }
}

/// 计算给定物理矩形和边框样式下的纯内容区尺寸
pub fn content_size(rect: &WindowRect, border: BorderStyle) -> (u16, u16) {
    let (oh_x, oh_y) = border.overhead();
    (rect.width.saturating_sub(oh_x), rect.height.saturating_sub(oh_y))
}

pub fn default_layout() -> Layout {
    Layout {
        layers: vec![LayoutLayer {
            name: None,
            z_index: 0,
            anchor: Anchor::FullScreen,
            visible: true,
            runtime_rect_override: None,
            root: LayoutNode::Window {
                border_chars: None,
                name: "Main".to_string(),
                size: None,
                border: BorderStyle::Box,
                draggable: false,
            },
        }],
    }
}

/// 向后兼容的单字符串解析入口
pub fn parse_layout(s: &str) -> Layout {
    Layout::parse(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_percentage() {
        let node = parse_window("area(50%):Main").unwrap();
        if let LayoutNode::Window { size, draggable: false, .. } = node {
            assert_eq!(size, Some(WindowSize::Percent(5000))); // 50% -> 5000
        } else {
            panic!("Expected Window node");
        }
    }

    #[test]
    fn test_parse_absolute() {
        let node = parse_window("area(3):Status").unwrap();
        if let LayoutNode::Window { size, draggable: false, .. } = node {
            assert_eq!(size, Some(WindowSize::Absolute(3)));
        } else {
            panic!("Expected Window node");
        }
    }

    #[test]
    fn test_parse_border_none() {
        let node = parse_window("area(1)[none]:Status").unwrap();
        if let LayoutNode::Window { border, draggable: false, .. } = node {
            assert_eq!(border, BorderStyle::None);
        } else {
            panic!("Expected Window node");
        }
    }

    #[test]
    fn test_flexbox_remainder_handling() {
        let layout_str = "horizontal(area(33%)[none]:A, area(33%)[none]:B, area(33%)[none]:C)";
        let layout = Layout::parse(layout_str);
        let rects = calc_window_rects(&layout.layers, 100, 10);

        assert_eq!(rects.len(), 3);

        let total_width: u16 = rects.iter().map(|(r, _, _, _)| r.width).sum();
        assert_eq!(total_width, 100);

        // 3 个 33% 的小数部分相同，第一个获得余数
        assert_eq!(rects[0].0.width, 34);
        assert_eq!(rects[1].0.width, 33);
        assert_eq!(rects[2].0.width, 33);
    }

    #[test]
    fn test_undeclared_nodes_share_remaining() {
        let layout_str = "horizontal(area(50%)[none]:A, area[none]:B, area[none]:C)";
        let layout = Layout::parse(layout_str);
        let rects = calc_window_rects(&layout.layers, 100, 10);

        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].0.width, 50);
        assert_eq!(rects[1].0.width, 25);
        assert_eq!(rects[2].0.width, 25);
    }

    #[test]
    fn test_anchored_layer_parsing() {
        let layout_str = "@(10,5) area(40,15)[box]:Popup";
        let layer = parse_anchored_layer(layout_str, 1).unwrap();
        assert_eq!(layer.z_index, 1);
        if let Anchor::ScreenAbsolute { x, y } = layer.anchor {
            assert_eq!(x, Coord::Pixels(10));
            assert_eq!(y, Coord::Pixels(5));
        } else {
            panic!("Expected ScreenAbsolute anchor");
        }
    }

    #[test]
    fn test_multi_layer_calc() {
        let layouts = vec![
            "horizontal(area(50%):A, area(50%):B)".to_string(),
            "@(10,5) area(20,10):Popup".to_string(),
        ];
        let layout = parse_layouts(&layouts);
        assert_eq!(layout.layers.len(), 2);

        let rects = calc_window_rects(&layout.layers, 100, 50);
        assert_eq!(rects.len(), 3);

        let popup = rects.iter().find(|(_, name, _, _)| name == "Popup").unwrap();
        assert_eq!(popup.0.start_col, 10);
        assert_eq!(popup.0.start_row, 5);
        assert_eq!(popup.0.width, 20);
        assert_eq!(popup.0.height, 10);
        assert_eq!(popup.3, 1);
    }
}
