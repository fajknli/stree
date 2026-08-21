// src/runner.rs

use crate::app::Engine;
use crossterm::{terminal, ExecutableCommand, cursor, event::{EnableMouseCapture, DisableMouseCapture}};
use std::io::{stdout, Write};
use std::process::{Command, Stdio};
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
    _columns: u16,
    _rows: u16,
) {
    if is_silent {
        // 【防死锁修复】静默执行必须放入后台线程，绝不能用 status() 阻塞主线程！
        let tx = engine.async_exec_tx.clone();
        let args: Vec<String> = full_cmd_args.to_vec();

        std::thread::spawn(move || {
            let status = Command::new(&args[0])
                .args(&args[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            if let Ok(s) = status {
                if s.success() {
                    let _ = tx.send(()); // 脚本成功退出后，通知主线程刷新
                }
            }
        });

        drain_terminal_events();
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

        // 交互式命令(如 vim)退出后，直接在主线程同步刷新
        refresh_engine_state(engine, _columns, _rows);

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

    // 3. 寻找有效的 Tree 上下文来刷新 View
    let tree_to_refresh = match &engine.focus.current {
        crate::app::Focus::Component(n) if matches!(engine.components.get(n), Some(crate::app::Component::Tree(_))) => Some(n.clone()),
        _ => engine.focus.main_tree_name.clone(),
    };

    if let Some(tree_name) = tree_to_refresh {
        engine.broadcast_selection_changed(&tree_name, columns, rows);
    }
}
