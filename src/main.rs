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

        // 【修复】接收异步重载数据
        while let Ok((tree_name, result)) = engine.async_reload_rx.try_recv() {
            match result {
                Ok(stdout) => {
                    engine.handle_ipc_update(&tree_name, &stdout, columns, rows);
                }
                Err(e) => {
                    engine.last_error = Some(format!("Reload failed for {}: {}", tree_name, e));
                }
            }
        }

        // 【新增】接收后台静默脚本执行完毕的信号，触发全局刷新
        while let Ok(()) = engine.async_exec_rx.try_recv() {
            // 1. 重新加载 Tree 数据源
            engine.trigger_reload();
            // 2. 清空 View 缓存
            for comp in engine.components.values_mut() {
                if let app::Component::View(v) = comp {
                    v.cached_entity_id = None;
                }
            }
            // 3. 刷新 View 内容
            if let app::Focus::Component(tree_name) = &engine.focus.current.clone() {
                let tree_name = tree_name.clone();
                engine.broadcast_selection_changed(&tree_name, columns, rows);
            }
            engine.mark_all_dirty();
        }

        // ==========================================
        // 1. 布局系统：只计算一次，得到物理真相
        // ==========================================
        // 【新增】在计算布局前，先解析 Auto 高度
        engine.precalculate_auto_sizes(rows);

        let mut all_rects = engine.calc_all_rects(columns, rows);

        if !engine.drag.active {
            engine.rebuild_draggable_edges(columns, rows);
        }

        // 【终极架构】拖拽时注入 Absolute 覆盖，让布局引擎自然计算，彻底消灭 AST 突变与布局偏移
        if engine.drag.active {
            // 【修复借用冲突】加上 .clone()，避免 engine 被同时可变与不可变借用
            if let Some(app::DragTarget::ResizeFloating(layer_name, edge_mask)) = engine.drag.resize_target.clone() {
                let dx = engine.drag.last_col as i32 - engine.drag.start_col as i32;
                let dy = engine.drag.last_row as i32 - engine.drag.start_row as i32;

                let mut new_x = engine.drag.initial_anchor_x as i32;
                let mut new_y = engine.drag.initial_anchor_y as i32;
                let mut new_w = engine.drag.initial_width as i32;
                let mut new_h = engine.drag.initial_height as i32;

                // 1. 根据掩码应用原始位移
                if edge_mask & 1 != 0 { // Left
                    new_x += dx;
                    new_w -= dx;
                }
                if edge_mask & 2 != 0 { // Right
                    new_w += dx;
                }
                if edge_mask & 4 != 0 { // Top
                    new_y += dy;
                    new_h -= dy;
                }
                if edge_mask & 8 != 0 { // Bottom
                    new_h += dy;
                }

                // 2. 最小尺寸限制（保证对侧边缘绝对不动！）
                const MIN_W: i32 = 2;
                const MIN_H: i32 = 2;
                if new_w < MIN_W {
                    // 如果是左侧收缩到极限，必须反向修正 x，保持右边缘不动
                    if edge_mask & 1 != 0 {
                        new_x = engine.drag.initial_anchor_x as i32 + (engine.drag.initial_width as i32 - MIN_W);
                    }
                    new_w = MIN_W;
                }
                if new_h < MIN_H {
                    // 如果是顶部收缩到极限，必须反向修正 y，保持下边缘不动
                    if edge_mask & 4 != 0 {
                        new_y = engine.drag.initial_anchor_y as i32 + (engine.drag.initial_height as i32 - MIN_H);
                    }
                    new_h = MIN_H;
                }

                // 3. 屏幕边界限制（同样保证对侧边缘不动！）
                let term_w = columns as i32;
                let term_h = rows as i32;
                if new_x < 0 {
                    // 如果左侧碰到了左边界，必须加宽，保持右边缘不动
                    if edge_mask & 1 != 0 {
                        new_w += new_x; // new_x 是负数，相当于加宽
                    }
                    new_x = 0;
                }
                if new_y < 0 {
                    // 如果顶部碰到了上边界，必须加高，保持下边缘不动
                    if edge_mask & 4 != 0 {
                        new_h += new_y;
                    }
                    new_y = 0;
                }
                if new_x + new_w > term_w {
                    // 右侧超出屏幕，直接截断宽度
                    new_w = term_w - new_x;
                }
                if new_y + new_h > term_h {
                    // 底部超出屏幕，直接截断高度
                    new_h = term_h - new_y;
                }

                let final_x = new_x as u16;
                let final_y = new_y as u16;
                let final_w = new_w as u16;
                let final_h = new_h as u16;

                // 【关键修复】必须同时注入 window_rect_overrides！
                engine.window_rect_overrides.insert(
                    layer_name.clone(),
                    crate::layout::WindowSize::Absolute2D(final_w, final_h)
                );

                // 更新画布尺寸（维持锚点位置）
                for layer in &mut engine.layout_layers {
                    if !matches!(layer.anchor, crate::layout::Anchor::ScreenAbsolute {..}) { continue; }
                    if app::Engine::layout_contains_window(layer, &layer_name) {
                        layer.runtime_rect_override = Some(crate::layout::WindowRect {
                            start_col: final_x,
                            start_row: final_y,
                            width: final_w,
                            height: final_h,
                        });
                        break;
                    }
                }

                all_rects = engine.calc_all_rects(columns, rows);
                engine.mark_all_dirty();

            } else if let Some(app::DragTarget::ResizeEdge(primary, neighbor, dir)) = engine.drag.resize_target.clone() {
                let has_moved = engine.drag.last_col != engine.drag.start_col
                    || engine.drag.last_row != engine.drag.start_row;

                // 【核心修复】只在首次真正拖拽时重组 AST，并立刻冻结物理像素
                if !engine.drag.is_restructured && has_moved {
                    // 1. 重组 AST（改变拓扑结构，将叶子拉平为兄弟）
                    engine.restructure_tree_after_drag(&primary, &neighbor, dir, &all_rects);
                    // 2. 立刻用旧物理坐标反算新 AST 百分比，杜绝拓扑突变带来的视觉跳跃
                    engine.force_recalculate_percentages(&all_rects);
                    engine.drag.is_restructured = true;
                    // 3. AST 变了，必须重算物理真相
                    all_rects = engine.calc_all_rects(columns, rows);
                }

                // 只有重组完成后，才进行物理坐标篡改
                if engine.drag.is_restructured {
                    let r1 = engine.drag.initial_t1_rect;
                    let r2 = engine.drag.initial_t2_rect;

                    let oh1 = all_rects.iter().find(|(_, n, _, _)| n == &primary)
                        .map(|(_, _, b, _)| {
                            let (ox, oy) = b.overhead();
                            if dir == crate::layout::Direction::Horizontal { ox } else { oy }
                        }).unwrap_or(0);

                    let oh2 = all_rects.iter().find(|(_, n, _, _)| n == &neighbor)
                        .map(|(_, _, b, _)| {
                            let (ox, oy) = b.overhead();
                            if dir == crate::layout::Direction::Horizontal { ox } else { oy }
                        }).unwrap_or(0);

                    match dir {
                        crate::layout::Direction::Horizontal => {
                            let min_split = r1.start_col.saturating_add(oh1.max(1));
                            let max_split = r2.start_col.saturating_add(r2.width).saturating_sub(oh2.max(1));
                            if min_split < max_split {
                                let split = engine.drag.last_col.clamp(min_split, max_split);
                                let new_w1 = split - r1.start_col;
                                let new_w2 = (r2.start_col + r2.width) - split;

                                engine.window_rect_overrides.insert(primary.clone(), crate::layout::WindowSize::Absolute(new_w1.saturating_sub(oh1)));
                                engine.window_rect_overrides.insert(neighbor.clone(), crate::layout::WindowSize::Absolute(new_w2.saturating_sub(oh2)));
                            }
                        }
                        crate::layout::Direction::Vertical => {
                            let min_split = r1.start_row.saturating_add(oh1.max(1));
                            let max_split = r2.start_row.saturating_add(r2.height).saturating_sub(oh2.max(1));
                            if min_split < max_split {
                                let split = engine.drag.last_row.clamp(min_split, max_split);
                                let new_h1 = split - r1.start_row;
                                let new_h2 = (r2.start_row + r2.height) - split;

                                engine.window_rect_overrides.insert(primary.clone(), crate::layout::WindowSize::Absolute(new_h1.saturating_sub(oh1)));
                                engine.window_rect_overrides.insert(neighbor.clone(), crate::layout::WindowSize::Absolute(new_h2.saturating_sub(oh2)));
                            }
                        }
                    }
                    // 覆盖注入后，必须重算 all_rects 才能拿到正确的物理坐标供渲染使用
                    all_rects = engine.calc_all_rects(columns, rows);
                }
            }
        }

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

        // 【修复】清理过期的状态栏临时消息
        let mut status_expired = false;
        for comp in engine.components.values_mut() {
            if let app::Component::StatusBar(s) = comp {
                if let Some(expire) = s.message_expire {
                    if std::time::Instant::now() >= expire {
                        s.message = None;
                        s.message_expire = None;
                        status_expired = true;
                    }
                }
            }
        }
        if status_expired {
            engine.mark_all_dirty();
        }

        // 【优化】在渲染前统一处理积压的状态变更
        engine.flush_pending_updates(columns, rows);

        // 【新增】预计算状态栏文本
        engine.update_status_bars(columns, rows, &all_rects);

        while let Ok((view_name, target_id, content_bytes, is_graphic)) = engine.async_view_rx.try_recv() {
            if let Some(app::Component::View(v)) = engine.components.get_mut(&view_name) {
                v.is_loading = false;
                if v.cached_entity_id == target_id {

                    // 【修复】检测内容是否真的改变了，防止相同图片重复渲染导致卡顿！
                    let changed = match &v.content {
                        app::view::ViewContent::Graphic(old_bytes) => {
                            if is_graphic {
                                // 对比新旧字节流，只有不一样才重绘
                                old_bytes.as_slice() != content_bytes.as_slice()
                            } else {
                                true
                            }
                        }
                        _ => true,
                    };

                    if changed {
                        // 【修复缺失的代码】在这里检测是否从图片切换到了非图片
                        let was_graphic = matches!(v.content, app::view::ViewContent::Graphic(_));
                        let will_be_graphic = is_graphic;

                        if was_graphic && !will_be_graphic {
                            v.needs_graphic_clear = true; // 触发物理擦除旧图片像素！
                        }

                        v.content = if is_graphic {
                            app::view::ViewContent::Graphic(content_bytes)
                        } else {
                            let text = String::from_utf8_lossy(&content_bytes).to_string();
                            app::view::ViewContent::Text(text)
                        };
                        v.scroll_offset = 0;
                        v.graphic_dirty = true; // 只有真正改变时才标记 dirty
                    }
                    engine.mark_dirty(&view_name);
                }
            }
        }

        // 如果之前有因加载中而被挂起的更新，现在重新触发
        if !engine.pending_view_reload.is_empty() {
            engine.pending_view_reload.clear();
            if let app::Focus::Component(tree_name) = &engine.focus.current {
                let tree_name = tree_name.clone(); // 【修复】提前 clone，释放不可变借用
                engine.broadcast_selection_changed(&tree_name, columns, rows);
            }
        }

        if !engine.is_initialized {
            engine.is_initialized = true;
            engine.init_views();
            if let app::Focus::Component(name) = &engine.focus.current.clone() {
                let name = name.clone();
                engine.broadcast_selection_changed(&name, columns, rows);
                // 【补丁】启动时触发 load 信号，符合契约
                engine.emit("load", columns, rows);
            }
        }

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
