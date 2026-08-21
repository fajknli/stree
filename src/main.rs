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
mod runner;

use clap::{Parser, Subcommand};
use crossterm::{terminal, ExecutableCommand};
use crossterm::event::{self, Event, EnableMouseCapture, DisableMouseCapture};
use std::time::Duration;
use std::io::{self, stdout, Read, Write, IsTerminal};

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

    #[arg(long = "bind", action = clap::ArgAction::Append)]
    binds: Vec<String>,
    #[arg(long = "border-chars", action = clap::ArgAction::Append)]
    border_chars: Vec<String>,
    #[arg(long, default_value = "")]
    status_col: String,
    /// 自定义 UI 颜色主题。
    /// 支持的键: border_focused, border_unfocused, view_focused, view_unfocused,
    /// statusbar_fg, input_prefix, input_buffer, selected_bg, error_fg, error_bg, empty_data_fg
    /// 格式: "border_focused=#a9b5d5,border_unfocused=#565d7e"
    #[arg(long = "ui-colors", default_value = "")]
    ui_colors: String,
    #[arg(long)]
    select: Option<String>,
    #[arg(long = "no-mouse", action = clap::ArgAction::SetTrue)]
    no_mouse: bool,
    #[arg(long, default_value_t = 1)]
    scroll_step: u8,
    #[arg(long, default_value_t = 1000)]
    max_lines: usize,
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

    let engine_config = app::EngineConfig {
        initial_dataset: full_dataset,
        layout_strings,
        key_bindings,
        mouse_enabled: !cli.no_mouse,
        border_chars: cli.border_chars,
        trees: cli.trees,
        views: cli.views,
        statusbars: cli.statusbars,
        inputs: cli.inputs,
        relations_path: cli.relations.clone(),
        max_lines: cli.max_lines,
        ui_colors: cli.ui_colors,
    };

    let mut engine = app::Engine::new(engine_config);

    if let Some(ref id) = cli.select {
        let focused_name = if let app::Focus::Component(name) = &engine.focus.current { name.clone() } else { String::new() };
        if !focused_name.is_empty() {
            engine.select_id(&focused_name, id);
        }
    }

    stdout().execute(terminal::EnterAlternateScreen)?;
    stdout().execute(EnableMouseCapture)?;
    stdout().execute(crossterm::cursor::Hide)?;
    terminal::enable_raw_mode()?;

    let scroll_step = cli.scroll_step;

    // 【性能优化】将事件队列移到循环外，避免每帧分配内存
    let mut key_events: Vec<crossterm::event::KeyEvent> = Vec::with_capacity(16);
    let mut mouse_events: Vec<crossterm::event::MouseEvent> = Vec::with_capacity(16);

    // 【优化1】将 BufWriter 移出主循环，避免每帧重复分配和析构
    let mut out = std::io::BufWriter::new(stdout());

    'main_loop: loop {
        if signal::check_and_clear_quit() { break 'main_loop; }

        let (columns, rows) = match crossterm::terminal::size() {
            Ok(s) if s.0 > 0 && s.1 > 0 => s,
            _ => {
                // 终端尺寸为 0 或获取失败时，休眠等待，避免除零或布局 panic
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        // 【修改】将尺寸判断结果提取为变量
        let layout_changed = (columns, rows) != engine.prev_term_size;
        if layout_changed {
            engine.window_rect_overrides.clear();
            engine.prev_term_size = (columns, rows);
            engine.prev_rects.clear();
            engine.mark_all_dirty();
        }
        let term_size = ui::TermSize { columns, rows };

        if signal::check_and_clear_reload() {
            engine.overlay_stack.clear(); // 【优先撤退】
            engine.trigger_reload();
        }

        ipc_server.try_accept_and_process(|target, data| {
            engine.handle_ipc_update(target, data, columns, rows);
        });

        // 【重构】统一收口异步通道消费逻辑
        engine.drain_async_channels(columns, rows);

        // ==========================================
        // 1. 布局系统：只计算一次，得到物理真相
        // ==========================================
        // 【新增】在计算布局前，先解析 Auto 高度
        engine.precalculate_auto_sizes(rows);

        let mut all_rects = engine.calc_all_rects(columns, rows);

        if !engine.drag.active {
            engine.rebuild_draggable_edges(columns, rows);
        }

        // 【重构】将拖拽逻辑委托给引擎处理，main.rs 保持清爽
        engine.process_drag_frame(&mut all_rects, columns, rows);

        let mut view_rects_info = std::collections::HashMap::new();

        for (rect, name, border, _z) in all_rects.iter() {
            if let Some(app::Component::View(_)) = engine.components.get(name) {
                let (inner_w, inner_h) = crate::layout::content_size(rect, *border);
                view_rects_info.insert(name.clone(), (inner_h as usize, inner_w, inner_h));
            }
        }
        engine.update_view_rects(view_rects_info);

        // ==========================================
        // 2. 事件处理 (拖拽系统：纯物理拦截)
        // ==========================================
        let poll_timeout = if engine.has_active_input() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(16)
        };

        if event::poll(poll_timeout)? {

            key_events.clear();
            mouse_events.clear();

            // 防御 crossterm 合并按键的 OSC 过滤状态机
            let mut osc_state = 0; // 0=正常, 1=看到了ESC, 2=确认在OSC序列中, 3=OSC中看到了ESC

            loop {
                match event::read()? {
                    Event::Key(k) => {
                        match osc_state {
                            0 => {
                                // 【关键修复】同时拦截单独的 Esc 和 crossterm 合并的 Alt+]
                                if k.code == crossterm::event::KeyCode::Esc {
                                    osc_state = 1;
                                } else if k.code == crossterm::event::KeyCode::Char(']') && k.modifiers.contains(crossterm::event::KeyModifiers::ALT) {
                                    osc_state = 2; // 捕获到合并键，直接进入丢弃模式
                                } else {
                                    key_events.push(k);
                                }
                            }
                            1 => {
                                if k.code == crossterm::event::KeyCode::Char(']') {
                                    osc_state = 2;
                                } else {
                                    key_events.push(crossterm::event::KeyEvent {
                                        code: crossterm::event::KeyCode::Esc,
                                        modifiers: crossterm::event::KeyModifiers::NONE,
                                        kind: crossterm::event::KeyEventKind::Press,
                                        state: crossterm::event::KeyEventState::NONE,
                                    });
                                    if k.code != crossterm::event::KeyCode::Esc {
                                        key_events.push(k);
                                        osc_state = 0;
                                    }
                                }
                            }
                            2 => {
                                // 在 OSC 序列内部，丢弃所有字符
                                if k.code == crossterm::event::KeyCode::Char('\u{7}') {
                                    osc_state = 0; // 遇到 BEL，结束
                                } else if k.code == crossterm::event::KeyCode::Esc {
                                    osc_state = 3;
                                }
                            }
                            3 => {
                                if k.code == crossterm::event::KeyCode::Char('\\') {
                                    osc_state = 0;
                                } else {
                                    osc_state = 0;
                                }
                            }
                            _ => {}
                        }
                    }
                    Event::Mouse(m) => {
                        if osc_state == 1 {
                            key_events.push(crossterm::event::KeyEvent {
                                code: crossterm::event::KeyCode::Esc,
                                modifiers: crossterm::event::KeyModifiers::NONE,
                                kind: crossterm::event::KeyEventKind::Press,
                                state: crossterm::event::KeyEventState::NONE,
                            });
                            osc_state = 0;
                        }

                        if matches!(m.kind, crossterm::event::MouseEventKind::Drag(_) | crossterm::event::MouseEventKind::Moved) {
                            let should_replace = if let Some(last_m) = mouse_events.last() {
                                matches!(last_m.kind, crossterm::event::MouseEventKind::Drag(_) | crossterm::event::MouseEventKind::Moved)
                            } else { false };

                            if should_replace { *mouse_events.last_mut().unwrap() = m; }
                            else { mouse_events.push(m); }
                        } else { mouse_events.push(m); }
                    }
                    _ => {}
                }

                if !event::poll(Duration::ZERO)? { break; }
                if key_events.len() + mouse_events.len() > 50 { break; }
                if engine.drag.active {
                    if let Some(last_m) = mouse_events.last() {
                        if matches!(last_m.kind, crossterm::event::MouseEventKind::Drag(_) | crossterm::event::MouseEventKind::Moved) { break; }
                    }
                }
            }

            if osc_state == 1 {
                key_events.push(crossterm::event::KeyEvent {
                    code: crossterm::event::KeyCode::Esc, modifiers: crossterm::event::KeyModifiers::NONE,
                    kind: crossterm::event::KeyEventKind::Press, state: crossterm::event::KeyEventState::NONE,
                });
            }

            for key_event in &key_events {
                if engine.handle_key_event(key_event, &all_rects, columns, rows) { break 'main_loop; }
            }
            for mouse_event in &mouse_events {
                engine.handle_mouse_event(mouse_event, &all_rects, columns, rows, scroll_step, layout_changed);
            }
        }

        // ==========================================
        // 3. 渲染 (渲染器只看 all_rects，不知道谁改了它)
        // ==========================================

        // 【关键修复】事件可能改变了图层显隐状态，如果触发了全屏重绘(prev_rects被清空)，
        // 必须重算 all_rects，确保渲染的是最新的布局真相，不留空白！
        if engine.prev_rects.is_empty() {
            all_rects = engine.calc_all_rects(columns, rows);
        }

        // 【重构】清理过期的状态栏临时消息
        engine.expire_status_messages();

        // 【优化】在渲染前统一处理积压的状态变更
        engine.flush_pending_updates(columns, rows);

        // 【新增】预计算状态栏文本
        engine.update_status_bars(columns, rows, &all_rects);

        // 【重构】统一收口异步视图接收逻辑
        engine.process_async_view_updates(columns, rows);

        // 【重构】初始化引擎状态
        engine.initialize_if_needed(columns, rows);

        let mut ctx = ui::RenderCtx { engine: &mut engine, style_engine: &style_engine, term_size };

        // 【优化2】直接使用外部的 out，不再重复创建
        if let Err(e) = ui::render_all(&mut ctx, &all_rects, &mut out) {
            terminal::disable_raw_mode()?;
            stdout().execute(DisableMouseCapture)?;
            stdout().execute(terminal::LeaveAlternateScreen)?;
            return Err(e.into());
        }

        // 【优化3】确保每帧渲染后刷新缓冲区到终端
        out.flush()?;
    }

    terminal::disable_raw_mode()?;
    stdout().execute(DisableMouseCapture)?;
    stdout().execute(crossterm::cursor::Show)?;
    stdout().execute(terminal::LeaveAlternateScreen)?;
    let _ = std::fs::remove_file(std::env::var("STREE_SOCK").unwrap_or_default());

    if let app::Focus::Component(name) = &engine.focus.current {
        if let Some(app::Component::Tree(t)) = engine.components.get(name) {
            if let Some(id) = &t.selected_id { println!("{}", id); }
        }
    }
    Ok(())
}
