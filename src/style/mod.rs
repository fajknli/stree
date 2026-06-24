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
