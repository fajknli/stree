// src/ui/buffer.rs

use crossterm::style::Color;
use crossterm::{cursor, style, QueueableCommand};
use std::io::Write;
use unicode_width::UnicodeWidthChar; // 【新增】引入宽度检测

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub symbol: char,
    pub fg: Color,
    pub bg: Option<Color>,
    pub bold: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { symbol: ' ', fg: Color::Reset, bg: None, bold: false }
    }
}

pub struct Buffer {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Cell>,
}

impl Buffer {
    pub fn empty(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
        }
    }

    pub fn clear(&mut self) {
        for cell in self.cells.iter_mut() {
            *cell = Cell::default();
        }
    }

    pub fn set_cell(&mut self, x: usize, y: usize, symbol: char, fg: Color, bg: Option<Color>, bold: bool) {
        if x < self.width && y < self.height {
            let idx = y * self.width + x;

            // 【终极解法】精准物理降级
            // 如果要写入的是实质字符（非空格），且左边是宽字符（如中文）
            if symbol != ' ' && x > 0 {
                let left_idx = y * self.width + (x - 1);
                let left_symbol = self.cells[left_idx].symbol;
                if UnicodeWidthChar::width(left_symbol).unwrap_or(0) == 2 {
                    // 必须将左边的中文强制降级为空格，否则终端会吞噬边框
                    self.cells[left_idx].symbol = ' ';
                }
            }

            self.cells[idx] = Cell { symbol, fg, bg, bold };
        }
    }

    pub fn get_cell(&self, x: usize, y: usize) -> Option<&Cell> {
        if x < self.width && y < self.height {
            Some(&self.cells[y * self.width + x])
        } else {
            None
        }
    }

    pub fn diff_and_flush<W: Write>(&self, prev: &Buffer, out: &mut W) -> std::io::Result<()> {
        let mut last_fg = Color::Reset;
        let mut last_bg: Option<Color> = None;
        let mut last_bold = false;

        // 【关键修复】每帧 Diff 开始前，强制重置终端的 SGR 全局状态！
        // 终端是状态机，如果不显式重置，上一帧末尾遗留的背景色/粗体会污染本帧初始的 Diff 判断。
        out.queue(style::SetAttribute(style::Attribute::Reset))?;

        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let curr = &self.cells[idx];
                let old = &prev.cells[idx];

                // 【终极修复】动态检测当前列是否是宽字符的右半部分
                let mut is_wide_right = false;
                if x > 0 {
                    let left_curr = &self.cells[y * self.width + (x - 1)];
                    if UnicodeWidthChar::width(left_curr.symbol).unwrap_or(0) == 2 {
                        is_wide_right = true;
                    }
                }

                if is_wide_right {
                    // 如果是宽字符的右半部分，绝对不能打印字符（会破坏左侧的宽字符），
                    // 但必须同步样式状态！防止状态机脱节导致后续颜色错乱和残影！
                    if curr.fg != last_fg {
                        out.queue(style::SetForegroundColor(curr.fg))?;
                        last_fg = curr.fg;
                    }
                    if curr.bg != last_bg {
                        if let Some(bg) = curr.bg {
                            out.queue(style::SetBackgroundColor(bg))?;
                        } else {
                            out.queue(style::SetBackgroundColor(Color::Reset))?;
                        }
                        last_bg = curr.bg;
                    }
                    if curr.bold != last_bold {
                        if curr.bold {
                            out.queue(style::SetAttribute(style::Attribute::Bold))?;
                        } else {
                            out.queue(style::SetAttribute(style::Attribute::NormalIntensity))?;
                        }
                        last_bold = curr.bold;
                    }
                    continue;
                }

                if curr != old {
                    out.queue(cursor::MoveTo(x as u16, y as u16))?;

                    if curr.fg != last_fg {
                        out.queue(style::SetForegroundColor(curr.fg))?;
                        last_fg = curr.fg;
                    }

                    if curr.bg != last_bg {
                        if let Some(bg) = curr.bg {
                            out.queue(style::SetBackgroundColor(bg))?;
                        } else {
                            out.queue(style::SetBackgroundColor(Color::Reset))?;
                        }
                        last_bg = curr.bg;
                    }

                    if curr.bold != last_bold {
                        if curr.bold {
                            out.queue(style::SetAttribute(style::Attribute::Bold))?;
                        } else {
                            out.queue(style::SetAttribute(style::Attribute::NormalIntensity))?;
                        }
                        last_bold = curr.bold;
                    }

                    out.queue(style::Print(curr.symbol))?;
                }
            }
        }
        Ok(())
    }
}
