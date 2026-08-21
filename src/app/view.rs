// src/app/view.rs

#[derive(Debug, Clone, PartialEq)]
pub enum ViewContent {
    Empty,
    Text(String),
    Graphic(Vec<u8>),
}

impl Default for ViewContent {
    fn default() -> Self {
        ViewContent::Empty
    }
}

#[derive(Debug)]
pub struct ViewState {
    pub content: ViewContent,
    pub cmd_template: String,
    pub scroll_offset: usize,
    pub cached_entity_id: Option<String>,
    pub max_offset: usize,
    pub rect_width: u16,
    pub rect_height: u16,
    pub h_scroll: usize,
    pub is_loading: bool,
    pub no_hover: bool,
    pub no_focus: bool,
    pub graphic_dirty: bool,
    pub needs_graphic_clear: bool,

    // 【修改】统一使用 PID (u32) 方案，干净利落
    pub child_pid: std::sync::Arc<std::sync::Mutex<Option<u32>>>,
    pub keymap: String,
}
