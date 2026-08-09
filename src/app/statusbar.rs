// src/app/statusbar.rs

#[derive(Debug, Clone)]
pub struct StatusBarState {
    pub format_template: String,
    pub message: Option<String>,
    pub message_expire: Option<std::time::Instant>,
}
