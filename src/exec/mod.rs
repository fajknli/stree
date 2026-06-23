// src/exec/mod.rs

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use crate::protocol::Entity;

const MAX_LINES: usize = 300;
const MAX_LINE_CHARS: usize = 500;
const MAX_TOTAL_BYTES: usize = 1024 * 1024;

/// 【扩展占位符】支持 {id}, {path}, {display}, {status}, {ids}, {paths}, {window}, {width}, {height}
pub fn replace_placeholders_in_args(
    template_args: &[String],
    selected_entity: Option<&Entity>,
    ids_str: &str,
    paths_str: &str,
    window_name: &str,
    width: &str,
    height: &str,
) -> Vec<String> {
    template_args.iter().map(|arg| {
        let mut res = arg.clone();
        if let Some(entity) = selected_entity {
            res = res.replace("{id}", &entity.id);
            res = res.replace("{path}", &entity.path);
            res = res.replace("{display}", &entity.display);
            res = res.replace("{tags}", &entity.tags);
        } else {
            res = res.replace("{id}", "");
            res = res.replace("{path}", "");
            res = res.replace("{display}", "");
            res = res.replace("{tags}", "");
        }
        res = res.replace("{ids}", ids_str);
        res = res.replace("{paths}", paths_str);
        res = res.replace("{window}", window_name);
        res = res.replace("{width}", width);
        res = res.replace("{height}", height);
        res
    }).collect()
}

pub fn execute_command_args(cmd_args: &[String]) -> std::io::Result<(i32, String)> {
    if cmd_args.is_empty() {
        return Ok((0, String::new()));
    }

    let mut child = Command::new(&cmd_args[0])
        .args(&cmd_args[1..])
        .env("FORCE_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .env("TERM", "xterm-256color")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()) // 捕获 stderr
        .spawn()?;

    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();
    let mut line_count = 0;
    let mut truncated_lines = 0;
    let mut total_bytes = 0;
    let mut killed_due_to_limit = false;

    if let Some(out) = child.stdout.take() {
        let reader = BufReader::new(out);

        for line in reader.lines() {
            let line = line?;

            if total_bytes + line.len() > MAX_TOTAL_BYTES {
                child.kill()?;
                killed_due_to_limit = true;
                break;
            }

            line_count += 1;

            if line_count > MAX_LINES {
                child.kill()?;
                killed_due_to_limit = true;
                truncated_lines += 1;
                break;
            }

            // 性能优化：不遍历整行，只取需要的长度
            let final_line = if line.len() > MAX_LINE_CHARS * 4 {
                let mut s: String = line.chars().take(MAX_LINE_CHARS - 3).collect();
                s.push_str("...");
                s
            } else {
                let mut iter = line.chars();
                let s: String = iter.by_ref().take(MAX_LINE_CHARS - 3).collect();
                if iter.next().is_some() {
                    format!("{}...", s)
                } else {
                    line
                }
            };

            total_bytes += final_line.len() + 1;
            stdout_buf.push_str(&final_line);
            stdout_buf.push('\n');
        }
    }

    // 读取 stderr
    if let Some(err) = child.stderr.take() {
        let reader = BufReader::new(err);
        for line in reader.lines() {
            let line = line?;
            stderr_buf.push_str(&line);
            stderr_buf.push('\n');
            if stderr_buf.len() > 1024 { break; }
        }
    }

    if truncated_lines > 0 || killed_due_to_limit {
        stdout_buf.push_str(&format!("\n... [stree: output truncated due to limits] ...\n"));
    }

    let status = child.wait()?;
    let code = status.code().unwrap_or(-1);

    // 如果退出码非0且 stdout 为空，返回 stderr 内容
    if code != 0 && stdout_buf.trim().is_empty() && !stderr_buf.trim().is_empty() {
        return Ok((code, format!("[ERR] {}\n", stderr_buf.trim())));
    }

    Ok((code, stdout_buf))
}

pub fn execute_reload_hook(hook_cmd: Option<&str>) -> std::io::Result<String> {
    let Some(cmd) = hook_cmd else { return Ok(String::new()); };
    let cmd = cmd.trim();
    if cmd.is_empty() { return Ok(String::new()); }

    let parts = crate::config::split_args(cmd);
    if parts.is_empty() { return Ok(String::new()); }

    let output = Command::new(&parts[0])
        .args(&parts[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()?;

    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("reload-hook 退出码非零: {:?}", output.status.code())
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// 【新增】静默执行命令：不接管 TTY，不输出到屏幕，绝不阻塞 UI
pub fn execute_command_silent(cmd_args: &[String]) -> std::io::Result<i32> {
    if cmd_args.is_empty() {
        return Ok(0);
    }

    // 【核心防御】将三个标准流全部设为 null，切断管道继承。
    // 使用 .status() 代替 .output()，只等主进程退出，绝不读取管道，彻底杜绝 fd 继承死锁。
    let status = Command::new(&cmd_args[0])
        .args(&cmd_args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null()) // 彻底丢弃 stderr，换取绝对的 UI 安全
        .status()?;

    Ok(status.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Entity;

    #[test]
    fn test_placeholder_expansion() {
        let entity = Entity {
            id: "U-01".into(),
            display: "Test".into(),
            path: "/path/with spaces.md".into(),
            tags: "live".into(),
        };

        let template = vec!["vim".into(), "{path}".into()];
        let result = replace_placeholders_in_args(
            &template,
            Some(&entity),
            "U-01",
            "\"/path/with spaces.md\"",
            "Main",
            "80",
            "24",
        );

        assert_eq!(result, vec!["vim", "/path/with spaces.md"]);
    }

    #[test]
    fn test_paths_quote_escaping() {
        let entity = Entity {
            id: "U-01".into(),
            display: "Test".into(),
            path: "/path/with\"quote.md".into(),
            tags: "live".into(),
        };

        let template = vec!["echo".into(), "{paths}".into()];
        let result = replace_placeholders_in_args(
            &template,
            Some(&entity),
            "U-01",
            "\"/path/with\\\"quote.md\"",
            "Main",
            "80",
            "24",
        );

        assert_eq!(result[1], "\"/path/with\\\"quote.md\"");
    }

    #[test]
    fn test_empty_entity() {
        let template = vec!["echo".into(), "{id}".into(), "{path}".into()];
        let result = replace_placeholders_in_args(
            &template,
            None,
            "",
            "",
            "Main",
            "80",
            "24",
        );

        assert_eq!(result, vec!["echo", "", ""]);
    }
}
