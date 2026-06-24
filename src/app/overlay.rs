// src/app/overlay.rs

#[derive(Debug)]
pub struct OverlayState {
    pub visible: bool,
    pub text: String,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
