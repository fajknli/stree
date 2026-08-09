// src/exec/mod.rs

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::collections::HashMap;

const MAX_LINE_CHARS: usize = 500;
const MAX_TOTAL_BYTES: usize = 1024 * 1024;

pub fn replace_placeholders_in_args(
    template_args: &[String],
    context: &HashMap<String, String>,
) -> Vec<String> {
    let mut keys: Vec<&String> = context.keys().collect();
    keys.sort();

    let mut full_args = Vec::new();
    for arg in template_args {
        let mut res = arg.clone();
        for key in &keys {
            if let Some(val) = context.get(*key) {
                res = res.replace(&format!("{{{}}}", key), val);
            }
        }

        // 【修复】{ids} 和 {paths} 可能展开为多个参数（包含空格），需要重新用 split_args 解析
        if res.contains(' ') && (arg.contains("{ids}") || arg.contains("{paths}")) {
            full_args.extend(crate::config::split_args(&res));
        } else {
            full_args.push(res);
        }
    }
    full_args
}

pub fn execute_command_args(cmd_args: &[String], max_lines: usize) -> std::io::Result<(i32, String)> {
    if cmd_args.is_empty() {
        return Ok((0, String::new()));
    }

    let mut child = Command::new(&cmd_args[0])
        .args(&cmd_args[1..])
        .env("FORCE_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .env("TERM", "xterm-256color")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let child_stderr = child.stderr.take();

    let stderr_thread = std::thread::spawn(move || {
        let mut stderr_buf = String::new();
        if let Some(err) = child_stderr {
            let reader = BufReader::new(err);
            for line in reader.lines().flatten() {
                stderr_buf.push_str(&line);
                stderr_buf.push('\n');
                if stderr_buf.len() > 8192 { break; }
            }
        }
        stderr_buf
    });

    let mut stdout_buf = String::new();
    let mut line_count = 0;
    let mut total_bytes = 0;
    let mut killed_due_to_limit = false;
    let mut truncated_lines = 0;

    if let Some(out) = child.stdout.take() {
        let reader = BufReader::new(out);

        for line in reader.lines() {
            let line = line?;

            if total_bytes + line.len() > MAX_TOTAL_BYTES {
                let _ = child.kill();
                killed_due_to_limit = true;
                break;
            }

            line_count += 1;

            if line_count > max_lines {
                let _ = child.kill();
                killed_due_to_limit = true;
                truncated_lines += 1;
                break;
            }

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

    let stderr_buf = stderr_thread.join().unwrap_or_default();

    if truncated_lines > 0 || killed_due_to_limit {
        stdout_buf.push_str(&format!("\n... [stree: output truncated due to limits] ...\n"));
    }

    let status = child.wait()?;
    let code = status.code().unwrap_or(-1);

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

// 【纯粹哲学】静默执行：不碰 stdout/stderr，不碰文件，杜绝一切死锁，只看退出码
pub fn execute_command_silent(cmd_args: &[String]) -> std::io::Result<i32> {
    if cmd_args.is_empty() {
        return Ok(0);
    }

    let status = Command::new(&cmd_args[0])
        .args(&cmd_args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    Ok(status.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder_expansion() {
        let mut ctx = HashMap::new();
        ctx.insert("path".to_string(), "/path/with spaces.md".to_string());

        let template = vec!["vim".into(), "{path}".into()];
        let result = replace_placeholders_in_args(&template, &ctx);
        assert_eq!(result, vec!["vim", "/path/with spaces.md"]);
    }

    #[test]
    fn test_empty_context() {
        let ctx = HashMap::new();
        let template = vec!["echo".into(), "{id}".into()];
        let result = replace_placeholders_in_args(&template, &ctx);
        assert_eq!(result, vec!["echo", "{id}"]);
    }
}
