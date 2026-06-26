// src/config/mod.rs

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Eq, PartialEq)]
struct KeyBindingKey(KeyCode, KeyModifiers);

impl Hash for KeyBindingKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
        self.1.hash(state);
    }
}

/// 解析后的绑定类型：物理按键 或 逻辑信号
enum ParsedBinding {
    Key(KeyEvent, Vec<String>, bool),
    Signal(Option<String>, String, Vec<String>, bool), // (Scope/Window, SignalName, Args, IsSilent)
}

#[derive(Debug, Clone)]
pub struct BindConfig {
    // 物理按键绑定
    bindings: HashMap<KeyBindingKey, (Vec<String>, bool)>,
    // 【新增】信号绑定: (Option<WindowName>, SignalName) -> (命令参数模板, 是否静默)
    signal_bindings: HashMap<(Option<String>, String), (Vec<String>, bool)>,
}

impl BindConfig {
    pub fn new() -> Self {
        let mut config = Self {
            bindings: HashMap::new(),
            signal_bindings: HashMap::new(),
        };
        let defaults = vec![
            ("q", "__EXIT__"), ("esc", "__ESC__"), ("tab", "__TAB__"),
            ("up", "__UP__"), ("k", "__UP__"), ("down", "__DOWN__"), ("j", "__DOWN__"),
            ("left", "__EXPAND__"), ("h", "__EXPAND__"), ("right", "__EXPAND__"), ("l", "__EXPAND__"),
            ("space", "__MARK__"), ("g", "__TOP__"), ("G", "__BOTTOM__"),
            ("/", "__ACTIVATE_SEARCH__"), (":", "__ACTIVATE_CMD__"), ("enter", "__ENTER__"),
        ];
        for (key_desc, cmd) in defaults {
            if let Some(key_event) = parse_key_desc(key_desc) {
                let args = split_args(cmd);
                let key = KeyBindingKey(key_event.code, key_event.modifiers);
                config.bindings.insert(key, (args, false));
            }
        }
        config
    }

    pub fn parse(bind_args: &[String]) -> Self {
        let mut config = BindConfig::new();
        for arg in bind_args {
            if let Some(parsed) = parse_binding(arg) {
                match parsed {
                    ParsedBinding::Key(key_event, args, is_silent) => {
                        let key = KeyBindingKey(key_event.code, key_event.modifiers);
                        config.bindings.insert(key, (args, is_silent));
                    }
                    ParsedBinding::Signal(scope, signal_name, args, is_silent) => {
                        config.signal_bindings.insert((scope, signal_name), (args, is_silent));
                    }
                }
            }
        }
        config
    }

    pub fn get(&self, key: &KeyEvent) -> Option<&(Vec<String>, bool)> {
        let k = KeyBindingKey(key.code, key.modifiers);
        self.bindings.get(&k)
    }

    /// 【新增】查找信号绑定。优先查找局部作用域 (Window:signal)，找不到则回退到全局 (signal)。
    pub fn get_signal_binding(&self, window_name: Option<&str>, signal: &str) -> Option<&(Vec<String>, bool)> {
        if let Some(win) = window_name {
            let local_key = (Some(win.to_string()), signal.to_string());
            if let Some(binding) = self.signal_bindings.get(&local_key) {
                return Some(binding);
            }
        }
        let global_key = (None, signal.to_string());
        self.signal_bindings.get(&global_key)
    }
}

fn parse_binding(input: &str) -> Option<ParsedBinding> {
    let trimmed = input.trim();
    if trimmed.is_empty() { return None; }

    let parts: Vec<&str> = trimmed.splitn(2, '=').collect();
    if parts.len() != 2 {
        eprintln!("[WARN] 绑定格式无效，缺少 '=': {}", trimmed);
        return None;
    }

    let left_side = parts[0].trim();
    let mut cmd_template = parts[1].trim().to_string();

    let is_silent = if cmd_template.starts_with('@') {
        cmd_template = cmd_template[1..].trim().to_string();
        true
    } else {
        false
    };

    if cmd_template.starts_with("activate_input ") {
        let input_name = cmd_template.trim_start_matches("activate_input ").trim();
        cmd_template = format!("__ACTIVATE_INPUT__ {}", input_name);
    }

    if left_side.is_empty() || cmd_template.is_empty() {
        eprintln!("[WARN] 绑定格式无效，键或命令为空: {}", trimmed);
        return None;
    }

    let args = split_args(&cmd_template);

    if let Some(key_event) = parse_key_desc(left_side) {
        return Some(ParsedBinding::Key(key_event, args, is_silent));
    }

    let (scope, signal_name) = if let Some(colon_pos) = left_side.find(':') {
        let scope = left_side[..colon_pos].trim().to_string();
        let signal = left_side[colon_pos + 1..].trim().to_string();
        if scope.is_empty() || signal.is_empty() {
            eprintln!("[WARN] 信号绑定格式无效（作用域或信号名为空）: {}", trimmed);
            return None;
        }
        (Some(scope), signal)
    } else {
        (None, left_side.to_string())
    };

    Some(ParsedBinding::Signal(scope, signal_name, args, is_silent))
}

