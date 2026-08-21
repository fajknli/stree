// src/layout/mod.rs

pub mod parser;

pub use parser::parse_layouts;

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
    Percent(u16), // 万分比精度：0 = 0.00%, 10000 = 100.00%
    Absolute(u16),
    Absolute2D(u16, u16),
    Percent2D(u16, u16), // 【新增】二维百分比
    Auto(u16), // 【新增】自适应高度，参数为 fallback 默认行数
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
    auto_overrides: &std::collections::HashMap<String, WindowSize>,
) -> Vec<(WindowRect, String, BorderStyle, usize)> {
    let mut all_rects = Vec::new();

    // 【关键修复】反转合并顺序！
    // 必须先克隆 auto_overrides，然后用 overrides (包含拖拽物理锁) 覆盖它。
    // 这样拖拽时的物理像素篡改才能生效！
    let mut effective_overrides = auto_overrides.clone();
    for (k, v) in overrides {
        effective_overrides.insert(k.clone(), *v);
    }

    for layer in layers {
        if !layer.visible { continue; }

        // 【新增】优先使用运行时拖拽覆盖（浮动窗口专用）
        let canvas = if let Some(override_rect) = layer.runtime_rect_override {
            override_rect
        } else {
            match &layer.anchor {
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
            }
        };

        let mut layer_rects = Vec::new();
        compute_rects(&layer.root, canvas, &mut layer_rects, &effective_overrides);

        for (rect, name, border) in layer_rects {
            all_rects.push((rect, name, border, layer.z_index));
        }
    }

    all_rects
}


