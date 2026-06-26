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

    /// 布局字符串，可多次声明。声明顺序即 Z 轴渲染顺序。
    /// 无前缀: 全屏 Flexbox 布局 (Z=0)
    /// @(x,y) 前缀: 屏幕绝对定位浮动布局 (Z=N)
    /// 例: --layout "horizontal(area(30%):Tree, area(70%):Preview)"
    ///     --layout "@(10,5) area(40,15)[box]:Popup"
    #[arg(long = "layout", action = clap::ArgAction::Append)]
    layouts: Vec<String>,

    #[arg(long = "tree", action = clap::ArgAction::Append)]
    trees: Vec<String>,
    #[arg(long = "view", action = clap::ArgAction::Append)]
    views: Vec<String>,
    #[arg(long = "statusbar", action = clap::ArgAction::Append)]
    statusbars: Vec<String>,
    #[arg(long = "input", action = clap::ArgAction::Append)]
    inputs: Vec<String>,
    // --overlay 已彻底删除，改用 --layout "@(x,y) ..."

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

    // 解析多图层布局
    let layout_strings = if cli.layouts.is_empty() {
        vec!["area:Main".to_string()]
    } else {
        cli.layouts.clone()
    };

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
        layout_strings,
        key_bindings,
        !cli.no_mouse,
        cli.border_chars,
        cli.trees,
        cli.views,
        cli.statusbars,
        cli.inputs,
        cli.relations.clone(),
    );

    if let Some(ref id) = cli.select {
        let focused_name = if let app::Focus::Component(name) = &engine.focused { name.clone() } else { String::new() };
        if !focused_name.is_empty() {
            engine.select_id(&focused_name, id, 0, 0);
        }
    }

    stdout().execute(terminal::EnterAlternateScreen)?;
    stdout().execute(EnableMouseCapture)?;
    stdout().execute(crossterm::cursor::Hide)?;
    terminal::enable_raw_mode()?;

    let scroll_step = cli.scroll_step;
    let mut last_event_time = std::time::Instant::now();

    // 追踪终端尺寸，用于触发延迟池激活
    let mut last_columns: u16 = 0;
    let mut last_rows: u16 = 0;

    'main_loop: loop {
        if signal::check_and_clear_quit() { break 'main_loop; }

        let (columns, rows) = match crossterm::terminal::size() { Ok(s) => s, Err(_) => continue };
        let term_size = crossterm::terminal::WindowSize { width: columns, height: rows, columns, rows };

        // 【新增】检测终端 Resize，激活延迟池中的 Percent
        if last_columns > 0 && last_rows > 0 && (columns != last_columns || rows != last_rows) {
            if !engine.pending_percent_overrides.is_empty() {
                // 终端尺寸变了！把延迟池里的 Percent 激活，覆盖 Absolute
                // Flexbox 会自动根据新的终端尺寸和 Percent 完美重算嵌套布局
                for (name, size) in engine.pending_percent_overrides.drain() {
                    engine.window_rect_overrides.insert(name, size);
                }
            }
        }
        last_columns = columns;
        last_rows = rows;

        if signal::check_and_clear_reload() {
            engine.trigger_reload(columns, rows);
        }

        ipc_server.try_accept_and_process(|target, data| {
            engine.handle_ipc_update(target, data, columns, rows);
        });

        // 【重构】使用多图层布局计算
        let all_rects = engine.calc_all_rects(columns, rows);
        let mut view_rects_info = std::collections::HashMap::new();
        for (rect, name, border, _z) in all_rects.iter() {
            if let Some(app::Component::View(_)) = engine.components.get(name) {
                // 【修复】：调用纯函数获取内容区尺寸，消灭 match border
                let (inner_w, inner_h) = crate::layout::content_size(rect, *border);

                view_rects_info.insert(name.clone(), (inner_h as usize, inner_w, inner_h));
            }
        }
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

                    // 【模态拖拽拦截】拖拽中吞掉所有键盘事件，直到鼠标松开
                    if engine.drag_active {
                        continue 'main_loop;
                    }

                    if engine.last_error.is_some() { engine.last_error = None; }

                    if engine.has_active_input() {
                        match key_event.code {
                            KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace | KeyCode::Left | KeyCode::Right
                            | KeyCode::Home | KeyCode::End | KeyCode::Char(_) => {
                                if let Some((input_name, result)) = engine.handle_input_key(key_event) {
                                    if result == "__CANCEL__" {
                                    } else {
                                        if let Some(app::Component::Input(input)) = engine.components.get(&input_name) {
                                            if input.prefix == "/" {
                                                engine.apply_search(&result, columns, rows);
                                            } else if let Some(ref cmd_template) = input.on_submit {
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
                                        "__TAB__" => engine.handle_tab(columns, rows),
                                        "__UP__" => engine.move_up(columns, rows),
                                        "__DOWN__" => engine.move_down(columns, rows),
                                        "__EXPAND__" => engine.toggle_expand(),
                                        "__MARK__" => engine.toggle_mark(),
                                        "__TOP__" => engine.jump_to_top(columns, rows),
                                        "__BOTTOM__" => engine.jump_to_bottom(columns, rows),
                                        "__ENTER__" => {
                                            engine.toggle_expand();
                                            engine.emit("confirm", columns, rows);
                                        }
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
                                        // 【新增】布局层显隐控制指令
                                        "__TOGGLE_LAYOUT__" => {
                                            // 需要第二个参数，但单参数指令无法传递
                                            // 这里仅作占位，实际使用需通过 __ACTIVATE_INPUT__ 或外部 IPC
                                        }
                                        _ => {
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
                                                        if s.success() { engine.trigger_reload(columns, rows); }
                                                        else { engine.last_error = Some(format!("退出码: {}", s.code().unwrap_or(-1))); }
                                                    }
                                                    Err(e) => { engine.last_error = Some(e.to_string()); }
                                                }
                                            }
                                        }
                                    }
                                }
                                else if full_cmd_args.len() == 2 && full_cmd_args[0] == "__ACTIVATE_INPUT__" {
                                    engine.activate_input(&full_cmd_args[1], "");
                                }
                                // 【重构】Overlay 指令替换为 Layout 层显隐控制
                                else if full_cmd_args.len() == 2 && full_cmd_args[0] == "__TOGGLE_LAYOUT__" {
                                    engine.toggle_layout_visible(&full_cmd_args[1]);
                                }
                                else if full_cmd_args.len() == 2 && full_cmd_args[0] == "__SHOW_LAYOUT__" {
                                    engine.set_layout_visible(&full_cmd_args[1], true);
                                }
                                else if full_cmd_args.len() == 2 && full_cmd_args[0] == "__HIDE_LAYOUT__" {
                                    engine.set_layout_visible(&full_cmd_args[1], false);
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
                                                if s.success() { engine.trigger_reload(columns, rows); }
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

                    // 【重构】模态拖拽拦截：拖拽边框时，无视 Z 轴和窗口边界，全局响应 Drag 和 Up
                    if engine.drag_active && engine.drag_resize_target.is_some() {
                        match mouse_event.kind {
                            crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                                if let Some(app::DragTarget::ResizeEdge(ref primary, ref neighbor, dir)) = engine.drag_resize_target {
                                    let all_rects_now = engine.calc_all_rects(columns, rows);
                                    let mr = all_rects_now.iter().find(|(_, n, _, _)| n == primary).map(|(r, _, b, _)| (*r, *b));
                                    let nr = all_rects_now.iter().find(|(_, n, _, _)| n == neighbor).map(|(r, _, b, _)| (*r, *b));

                                    if let (Some((m, m_border)), Some((n, n_border))) = (mr, nr) {
                                        match dir {
                                            crate::layout::Direction::Horizontal => {
                                                let (left, right, left_name, right_name, left_border, right_border) = if m.start_col < n.start_col {
                                                    (m, n, primary, neighbor, m_border, n_border)
                                                } else {
                                                    (n, m, neighbor, primary, n_border, m_border)
                                                };

                                                let left_extra = match left_border { crate::layout::BorderStyle::Box => 2, crate::layout::BorderStyle::Line => 1, _ => 0 };
                                                let right_extra = match right_border { crate::layout::BorderStyle::Box => 2, crate::layout::BorderStyle::Line => 1, _ => 0 };

                                                let min_split = left.start_col + left_extra;
                                                let max_split = right.start_col + right.width.saturating_sub(right_extra);
                                                if min_split > max_split { continue 'main_loop; }

                                                let split = mouse_event.column.clamp(min_split, max_split);
                                                let new_left_physical = split - left.start_col;
                                                let new_left_content = new_left_physical.saturating_sub(left_extra);

                                                // 【核心 1】：计算 A+B 的总内容宽度，保证拖拽时总宽度守恒，C 窗口绝对不动
                                                let total_ab_physical = left.width + right.width;
                                                let total_ab_content = total_ab_physical.saturating_sub(left_extra + right_extra);
                                                let new_right_content = total_ab_content.saturating_sub(new_left_content);

                                                // 写入 Absolute，实现绝对跟手
                                                engine.window_rect_overrides.insert(left_name.clone(), crate::layout::WindowSize::Absolute(new_left_content));
                                                engine.window_rect_overrides.insert(right_name.clone(), crate::layout::WindowSize::Absolute(new_right_content));
                                            }
                                            crate::layout::Direction::Vertical => {
                                                let (top, bottom, top_name, bottom_name, top_border, bottom_border) = if m.start_row < n.start_row {
                                                    (m, n, primary, neighbor, m_border, n_border)
                                                } else {
                                                    (n, m, neighbor, primary, n_border, m_border)
                                                };

                                                let top_extra = match top_border { crate::layout::BorderStyle::Box => 2, crate::layout::BorderStyle::Line => 1, _ => 0 };
                                                let bottom_extra = match bottom_border { crate::layout::BorderStyle::Box => 2, crate::layout::BorderStyle::Line => 1, _ => 0 };

                                                let min_split = top.start_row + top_extra;
                                                let max_split = bottom.start_row + bottom.height.saturating_sub(bottom_extra);
                                                if min_split > max_split { continue 'main_loop; }

                                                let split = mouse_event.row.clamp(min_split, max_split);
                                                let new_top_physical = split - top.start_row;
                                                let new_top_content = new_top_physical.saturating_sub(top_extra);

                                                let total_ab_physical = top.height + bottom.height;
                                                let total_ab_content = total_ab_physical.saturating_sub(top_extra + bottom_extra);
                                                let new_bottom_content = total_ab_content.saturating_sub(new_top_content);

                                                engine.window_rect_overrides.insert(top_name.clone(), crate::layout::WindowSize::Absolute(new_top_content));
                                                engine.window_rect_overrides.insert(bottom_name.clone(), crate::layout::WindowSize::Absolute(new_bottom_content));
                                            }
                                        }
                                    }
                                }
                            }
                            crossterm::event::MouseEventKind::Up(_) => {
                                // 【延迟池方案】：松手时不改变当前帧的 overrides（保持 Absolute，0 跳动）
                                // 算好的 Percent 存入延迟池，等待终端 Resize 时再激活
                                if let Some(app::DragTarget::ResizeEdge(ref primary, ref neighbor, dir)) = engine.drag_resize_target {
                                    // 1. 查户口：获取原始百分比总和（保证多窗口不挤压）
                                    let original_sum_pct = engine.get_sibling_percent_sum(primary, neighbor).unwrap_or(100);

                                    let all_rects_now = engine.calc_all_rects(columns, rows);
                                    let mr = all_rects_now.iter().find(|(_, n, _, _)| n == primary).map(|(r, _, b, _)| (*r, *b));
                                    let nr = all_rects_now.iter().find(|(_, n, _, _)| n == neighbor).map(|(r, _, b, _)| (*r, *b));

                                    if let (Some((m, m_border)), Some((n, n_border))) = (mr, nr) {
                                        match dir {
                                            crate::layout::Direction::Horizontal => {
                                                let (left, right, left_name, right_name, left_border, right_border) = if m.start_col < n.start_col {
                                                    (m, n, primary, neighbor, m_border, n_border)
                                                } else {
                                                    (n, m, neighbor, primary, n_border, m_border)
                                                };

                                                let left_extra = match left_border { crate::layout::BorderStyle::Box => 2, crate::layout::BorderStyle::Line => 1, _ => 0 };
                                                let right_extra = match right_border { crate::layout::BorderStyle::Box => 2, crate::layout::BorderStyle::Line => 1, _ => 0 };

                                                let total_ab_physical = left.width + right.width;
                                                let total_ab_content = total_ab_physical.saturating_sub(left_extra + right_extra);

                                                if total_ab_content > 0 {
                                                    let left_content_w = left.width.saturating_sub(left_extra);
                                                    // 向上取整，对冲 Flexbox 的向下取整
                                                    let numerator = left_content_w as u32 * original_sum_pct as u32;
                                                    let denominator = total_ab_content as u32;
                                                    let global_left_pct = (numerator + denominator - 1) / denominator;
                                                    let global_left_pct = global_left_pct.min(original_sum_pct as u32) as u16;
                                                    let global_right_pct = original_sum_pct - global_left_pct;

                                                    // 【核心】：只存入延迟池，绝对不修改 window_rect_overrides！
                                                    engine.pending_percent_overrides.insert(left_name.clone(), crate::layout::WindowSize::Percent(global_left_pct));
                                                    engine.pending_percent_overrides.insert(right_name.clone(), crate::layout::WindowSize::Percent(global_right_pct));
                                                }
                                            }
                                            crate::layout::Direction::Vertical => {
                                                let (top, bottom, top_name, bottom_name, top_border, bottom_border) = if m.start_row < n.start_row {
                                                    (m, n, primary, neighbor, m_border, n_border)
                                                } else {
                                                    (n, m, neighbor, primary, n_border, m_border)
                                                };

                                                let top_extra = match top_border { crate::layout::BorderStyle::Box => 2, crate::layout::BorderStyle::Line => 1, _ => 0 };
                                                let bottom_extra = match bottom_border { crate::layout::BorderStyle::Box => 2, crate::layout::BorderStyle::Line => 1, _ => 0 };

                                                let total_ab_physical = top.height + bottom.height;
                                                let total_ab_content = total_ab_physical.saturating_sub(top_extra + bottom_extra);

                                                if total_ab_content > 0 {
                                                    let top_content_h = top.height.saturating_sub(top_extra);
                                                    let numerator = top_content_h as u32 * original_sum_pct as u32;
                                                    let denominator = total_ab_content as u32;
                                                    let global_top_pct = (numerator + denominator - 1) / denominator;
                                                    let global_top_pct = global_top_pct.min(original_sum_pct as u32) as u16;
                                                    let global_bottom_pct = original_sum_pct - global_top_pct;

                                                    engine.pending_percent_overrides.insert(top_name.clone(), crate::layout::WindowSize::Percent(global_top_pct));
                                                    engine.pending_percent_overrides.insert(bottom_name.clone(), crate::layout::WindowSize::Percent(global_bottom_pct));
                                                }
                                            }
                                        }
                                    }
                                }

                                // 清理拖拽状态
                                engine.drag_active = false;
                                engine.drag_resize_target = None;
                            }
                            _ => {}
                        }
                        continue 'main_loop; // 拖拽模态下，屏蔽所有其他鼠标事件
                    }

                    // 【重构】使用多图层布局计算，并按 Z 轴逆向遍历实现高 Z 层优先命中
                    let all_rects = engine.calc_all_rects(columns, rows);

                    // 按 z_index 降序遍历（高 Z 层先检测）
                    let mut sorted_rects: Vec<_> = all_rects.iter().collect();
                    sorted_rects.sort_by(|a, b| b.3.cmp(&a.3));

                    for (rect, name, _border, _z) in sorted_rects.iter() {
                        let in_x = mouse_event.column >= rect.start_col && mouse_event.column < rect.start_col + rect.width;
                        let in_y = mouse_event.row >= rect.start_row && mouse_event.row < rect.start_row + rect.height;
                        if !in_x || !in_y { continue; }

                        // 【窗口边框拖拽命中检测】
                        let on_border = {
                            let top = mouse_event.row == rect.start_row;
                            let bottom = mouse_event.row == rect.start_row + rect.height.saturating_sub(1);
                            let left = mouse_event.column == rect.start_col;
                            let right = mouse_event.column == rect.start_col + rect.width.saturating_sub(1);
                            if top { Some(app::BorderSide::Top) }
                            else if bottom { Some(app::BorderSide::Bottom) }
                            else if left { Some(app::BorderSide::Left) }
                            else if right { Some(app::BorderSide::Right) }
                            else { None }
                        };

                        if let Some(side) = on_border {
                            // 检查该窗口是否标记为 draggable
                            let is_draggable = engine.layout_layers.iter().any(|layer| {
                                fn check(node: &crate::layout::LayoutNode, name: &str) -> bool {
                                    match node {
                                        crate::layout::LayoutNode::Window { name: n, draggable, .. } => n == name && *draggable,
                                        crate::layout::LayoutNode::Container { children, .. } => children.iter().any(|c| check(c, name)),
                                    }
                                }
                                layer.layout.layers.iter().any(|l| check(&l.root, name))
                            });

                            if is_draggable {
                                // 步骤1：树查找候选邻居
                                if let Some((neighbor_name, dir)) = engine.find_drag_neighbor(name, side) {
                                    // 步骤2：几何对齐校验
                                    let all_rects_for_check = engine.calc_all_rects(columns, rows);
                                    let my_rect = all_rects_for_check.iter().find(|(_, n, _, _)| n == name).map(|(r, _, _, _)| *r);
                                    let nb_rect = all_rects_for_check.iter().find(|(_, n, _, _)| n == &neighbor_name).map(|(r, _, _, _)| *r);

                                    if let (Some(mr), Some(nr)) = (my_rect, nb_rect) {
                                        if engine.is_edge_aligned(&mr, &nr, side) {
                                            // 合法！进入拖拽模态
                                            if let crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) = mouse_event.kind {
                                                engine.drag_active = true;
                                                engine.drag_resize_target = Some(app::DragTarget::ResizeEdge(name.clone(), neighbor_name, dir));
                                                continue 'main_loop; // 跳过后续的焦点切换和内容区点击
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // 命中！焦点切换 + 门禁检查
                        let old_focus = engine.focused.clone();
                        engine.focused = app::Focus::Component(name.clone());

                        if old_focus != engine.focused {
                            if let Some(app::Component::Tree(t)) = engine.components.get(name) {
                                if t.focus_to_fire {
                                    engine.emit("focus", columns, rows);
                                }
                            }
                        }

                        match engine.components.get(name) {
                            Some(app::Component::Tree(_)) => {
                                let click_to_fire = if let Some(app::Component::Tree(t)) = engine.components.get(name) {
                                    t.click_to_fire
                                } else {
                                    false
                                };

                                if let Some(app::Component::Tree(t)) = engine.components.get(name) {
                                    let max_rows = (rect.height as usize).saturating_sub(2);
                                    let scroll_offset = ui::calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows);

                                    // 1. 计算 target_idx (修复之前的缺失)
                                    let target_idx = scroll_offset + mouse_event.row.saturating_sub(rect.start_row).saturating_sub(1) as usize;

                                    // 2. 安全提取 clicked_id，不再 break (修复空白处失效)
                                    let is_valid_click = target_idx < t.visible_ids.len();
                                    let clicked_id = if is_valid_click { Some(t.visible_ids[target_idx].clone()) } else { None };

                                    match mouse_event.kind {
                                        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                            if let Some(ref cid) = clicked_id {
                                                let now = std::time::Instant::now();
                                                let is_double_click = engine.last_click_time
                                                    .map_or(false, |t| now.duration_since(t).as_millis() < 300)
                                                    && engine.last_clicked_id.as_deref() == Some(cid.as_str());

                                                let tree_name = name.clone();

                                                if is_double_click {
                                                    engine.select_id(&tree_name, cid, columns, rows);
                                                    engine.toggle_expand();
                                                    engine.emit("confirm", columns, rows);
                                                    engine.last_click_time = None;
                                                } else {
                                                    engine.select_id(&tree_name, cid, columns, rows);
                                                    engine.last_click_time = Some(now);
                                                    engine.last_clicked_id = Some(cid.clone());
                                                    if click_to_fire {
                                                        engine.emit("click", columns, rows);
                                                    }
                                                }
                                            }
                                        }
                                        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Right) => {
                                            if let Some(ref cid) = clicked_id {
                                                if let Some(app::Component::Tree(t)) = engine.components.get_mut(name) {
                                                    let was_marked = t.marked_ids.contains(cid);
                                                    if was_marked {
                                                        t.marked_ids.remove(cid);
                                                    } else {
                                                        t.marked_ids.insert(cid.clone());
                                                    }
                                                    engine.drag_mode = !was_marked;
                                                }
                                                engine.drag_start_idx = Some(target_idx);
                                                engine.drag_active = true;
                                            }
                                        }
                                        crossterm::event::MouseEventKind::ScrollUp => {
                                            engine.move_up_n(scroll_step as usize, columns, rows);
                                        }
                                        crossterm::event::MouseEventKind::ScrollDown => {
                                            engine.move_down_n(scroll_step as usize, columns, rows);
                                        }
                                        crossterm::event::MouseEventKind::Up(_) => {
                                            engine.drag_active = false;
                                            engine.drag_start_idx = None;
                                            engine.drag_resize_target = None; // 清理边框拖拽状态
                                        }
                                        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Right) => {
                                            if engine.drag_active {
                                                if let Some(start_idx) = engine.drag_start_idx {
                                                    let clamped_target = target_idx.min(t.visible_ids.len().saturating_sub(1));
                                                    let range = if clamped_target >= start_idx {
                                                        start_idx..=clamped_target
                                                    } else {
                                                        clamped_target..=start_idx
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
                                        _ => {} // 删除了原来的 Drag(Left)，因为已经移到最外层拦截了
                                    }
                                }
                            }
                            Some(app::Component::View(_)) => {
                                match mouse_event.kind {
                                    crossterm::event::MouseEventKind::ScrollUp => {
                                        engine.move_up_n(scroll_step as usize, columns, rows);
                                    }
                                    crossterm::event::MouseEventKind::ScrollDown => {
                                        engine.move_down_n(scroll_step as usize, columns, rows);
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                        break; // 高 Z 层命中后不再往下检测
                    }
                }

                _ => {}
            }
        }
    }

    terminal::disable_raw_mode()?;
    stdout().execute(DisableMouseCapture)?;
    stdout().execute(crossterm::cursor::Show)?;
    stdout().execute(terminal::LeaveAlternateScreen)?;
    let _ = std::fs::remove_file(std::env::var("STREE_SOCK").unwrap_or_default());

    if let app::Focus::Component(name) = &engine.focused {
        if let Some(app::Component::Tree(t)) = engine.components.get(name) {
            if let Some(id) = &t.selected_id { println!("{}", id); }
        }
    }
    Ok(())
}
