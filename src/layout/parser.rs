// src/layout/parser.rs

use super::{
    Anchor, BorderStyle, Coord, Direction, Layout, LayoutLayer, LayoutNode, WindowSize,
    default_layout, generate_container_id_pub,
};

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

    // 【新增】支持 | 前缀声明初始隐藏图层
    let mut visible = true;
    let s = if let Some(rest) = s.strip_prefix('|') {
        visible = false;
        rest.trim()
    } else {
        s
    };

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
        visible, // 使用动态判断的 visible
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
            Some(WindowSize::Percent(parse_percent_to_mille(p_str)?))
        } else if size_str.is_empty() {
            None
        } else if let Some(comma_pos) = size_str.find(',') {
            // 【新增】解析 "40,15" 这样的二维尺寸
            let w = size_str[..comma_pos].trim().parse::<u16>().ok()?;
            let h = size_str[comma_pos+1..].trim().parse::<u16>().ok()?;
            Some(WindowSize::Absolute2D(w, h))
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
            // 【新增】为二维尺寸生成默认名
            Some(WindowSize::Absolute2D(w, h)) => format!("{}x{}", w, h),
            None => "unnamed".to_string(),
        })
    };

    if name.is_empty() {
        eprintln!("[WARN] 窗口名称为空: {}", s);
        return None;
    }

    Some(LayoutNode::Window { name, size, border, border_chars: None, draggable })
}

/// 向后兼容的单字符串解析入口
pub fn parse_layout(s: &str) -> Layout {
    Layout::parse(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{calc_window_rects, WindowRect};

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