// 纯粹的 Flexbox 空间分配算法（最大余数法）
// 纯粹的 Flexbox 空间分配算法（最大余数法）
fn compute_rects(
    node: &LayoutNode,
    rect: WindowRect,
    rects: &mut Vec<(WindowRect, String, BorderStyle)>,
    overrides: &std::collections::HashMap<String, WindowSize>,
) {
    match node {
        LayoutNode::Window { name, border, size, .. } => {
            let mut final_rect = rect;

            // 【终极修复】必须优先使用 overrides 中的尺寸！
            let effective_size = overrides.get(name).copied().or(*size);

            match effective_size {
                Some(WindowSize::Absolute2D(w, h)) => {
                    final_rect.width = w;   // 去掉 * 号
                    final_rect.height = h;  // 去掉 * 号
                }
                Some(WindowSize::Percent2D(w, h)) => {
                    final_rect.width = (w as u32 * rect.width as u32 / 10000) as u16;   // 去掉 * 号
                    final_rect.height = (h as u32 * rect.height as u32 / 10000) as u16;  // 去掉 * 号
                }
                _ => {}
            }
            rects.push((final_rect, name.clone(), *border));
        }
        LayoutNode::Container { direction, children, .. } => {
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

            // ================ Phase 0: 统计基础数据 ================
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

                let child_size = get_child_size(child, overrides);

                match child_size {
                    Some(WindowSize::Absolute(n)) | Some(WindowSize::Auto(n)) => {
                        absolute_content_len = absolute_content_len.saturating_add(n);
                    }
                    Some(WindowSize::Absolute2D(w, h)) => {
                        let main_axis_len = if *direction == Direction::Horizontal { w } else { h };
                        absolute_content_len = absolute_content_len.saturating_add(main_axis_len);
                    }
                    Some(WindowSize::Percent(p)) => {
                        declared_pct_sum = declared_pct_sum.saturating_add(p);
                    }
                    Some(WindowSize::Percent2D(_, _)) | None => {
                        undeclared_count += 1;
                    }
                }
            }

            let available_for_content = total_len.saturating_sub(total_border_overhead);
            let flex_len = available_for_content.saturating_sub(absolute_content_len);
            let pct_base = if undeclared_count > 0 && declared_pct_sum <= 10000 { 10000 } else { declared_pct_sum.max(1) };

            let mut content_sizes: Vec<u16> = vec![0; children.len()];

            // ================ Phase 1: 基础整数分配 ================
            phase1_base_allocation(children, overrides, *direction, flex_len, pct_base, &mut content_sizes);

            // ================ Phase 2: 为 undeclared 节点分配公平份额 ================
            if undeclared_count > 0 {
                phase2_undeclared_sharing(children, overrides, flex_len, undeclared_count, &mut content_sizes);
            }

            // ================ Phase 3: 最大余数法分配全局余数 ================
            phase3_remainder_distribution(children, overrides, *direction, flex_len, pct_base, undeclared_count, declared_pct_sum, &mut content_sizes);

            // ================ 物理坐标计算 ================
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

fn get_child_size(child: &LayoutNode, overrides: &std::collections::HashMap<String, WindowSize>) -> Option<WindowSize> {
    match child {
        LayoutNode::Window { name, size, .. } => overrides.get(name).copied().or(*size),
        LayoutNode::Container { id, percent, .. } => overrides.get(id).copied().or(percent.map(WindowSize::Percent)),
    }
}

fn phase1_base_allocation(
    children: &[LayoutNode],
    overrides: &std::collections::HashMap<String, WindowSize>,
    direction: Direction,
    flex_len: u16,
    pct_base: u16,
    content_sizes: &mut [u16],
) {
    for (i, child) in children.iter().enumerate() {
        let child_size = get_child_size(child, overrides);
        let s = match child_size {
            Some(WindowSize::Absolute(n)) | Some(WindowSize::Auto(n)) => n,
            Some(WindowSize::Absolute2D(w, h)) => {
                if direction == Direction::Horizontal { w } else { h }
            }
            Some(WindowSize::Percent(p)) => {
                if pct_base == 0 { 0 } else {
                    (flex_len as u32 * p as u32 / pct_base as u32) as u16
                }
            }
            Some(WindowSize::Percent2D(_, _)) | None => 0,
        };
        content_sizes[i] = s;
    }
}

fn phase2_undeclared_sharing(
    children: &[LayoutNode],
    overrides: &std::collections::HashMap<String, WindowSize>,
    flex_len: u16,
    undeclared_count: u16,
    content_sizes: &mut [u16],
) {
    let declared_allocated: u16 = children.iter().enumerate()
        .filter_map(|(i, child)| {
            let child_size = get_child_size(child, overrides);
            if matches!(child_size, Some(WindowSize::Percent(_))) { Some(content_sizes[i]) } else { None }
        })
        .sum();

    let undeclared_total = flex_len.saturating_sub(declared_allocated);
    let undeclared_share = undeclared_total / undeclared_count;
    let mut undeclared_rem = undeclared_total % undeclared_count;

    for (i, child) in children.iter().enumerate() {
        let child_size = get_child_size(child, overrides);
        if child_size.is_none() {
            content_sizes[i] = undeclared_share;
            if undeclared_rem > 0 {
                content_sizes[i] += 1;
                undeclared_rem -= 1;
            }
        }
    }
}

fn phase3_remainder_distribution(
    children: &[LayoutNode],
    overrides: &std::collections::HashMap<String, WindowSize>,
    _direction: Direction,
    flex_len: u16,
    pct_base: u16,
    undeclared_count: u16,
    declared_pct_sum: u16,
    content_sizes: &mut [u16],
) {
    let non_absolute_sum: u16 = children.iter().enumerate()
        .filter_map(|(i, child)| {
            let child_size = get_child_size(child, overrides);
            if !matches!(child_size, Some(WindowSize::Absolute(_)) | Some(WindowSize::Absolute2D(_, _)) | Some(WindowSize::Auto(_))) {
                Some(content_sizes[i])
            } else { None }
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
            let child_size = get_child_size(child, overrides);
            if !matches!(child_size, Some(WindowSize::Absolute(_)) | Some(WindowSize::Absolute2D(_, _)) | Some(WindowSize::Auto(_))) {
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

/// 将布局 AST 转换为紧凑的字符串表示
pub fn layout_ast_to_string(node: &LayoutNode) -> String {
    match node {
        LayoutNode::Window { name, size, border, .. } => {
            let size_str = match size {
                Some(WindowSize::Percent(p)) => format!("{}%", *p as f64 / 100.0),
                Some(WindowSize::Absolute(n)) => format!("{}", n),
                Some(WindowSize::Absolute2D(w, h)) => format!("{}x{}", w, h),
                Some(WindowSize::Percent2D(w, h)) => format!("{:.0}%x{:.0}%", *w as f64 / 100.0, *h as f64 / 100.0),
                Some(WindowSize::Auto(n)) => format!("auto:{}", n), // 【新增】
                None => String::new(),
            };
            let border_str = match border {
                BorderStyle::Box => "",
                BorderStyle::Line => "[line]",
                BorderStyle::None => "[none]",
            };
            if size_str.is_empty() {
                format!("area{}:{}", border_str, name)
            } else {
                format!("area({}){}:{}", size_str, border_str, name)
            }
        }
        LayoutNode::Container { direction, children, .. } => {
            let dir_str = match direction {
                Direction::Horizontal => "H",
                Direction::Vertical => "V",
            };
            let children_str: Vec<String> = children.iter().map(layout_ast_to_string).collect();
            format!("{}({})", dir_str, children_str.join(", "))
        }
    }
}

/// 获取全局容器 ID 计数器当前值
pub fn get_container_count() -> usize {
    CONTAINER_ID_COUNTER.load(Ordering::SeqCst)
}
