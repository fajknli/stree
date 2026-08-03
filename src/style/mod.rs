// src/style/mod.rs

use crossterm::style::Color;
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct StyleRule {
    matcher: RuleMatcher,
    fg_color: Option<Color>,
    is_bold: bool,
}

#[derive(Debug, Clone)]
enum RuleMatcher {
    Exact(String),
    Regex(Regex),
}

#[derive(Debug, Clone)]
pub struct StyleEngine {
    rules: Vec<StyleRule>,
    color_map: HashMap<String, Color>,
}

impl StyleEngine {
    pub fn parse(input: &str) -> Self {
        let mut engine = Self {
            rules: Vec::new(),
            color_map: Self::build_color_map(),
        };

        engine.add_rule("__marked__", "yellow,bold");

        if input.trim().is_empty() {
            return engine;
        }

        let parts: Vec<&str> = input.split('=').collect();
        if parts.len() < 2 {
            eprintln!("[WARN] 样式配置格式无效，缺少 '=': {}", input);
            return engine;
        }

        let mut current_pattern = parts[0].trim().to_string();

        for i in 1..parts.len() {
            let segment = parts[i];
            if i == parts.len() - 1 {
                engine.add_rule(&current_pattern, segment);
                break;
            }
            if let Some(last_comma_idx) = segment.rfind(',') {
                let styles_part = &segment[..last_comma_idx];
                let next_pattern = &segment[last_comma_idx + 1..];
                engine.add_rule(&current_pattern, styles_part);
                current_pattern = next_pattern.trim().to_string();
            } else {
                engine.add_rule(&current_pattern, segment);
                current_pattern = String::new();
            }
        }

        engine
    }

    /// 第四列现在是逗号分隔的标签集，如 "live"、"archived" 或 "type_note,state_archived"。
    /// 匹配逻辑：遍历所有规则，对每条规则检查是否命中标签集里任意一个标签。
    /// 后命中的规则覆盖先命中的规则，实现"配置顺序即优先级"。
    /// 这样多标签可以叠加样式，最后写的规则优先级最高。
    pub fn get_style(&self, tags_str: &str) -> (Option<Color>, bool) {
        // 拆分标签集，空字符串当单标签处理（向后兼容旧的单值 status）
        let tags: Vec<&str> = tags_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        let mut final_color: Option<Color> = None;
        let mut final_bold = false;

        for rule in &self.rules {
            let matched = tags.iter().any(|tag| match &rule.matcher {
                RuleMatcher::Exact(s) => s == tag,
                RuleMatcher::Regex(r) => r.is_match(tag),
            });

            if matched {
                // 后命中覆盖先命中
                if rule.fg_color.is_some() {
                    final_color = rule.fg_color;
                }
                if rule.is_bold {
                    final_bold = true;
                }
            }
        }

        (final_color, final_bold)
    }

    fn add_rule(&mut self, pattern: &str, styles_str: &str) {
        if pattern.is_empty() { return; }

        let matcher = if pattern.starts_with('^')
            || pattern.contains('*')
            || pattern.contains('.')
            || pattern.contains('+')
            || pattern.contains('?')
            || pattern.contains('[')
        {
            match Regex::new(pattern) {
                Ok(r) => RuleMatcher::Regex(r),
                Err(e) => {
                    eprintln!("[WARN] 正则编译失败 '{}': {}，降级为精确匹配", pattern, e);
                    RuleMatcher::Exact(pattern.to_string())
                }
            }
        } else {
            RuleMatcher::Exact(pattern.to_string())
        };

        let mut fg_color = None;
        let mut is_bold = false;

        for style_part in styles_str.split(',') {
            let s = style_part.trim().to_lowercase();
            if s == "bold" {
                is_bold = true;
            } else if s.starts_with('#') {
                if let Some(rgb) = parse_hex_color(&s) { // 去掉 Self::
                    fg_color = Some(rgb);
                } else {
                    eprintln!("[WARN] 无效的十六进制颜色: {}", s);
                }
            } else if let Some(&color) = self.color_map.get(&s) {
                fg_color = Some(color);
            } else if !s.is_empty() {
                eprintln!("[WARN] 未知样式属性: {}", s);
            }
        }

        self.rules.push(StyleRule { matcher, fg_color, is_bold });
    }

    fn build_color_map() -> HashMap<String, Color> {
        let mut map = HashMap::new();
        map.insert("black".into(),    Color::Black);
        map.insert("red".into(),      Color::Red);
        map.insert("green".into(),    Color::Green);
        map.insert("yellow".into(),   Color::Yellow);
        map.insert("blue".into(),     Color::Blue);
        map.insert("magenta".into(),  Color::Magenta);
        map.insert("cyan".into(),     Color::Cyan);
        map.insert("white".into(),    Color::White);
        map.insert("gray".into(),     Color::DarkGrey);
        map.insert("grey".into(),     Color::DarkGrey);
        map.insert("darkgray".into(), Color::DarkGrey);
        map.insert("darkgrey".into(), Color::DarkGrey);
        map
    }
}

