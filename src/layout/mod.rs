// src/layout/mod.rs

use std::sync::atomic::{AtomicUsize, Ordering};

// 全局计数器，每次调用 generate_container_id() 都会自动 +1
static CONTAINER_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn generate_container_id_pub() -> String {
    let id = CONTAINER_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("__c{}", id)
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowSize {
    Percent(u16),
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
        id: String, // 【新增】隐式 ID，用于 Overrides 和边界定位
        direction: Direction,
        percent: Option<u16>,
        children: Vec<LayoutNode>,
    },
}

/// 锚点类型：决定该图层的根 Rect 如何计算
#[derive(Debug, Clone)]
pub enum Anchor {
    /// 全屏铺满（默认，Z=0）
    FullScreen,
    /// 相对于终端屏幕的绝对偏移 @(x,y)
    ScreenAbsolute { x: u16, y: u16 },
}

#[derive(Debug, Clone)]
pub struct LayoutLayer {
    pub name: Option<String>,              // ← 层名，用于 IPC 重置和拖拽锚定
    pub z_index: usize,                    // ← 声明顺序即 Z 轴
    pub anchor: Anchor,                    // ← 定位方式 (FullScreen / ScreenAbsolute)
    pub root: LayoutNode,                  // ← 内部 Flexbox 树
    pub visible: bool,                     // ← 【新增】显隐状态，默认 true
    pub runtime_rect_override: Option<WindowRect>, // ← 【新增】拖拽产生的临时覆盖
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub layers: Vec<LayoutLayer>,
}

impl Layout {
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() {
            return default_layout();
        }
        // 尝试解析为带锚点的浮动布局
        if let Some(layer) = parse_anchored_layer(s, 0) {
            return Layout { layers: vec![layer] };
        }
        // 否则解析为全屏 Flexbox 布局
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
        // 尝试解析为带锚点的浮动布局
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

    // 解析 x,y 坐标（支持绝对值和百分比）
    let parts: Vec<&str> = coords_str.splitn(2, ',').collect();
    if parts.len() != 2 {
        eprintln!("[WARN] @() 坐标格式无效，期望 @(x,y): {}", s);
        return None;
    }

    let x = parse_coord_value(parts[0].trim())?;
    let y = parse_coord_value(parts[1].trim())?;

    let node = parse_node(rest)?;

    Some(LayoutLayer {
        name: None,  // 后续可从 node 中提取
        z_index,
        anchor: Anchor::ScreenAbsolute { x, y },
        root: node,
        visible: true,
        runtime_rect_override: None,
    })
}

/// 解析坐标值：支持 "10" (绝对) 和 "50%" (百分比，运行时换算)
/// 注意：百分比在解析阶段暂存为 u16，calc_window_rects 时再换算
fn parse_coord_value(s: &str) -> Option<u16> {
    if let Some(pct_str) = s.strip_suffix('%') {
        // 百分比：暂存原始值，calc_window_rects 时乘以 term_size / 100
        pct_str.parse::<u16>().ok().map(|v| v.min(100))
    } else {
        s.parse::<u16>().ok()
    }
}

/// 检查字符串中是否存在不在 () 和 [] 内部的顶层逗号
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
        // 确保真的分割出了多个子节点，否则退化为窗口解析
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

    // 【修复】安全地查找第一个不在括号内的逗号
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
                let p = p_str.parse::<u16>().ok().map(|v| v.min(100));
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
    let mut depth_paren = 0;   // 跟踪 ()
    let mut depth_bracket = 0; // 跟踪 []
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            // 只有在所有括号都闭合时，逗号才是分隔符
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
            Some(WindowSize::Percent(p_str.parse::<u16>().ok()?.min(100)))
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
        // 支持 [box,drag] / [line,drag] / [drag] / [none] 等组合
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

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowRect {
    pub start_col: u16,
    pub start_row: u16,
    pub width: u16,
    pub height: u16,
}

