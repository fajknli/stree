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
    // 【修复缺陷】收集 keys 并排序，确保嵌套占位符替换的确定性
    let mut keys: Vec<&String> = context.keys().collect();
    keys.sort();

    template_args.iter().map(|arg| {
        let mut res = arg.clone();
        // 【修复 E0507】使用 &keys 进行引用遍历，避免消耗 keys 的所有权
        for key in &keys {
            if let Some(val) = context.get(*key) {
                res = res.replace(&format!("{{{}}}", key), val);
            }
        }
        res
    }).collect()
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

    // 【修复 Bug #2】使用独立线程读取 stderr，防止管道死锁
    let stderr_thread = std::thread::spawn(move || {
        let mut stderr_buf = String::new();
        if let Some(err) = child_stderr {
            let reader = BufReader::new(err);
            for line in reader.lines().flatten() {
                stderr_buf.push_str(&line);
                stderr_buf.push('\n');
                if stderr_buf.len() > 1024 { break; }
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
