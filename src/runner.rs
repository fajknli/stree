// src/runner.rs

use crate::app::Engine;
use crate::exec;
use crossterm::{terminal, ExecutableCommand, cursor, event::{EnableMouseCapture, DisableMouseCapture}};
use std::io::{stdout, Write};
use std::process::Command;
use std::time::Duration;

pub fn drain_terminal_events() {
    let mut drain_count = 0;
    while crossterm::event::poll(Duration::ZERO).unwrap_or(false) && drain_count < 100 {
        let _ = crossterm::event::read();
        drain_count += 1;
    }
}

pub fn execute_binding(
    engine: &mut Engine,
    full_cmd_args: &[String],
    is_silent: bool,
    columns: u16,
    rows: u16,
) {
    if is_silent {
        match exec::execute_command_silent(full_cmd_args) {
            Ok(code) => {
                if code == 0 {
                    // 1. 重新加载所有 Tree 组件的数据
                    engine.trigger_reload(columns, rows);

                    // 2. 静默命令可能修改了文件内容，清空 View 缓存强制刷新
                    for comp in engine.components.values_mut() {
                        if let crate::app::Component::View(v) = comp {
                            v.cached_entity_id = None;
                        }
                    }
                    if let crate::app::Focus::Component(name) = &engine.focus.current.clone() {
                        let name = name.clone();
                        engine.broadcast_selection_changed(&name, columns, rows);
                    }
                } else {
                    engine.last_error = Some(format!("Command failed with code {}", code));
                }
            }
            Err(e) => {
                engine.last_error = Some(format!("Failed to execute: {}", e));
            }
        }
        drain_terminal_events()
    } else {
        // 【终极修复】非静默命令（如 vi, less 等）通常是交互式 TUI 程序。
        // 它们必须直接接管终端的 stdin/stdout/stderr。
        // 绝不能使用 exec::execute_command_args（它会 pipe stdout），
        // 否则子进程会发现 stdout 不是 TTY，导致 vi 无法渲染界面并死锁卡死！

        let mut out = stdout();

        // 1. 挂起 stree 的 TUI 接管状态，把终端还原给普通 Shell
        let _ = terminal::disable_raw_mode();
        let _ = out.execute(DisableMouseCapture);
        let _ = out.execute(terminal::LeaveAlternateScreen);
        let _ = out.execute(cursor::Show);
        let _ = out.flush();

        // 2. 直接继承终端执行命令，不捕获 stdout
        let status = Command::new(&full_cmd_args[0])
            .args(&full_cmd_args[1..])
            .status();

        // 3. 命令结束后，立刻恢复 stree 的 TUI 接管状态
        let _ = out.execute(terminal::EnterAlternateScreen);
        let _ = out.execute(EnableMouseCapture);
        let _ = out.execute(cursor::Hide);
        let _ = terminal::enable_raw_mode();
        let _ = out.flush();

        // 4. 处理返回结果
        match status {
            Ok(s) => {
                if !s.success() {
                    engine.last_error = Some(format!("Command exited with code {}", s.code().unwrap_or(-1)));
                }
            }
            Err(e) => {
                engine.last_error = Some(format!("Failed to execute: {}", e));
            }
        }

        // 【关键修复】交互式命令（如 nvim 修改文件后），也必须强制刷新数据和预览！
        // 否则退回 stree 后 Preview 依然是旧内容。
        engine.trigger_reload(columns, rows);
        for comp in engine.components.values_mut() {
            if let crate::app::Component::View(v) = comp {
                v.cached_entity_id = None;
            }
        }
        if let crate::app::Focus::Component(name) = &engine.focus.current.clone() {
            let name = name.clone();
            engine.broadcast_selection_changed(&name, columns, rows);
        }

        // 5. 清理在阻塞期间积压的终端事件，并强制下一帧全屏重绘
        drain_terminal_events();
        engine.mark_all_dirty();
        engine.prev_rects.clear();
    }
}