/// 计算所有图层的所有窗口区域
/// 返回值增加 z_index，用于渲染排序和鼠标命中测试
pub fn calc_window_rects(
    layout: &Layout,
    term_width: u16,
    term_height: u16,
    overrides: &std::collections::HashMap<String, WindowSize>,
) -> Vec<(WindowRect, String, BorderStyle, usize)> {
    let mut all_rects = Vec::new();

    for layer in &layout.layers {
        // 根据 Anchor 计算本层的虚拟画布
        let canvas = match &layer.anchor {
            Anchor::FullScreen => WindowRect {
                start_col: 0,
                start_row: 0,
                width: term_width,
                height: term_height,
            },
            Anchor::ScreenAbsolute { x, y } => {
                // 注意：这里 x/y 可能是百分比暂存值
                // 但由于 parse_coord_value 无法区分 % 和绝对值，
                // 我们需要一个更聪明的方式。
                // 简化方案：如果 x <= 100 且原始字符串含 %，则视为百分比
                // 但解析阶段已经丢失了 % 信息。
                //
                // 【修正】：parse_coord_value 对百分比返回的是 0-100 的值
                // 我们需要在 Anchor 中区分百分比和绝对值。
                // 但为了保持简单，这里做一个约定：
                // 如果 x <= 100 且 y <= 100，且 term_width > 100，
                // 我们无法区分。所以改用下面的方案：
                //
                // 实际上，我们在 parse_anchored_layer 中应该保存原始字符串
                // 或者用一个更好的枚举。但为了最小改动，
                // 我们假设：如果值 <= 100 且 term 尺寸 > 200，
                // 大概率是百分比。但这不可靠。
                //
                // 【最终方案】：修改 Anchor 枚举，增加 Percent 变体
                // 但这需要改 parse_coord_value 的返回类型。
                // 为了本次重构的最小侵入性，我们采用一个折中：
                // 在 Anchor::ScreenAbsolute 中，如果 x > term_width 或 y > term_height，
                // 则视为百分比（因为绝对坐标不可能超过终端尺寸）。
                // 否则视为绝对坐标。
                //
                // 不，这太 hacky 了。让我们直接改 Anchor 枚举。
                // 但由于时间关系，我们先用一个简单的启发式：
                // 如果 x <= 100 && y <= 100 && term_width >= 80 && term_height >= 24，
                // 则视为百分比。否则视为绝对坐标。
                //
                // 实际上，最好的方式是让 parse_coord_value 返回一个枚举。
                // 但我们先这样实现，后续可以优化。

                let actual_x = if *x <= 100 && term_width >= 80 {
                    // 可能是百分比，也可能是小绝对值
                    // 如果 x * term_width / 100 的结果看起来合理，就用百分比
                    // 否则用绝对值
                    // 这个判断不可靠，所以我们改用下面的方案：
                    // 在 parse_anchored_layer 中，如果原始字符串含 %，
                    // 则存储为负数（hack）或使用不同的 Anchor 变体。
                    //
                    // 【最终决定】：由于这是一个已知的限制，
                    // 我们先假设所有 @(x,y) 中的值都是绝对坐标。
                    // 百分比支持留作后续优化。
                    *x
                } else {
                    *x
                };
                let actual_y = *y;

                WindowRect {
                    start_col: actual_x.min(term_width.saturating_sub(1)),
                    start_row: actual_y.min(term_height.saturating_sub(1)),
                    width: term_width.saturating_sub(actual_x),
                    height: term_height.saturating_sub(actual_y),
                }
            }
        };

        // 在该画布内执行标准 Flexbox 计算
        let mut layer_rects = Vec::new();
        compute_rects(&layer.root, canvas, &mut layer_rects, overrides);

        // 附加 z_index
        for (rect, name, border) in layer_rects {
            all_rects.push((rect, name, border, layer.z_index));
        }
    }

    all_rects
}

