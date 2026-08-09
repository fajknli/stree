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
                    // 成功：刷新世界
                    refresh_engine_state(engine, columns, rows);
                } else {
                    // 【彻底放权】失败时，引擎保持沉默，绝不设置 last_error！
                    // 错误提示的责任 100% 交给脚本通过 IPC (ipc_msg) 主动推送到 Status 栏。
                    // Status 栏是透明背景，完美符合“无红色背景”的需求。
                }
            }
            Err(e) => {
                // 只有 Rust 引擎层面的失败（如找不到可执行文件、权限拒绝）才兜底报错
                engine.last_error = Some(format!("Exec failed: {}", e));
            }
        }
        drain_terminal_events()
    } else {
        let mut out = stdout();

        // 1. 挂起 TUI，让出终端控制权给交互式程序
        let _ = terminal::disable_raw_mode();
        let _ = out.execute(DisableMouseCapture);
        let _ = out.execute(terminal::LeaveAlternateScreen);
        let _ = out.execute(cursor::Show);
        let _ = out.flush();

        // 2. 继承终端执行 (不捕获任何管道)
        let status = Command::new(&full_cmd_args[0])
            .args(&full_cmd_args[1..])
            .status();

        // 3. 恢复 TUI 接管
        let _ = out.execute(terminal::EnterAlternateScreen);
        let _ = out.execute(EnableMouseCapture);
        let _ = out.execute(cursor::Hide);
        let _ = terminal::enable_raw_mode();
        let _ = out.flush();

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

        // 通用哲学：从外部交互式命令返回，必定触发一次刷新
        refresh_engine_state(engine, columns, rows);

        drain_terminal_events();
        engine.mark_all_dirty();
        engine.prev_rects.clear();
    }
}

// 引擎状态刷新的通用函数
fn refresh_engine_state(engine: &mut Engine, columns: u16, rows: u16) {
    // 1. 重新加载所有 Tree 组件的数据源
    engine.trigger_reload();

    // 2. 清空所有 View 的缓存，强制它们在下一帧重新执行命令获取最新内容
    for comp in engine.components.values_mut() {
        if let crate::app::Component::View(v) = comp {
            v.cached_entity_id = None;
        }
    }

    // 3. 广播当前选中状态，让刚被清空缓存的 View 立刻去加载
    if let crate::app::Focus::Component(name) = &engine.focus.current.clone() {
        let name = name.clone();
        engine.broadcast_selection_changed(&name, columns, rows);
    }
}