fn parse_key_desc(desc: &str) -> Option<KeyEvent> {
    let lower = desc.to_lowercase();

    let (modifier, key_part) = if lower.starts_with("ctrl-") || lower.starts_with("ctrl+") {
        let rest = lower.trim_start_matches("ctrl-").trim_start_matches("ctrl+");
        (KeyModifiers::CONTROL, rest)
    } else if lower.starts_with("alt-") || lower.starts_with("alt+") {
        let rest = lower.trim_start_matches("alt-").trim_start_matches("alt+");
        (KeyModifiers::ALT, rest)
    } else if lower.starts_with("shift-") || lower.starts_with("shift+") {
        let rest = lower.trim_start_matches("shift-").trim_start_matches("shift+");
        (KeyModifiers::SHIFT, rest)
    } else {
        (KeyModifiers::NONE, lower.as_str())
    };

    let code = match key_part {
        "f1" => KeyCode::F(1), "f2" => KeyCode::F(2), "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4), "f5" => KeyCode::F(5), "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7), "f8" => KeyCode::F(8), "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10), "f11" => KeyCode::F(11), "f12" => KeyCode::F(12),
        "enter" => KeyCode::Enter, "esc" => KeyCode::Esc, "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace, "delete" => KeyCode::Delete,
        "space" => KeyCode::Char(' '), "up" => KeyCode::Up, "down" => KeyCode::Down,
        "left" => KeyCode::Left, "right" => KeyCode::Right, "home" => KeyCode::Home,
        "end" => KeyCode::End, "pageup" => KeyCode::PageUp, "pagedown" => KeyCode::PageDown,
        s if s.chars().count() == 1 => KeyCode::Char(s.chars().next().unwrap()),
        _ => {
            return None;
        }
    };

    Some(KeyEvent {
        code, modifiers: modifier,
        kind: crossterm::event::KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    })
}

pub(crate) fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for c in s.chars() {
        match in_quote {
            Some(q) if c == q => { in_quote = None; }
            None if c == '\'' || c == '"' => { in_quote = Some(c); }
            None if c.is_whitespace() => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => { current.push(c); }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_key() {
        if let ParsedBinding::Key(key, args, silent) = parse_binding("enter=edit {path}").unwrap() {
            assert_eq!(key.code, KeyCode::Enter);
            assert_eq!(args, vec!["edit", "{path}"]);
            assert!(!silent);
        } else {
            panic!("Expected Key binding");
        }
    }

    #[test]
    fn test_parse_silent_key() {
        if let ParsedBinding::Key(key, args, silent) = parse_binding("ctrl-t=@switch-view.sh").unwrap() {
            assert_eq!(key.code, KeyCode::Char('t'));
            assert_eq!(args, vec!["switch-view.sh"]);
            assert!(silent);
        } else {
            panic!("Expected Key binding");
        }
    }

    #[test]
    fn test_parse_signal_binding_global() {
        let config = BindConfig::parse(&["select=@echo {id}".to_string()]);
        let binding = config.get_signal_binding(None, "select").unwrap();
        assert_eq!(binding.0, vec!["echo", "{id}"]);
        assert!(binding.1);
    }

    #[test]
    fn test_parse_signal_binding_scoped() {
        let config = BindConfig::parse(&["TreeA:select=bat {path}".to_string()]);
        let binding_local = config.get_signal_binding(Some("TreeA"), "select").unwrap();
        assert_eq!(binding_local.0, vec!["bat", "{path}"]);
        let binding_global = config.get_signal_binding(None, "select");
        assert!(binding_global.is_none());
        let binding_other = config.get_signal_binding(Some("TreeB"), "select");
        assert!(binding_other.is_none());
    }

    #[test]
    fn test_signal_scoping_fallback() {
        let config = BindConfig::parse(&[
            "select=cat {path}".to_string(),
            "TreeA:select=bat {path}".to_string()
        ]);
        let binding_a = config.get_signal_binding(Some("TreeA"), "select").unwrap();
        assert_eq!(binding_a.0, vec!["bat", "{path}"]);
        let binding_b = config.get_signal_binding(Some("TreeB"), "select").unwrap();
        assert_eq!(binding_b.0, vec!["cat", "{path}"]);
    }

    #[test]
    fn test_split_args_basic() {
        let args = split_args("vi /tmp/note.md");
        assert_eq!(args, vec!["vi", "/tmp/note.md"]);
    }

    #[test]
    fn test_split_args_with_quotes() {
        let args = split_args("vi '/tmp/my note.md'");
        assert_eq!(args, vec!["vi", "/tmp/my note.md"]);

        let args2 = split_args("echo \"hello world\" {path}");
        assert_eq!(args2, vec!["echo", "hello world", "{path}"]);
    }
}