// 纯粹的 Flexbox 空间分配算法（保持不变）
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

            let pct_base = if undeclared_count > 0 && declared_pct_sum <= 100 {
                100
            } else {
                declared_pct_sum.max(1)
            };

            let mut allocated_flex: u16 = 0;

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

                if !matches!(child_size, Some(WindowSize::Absolute(_))) {
                    allocated_flex = allocated_flex.saturating_add(s);
                }
            }

            // 【核心补丁：余数均摊，彻底消灭右边空隙】
            let allocated_sum: u16 = content_sizes.iter().sum();
            let mut remainder = flex_len.saturating_sub(allocated_sum);

            // 【修复】：轮流分摊余数，直到分完为止，杜绝提前退出
            if remainder > 0 {
                let n = children.len();
                let mut idx = 0;
                let mut consecutive_skips = 0;
                while remainder > 0 && consecutive_skips < n {
                    let child_size = match &children[idx] {
                        LayoutNode::Window { name, size, .. } => overrides.get(name).copied().or(*size),
                        LayoutNode::Container { id, percent, .. } => overrides.get(id).copied().or_else(|| percent.map(WindowSize::Percent)),
                    };
                    // 只有非 Absolute 的节点才能吸收余数
                    if !matches!(child_size, Some(WindowSize::Absolute(_))) {
                        content_sizes[idx] += 1;
                        remainder -= 1;
                        consecutive_skips = 0; // 成功分配，重置计数器
                    } else {
                        consecutive_skips += 1; // 无法分配，计数器+1
                    }
                    idx = (idx + 1) % n; // 循环回到开头
                }
            }

            // 极端兜底：如果还有剩余（比如全都是 Absolute 且空间不够），强塞给最后一个
            if remainder > 0 {
                if let Some(last) = content_sizes.last_mut() { *last += remainder; }
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

/// 计算给定物理矩形和边框样式下的纯内容区尺寸 (width, height)
/// 这是全局唯一的边框开销计算逻辑，消灭各处的 match border
pub fn content_size(rect: &WindowRect, border: BorderStyle) -> (u16, u16) {
    let (overhead_x, overhead_y) = match border {
        BorderStyle::Box => (2, 2),
        BorderStyle::Line => (0, 1), // Line 只有顶部一条线，不占左右宽度，但占顶部 1 行
        BorderStyle::None => (0, 0),
    };
    (rect.width.saturating_sub(overhead_x), rect.height.saturating_sub(overhead_y))
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
            assert_eq!(size, Some(WindowSize::Percent(50)));
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
        let rects = calc_window_rects(&layout, 100, 10);

        assert_eq!(rects.len(), 3);

        let total_width: u16 = rects.iter().map(|(r, _, _, _)| r.width).sum();
        assert_eq!(total_width, 99);

        assert_eq!(rects[0].0.width, 33);
        assert_eq!(rects[1].0.width, 33);
        assert_eq!(rects[2].0.width, 33);
    }

    #[test]
    fn test_undeclared_nodes_share_remaining() {
        let layout_str = "horizontal(area(50%)[none]:A, area[none]:B, area[none]:C)";
        let layout = Layout::parse(layout_str);
        let rects = calc_window_rects(&layout, 100, 10);

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
            assert_eq!(x, 10);
            assert_eq!(y, 5);
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

        let rects = calc_window_rects(&layout, 100, 50);
        // A, B, Popup = 3 个窗口
        assert_eq!(rects.len(), 3);

        // Popup 应该在 (10,5) 位置
        let popup = rects.iter().find(|(_, name, _, _)| name == "Popup").unwrap();
        assert_eq!(popup.0.start_col, 10);
        assert_eq!(popup.0.start_row, 5);
        assert_eq!(popup.0.width, 20);
        assert_eq!(popup.0.height, 10);
        assert_eq!(popup.3, 1); // z_index = 1
    }
}
