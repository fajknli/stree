// src/layout/mod.rs

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

// 【新增】尺寸枚举，彻底消灭互斥的 Option，让非法状态无法被表示
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowSize {
    Percent(u16),
    Absolute(u16),
}

#[derive(Debug, Clone)]
pub enum LayoutNode {
    Window {
        name: String,
        size: Option<WindowSize>, // None 表示未声明，参与剩余空间均分
        border: BorderStyle,
    },
    Container {
        direction: Direction,
        percent: Option<u16>,
        children: Vec<LayoutNode>,
    },
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: LayoutNode,
}

impl Layout {
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() {
            return default_layout();
        }
        match parse_node(s) {
            Some(node) => Layout { root: node },
            None => {
                eprintln!("[WARN] 布局字符串无效，使用默认布局: {}", s);
                default_layout()
            }
        }
    }
}

fn parse_node(s: &str) -> Option<LayoutNode> {
    let s = s.trim();
    if s.starts_with("horizontal(") {
        parse_container(s, Direction::Horizontal)
    } else if s.starts_with("vertical(") {
        parse_container(s, Direction::Vertical)
    } else if s.contains(',') {
        let children = parse_children(s)?;
        Some(LayoutNode::Container {
            direction: Direction::Horizontal,
            percent: None,
            children,
        })
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

    // 容器百分比解析歧义修复：强制要求带 % 号
    let (percent, children_str): (Option<u16>, &str) = if let Some(comma_pos) = inside.find(',') {
        let potential_pct = inside[..comma_pos].trim();
        if let Some(p_str) = potential_pct.strip_suffix('%') {
            let p = p_str.parse::<u16>().ok().map(|v| v.min(100));
            (p, &inside[comma_pos + 1..])
        } else {
            (None, inside)
        }
    } else {
        (None, inside)
    };

    let children = parse_children(children_str)?;
    if children.is_empty() {
        return None;
    }

    Some(LayoutNode::Container {
        direction: dir,
        percent,
        children,
    })
}

fn parse_children(s: &str) -> Option<Vec<LayoutNode>> {
    let mut children = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
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

/// 解析单个窗口节点
fn parse_window(s: &str) -> Option<LayoutNode> {
    let s = s.trim();

    if !s.starts_with("area") {
        eprintln!("[WARN] 未知窗口类型，期望 'area': {}", s);
        return None;
    }

    let rest = &s[4..]; // 去掉 "area"

    // 解析可选的，支持 50% 或 3
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

    // 解析可选的 [border]
    let (rest_after_border, border) = if rest_after_size.starts_with('[') {
        let close = rest_after_size.find(']')?;
        let b_str = rest_after_size[1..close].trim();
        let b = match b_str {
            "line" => BorderStyle::Line,
            "none" => BorderStyle::None,
            _ => BorderStyle::Box,
        };
        (&rest_after_size[close + 1..], b)
    } else {
        (rest_after_size, BorderStyle::Box)
    };

    // 解析可选的 :Name
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

    Some(LayoutNode::Window { name, size, border })
}

#[derive(Debug, Clone, Copy)]
pub struct WindowRect {
    pub start_col: u16,
    pub start_row: u16,
    pub width: u16,
    pub height: u16,
}

/// 计算所有窗口的屏幕区域
pub fn calc_window_rects(layout: &Layout, term_width: u16, term_height: u16) -> Vec<(WindowRect, String, BorderStyle)> {
    let root_rect = WindowRect {
        start_col: 0,
        start_row: 0,
        width: term_width,
        height: term_height,
    };

    let mut rects = Vec::new();
    compute_rects(&layout.root, root_rect, &mut rects);
    rects
}

// 【彻底重写】纯粹的 Flexbox 空间分配算法，修复混合声明时的基数漏洞
fn compute_rects(node: &LayoutNode, rect: WindowRect, rects: &mut Vec<(WindowRect, String, BorderStyle)>) {
    match node {
        LayoutNode::Window { name, border, .. } => {
            rects.push((rect, name.clone(), *border));
        }
        LayoutNode::Container { direction, children, .. } => {
            let total_len = match direction {
                Direction::Horizontal => rect.width,
                Direction::Vertical => rect.height,
            };

            // 1. 【核心改动】先统计所有节点的“边框开销”，并从总空间中扣除
            let mut total_border_overhead: u16 = 0;
            let mut absolute_content_len: u16 = 0;
            let mut declared_pct_sum: u16 = 0;
            let mut undeclared_count: u16 = 0;

            for child in children.iter() {
                // 计算当前节点的边框开销 (Box=2, Line=1, None/Container=0)
                let border_extra = match child {
                    LayoutNode::Window { border: BorderStyle::Box, .. } => 2,
                    LayoutNode::Window { border: BorderStyle::Line, .. } => 1,
                    _ => 0,
                };
                total_border_overhead = total_border_overhead.saturating_add(border_extra);

                match child {
                    LayoutNode::Window { size: Some(WindowSize::Absolute(n)), .. } => {
                        absolute_content_len = absolute_content_len.saturating_add(*n);
                    }
                    LayoutNode::Window { size: Some(WindowSize::Percent(p)), .. } => {
                        declared_pct_sum = declared_pct_sum.saturating_add(*p);
                    }
                    LayoutNode::Window { size: None, .. } => {
                        undeclared_count += 1;
                    }
                    LayoutNode::Container { percent: Some(p), .. } => {
                        declared_pct_sum = declared_pct_sum.saturating_add(*p);
                    }
                    LayoutNode::Container { percent: None, .. } => {
                        undeclared_count += 1;
                    }
                }
            }

            // 可用于分配给“内容”的总空间 = 物理总空间 - 所有边框开销
            let available_for_content = total_len.saturating_sub(total_border_overhead);

            // 减去 Absolute 内容区占用的空间，剩下的才是 Flex 空间
            let flex_len = available_for_content.saturating_sub(absolute_content_len);

            // 2. 计算每个节点的“内容区”大小
            let mut content_sizes: Vec<u16> = Vec::with_capacity(children.len());

            let pct_base = if undeclared_count > 0 && declared_pct_sum <= 100 {
                100
            } else {
                declared_pct_sum.max(1)
            };

            let mut allocated_flex: u16 = 0;

            for child in children.iter() {
                let s = match child {
                    LayoutNode::Window { size: Some(WindowSize::Absolute(n)), .. } => *n,
                    LayoutNode::Window { size: Some(WindowSize::Percent(p)), .. } => {
                        if pct_base == 0 { 0 } else {
                            (flex_len as u32 * *p as u32 / pct_base as u32) as u16
                        }
                    }
                    LayoutNode::Container { percent: Some(p), .. } => {
                        if pct_base == 0 { 0 } else {
                            (flex_len as u32 * *p as u32 / pct_base as u32) as u16
                        }
                    }
                    _ => 0,
                };
                content_sizes.push(s);

                if !matches!(child, LayoutNode::Window { size: Some(WindowSize::Absolute(_)), .. }) {
                    allocated_flex = allocated_flex.saturating_add(s);
                }
            }

            // 3. 将剩余的残渣分配给未声明尺寸的节点（内容区）
            let rem_flex = flex_len.saturating_sub(allocated_flex);
            if undeclared_count > 0 {
                let per = rem_flex / undeclared_count;
                let extra = rem_flex % undeclared_count;
                let mut rem_idx = 0;
                for (i, child) in children.iter().enumerate() {
                    let is_undeclared = match child {
                        LayoutNode::Window { size: None, .. } => true,
                        LayoutNode::Container { percent: None, .. } => true,
                        _ => false,
                    };
                    if is_undeclared {
                        content_sizes[i] = per + if rem_idx < extra { 1 } else { 0 };
                        rem_idx += 1;
                    }
                }
            }

            // 4. 【核心改动】将“内容区大小”转换为“物理总大小”（加上各自的边框）
            let mut physical_sizes: Vec<u16> = Vec::with_capacity(children.len());
            for (i, child) in children.iter().enumerate() {
                let border_extra = match child {
                    LayoutNode::Window { border: BorderStyle::Box, .. } => 2,
                    LayoutNode::Window { border: BorderStyle::Line, .. } => 1,
                    _ => 0,
                };
                physical_sizes.push(content_sizes[i].saturating_add(border_extra));
            }

            // 5. 按顺序排布生成 Rect（使用物理总大小）
            let mut current_pos = match direction {
                Direction::Horizontal => rect.start_col,
                Direction::Vertical => rect.start_row,
            };

            for (i, child) in children.iter().enumerate() {
                let child_len = physical_sizes[i]; // 使用物理总大小
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
                compute_rects(child, child_rect, rects);
            }
        }
    }
}

pub fn default_layout() -> Layout {
    Layout {
        root: LayoutNode::Window {
            name: "Main".to_string(),
            size: None, // None 表示不声明尺寸，自动占满全屏
            border: BorderStyle::Box,
        },
    }
}

pub fn parse_layout(s: &str) -> Layout {
    Layout::parse(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_percentage() {
        let node = parse_window("area(50%):Main").unwrap();
        if let LayoutNode::Window { size, .. } = node {
            assert_eq!(size, Some(WindowSize::Percent(50)));
        } else {
            panic!("Expected Window node");
        }
    }

    #[test]
    fn test_parse_absolute() {
        let node = parse_window("area(3):Status").unwrap();
        if let LayoutNode::Window { size, .. } = node {
            assert_eq!(size, Some(WindowSize::Absolute(3)));
        } else {
            panic!("Expected Window node");
        }
    }

    #[test]
    fn test_parse_border_none() {
        let node = parse_window("area(1)[none]:Status").unwrap();
        if let LayoutNode::Window { border, .. } = node {
            assert_eq!(border, BorderStyle::None);
        } else {
            panic!("Expected Window node");
        }
    }

    #[test]
    fn test_flexbox_remainder_handling() {
        // 显式加上 [none] 排除边框开销干扰
        let layout_str = "horizontal(area(33%)[none]:A, area(33%)[none]:B, area(33%)[none]:C)";
        let layout = Layout::parse(layout_str);
        let rects = calc_window_rects(&layout, 100, 10);

        assert_eq!(rects.len(), 3);

        let total_width: u16 = rects.iter().map(|(r, _, _)| r.width).sum();
        assert_eq!(total_width, 99);

        assert_eq!(rects[0].0.width, 33);
        assert_eq!(rects[1].0.width, 33);
        assert_eq!(rects[2].0.width, 33);
    }

    #[test]
    fn test_undeclared_nodes_share_remaining() {
        // 显式加上 [none] 排除边框开销干扰
        let layout_str = "horizontal(area(50%)[none]:A, area[none]:B, area[none]:C)";
        let layout = Layout::parse(layout_str);
        let rects = calc_window_rects(&layout, 100, 10);

        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].0.width, 50);
        assert_eq!(rects[1].0.width, 25);
        assert_eq!(rects[2].0.width, 25);
    }
}
