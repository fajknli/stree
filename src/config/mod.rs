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

enum ParsedBinding {
    Key(Option<String>, KeyEvent, Vec<String>, bool), // (Scope, Key, Args, IsSilent)
    Signal(Option<String>, String, Vec<String>, bool),
}

#[derive(Debug, Clone)]
pub struct BindConfig {
    // 结构改为：HashMap<作用域, HashMap<按键, (命令, 是否静默)>>
    // None 作用域代表全局基础层
    bindings: HashMap<Option<String>, HashMap<KeyBindingKey, (Vec<String>, bool)>>,
    signal_bindings: HashMap<(Option<String>, String), (Vec<String>, bool)>,
}

impl BindConfig {
    pub fn new() -> Self {
        let mut config = Self {
            bindings: HashMap::new(),
            signal_bindings: HashMap::new(),
        };
        let defaults = vec![
            ("q", "__EXIT__"), ("esc", "__CLOSE_TOP_OVERLAY__"),
            ("tab", "__CYCLE_LAYER__"),
            ("up", "__UP__"), ("k", "__UP__"), ("down", "__DOWN__"), ("j", "__DOWN__"),
            ("left", "__EXPAND__"), ("h", "__EXPAND__"), ("right", "__EXPAND__"), ("l", "__EXPAND__"),
            ("space", "__MARK__"), ("g", "__TOP__"), ("G", "__BOTTOM__"),
            ("H", "__SCROLL_LEFT__"), ("L", "__SCROLL_RIGHT__"),
            ("enter", "__ENTER__"),
            ("ctrl-h", "__FOCUS_LEFT__"), ("ctrl-l", "__FOCUS_RIGHT__"),
            ("ctrl-k", "__FOCUS_UP__"), ("ctrl-j", "__FOCUS_DOWN__"),
        ];

        let global_map = config.bindings.entry(None).or_default();
        for (key_desc, cmd) in defaults {
            if let Some(key_event) = parse_key_desc(key_desc) {
                let args = split_args(cmd);
                let key = KeyBindingKey(key_event.code, key_event.modifiers);
                global_map.insert(key, (args, false));
            }
        }
        config
    }

    pub fn parse(bind_args: &[String]) -> Self {
        let mut config = BindConfig::new();
        for arg in bind_args {
            if let Some(parsed) = parse_binding(arg) {
                match parsed {
                    ParsedBinding::Key(scope, key_event, args, is_silent) => {
                        let key = KeyBindingKey(key_event.code, key_event.modifiers);
                        let map = config.bindings.entry(scope).or_default();
                        map.insert(key, (args, is_silent));
                    }
                    ParsedBinding::Signal(scope, signal_name, args, is_silent) => {
                        config.signal_bindings.insert((scope, signal_name), (args, is_silent));
                    }
                }
            }
        }
        config
    }

    pub fn get_scoped(&self, scope: Option<&str>, key: &KeyEvent) -> Option<&(Vec<String>, bool)> {
        let scope_str = scope.map(|s| s.to_string());
        if let Some(map) = self.bindings.get(&scope_str) {
            let mut code = key.code;
            let mut modifiers = key.modifiers;

            if modifiers == KeyModifiers::SHIFT {
                if let KeyCode::Char(c) = code {
                    if c.is_ascii_alphabetic() {
                        modifiers = KeyModifiers::NONE;
                        if c.is_ascii_lowercase() {
                            code = KeyCode::Char(c.to_ascii_uppercase());
                        }
                    }
                }
            }

            let k = KeyBindingKey(code, modifiers);
            if let Some(res) = map.get(&k) {
                return Some(res);
            }
        }
        None
    }

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

    // 解析 Scope:Key 格式
    let (scope, key_desc) = if let Some(colon_pos) = left_side.rfind(':') {
        let potential_scope = left_side[..colon_pos].trim();
        let potential_key = left_side[colon_pos + 1..].trim();
        // 如果右边部分能解析为按键，且左边不是空的，就认为是带作用域的
        if !potential_scope.is_empty() && parse_key_desc(potential_key).is_some() {
            (Some(potential_scope.to_string()), potential_key)
        } else {
            (None, left_side)
        }
    } else {
        (None, left_side)
    };

