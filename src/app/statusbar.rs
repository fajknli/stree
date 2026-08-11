// src/app/statusbar.rs

#[derive(Debug, Clone)]
pub struct StatusBarState {
    pub format_template: String,
    pub message: Option<String>,
    pub message_expire: Option<std::time::Instant>,
    pub current_text: String, // 【新增】缓存最终要渲染的文本
}

impl Default for StatusBarState {
    fn default() -> Self {
        Self {
            format_template: String::new(),
            message: None,
            message_expire: None,
            current_text: String::new(),
        }
    }
}
