// src/runner.rs

use crate::app::Engine;
use crate::exec;
use crossterm::{terminal, ExecutableCommand, cursor, event::{EnableMouseCapture, DisableMouseCapture}};
use std::io::{stdout, Write};
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
                if code != 0 {
                    engine.last_error = Some(format!("Silent cmd exited with code {}", code));
                }
            }
            Err(e) => {
                engine.last_error = Some(e.to_string());
            }
        }
        drain_terminal_events();
    } else {
        let _ = terminal::disable_raw_mode();
        let _ = stdout().execute(DisableMouseCapture);
        let _ = stdout().execute(terminal::LeaveAlternateScreen);
        let _ = stdout().execute(cursor::Show);
        let _ = stdout().flush();

        let mut cmd = std::process::Command::new(&full_cmd_args[0]);
        cmd.args(&full_cmd_args[1..]);
        if let Ok(tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") {
            if let Ok(stdin_c) = tty.try_clone() { cmd.stdin(stdin_c); }
            if let Ok(stdout_c) = tty.try_clone() { cmd.stdout(stdout_c); }
            cmd.stderr(tty);
        }

        let status = cmd.status();
        drain_terminal_events();

        let _ = stdout().execute(cursor::Hide);
        let _ = stdout().execute(terminal::EnterAlternateScreen);
        let _ = stdout().execute(EnableMouseCapture);
        let _ = terminal::enable_raw_mode();

        match status {
            Ok(s) => {
                if s.success() { engine.trigger_reload(columns, rows); }
                else { engine.last_error = Some(format!("退出码: {}", s.code().unwrap_or(-1))); }
            }
            Err(e) => { engine.last_error = Some(e.to_string()); }
        }
    }
}
