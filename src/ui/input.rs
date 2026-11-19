use ratatui::{
    layout::Rect,
    style::Style,
    text::Text,
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Editor};

use super::theme::Palette;

pub(super) fn input(frame: &mut Frame, app: &App, area: Rect, p: Palette) {
    let border = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(if app.command_focus {
            p.accent()
        } else {
            Style::default().fg(p.line)
        })
        .style(Style::default().bg(p.panel));
    let inner = border.inner(area);
    frame.render_widget(border, area);
    frame.render_widget(
        Paragraph::new("❯").style(p.accent()),
        Rect {
            x: inner.x + 1,
            width: 2.min(inner.width),
            ..inner
        },
    );
    let text_area = Rect {
        x: inner.x + 4,
        width: inner.width.saturating_sub(5),
        ..inner
    };
    draw_editor(
        frame,
        &app.input,
        text_area,
        app.command_focus && app.overlay.is_none(),
        p,
        if app.current.is_some() {
            "/toc · /shelf · /help"
        } else {
            "/demo 开始阅读，/help 查看帮助"
        },
    );
}

pub(super) fn draw_editor(
    frame: &mut Frame,
    editor: &Editor,
    area: Rect,
    focused: bool,
    p: Palette,
    placeholder: &str,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if editor.text.is_empty() {
        frame.render_widget(Paragraph::new(placeholder).style(p.muted()), area);
        if focused {
            frame.set_cursor_position((area.x, area.y));
        }
        return;
    }
    let available = area.width.saturating_sub(1) as usize;
    let before = &editor.text[..editor.cursor];
    let mut used = 0;
    let mut start = editor.cursor;
    for (index, grapheme) in before.grapheme_indices(true).rev() {
        let width = UnicodeWidthStr::width(grapheme);
        if used + width > available {
            break;
        }
        used += width;
        start = index;
    }
    let text = &editor.text[start..];
    frame.render_widget(Paragraph::new(Text::raw(text)).style(p.text()), area);
    if focused {
        frame.set_cursor_position((area.x + used as u16, area.y));
    }
}
