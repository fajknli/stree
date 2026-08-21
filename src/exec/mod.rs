// src/exec/mod.rs

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::os::unix::process::CommandExt;

const MAX_LINE_CHARS: usize = 500;
const MAX_TOTAL_BYTES: usize = 1024 * 1024 * 4;

// 杀掉整个进程树的标准 POSIX 函数
pub fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL); }
    }
}

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

        if res.contains(' ') && (arg.contains("{ids}") || arg.contains("{paths}")) {
            full_args.extend(crate::config::split_args(&res));
        } else {
            full_args.push(res);
        }
    }
    full_args
}

pub fn execute_command_args(
    cmd_args: &[String],
    max_lines: usize,
    child_pid: Arc<Mutex<Option<u32>>>
) -> std::io::Result<(i32, Vec<u8>, bool)> {
    if cmd_args.is_empty() {
        return Ok((0, Vec::new(), false));
    }

    let mut command = Command::new(&cmd_args[0]);
    command.args(&cmd_args[1..])
        .env("FORCE_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0); // 放入独立进程组

    let mut child = command.spawn()?;

    // 记录 PID 供主线程取消使用
    *child_pid.lock().unwrap() = Some(child.id());

    let child_stdout = child.stdout.take();
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

    let mut stdout_buf: Vec<u8> = Vec::new();
    let mut is_graphic = false;

    if let Some(out) = child_stdout {
        let mut reader = BufReader::new(out);

        let mut peek_buf = [0u8; 15];
        let n = reader.read(&mut peek_buf)?;
        let peek_str = String::from_utf8_lossy(&peek_buf[..n]);

        if peek_str.starts_with("STREE_GRAPHIC\n") {
            stdout_buf = parse_graphic_output(&mut reader, &peek_buf, n)?;
            is_graphic = true;
        } else {
            let cursor = std::io::Cursor::new(peek_buf[..n].to_vec());
            let mut chained = cursor.chain(reader);
            stdout_buf = parse_text_output(&mut chained, max_lines)?;
        }
    }

    let stderr_buf = stderr_thread.join().unwrap_or_default();

    // 清理 PID 记录并 wait 子进程
    child_pid.lock().unwrap().take();
    let status = child.wait()?;
    let code = status.code().unwrap_or(-1);

    if code != 0 && stdout_buf.is_empty() && !stderr_buf.trim().is_empty() {
        return Ok((code, format!("[ERR] {}\n", stderr_buf.trim()).into_bytes(), false));
    }

    Ok((code, stdout_buf, is_graphic))
}

pub fn execute_reload_hook(hook_cmd: Option<&str>) -> std::io::Result<String> {
    let Some(cmd) = hook_cmd else { return Ok(String::new()); };
    let cmd = cmd.trim();
    if cmd.is_empty() { return Ok(String::new()); }

    let parts = crate::config::split_args(cmd);
    if parts.is_empty() { return Ok(String::new()); }

    let output = Command::new(&parts[0])
        .args(&parts[1..])
        .stdin(Stdio::null())
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

// ================= 输出解析提取逻辑 =================

fn parse_graphic_output(
    reader: &mut impl std::io::Read,
    peek_buf: &[u8],
    n: usize,
) -> std::io::Result<Vec<u8>> {
    let mut rest_buf = Vec::new();
    reader.read_to_end(&mut rest_buf)?;

    let mut full_bytes = peek_buf[..n].to_vec();
    full_bytes.extend_from_slice(&rest_buf);

    if full_bytes.len() > MAX_TOTAL_BYTES {
        full_bytes.truncate(MAX_TOTAL_BYTES);
    }

    if let Some(pos) = full_bytes.iter().position(|&b| b == b'\n') {
        let graphic_bytes = full_bytes[pos+1..].to_vec();

        let mut filtered_bytes = Vec::with_capacity(graphic_bytes.len());
        let mut i = 0;
        while i < graphic_bytes.len() {
            if graphic_bytes[i] == 0x1b && i + 1 < graphic_bytes.len() && graphic_bytes[i+1] == b']' {
                i += 2;
                while i < graphic_bytes.len() {
                    if graphic_bytes[i] == 0x07 { i += 1; break; }
                    if graphic_bytes[i] == 0x1b && i + 1 < graphic_bytes.len() && graphic_bytes[i+1] == b'\\' { i += 2; break; }
                    i += 1;
                }
            } else {
                filtered_bytes.push(graphic_bytes[i]);
                i += 1;
            }
        }
        Ok(filtered_bytes)
    } else {
        Ok(full_bytes)
    }
}

fn parse_text_output(
    reader: &mut impl std::io::BufRead,
    max_lines: usize,
) -> std::io::Result<Vec<u8>> {
    let mut text_buf = String::new();
    let mut total_bytes = 0;
    let mut line_count = 0;
    let mut truncated_lines = 0;

    for line in reader.lines() {
        let line = line?;

        if total_bytes + line.len() > MAX_TOTAL_BYTES {
            break;
        }

        line_count += 1;

        if line_count > max_lines {
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
        text_buf.push_str(&final_line);
        text_buf.push('\n');
    }

    if truncated_lines > 0 {
        text_buf.push_str(&format!("\n... [stree: output truncated due to limits] ...\n"));
    }
    Ok(text_buf.into_bytes())
}
