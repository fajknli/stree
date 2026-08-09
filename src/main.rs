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
use crossterm::event::{self, Event, KeyCode, EnableMouseCapture, DisableMouseCapture};
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
    #[arg(long, default_value_t = 3)]
    scroll_step: u8,
    #[arg(long, default_value_t = 500)]
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
        cli.max_lines,
        &cli.ui_colors,
    );

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
    let mut last_event_time = std::time::Instant::now();

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
        // 【终极哲学：终端尺寸变化时，解除物理锁，让 AST 比例接管】
        if (columns, rows) != engine.prev_term_size {
            engine.window_rect_overrides.clear();
            engine.prev_term_size = (columns, rows);
            engine.prev_rects.clear(); // 【修复】清空上一帧的物理快照，强制触发 force_full 全屏重绘！
            engine.mark_all_dirty();
        }
        let term_size = ui::TermSize { columns, rows };

        if signal::check_and_clear_reload() {
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

        // ==========================================
        // 1. 布局系统：只计算一次，得到物理真相
        // ==========================================
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

                let mut new_w = engine.drag.initial_width as i32;
                let mut new_h = engine.drag.initial_height as i32;
                let mut new_x = engine.drag.initial_anchor_x as i32;
                let mut new_y = engine.drag.initial_anchor_y as i32;

                // 【核心魔法】根据拖拽的边，计算新尺寸和新坐标，保证对侧不动
                if edge_mask & 2 != 0 { new_w = engine.drag.initial_width as i32 + dx; }
                if edge_mask & 1 != 0 { new_w = engine.drag.initial_width as i32 - dx; new_x = engine.drag.initial_anchor_x as i32 + dx; }
                if edge_mask & 8 != 0 { new_h = engine.drag.initial_height as i32 + dy; }
                if edge_mask & 4 != 0 { new_h = engine.drag.initial_height as i32 - dy; new_y = engine.drag.initial_anchor_y as i32 + dy; }

                // only keep border
                let min_w = 2;
                let min_h = 2;
                if new_w < min_w {
                    if edge_mask & 1 != 0 { new_x -= min_w - new_w; }
                    new_w = min_w;
                }
                if new_h < min_h {
                    if edge_mask & 4 != 0 { new_y -= min_h - new_h; }
                    new_h = min_h;
                }

                // 递归找到图层中的 Window 节点并篡改它的 Root 尺寸和锚点坐标
                fn modify_node(node: &mut crate::layout::LayoutNode, name: &str, new_w: i32, new_h: i32) -> bool {
                    match node {
                        crate::layout::LayoutNode::Window { name: n, size, .. } => {
                            if n == name {
                                *size = Some(crate::layout::WindowSize::Absolute2D(new_w as u16, new_h as u16));
                                return true;
                            }
                        }
                        crate::layout::LayoutNode::Container { children, .. } => {
                            for c in children {
                                if modify_node(c, name, new_w, new_h) { return true; }
                            }
                        }
                    }
                    false
                }

                for layer in &mut engine.layout_layers {
                    if !matches!(layer.anchor, crate::layout::Anchor::ScreenAbsolute {..}) { continue; }
                    if modify_node(&mut layer.root, &layer_name, new_w, new_h) {
                        layer.anchor = crate::layout::Anchor::ScreenAbsolute {
                            x: crate::layout::Coord::Pixels(new_x.max(0) as u16),
                            y: crate::layout::Coord::Pixels(new_y.max(0) as u16),
                        };
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
            if last_event_time.elapsed() < Duration::from_secs(1) { Duration::from_millis(10) }
            else if last_event_time.elapsed() > Duration::from_secs(5) { Duration::from_millis(200) }
            else { Duration::from_millis(50) }
        };

        if event::poll(poll_timeout)? {
            last_event_time = std::time::Instant::now();

            // 清空但保留底层容量
            key_events.clear();
            mouse_events.clear();

            loop {
                match event::read()? {
                    Event::Key(k) => key_events.push(k),
                    Event::Mouse(m) => {
                        if matches!(m.kind, crossterm::event::MouseEventKind::Drag(_) | crossterm::event::MouseEventKind::Moved) {
                            let should_replace = if let Some(last_m) = mouse_events.last() {
                                matches!(last_m.kind, crossterm::event::MouseEventKind::Drag(_) | crossterm::event::MouseEventKind::Moved)
                            } else {
                                false
                            };

                            if should_replace {
                                *mouse_events.last_mut().unwrap() = m;
                            } else {
                                mouse_events.push(m);
                            }
                        } else {
                            mouse_events.push(m);
                        }
                    }
                    _ => {}
                }

                if !event::poll(Duration::ZERO)? { break; }

                // 【修复】限制单帧最大处理事件数，防止大量鼠标移动事件导致渲染饥饿
                if key_events.len() + mouse_events.len() > 50 { break; }

                // 【极致丝滑优化】如果在拖拽中，并且已经拿到了最新的鼠标位置，立刻退出去渲染！
                // 队列里积压的旧 Drag 事件会在下一轮循环中被快速丢弃，绝不阻塞渲染。
                if engine.drag.active {
                    if let Some(last_m) = mouse_events.last() {
                        if matches!(last_m.kind, crossterm::event::MouseEventKind::Drag(_) | crossterm::event::MouseEventKind::Moved) {
                            break;
                        }
                    }
                }
            }

            // 1. 优先处理所有键盘事件
            for key_event in &key_events {
                if key_event.kind != crossterm::event::KeyEventKind::Press { continue; }

                if engine.drag.active {
                    if key_event.code == KeyCode::Esc {
                        engine.drag.active = false;
                        engine.drag.resize_target = None;
                        engine.drag.start_idx = None;
                    }
                    continue;
                }

                if engine.last_error.is_some() { engine.last_error = None; }

                if engine.has_active_input() {
                    match key_event.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace | KeyCode::Left | KeyCode::Right
                        | KeyCode::Home | KeyCode::End | KeyCode::Char(_) => {

                            // 提前判断是否是搜索框激活
                            let is_search = engine.components.values()
                                .any(|c| matches!(c, app::Component::Input(i) if i.is_active && i.prefix == "/"));

                            if let Some((input_name, result)) = engine.handle_input_key(*key_event) {
                                if result == "__CANCEL__" {
                                    // 【修复】如果是搜索框取消，必须清空搜索状态，恢复全列表！
                                    if is_search {
                                        if let app::Focus::Component(focused_name) = engine.focus.current.clone() {
                                            if let Some(app::Component::Tree(t)) = engine.components.get_mut(&focused_name) {
                                                if t.search_query.take().is_some() {
                                                    t.rebuild_visible_ids();
                                                    if !t.visible_ids.is_empty() {
                                                        t.selected_idx = 0;
                                                        t.selected_id = Some(t.visible_ids[0].clone());
                                                    }
                                                }
                                            }
                                            engine.pending_selection_changed = Some(focused_name);
                                        }
                                        engine.mark_all_dirty();
                                    }
                                } else {
                                    if is_search {
                                        engine.apply_search(&result, columns, rows);
                                    } else {
                                        engine.submit_input(&input_name, &result, columns, rows);
                                    }
                                }
                            } else if is_search && key_event.code != KeyCode::Esc && key_event.code != KeyCode::Enter {
                                // 【新增】fzf 式实时搜索过滤
                                if let Some(app::Component::Input(i)) = engine.components.values().find(|c| matches!(c, app::Component::Input(i) if i.is_active)) {
                                    let current_buffer = i.buffer.clone();
                                    engine.apply_search(&current_buffer, columns, rows);
                                }
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
                match key_event.code {
                    _ => {
                        if let Some((full_cmd_args, is_silent)) = engine.prepare_key_binding_args(key_event, columns, rows) {
                            if let Some(internal_cmd) = app::InternalCommand::from_args(&full_cmd_args) {
                                match internal_cmd {
                                    app::InternalCommand::Exit => break 'main_loop,
                                    app::InternalCommand::Esc => {
                                        if engine.has_active_input() {
                                            engine.cancel_input();
                                        } else {
                                            // 【新增】如果没有激活的 Input，按 Esc 退出搜索结果，恢复全列表
                                            let mut dirty = false;
                                            for (name, comp) in engine.components.iter_mut() {
                                                if let app::Component::Tree(t) = comp {
                                                    if t.search_query.take().is_some() {
                                                        t.rebuild_visible_ids();
                                                        if !t.visible_ids.is_empty() {
                                                            t.selected_idx = 0;
                                                            t.selected_id = Some(t.visible_ids[0].clone());
                                                        }
                                                        engine.pending_selection_changed = Some(name.clone());
                                                        dirty = true;
                                                    }
                                                }
                                            }
                                            if dirty { engine.mark_all_dirty(); }
                                        }
                                    }
                                    app::InternalCommand::Tab => engine.handle_tab(columns, rows),
                                    app::InternalCommand::Expand => engine.toggle_expand(),
                                    app::InternalCommand::Mark => engine.toggle_mark(),
                                    app::InternalCommand::Up => engine.move_up(),
                                    app::InternalCommand::Down => engine.move_down(),
                                    app::InternalCommand::Top => engine.jump_to_top(),
                                    app::InternalCommand::Bottom => engine.jump_to_bottom(),
                                    app::InternalCommand::Enter => {
                                        engine.toggle_expand();
                                        engine.emit("confirm", columns, rows);
                                    }
                                    app::InternalCommand::ActivateSearch => {
                                        let input_name = engine.components.iter()
                                            .find(|(_, c)| matches!(c, app::Component::Input(_)))
                                            .map(|(n, _)| n.clone());
                                        if let Some(name) = input_name {
                                            engine.activate_input(&name, "/");
                                        }
                                    }
                                    app::InternalCommand::ActivateCmd => {
                                        let input_name = engine.components.iter()
                                            .find(|(_, c)| matches!(c, app::Component::Input(_)))
                                            .map(|(n, _)| n.clone());
                                        if let Some(name) = input_name {
                                            engine.activate_input(&name, ":");
                                        }
                                    }
                                    app::InternalCommand::ActivateInput(name) => {
                                        engine.activate_input(&name, "");
                                    }
                                    app::InternalCommand::ToggleLayout(name) => engine.toggle_layout_visible(&name),
                                    app::InternalCommand::ShowLayout(name) => engine.set_layout_visible(&name, true),
                                    app::InternalCommand::HideLayout(name) => engine.set_layout_visible(&name, false),
                                    app::InternalCommand::ScrollLeft => engine.scroll_left(),
                                    app::InternalCommand::ScrollRight => engine.scroll_right(),
                                    // 【新增指令派发】
                                    app::InternalCommand::CycleLayer => engine.cycle_layer(&all_rects),
                                    app::InternalCommand::FocusLeft => engine.focus_direction("left", &all_rects),
                                    app::InternalCommand::FocusRight => engine.focus_direction("right", &all_rects),
                                    app::InternalCommand::FocusUp => engine.focus_direction("up", &all_rects),
                                    app::InternalCommand::FocusDown => engine.focus_direction("down", &all_rects),
                                }
                            } else {
                                runner::execute_binding(&mut engine, &full_cmd_args, is_silent, columns, rows);
                            }
                        }
                    }
                }
            }

            // 2. 处理鼠标事件
            // 【优化2】将排序和收集移到循环外，避免每个鼠标事件都重复分配和排序
            let mut sorted_rects: Vec<_> = all_rects.iter().collect();
            sorted_rects.sort_by(|a, b| b.3.cmp(&a.3));

            // 【修复借用冲突】使用 clone 截断对 engine 的不可变借用，允许循环内部对 engine 进行可变操作
            let mut sorted_edges: Vec<_> = engine.drag.cached_edges.clone();
            sorted_edges.sort_by(|a, b| b.z_index.cmp(&a.z_index));
            for mouse_event in &mouse_events {
                if !engine.mouse.enabled { continue; }

                // ==========================================
                // 【0. 悬停获取焦点
                // ==========================================
                if matches!(mouse_event.kind, crossterm::event::MouseEventKind::Moved) && !engine.drag.active {
                    for (rect, name, _, _) in sorted_rects.iter() {
                        let in_x = mouse_event.column >= rect.start_col && mouse_event.column < rect.start_col + rect.width;
                        let in_y = mouse_event.row >= rect.start_row && mouse_event.row < rect.start_row + rect.height;

                        if in_x && in_y {
                            // 绝不允许 StatusBar 获得焦点
                            if let Some(app::Component::StatusBar(_)) = engine.components.get(name) { break; }

                            // 如果焦点发生了变化，更新状态并标脏
                            if engine.focus.current != app::Focus::Component(name.clone()) {
                                let old_focus = engine.focus.current.clone();
                                engine.focus.current = app::Focus::Component(name.clone());

                                if let app::Focus::Component(old_name) = &old_focus {
                                    engine.mark_dirty(old_name);
                                }
                                engine.mark_dirty(name);

                                // 状态栏也需要刷新
                                for (n, c) in &engine.components {
                                    if matches!(c, app::Component::StatusBar(_)) {
                                        engine.dirty_components.insert(n.clone());
                                    }
                                }

                                // 如果该组件配置了 focus 信号，则触发
                                if let Some(app::Component::Tree(t)) = engine.components.get(name) {
                                    if t.focus_to_fire {
                                        engine.emit("focus", columns, rows);
                                    }
                                }
                            }
                            break; // 只要命中了最顶层的窗口，就立刻退出
                        }
                    }
                    continue; // 悬停事件只用于改焦点，不进入后续点击逻辑
                }

                if matches!(mouse_event.kind, crossterm::event::MouseEventKind::Up(_)) {
                    if engine.drag.active {
                        if let Some(app::DragTarget::ResizeFloating(_, _)) = &engine.drag.resize_target {
                            engine.mark_all_dirty(); // 标记重绘
                        } else if engine.drag.resize_target.is_some() {
                            let has_dragged = engine.drag.last_col != engine.drag.start_col
                                || engine.drag.last_row != engine.drag.start_row;

                            if has_dragged {
                                // AST 已在拖拽首帧重组完毕，这里只需用最终物理真相反算百分比
                                engine.force_recalculate_percentages(&all_rects);
                            }
                            // 清除覆盖，让新 AST 接管
                            engine.window_rect_overrides.clear();
                            engine.mark_all_dirty();
                        }
                        engine.drag.active = false;
                        engine.drag.resize_target = None;
                        engine.drag.start_idx = None;
                        continue;
                    }
                }

                if engine.drag.active && engine.drag.resize_target.is_some() {
                    match mouse_event.kind {
                        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                            // 【修改】只记录最新坐标，具体篡改交给帧头处理
                            engine.drag.last_col = mouse_event.column;
                            engine.drag.last_row = mouse_event.row;
                        }
                        _ => {}
                    }
                    continue;
                }

                // ==========================================
                // 非拖拽状态：碰撞检测 (优先级：顶层浮动边缘 > 底层 Flexbox 边缘 > 窗口内部)
                // ==========================================

                let mut hit_floating_edge = false;
                let mut hit_floating_inside = false;

                // 1. 优先检测：浮动窗口边缘拉伸
                if let crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) = mouse_event.kind {
                    let (edge_hit, inside) = engine.check_floating_edge_hit(mouse_event.column, mouse_event.row, &all_rects);
                    hit_floating_inside = inside;

                    if let Some((win_name, edge_mask, init_x, init_y, w, h)) = edge_hit {
                        engine.focus.current = app::Focus::Component(win_name.clone());
                        engine.mark_dirty(&win_name);

                        engine.drag.active = true;
                        engine.drag.resize_target = Some(app::DragTarget::ResizeFloating(win_name.clone(), edge_mask));
                        engine.drag.start_col = mouse_event.column;
                        engine.drag.start_row = mouse_event.row;
                        engine.drag.last_col = mouse_event.column;
                        engine.drag.last_row = mouse_event.row;
                        engine.drag.initial_width = w;
                        engine.drag.initial_height = h;
                        engine.drag.initial_anchor_x = init_x;
                        engine.drag.initial_anchor_y = init_y;
                        hit_floating_edge = true;
                    }
                }

                if hit_floating_edge {
                    continue; // 已经开始拖拽，拦截事件
                }

                // 2. 次优先检测：底层 Flexbox 边缘
                // 【优化2】去除了原本在这里的 sorted_edges 收集和排序，直接使用外部的 sorted_edges
                let mut hit_edge = None;
                // 【关键修复】如果鼠标点在了浮动窗口内部，直接跳过 Flexbox 边缘检测！
                if !hit_floating_inside {
                    for edge in &sorted_edges {
                        let in_x = mouse_event.column >= edge.hit_rect.start_col
                            && mouse_event.column < edge.hit_rect.start_col + edge.hit_rect.width;
                        let in_y = mouse_event.row >= edge.hit_rect.start_row
                            && mouse_event.row < edge.hit_rect.start_row + edge.hit_rect.height;

                        if in_x && in_y {
                            hit_edge = Some(edge.clone());
                            break;
                        }
                    }
                }

                let mut in_intersection = false;
                for &(x, y) in &engine.drag.cached_intersections {
                    if (mouse_event.column == x || mouse_event.column == x - 1) &&
                       (mouse_event.row == y || mouse_event.row == y - 1) {
                        in_intersection = true;
                        break;
                    }
                }

                if !in_intersection {
                    if let Some(edge) = hit_edge {
                        if let crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) = mouse_event.kind {
                            let primary_id = edge.primary_id.clone();
                            let neighbor_id = edge.neighbor_id.clone();
                            let dir = edge.direction;

                            // 【修复 2】点击边框时赋予焦点
                            engine.focus.current = app::Focus::Component(primary_id.clone());
                            engine.mark_dirty(&primary_id);

                            // 1. 记录叶子节点的初始物理尺寸
                            let r1 = engine.get_node_current_bbox(&primary_id, columns, rows).map(|(r, _)| r).unwrap_or_default();
                            let r2 = engine.get_node_current_bbox(&neighbor_id, columns, rows).map(|(r, _)| r).unwrap_or_default();

                            engine.drag.active = true;
                            engine.drag.is_restructured = false;
                            engine.drag.resize_target = Some(app::DragTarget::ResizeEdge(primary_id, neighbor_id, dir));
                            engine.drag.start_col = mouse_event.column;
                            engine.drag.start_row = mouse_event.row;
                            engine.drag.last_col = mouse_event.column;
                            engine.drag.last_row = mouse_event.row;
                            engine.drag.initial_t1_rect = r1;
                            engine.drag.initial_t2_rect = r2;
                            continue;
                        }
                    }
                }

                // ==========================================
                // 窗口内容区命中逻辑
                // ==========================================
                for (rect, name, _border, _z) in sorted_rects.iter() {
                    let in_x = mouse_event.column >= rect.start_col && mouse_event.column < rect.start_col + rect.width;
                    let in_y = mouse_event.row >= rect.start_row && mouse_event.row < rect.start_row + rect.height;
                    if !in_x || !in_y { continue; }

                    // 【终极防御】鼠标点击 StatusBar 时，直接跳过，绝不允许它获得焦点！
                    if let Some(app::Component::StatusBar(_)) = engine.components.get(name) {
                        continue;
                    }

                    // 【修复 1】只在鼠标按下时才切换焦点，悬停不再改变样式！
                    let is_press = matches!(mouse_event.kind,
                        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) |
                        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Right)
                    );

                    if is_press {
                        let old_focus = engine.focus.current.clone();
                        engine.focus.current = app::Focus::Component(name.clone());

                        if old_focus != engine.focus.current {
                            if let app::Focus::Component(old_name) = &old_focus {
                                engine.mark_dirty(old_name);
                            }
                            engine.mark_dirty(name);
                            // 【修复】焦点变化时，状态栏也需要更新
                            for (n, c) in &engine.components {
                                if matches!(c, app::Component::StatusBar(_)) {
                                    engine.dirty_components.insert(n.clone());
                                }
                            }
                            if let Some(app::Component::Tree(t)) = engine.components.get(name) {
                                if t.focus_to_fire {
                                    engine.emit("focus", columns, rows);
                                }
                            }
                        }

                        // 【修复 2】鼠标点击切换焦点时，强制关闭激活的输入框！彻底解决 Space 失灵
                        if engine.has_active_input() {
                            engine.cancel_input();
                        }
                    }

                    match engine.components.get(name) {
                        Some(app::Component::Tree(_)) => {
                            let click_to_fire = if let Some(app::Component::Tree(t)) = engine.components.get(name) {
                                t.click_to_fire
                            } else {
                                false
                            };

                            // 【关键修复】提前提取所有需要的数据，结束不可变借用！
                            let (target_idx, clicked_id, visible_len, tree_name) =
                            if let Some(app::Component::Tree(t)) = engine.components.get(name) {
                                let max_rows = (rect.height as usize).saturating_sub(2);
                                // 【修复滚动跳跃】传入 t.v_scroll
                                let scroll_offset = ui::calc_scroll_offset(t.selected_idx, t.visible_ids.len(), max_rows, t.v_scroll);
                                let target_idx = scroll_offset + mouse_event.row.saturating_sub(rect.start_row).saturating_sub(1) as usize;
                                let is_valid_click = target_idx < t.visible_ids.len();
                                let clicked_id = if is_valid_click { Some(t.visible_ids[target_idx].clone()) } else { None };
                                (target_idx, clicked_id, t.visible_ids.len(), name.clone())
                            } else {
                                (0, None, 0, String::new())
                            };

                            match mouse_event.kind {
                                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                    if let Some(ref cid) = clicked_id {
                                        let now = std::time::Instant::now();
                                        let is_double_click = engine.mouse.last_click_time
                                            .map_or(false, |t| now.duration_since(t).as_millis() < 300)
                                            && engine.mouse.last_clicked_id.as_deref() == Some(cid.as_str());

                                        if is_double_click {
                                            engine.select_id(&tree_name, cid);
                                            engine.toggle_expand();
                                            engine.emit("confirm", columns, rows);
                                            engine.mouse.last_click_time = None;
                                        } else {
                                            engine.select_id(&tree_name, cid);
                                            engine.mouse.last_click_time = Some(now);
                                            engine.mouse.last_clicked_id = Some(cid.clone());
                                            if click_to_fire {
                                                engine.emit("click", columns, rows);
                                            }
                                        }
                                    }
                                }
                                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Right) => {
                                    if let Some(ref cid) = clicked_id {
                                        if let Some(app::Component::Tree(t)) = engine.components.get_mut(&tree_name) {
                                            let was_marked = t.marked_ids.contains(cid);
                                            if was_marked {
                                                t.marked_ids.remove(cid);
                                            } else {
                                                t.marked_ids.insert(cid.clone());
                                            }
                                            engine.drag.is_marking = !was_marked;
                                        }
                                        engine.drag.start_idx = Some(target_idx);
                                        engine.drag.active = true;
                                        engine.mark_dirty(&tree_name); // 【修复】立刻标脏，触发重绘
                                    }
                                }
                                crossterm::event::MouseEventKind::ScrollUp => {
                                    engine.move_up_n(scroll_step as usize);
                                }
                                crossterm::event::MouseEventKind::ScrollDown => {
                                    engine.move_down_n(scroll_step as usize);
                                }
                                crossterm::event::MouseEventKind::Up(_) => {
                                    engine.drag.active = false;
                                    engine.drag.start_idx = None;
                                    engine.drag.resize_target = None;
                                    engine.mark_dirty(&tree_name); // 【优化】释放时也标脏，避免残影
                                }
                                crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Right) => {
                                    if engine.drag.active {
                                        if let Some(start_idx) = engine.drag.start_idx {
                                            let clamped_target = target_idx.min(visible_len.saturating_sub(1));
                                            let range = if clamped_target >= start_idx {
                                                start_idx..=clamped_target
                                            } else {
                                                clamped_target..=start_idx
                                            };
                                            // 【修复】现在可以安全地 get_mut 了
                                            if let Some(app::Component::Tree(t)) = engine.components.get_mut(&tree_name) {
                                                for i in range {
                                                    if let Some(id) = t.visible_ids.get(i) {
                                                        if engine.drag.is_marking {
                                                            t.marked_ids.insert(id.clone());
                                                        } else {
                                                            t.marked_ids.remove(id);
                                                        }
                                                    }
                                                }
                                                engine.mark_dirty(&tree_name);
                                            }
                                        }
                                    }
                                }
                                _ => {}
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

        // 【异步接收】检查是否有后台预览命令执行完毕
        while let Ok((view_name, target_id, content)) = engine.async_view_rx.try_recv() {
            if let Some(app::Component::View(v)) = engine.components.get_mut(&view_name) {
                v.is_loading = false;
                // 只有当返回的结果对应于当前最新选中的 ID 时，才更新缓冲区
                if v.cached_entity_id == target_id {
                    // 【修复】如果内容没变，保留滚动位置，防止跳动
                    if v.content_buffer != content {
                        v.content_buffer = content;
                        v.scroll_offset = 0;
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
