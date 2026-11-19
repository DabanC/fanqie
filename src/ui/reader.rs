use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::app::App;

use super::theme::Palette;

pub(super) fn reader(frame: &mut Frame, app: &mut App, area: Rect, p: Palette) {
    let width = area.width.min(app.settings.reader_width.max(20));
    let column = Rect {
        x: area.x + (area.width - width) / 2,
        width,
        ..area
    };
    let regions = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(column);
    let book = app.current.as_ref().unwrap();
    let chapter_title = book
        .chapters
        .get(app.chapter)
        .map(|chapter| chapter.title.as_str())
        .unwrap_or("章节");
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(&book.title, p.accent()),
            Span::styled(
                format!(
                    "  /  {}",
                    if book.author.is_empty() {
                        "佚名"
                    } else {
                        &book.author
                    }
                ),
                p.muted(),
            ),
        ])),
        regions[0],
    );
    frame.render_widget(
        Paragraph::new(chapter_title).style(p.text().add_modifier(Modifier::BOLD)),
        regions[1],
    );
    app.prepare_lines(column.width as usize, regions[2].height as usize);
    let visible: Vec<_> = app
        .lines
        .iter()
        .skip(app.scroll)
        .take(app.viewport)
        .map(|line| Line::raw(line.as_str()))
        .collect();
    frame.render_widget(Paragraph::new(visible).style(p.text()), regions[2]);
    let book = app.current.as_ref().unwrap();
    let fraction = if app.max_scroll() == 0 {
        1.0
    } else {
        app.fraction()
    };
    let chapter_percent = (fraction * 100.0).round() as usize;
    let book_percent = ((app.chapter as f64 + fraction) / book.chapters.len().max(1) as f64 * 100.0)
        .round() as usize;
    let progress = format!(
        "第 {} / {} 章  ·  本章 {}%  ·  全书 {}%",
        app.chapter + 1,
        book.chapters.len(),
        chapter_percent,
        book_percent
    );
    let meter_width = (column.width as usize)
        .saturating_sub(UnicodeWidthStr::width(progress.as_str()) + 3)
        .min(24);
    let filled = (fraction * meter_width as f64).round() as usize;
    let line = Line::from(vec![
        Span::styled("━".repeat(filled), p.accent()),
        Span::styled(
            "━".repeat(meter_width.saturating_sub(filled)),
            Style::default().fg(p.line),
        ),
        Span::styled(format!("  {progress}"), p.muted()),
    ]);
    frame.render_widget(Paragraph::new(vec![Line::from(""), line]), regions[3]);
}
