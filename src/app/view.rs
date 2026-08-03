// src/app/view.rs

#[derive(Debug)]
pub struct ViewState {
    pub cmd_template: String,
    pub scroll_offset: usize,
    pub content_buffer: String,
    pub cached_entity_id: Option<String>,
    pub max_offset: usize,
    /// 当前格子宽度（由 update_view_rects 写入），用于 {width} 占位符
    pub rect_width: u16,
    /// 当前格子高度（内容区高度），用于 {height} 占位符
    pub rect_height: u16,
    pub h_scroll: usize,
    pub is_loading: bool, // 【新增】标记是否正在后台加载
}