    if let Some(key_event) = parse_key_desc(key_desc) {
        return Some(ParsedBinding::Key(scope, key_event, args, is_silent));
    }

    let (signal_scope, signal_name) = if let Some(colon_pos) = left_side.find(':') {
        let s = left_side[..colon_pos].trim().to_string();
        let sig = left_side[colon_pos + 1..].trim().to_string();
        if s.is_empty() || sig.is_empty() {
            eprintln!("[WARN] 信号绑定格式无效: {}", trimmed);
            return None;
        }
        (Some(s), sig)
    } else {
        (None, left_side.to_string())
    };

    Some(ParsedBinding::Signal(signal_scope, signal_name, args, is_silent))
}

fn parse_key_desc(desc: &str) -> Option<KeyEvent> {
    let lower = desc.to_lowercase();

    let (modifier, key_part_original) = if lower.starts_with("ctrl-") || lower.starts_with("ctrl+") {
        (KeyModifiers::CONTROL, &desc[5..])
    } else if lower.starts_with("alt-") || lower.starts_with("alt+") {
        (KeyModifiers::ALT, &desc[4..])
    } else if lower.starts_with("shift-") || lower.starts_with("shift+") {
        (KeyModifiers::SHIFT, &desc[6..])
    } else {
        (KeyModifiers::NONE, desc)
    };

    let key_part = key_part_original.to_lowercase();

    let code = match key_part.as_str() {
        "f1" => KeyCode::F(1), "f2" => KeyCode::F(2), "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4), "f5" => KeyCode::F(5), "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7), "f8" => KeyCode::F(8), "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10), "f11" => KeyCode::F(11), "f12" => KeyCode::F(12),
        "enter" => KeyCode::Enter, "esc" => KeyCode::Esc, "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace, "delete" => KeyCode::Delete,
        "space" => KeyCode::Char(' '), "up" => KeyCode::Up, "down" => KeyCode::Down,
        "left" => KeyCode::Left, "right" => KeyCode::Right, "home" => KeyCode::Home,
        "end" => KeyCode::End, "pageup" => KeyCode::PageUp, "pagedown" => KeyCode::PageDown,
        s if s.chars().count() == 1 => {
            let c = key_part_original.chars().next().unwrap();
            KeyCode::Char(c)
        },
        _ => {
            return None;
        }
    };

    let final_modifiers = if modifier == KeyModifiers::SHIFT {
        if let KeyCode::Char(c) = code {
            if c.is_ascii_alphabetic() {
                KeyModifiers::NONE
            } else {
                modifier
            }
        } else {
            modifier
        }
    } else {
        modifier
    };

    let final_code = if let (KeyCode::Char(c), KeyModifiers::NONE) = (code, final_modifiers) {
        if c.is_ascii_lowercase() && modifier == KeyModifiers::SHIFT {
            KeyCode::Char(c.to_ascii_uppercase())
        } else {
            code
        }
    } else {
        code
    };

    Some(KeyEvent {
        code: final_code,
        modifiers: final_modifiers,
        kind: crossterm::event::KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    })
}

pub(crate) fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next_c) = chars.peek() {
                current.push(next_c);
                chars.next();
            }
            continue;
        }

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
        if let ParsedBinding::Key(scope, key, args, silent) = parse_binding("enter=edit {path}").unwrap() {
            assert_eq!(scope, None);
            assert_eq!(key.code, KeyCode::Enter);
            assert_eq!(args, vec!["edit", "{path}"]);
            assert!(!silent);
        } else {
            panic!("Expected Key binding");
        }
    }

    #[test]
    fn test_parse_scoped_key() {
        if let ParsedBinding::Key(scope, key, args, _silent) = parse_binding("RenameInput:enter=edit {path}").unwrap() {
            assert_eq!(scope, Some("RenameInput".to_string()));
            assert_eq!(key.code, KeyCode::Enter);
            assert_eq!(args, vec!["edit", "{path}"]);
        } else {
            panic!("Expected Scoped Key binding");
        }
    }

    #[test]
    fn test_parse_silent_key() {
        if let ParsedBinding::Key(_, key, args, silent) = parse_binding("ctrl-t=@switch-view.sh").unwrap() {
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
