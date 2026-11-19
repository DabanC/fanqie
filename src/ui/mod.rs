mod chrome;
mod home;
mod input;
mod picker;
mod reader;
mod theme;

#[cfg(test)]
mod tests;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::{Block, Paragraph, Wrap},
    Frame,
};

use crate::app::App;

use self::{
    chrome::{header, hint, status},
    home::home,
    input::input,
    picker::overlay,
    reader::reader,
    theme::Palette,
};

pub(crate) fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let palette = Palette::new(app.settings.light_theme);
    frame.render_widget(
        Block::default().style(Style::default().bg(palette.bg).fg(palette.text)),
        area,
    );
    if area.width < 30 || area.height < 12 {
        frame.render_widget(
            Paragraph::new("Fanqie\n\n请放大终端至至少 30 × 12\nCtrl+Q 退出")
                .style(palette.accent())
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let margin = if area.width >= 70 { 3 } else { 1 };
    let outer = Rect {
        x: margin,
        y: 0,
        width: area.width.saturating_sub(margin * 2),
        height: area.height,
    };
    let sections = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(outer);
    header(frame, app, sections[0], palette);
    if app.current.is_some() {
        reader(frame, app, sections[1], palette);
    } else {
        home(frame, app, sections[1], palette);
    }
    status(frame, app, sections[2], palette);
    hint(frame, app, sections[3], palette);
    input(frame, app, sections[4], palette);
    let footer = if app.command_focus {
        "  Tab 补全 / 切换焦点   ↑↓ 历史   Ctrl+K 命令面板   Ctrl+Q 退出"
    } else if app.current.is_some() {
        "  Space 翻页/章末换章   ←→ 换章   t 目录   / 命令   Esc 输入   Ctrl+Q 退出"
    } else {
        "  ↑↓ 选择书籍   Enter 打开   d 离线示例   / 命令   Ctrl+Q 退出"
    };
    frame.render_widget(Paragraph::new(footer).style(palette.muted()), sections[5]);
    if app.overlay.is_some() {
        overlay(frame, app, area, palette);
    }
}