// 提取为独立函数，供 StyleEngine 和 UiTheme 共用
pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Color::Rgb { r, g, b })
    } else if hex.len() == 3 {
        // 支持 3 位简写，如 #fff -> #ffffff
        let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
        let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
        let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
        Some(Color::Rgb { r: r * 17, g: g * 17, b: b * 17 })
    } else {
        None
    }
}

// ================= 终极 UI 主题配置 =================
#[derive(Debug, Clone)]
pub struct UiTheme {
    pub border_focused: Color,
    pub border_unfocused: Color,
    pub view_focused: Color,
    pub view_unfocused: Color,
    pub statusbar_fg: Color,
    pub input_prefix: Color,
    pub input_buffer: Color,
    pub selected_bg: Color,
    pub error_fg: Color,
    pub error_bg: Color,
    pub empty_data_fg: Color,
}

impl Default for UiTheme {
    fn default() -> Self {
        // 默认适配冷蓝主题
        Self {
            border_focused: Color::Rgb { r: 169, g: 181, b: 213 },   // #a9b5d5
            border_unfocused: Color::Rgb { r: 86, g: 93, b: 126 },   // #565d7e
            view_focused: Color::Rgb { r: 169, g: 181, b: 213 },     // #a9b5d5
            view_unfocused: Color::Rgb { r: 86, g: 93, b: 126 },     // #565d7e
            statusbar_fg: Color::Rgb { r: 212, g: 220, b: 242 },     // #d4dcf2
            input_prefix: Color::Rgb { r: 201, g: 59, b: 59 },       // #c93b3b
            input_buffer: Color::Rgb { r: 169, g: 181, b: 213 },     // #a9b5d5
            selected_bg: Color::Rgb { r: 36, g: 40, b: 56 },         // #242838
            error_fg: Color::Rgb { r: 255, g: 255, b: 255 },
            error_bg: Color::Rgb { r: 201, g: 59, b: 59 },
            empty_data_fg: Color::Rgb { r: 86, g: 93, b: 126 },
        }
    }
}

impl UiTheme {
    pub fn parse(input: &str) -> Self {
        let mut theme = Self::default();
        if input.trim().is_empty() {
            return theme;
        }

        for part in input.split(',') {
            let kv: Vec<&str> = part.splitn(2, '=').collect();
            if kv.len() != 2 { continue; }
            let key = kv[0].trim();
            let val = kv[1].trim();

            if let Some(color) = parse_hex_color(val) {
                match key {
                    "border_focused" => theme.border_focused = color,
                    "border_unfocused" => theme.border_unfocused = color,
                    "view_focused" => theme.view_focused = color,
                    "view_unfocused" => theme.view_unfocused = color,
                    "statusbar_fg" => theme.statusbar_fg = color,
                    "input_prefix" => theme.input_prefix = color,
                    "input_buffer" => theme.input_buffer = color,
                    "selected_bg" => theme.selected_bg = color,
                    "error_fg" => theme.error_fg = color,
                    "error_bg" => theme.error_bg = color,
                    "empty_data_fg" => theme.empty_data_fg = color,
                    _ => eprintln!("[WARN] 未知 UI 颜色键: {}", key),
                }
            } else {
                eprintln!("[WARN] 无效的十六进制颜色: {}", val);
            }
        }
        theme
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_tag_backward_compat() {
        // 旧用法，单值 status，向后兼容
        let engine = StyleEngine::parse("live=white,archived=gray");
        let (color, bold) = engine.get_style("live");
        assert_eq!(color, Some(Color::White));
        assert!(!bold);

        let (color2, _) = engine.get_style("archived");
        assert_eq!(color2, Some(Color::DarkGrey));
    }

    #[test]
    fn test_multi_tag_any_match() {
        // 多标签，任意一个命中即生效
        let engine = StyleEngine::parse("archived=gray,live=white");
        let (color, _) = engine.get_style("type_note,archived");
        assert_eq!(color, Some(Color::DarkGrey));
    }

    #[test]
    fn test_multi_tag_later_rule_wins() {
        // 多标签同时命中多条规则，后写的规则优先级更高
        let engine = StyleEngine::parse("archived=gray,prio_high=red");
        let (color, _) = engine.get_style("archived,prio_high");
        assert_eq!(color, Some(Color::Red)); // prio_high 规则在后，覆盖 archived
    }

    #[test]
    fn test_bold_accumulates() {
        // bold 是累加的，任意规则命中 bold 就生效
        let engine = StyleEngine::parse("archived=gray,prio_high=red,bold");
        let (_, bold) = engine.get_style("archived,prio_high");
        assert!(bold);
    }

    #[test]
    fn test_regex_match_in_tags() {
        let engine = StyleEngine::parse("^fail.*=red,bold");
        let (color, bold) = engine.get_style("type_note,failed");
        assert_eq!(color, Some(Color::Red));
        assert!(bold);
    }

    #[test]
    fn test_no_match() {
        let engine = StyleEngine::parse("live=white,archived=gray");
        let (color, bold) = engine.get_style("unknown");
        assert_eq!(color, None);
        assert!(!bold);
    }
}
