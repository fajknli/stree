// src/main.rs

mod app;
mod config;
mod exec;
mod ipc;
mod layout;
mod protocol;
mod search;
mod signal;
mod style;
mod tree;
mod ui;

use clap::{Parser, Subcommand};
use crate::app::Component;
use crossterm::{terminal, ExecutableCommand};
use crossterm::event::{self, Event, KeyCode, EnableMouseCapture, DisableMouseCapture};
use std::io::{self, stdout, Read, Write, IsTerminal};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long)]
    relations: Option<String>,

    /// 布局字符串，只管格子位置/大小/边框/名字
    /// 语法: area(size)[border]:Name
    /// 例: vertical(horizontal(area(50%):Tree, area(50%):Preview), area(1)[none]:Status)
    // 【修改】默认值改为中性的单窗口全屏布局，不再硬编码业务名称
    #[arg(long, default_value = "area:Main")]
    layout: String,

    #[arg(long = "tree", action = clap::ArgAction::Append)]
    trees: Vec<String>,
    #[arg(long = "view", action = clap::ArgAction::Append)]
    views: Vec<String>,
    #[arg(long = "statusbar", action = clap::ArgAction::Append)]
    statusbars: Vec<String>,
    #[arg(long = "input", action = clap::ArgAction::Append)]
    inputs: Vec<String>,
    #[arg(long = "overlay", action = clap::ArgAction::Append)]
    overlays: Vec<String>,

    #[arg(long = "bind", action = clap::ArgAction::Append)]
    binds: Vec<String>,
    #[arg(long = "border-chars", action = clap::ArgAction::Append)]
    border_chars: Vec<String>,
    #[arg(long, default_value = "")]
    status_col: String,
    #[arg(long)]
    select: Option<String>,
    #[arg(long = "no-mouse", action = clap::ArgAction::SetTrue)]
    no_mouse: bool,
    #[arg(long, default_value_t = 3)]
    scroll_step: u8,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Update { target: String },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if let Some(Commands::Update { target }) = cli.command {
        return ipc::run_ctrl_command(&target);
    }

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = terminal::disable_raw_mode();
        let _ = stdout().execute(DisableMouseCapture);
        let _ = stdout().execute(terminal::LeaveAlternateScreen);
        let _ = stdout().execute(crossterm::cursor::Show);
        if let Ok(sock_path) = std::env::var("STREE_SOCK") { let _ = std::fs::remove_file(sock_path); }
        original_hook(panic_info);
    }));

    let ipc_server = ipc::IpcServer::new()?;
    std::env::set_var("STREE_SOCK", ipc_server.socket_path.clone());

    if let Err(e) = signal::init_signal_handler() {
        eprintln!("[WARN] 信号监听初始化失败: {}", e);
    }

    let layout = layout::parse_layout(&cli.layout);
    let key_bindings = config::BindConfig::parse(&cli.binds);

    let full_dataset = if std::io::stdin().is_terminal() {
        protocol::Dataset::new()
    } else {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        let cursor = std::io::Cursor::new(input);
        protocol::parse_entities(cursor)?
    };

    let style_engine = style::StyleEngine::parse(&cli.status_col);

    let mut engine = app::Engine::new(
        full_dataset,
        layout,
        key_bindings,
        !cli.no_mouse,
        cli.border_chars,
        cli.trees,
        cli.views,
        cli.statusbars,
        cli.inputs,
        cli.relations.clone(),
        cli.overlays,
    );

    if let Some(ref id) = cli.select {
        let focused_name = if let app::Focus::Component(name) = &engine.focused { name.clone() } else { String::new() };
        if !focused_name.is_empty() {
            engine.select_id(&focused_name, id);
        }
    }

    stdout().execute(terminal::EnterAlternateScreen)?;
    stdout().execute(EnableMouseCapture)?;
    stdout().execute(crossterm::cursor::Hide)?;
    terminal::enable_raw_mode()?;

    let scroll_step = cli.scroll_step;
    let mut last_event_time = std::time::Instant::now();

    'main_loop: loop {
        if signal::check_and_clear_quit() { break 'main_loop; }

        // SIGUSR1 触发热重载
        if signal::check_and_clear_reload() {
            engine.trigger_reload();
        }

        ipc_server.try_accept_and_process(|target, data| {
            engine.handle_ipc_update(target, data);
        });

        let (columns, rows) = match crossterm::terminal::size() { Ok(s) => s, Err(_) => continue };
        let term_size = crossterm::terminal::WindowSize { width: columns, height: rows, columns, rows };

        // 【核心修复】计算各 View 窗口的最大滚动偏移量和真实几何尺寸
        let rects = crate::layout::calc_window_rects(&engine.layout, columns, rows);
        let mut view_rects_info = std::collections::HashMap::new();
        for (rect, name, border) in rects.iter() {
            if let Some(app::Component::View(_)) = engine.components.get(name) {
                let inner_h = match border {
                    crate::layout::BorderStyle::Box => (rect.height as usize).saturating_sub(2),
                    _ => rect.height as usize,
                };
                let inner_w = match border {
                    crate::layout::BorderStyle::Box => rect.width.saturating_sub(2),
                    _ => rect.width,
                };
                // 传入 (max_rows, width, height)
                view_rects_info.insert(name.clone(), (inner_h, inner_w, inner_h as u16));
            }
        }
        // 统一更新 View 的尺寸和滚动上限
        engine.update_view_rects(view_rects_info);

        let ctx = ui::RenderCtx { engine: &engine, style_engine: &style_engine, term_size };

        let mut out = stdout();
        if let Err(e) = ui::render_all(&ctx, &mut out) {
            terminal::disable_raw_mode()?;
            stdout().execute(DisableMouseCapture)?;
            stdout().execute(terminal::LeaveAlternateScreen)?;
            return Err(e.into());
        }

        let poll_timeout = if engine.has_active_input() {
            Duration::from_millis(16)
        } else {
            if last_event_time.elapsed() < Duration::from_secs(1) { Duration::from_millis(10) }
            else if last_event_time.elapsed() > Duration::from_secs(5) { Duration::from_millis(200) }
            else { Duration::from_millis(50) }
        };

        if event::poll(poll_timeout)? {
            last_event_time = std::time::Instant::now();
            let ev = event::read()?;
            match ev {
                Event::Key(key_event) => {
                    if key_event.kind != crossterm::event::KeyEventKind::Press { continue 'main_loop; }
                    if engine.last_error.is_some() { engine.last_error = None; }

                    let focused_name = if let app::Focus::Component(name) = &engine.focused { Some(name.clone()) } else { None };

                    // Input 模式拦截（优先级高于搜索模式）
                    if engine.has_active_input() {
                        match key_event.code {
                            KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace | KeyCode::Left | KeyCode::Right
                            | KeyCode::Home | KeyCode::End | KeyCode::Char(_) => {
                                if let Some((input_name, result)) = engine.handle_input_key(key_event) {
                                    if result == "__CANCEL__" {
                                        // 取消，什么都不做
                                    } else {
                                        if let Some(app::Component::Input(input)) = engine.components.get(&input_name) {
                                            if input.prefix == "/" {
                                                // 搜索：内部消化
                                                engine.apply_search(&result);
                                            } else if let Some(ref cmd_template) = input.on_submit {
                                                // 命令：走外部脚本
                                                let cmd = cmd_template.replace("{input}", &result);
                                                let args = crate::config::split_args(&cmd);
                                                if !args.is_empty() {
                                                    match exec::execute_command_silent(&args) {
                                                        Ok(code) => {
                                                            if code != 0 {
                                                                engine.last_error = Some(format!("Input cmd exited with code {}", code));
                                                            }
                                                        }
                                                        Err(e) => {
                                                            engine.last_error = Some(e.to_string());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                continue 'main_loop;
                            }
                            _ => {}
                        }
                    }

                    // 普通按键
                    match key_event.code {
                        _ => {
                            if let Some((full_cmd_args, is_silent)) = engine.prepare_key_binding_args(&key_event, columns, rows) {
                                if full_cmd_args.len() == 1 {
                                    match full_cmd_args[0].as_str() {
                                        "__EXIT__" => break 'main_loop,
                                        "__ESC__" => {
                                            if engine.has_active_input() {
                                                engine.cancel_input();
                                            }
                                        }
                                        "__TAB__" => engine.handle_tab(),
                                        "__UP__" => engine.move_up(),
                                        "__DOWN__" => engine.move_down(),
                                        "__EXPAND__" => engine.toggle_expand(),
                                        "__MARK__" => engine.toggle_mark(),
                                        "__TOP__" => engine.jump_to_top(),
                                        "__BOTTOM__" => engine.jump_to_bottom(),
                                        "__ENTER__" => engine.toggle_expand(),
                                        "__ACTIVATE_SEARCH__" => {
                                            if engine.components.contains_key("Cmd") {
                                                engine.activate_input("Cmd", "/");
                                            }
                                        }
                                        "__ACTIVATE_CMD__" => {
                                            if engine.components.contains_key("Cmd") {
                                                engine.activate_input("Cmd", ":");
                                            }
                                        }
                                        _ => {
                                            // 外部命令（单参数）
                                            if is_silent {
                                                match exec::execute_command_silent(&full_cmd_args) {
                                                    Ok(code) => {
                                                        if code != 0 {
                                                            engine.last_error = Some(format!("Silent cmd exited with code {}", code));
                                                        }
                                                    }
                                                    Err(e) => {
                                                        engine.last_error = Some(e.to_string());
                                                    }
                                                }
                                                let mut drain_count = 0;
                                                while crossterm::event::poll(Duration::ZERO).unwrap_or(false) && drain_count < 100 {
                                                    let _ = crossterm::event::read();
                                                    drain_count += 1;
                                                }
                                            } else {
                                                let _ = terminal::disable_raw_mode();
                                                let _ = stdout().execute(DisableMouseCapture);
                                                let _ = stdout().execute(terminal::LeaveAlternateScreen);
                                                let _ = stdout().execute(crossterm::cursor::Show);
                                                let _ = stdout().flush();
                                                let mut cmd = std::process::Command::new(&full_cmd_args[0]);
                                                cmd.args(&full_cmd_args[1..]);
                                                if let Ok(tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") {
                                                    if let Ok(stdin_c) = tty.try_clone() { cmd.stdin(stdin_c); }
                                                    if let Ok(stdout_c) = tty.try_clone() { cmd.stdout(stdout_c); }
                                                    cmd.stderr(tty);
                                                }
                                                let status = cmd.status();
                                                let mut drain_count = 0;
                                                while crossterm::event::poll(Duration::ZERO).unwrap_or(false) && drain_count < 100 {
                                                    let _ = crossterm::event::read();
                                                    drain_count += 1;
                                                }
                                                let _ = stdout().execute(crossterm::cursor::Hide);
                                                let _ = stdout().execute(terminal::EnterAlternateScreen);
                                                let _ = stdout().execute(EnableMouseCapture);
                                                let _ = terminal::enable_raw_mode();
                                                match status {
                                                    Ok(s) => {
                                                        if s.success() { engine.trigger_reload(); }
                                                        else { engine.last_error = Some(format!("退出码: {}", s.code().unwrap_or(-1))); }
                                                    }
                                                    Err(e) => { engine.last_error = Some(e.to_string()); }
                                                }
                                            }
                                        }
                                    }
                                }
                                // @activate_input 处理
                                else if full_cmd_args.len() == 2 && full_cmd_args[0] == "__ACTIVATE_INPUT__" {
                                    engine.activate_input(&full_cmd_args[1], "");
                                }
                                // Overlay 显隐
                                else if full_cmd_args.len() == 2 && full_cmd_args[0] == "__OVERLAY_SHOW__" {
                                    if let Some(Component::Overlay(o)) = engine.components.get_mut(&full_cmd_args[1]) {
                                        o.visible = true;
                                    }
                                }
                                else if full_cmd_args.len() == 2 && full_cmd_args[0] == "__OVERLAY_HIDE__" {
                                    if let Some(Component::Overlay(o)) = engine.components.get_mut(&full_cmd_args[1]) {
                                        o.visible = false;
                                    }
                                }
                                else {
                                    if is_silent {
                                        match exec::execute_command_silent(&full_cmd_args) {
                                            Ok(code) => {
                                                if code != 0 {
                                                    engine.last_error = Some(format!("Silent cmd exited with code {}", code));
                                                }
                                            }
                                            Err(e) => {
                                                engine.last_error = Some(e.to_string());
                                            }
                                        }
                                        let mut drain_count = 0;
                                        while crossterm::event::poll(Duration::ZERO).unwrap_or(false) && drain_count < 100 {
                                            let _ = crossterm::event::read();
                                            drain_count += 1;
                                        }
                                    } else {
                                        let _ = terminal::disable_raw_mode();
                                        let _ = stdout().execute(DisableMouseCapture);
                                        let _ = stdout().execute(terminal::LeaveAlternateScreen);
                                        let _ = stdout().execute(crossterm::cursor::Show);
                                        let _ = stdout().flush();
                                        let mut cmd = std::process::Command::new(&full_cmd_args[0]);
                                        cmd.args(&full_cmd_args[1..]);
                                        if let Ok(tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") {
                                            if let Ok(stdin_c) = tty.try_clone() { cmd.stdin(stdin_c); }
                                            if let Ok(stdout_c) = tty.try_clone() { cmd.stdout(stdout_c); }
                                            cmd.stderr(tty);
                                        }
                                        let status = cmd.status();
                                        let mut drain_count = 0;
                                        while crossterm::event::poll(Duration::ZERO).unwrap_or(false) && drain_count < 100 {
                                            let _ = crossterm::event::read();
                                            drain_count += 1;
                                        }
                                        let _ = stdout().execute(crossterm::cursor::Hide);
                                        let _ = stdout().execute(terminal::EnterAlternateScreen);
                                        let _ = stdout().execute(EnableMouseCapture);
                                        let _ = terminal::enable_raw_mode();
                                        match status {
                                            Ok(s) => {
                                                if s.success() { engine.trigger_reload(); }
                                                else { engine.last_error = Some(format!("退出码: {}", s.code().unwrap_or(-1))); }
                                            }
                                            Err(e) => { engine.last_error = Some(e.to_string()); }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Event::Mouse(mouse_event) => {
                    if !engine.mouse_enabled { continue 'main_loop; }
                    let rects = crate::layout::calc_window_rects(&engine.layout, columns, rows);
                    for (rect, name, _border) in rects.iter() {
                        let in_x = mouse_event.column >= rect.start_col && mouse_event.column < rect.start_col + rect.width;
                        let in_y = mouse_event.row >= rect.start_row && mouse_event.row < rect.start_row + rect.height;
                        if !in_x || !in_y { continue; }

                        engine.focused = app::Focus::Component(name.clone());

                        // 按组件类型（而非 WindowKind）决定鼠标行为
                        match engine.components.get(name).map(|c| c) {
                            Some(app::Component::Tree(_)) => {
                                if let Some(app::Component::Tree(t)) = engine.components.get(name) {
                                    let max_rows = (rect.height as usize).saturating_sub(2);
                                    let scroll_offset = ui::calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows);
                                    let target_idx = scroll_offset + mouse_event.row.saturating_sub(rect.start_row).saturating_sub(1) as usize;
                                    if target_idx >= t.visible_ids.len() { break; }
                                    let clicked_id = t.visible_ids[target_idx].clone();

                                    match mouse_event.kind {
                                        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                            let now = std::time::Instant::now();
                                            let is_double_click = engine.last_click_time
                                                .map_or(false, |t| now.duration_since(t).as_millis() < 300)
                                                && engine.last_clicked_id.as_deref() == Some(&clicked_id);
                                            if is_double_click {
                                                engine.select_id(name, &clicked_id);
                                                engine.toggle_expand();
                                                engine.last_click_time = None;
                                            } else {
                                                engine.select_id(name, &clicked_id);
                                                engine.last_click_time = Some(now);
                                                engine.last_clicked_id = Some(clicked_id);
                                            }
                                        }
                                        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Right) => {
                                            if let Some(app::Component::Tree(t)) = engine.components.get_mut(name) {
                                                let was_marked = t.marked_ids.contains(&clicked_id);
                                                if was_marked {
                                                    t.marked_ids.remove(&clicked_id);
                                                } else {
                                                    t.marked_ids.insert(clicked_id.clone());
                                                }
                                                engine.drag_mode = !was_marked;
                                            }
                                            engine.drag_start_idx = Some(target_idx);
                                            engine.drag_active = true;
                                        }
                                        crossterm::event::MouseEventKind::ScrollUp => {
                                            engine.move_up_n(scroll_step as usize);
                                        }
                                        crossterm::event::MouseEventKind::ScrollDown => {
                                            engine.move_down_n(scroll_step as usize);
                                        }
                                        crossterm::event::MouseEventKind::Up(_) => {
                                            engine.drag_active = false;
                                            engine.drag_start_idx = None;
                                        }
                                        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Right) => {
                                            if engine.drag_active {
                                                if let Some(start_idx) = engine.drag_start_idx {
                                                    let range = if target_idx >= start_idx {
                                                        start_idx..=target_idx
                                                    } else {
                                                        target_idx..=start_idx
                                                    };
                                                    if let Some(app::Component::Tree(t)) = engine.components.get_mut(name) {
                                                        for i in range {
                                                            if let Some(id) = t.visible_ids.get(i) {
                                                                if engine.drag_mode {
                                                                    t.marked_ids.insert(id.clone());
                                                                } else {
                                                                    t.marked_ids.remove(id);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Some(app::Component::View(_)) => {
                                match mouse_event.kind {
                                    crossterm::event::MouseEventKind::ScrollUp => {
                                        engine.move_up_n(scroll_step as usize);
                                    }
                                    crossterm::event::MouseEventKind::ScrollDown => {
                                        engine.move_down_n(scroll_step as usize);
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                        break;
                    }
                }

                _ => {}
            }
        }
    }

    // 清理
    terminal::disable_raw_mode()?;
    stdout().execute(DisableMouseCapture)?;
    stdout().execute(crossterm::cursor::Show)?;
    stdout().execute(terminal::LeaveAlternateScreen)?;
    let _ = std::fs::remove_file(std::env::var("STREE_SOCK").unwrap_or_default());

    // 退出时把最终选中的 id 打到 stdout，供调用方脚本捕获
    if let app::Focus::Component(name) = &engine.focused {
        if let Some(app::Component::Tree(t)) = engine.components.get(name) {
            if let Some(id) = &t.selected_id { println!("{}", id); }
        }
    }
    Ok(())
}
