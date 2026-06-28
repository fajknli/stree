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
        let focused_name = if let app::Focus::Component(name) = &engine.focus.current { name.clone() } else { String::new() };
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

    'main_loop: loop {
        if signal::check_and_clear_quit() { break 'main_loop; }

        let (columns, rows) = match crossterm::terminal::size() { Ok(s) => s, Err(_) => continue };
        let term_size = crossterm::terminal::WindowSize { width: columns, height: rows, columns, rows };

        if signal::check_and_clear_reload() {
            engine.trigger_reload(columns, rows);
        }

        ipc_server.try_accept_and_process(|target, data| {
            engine.handle_ipc_update(target, data, columns, rows);
        });

        // 【重构】使用多图层布局计算
        let all_rects = engine.calc_all_rects(columns, rows);

        engine.rebuild_draggable_edges(columns, rows);

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
                    if engine.drag.active {
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
                                if let Some(internal_cmd) = app::InternalCommand::from_args(&full_cmd_args) {
                                    match internal_cmd {
                                        app::InternalCommand::Exit => break 'main_loop,
                                        app::InternalCommand::Esc => {
                                            if engine.has_active_input() { engine.cancel_input(); }
                                        }
                                        app::InternalCommand::Tab => engine.handle_tab(columns, rows),
                                        app::InternalCommand::Up => engine.move_up(columns, rows),
                                        app::InternalCommand::Down => engine.move_down(columns, rows),
                                        app::InternalCommand::Expand => engine.toggle_expand(),
                                        app::InternalCommand::Mark => engine.toggle_mark(),
                                        app::InternalCommand::Top => engine.jump_to_top(columns, rows),
                                        app::InternalCommand::Bottom => engine.jump_to_bottom(columns, rows),
                                        app::InternalCommand::Enter => {
                                            engine.toggle_expand();
                                            engine.emit("confirm", columns, rows);
                                        }
                                        app::InternalCommand::ActivateSearch => {
                                            if engine.components.contains_key("Cmd") { engine.activate_input("Cmd", "/"); }
                                        }
                                        app::InternalCommand::ActivateCmd => {
                                            if engine.components.contains_key("Cmd") { engine.activate_input("Cmd", ":"); }
                                        }
                                        app::InternalCommand::ActivateInput(name) => {
                                            engine.activate_input(&name, "");
                                        }
                                        app::InternalCommand::ToggleLayout(name) => engine.toggle_layout_visible(&name),
                                        app::InternalCommand::ShowLayout(name) => engine.set_layout_visible(&name, true),
                                        app::InternalCommand::HideLayout(name) => engine.set_layout_visible(&name, false),
                                    }
                                } else {
                                    execute_binding(&mut engine, &full_cmd_args, is_silent, columns, rows);
                                }
                            }
                        }
                    }
                }

                Event::Mouse(mouse_event) => {
                    if !engine.mouse.enabled { continue 'main_loop; }

                    // 【重构】模态拖拽拦截：拖拽边框时，无视 Z 轴和窗口边界，全局响应 Drag 和 Up
                    if engine.drag.active && engine.drag.resize_target.is_some() {
                        match mouse_event.kind {
                            crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                                if let Some(app::DragTarget::ResizeEdge(ref primary, ref neighbor, dir)) = engine.drag.resize_target {
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
                                                let new_left_content = split - left.start_col - left_extra;
                                                let total_ab_content = (left.width + right.width).saturating_sub(left_extra + right_extra);
                                                let new_right_content = total_ab_content.saturating_sub(new_left_content);

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
                                                let new_top_content = split - top.start_row - top_extra;
                                                let total_ab_content = (top.height + bottom.height).saturating_sub(top_extra + bottom_extra);
                                                let new_bottom_content = total_ab_content.saturating_sub(new_top_content);

                                                engine.window_rect_overrides.insert(top_name.clone(), crate::layout::WindowSize::Absolute(new_top_content));
                                                engine.window_rect_overrides.insert(bottom_name.clone(), crate::layout::WindowSize::Absolute(new_bottom_content));
                                            }
                                        }
                                    }
                                }
                            }
                            crossterm::event::MouseEventKind::Up(_) => {
                                // 1. 获取当前物理真相（带着拖拽时的 Absolute 约束）
                                let all_rects_now = engine.calc_all_rects(columns, rows);

                                // 2. 【关键修复】：无论是否重组过，都必须把当前的 Absolute 转为 Percent 写回树里！
                                // 这样才能保证同容器拖拽不回弹！
                                engine.force_recalculate_percentages(&all_rects_now);

                                // 3. 清空所有约束，把控制权交还给 Flexbox
                                engine.window_rect_overrides.clear();
                                engine.drag.active = false;
                                engine.drag.resize_target = None;
                                continue 'main_loop;
                            }
                            _ => {}
                        }
                        continue 'main_loop; // 拖拽模态下，屏蔽所有其他鼠标事件
                    }

                    // 【终极重构 1】全局边界碰撞检测：无视 [drag] 属性，有缝就能拖
                    // 按 z_index 降序排序，保证高 Z 层（浮动窗口）的边界优先命中
                    let mut sorted_edges: Vec<_> = engine.drag.cached_edges.iter().collect();
                    sorted_edges.sort_by(|a, b| b.z_index.cmp(&a.z_index));

                    let mut hit_edge = None;
                    for edge in sorted_edges {
                        let in_x = mouse_event.column >= edge.hit_rect.start_col
                            && mouse_event.column < edge.hit_rect.start_col + edge.hit_rect.width;
                        let in_y = mouse_event.row >= edge.hit_rect.start_row
                            && mouse_event.row < edge.hit_rect.start_row + edge.hit_rect.height;

                        if in_x && in_y {
                            hit_edge = Some(edge);
                            break;
                        }
                    }

                    // 如果命中了边界，且是鼠标左键按下，直接进入拖拽模态
                    // 【新增】交点盲区剔除：如果鼠标在交点附近，不响应边的拖拽
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

                                // 【关键修复】：按下时，如果对齐，立刻重组树！
                                let all_rects_before = engine.calc_all_rects(columns, rows);
                                let mr = all_rects_before.iter().find(|(_, n, _, _)| n == &primary_id).map(|(r, _, _, _)| *r);
                                let nr = all_rects_before.iter().find(|(_, n, _, _)| n == &neighbor_id).map(|(r, _, _, _)| *r);

                                if let (Some(m), Some(n)) = (mr, nr) {
                                    let is_aligned = match dir {
                                        crate::layout::Direction::Horizontal => m.height == n.height,
                                        crate::layout::Direction::Vertical => m.width == n.width,
                                    };

                                    if is_aligned {
                                        let restructured = engine.restructure_tree_after_drag(&primary_id, &neighbor_id, dir, &all_rects_before);
                                        if restructured {
                                            // 重组后，用旧物理尺寸重算新树百分比，保证按下瞬间零跳动
                                            engine.force_recalculate_percentages(&all_rects_before);
                                        }
                                    }
                                }

                                engine.drag.active = true;
                                engine.drag.resize_target = Some(app::DragTarget::ResizeEdge(
                                    primary_id,
                                    neighbor_id,
                                    dir
                                ));
                                continue 'main_loop;
                            }
                        }
                    }

                    // 【终极重构 2】如果没有命中边界，执行原有的窗口内容区命中逻辑（焦点切换、点击等）
                    let all_rects = engine.calc_all_rects(columns, rows);
                    let mut sorted_rects: Vec<_> = all_rects.iter().collect();
                    sorted_rects.sort_by(|a, b| b.3.cmp(&a.3));

                    for (rect, name, _border, _z) in sorted_rects.iter() {
                        let in_x = mouse_event.column >= rect.start_col && mouse_event.column < rect.start_col + rect.width;
                        let in_y = mouse_event.row >= rect.start_row && mouse_event.row < rect.start_row + rect.height;
                        if !in_x || !in_y { continue; }

                        // 命中！焦点切换 + 门禁检查
                        let old_focus = engine.focus.current.clone();
                        engine.focus.current = app::Focus::Component(name.clone());

                        if old_focus != engine.focus.current {
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

                                    let target_idx = scroll_offset + mouse_event.row.saturating_sub(rect.start_row).saturating_sub(1) as usize;
                                    let is_valid_click = target_idx < t.visible_ids.len();
                                    let clicked_id = if is_valid_click { Some(t.visible_ids[target_idx].clone()) } else { None };

                                    match mouse_event.kind {
                                        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                            if let Some(ref cid) = clicked_id {
                                                let now = std::time::Instant::now();
                                                let is_double_click = engine.mouse.last_click_time
                                                    .map_or(false, |t| now.duration_since(t).as_millis() < 300)
                                                    && engine.mouse.last_clicked_id.as_deref() == Some(cid.as_str());

                                                let tree_name = name.clone();

                                                if is_double_click {
                                                    engine.select_id(&tree_name, cid, columns, rows);
                                                    engine.toggle_expand();
                                                    engine.emit("confirm", columns, rows);
                                                    engine.mouse.last_click_time = None;
                                                } else {
                                                    engine.select_id(&tree_name, cid, columns, rows);
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
                                                if let Some(app::Component::Tree(t)) = engine.components.get_mut(name) {
                                                    let was_marked = t.marked_ids.contains(cid);
                                                    if was_marked {
                                                        t.marked_ids.remove(cid);
                                                    } else {
                                                        t.marked_ids.insert(cid.clone());
                                                    }
                                                    engine.drag.mode = !was_marked;
                                                }
                                                engine.drag.start_idx = Some(target_idx);
                                                engine.drag.active = true;
                                            }
                                        }
                                        crossterm::event::MouseEventKind::ScrollUp => {
                                            engine.move_up_n(scroll_step as usize, columns, rows);
                                        }
                                        crossterm::event::MouseEventKind::ScrollDown => {
                                            engine.move_down_n(scroll_step as usize, columns, rows);
                                        }
                                        crossterm::event::MouseEventKind::Up(_) => {
                                            engine.drag.active = false;
                                            engine.drag.start_idx = None;
                                            engine.drag.resize_target = None;
                                        }
                                        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Right) => {
                                            if engine.drag.active {
                                                if let Some(start_idx) = engine.drag.start_idx {
                                                    let clamped_target = target_idx.min(t.visible_ids.len().saturating_sub(1));
                                                    let range = if clamped_target >= start_idx {
                                                        start_idx..=clamped_target
                                                    } else {
                                                        clamped_target..=start_idx
                                                    };
                                                    if let Some(app::Component::Tree(t)) = engine.components.get_mut(name) {
                                                        for i in range {
                                                            if let Some(id) = t.visible_ids.get(i) {
                                                                if engine.drag.mode {
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

    if let app::Focus::Component(name) = &engine.focus.current {
        if let Some(app::Component::Tree(t)) = engine.components.get(name) {
            if let Some(id) = &t.selected_id { println!("{}", id); }
        }
    }
    Ok(())
}

fn drain_terminal_events() {
    let mut drain_count = 0;
    while crossterm::event::poll(Duration::ZERO).unwrap_or(false) && drain_count < 100 {
        let _ = crossterm::event::read();
        drain_count += 1;
    }
}

fn execute_binding(
    engine: &mut app::Engine,
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
        drain_terminal_events();

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
