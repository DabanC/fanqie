use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::app::App;

use super::{input::draw_editor, theme::Palette};

pub(super) fn overlay(frame: &mut Frame, app: &mut App, area: Rect, p: Palette) {
    let width = area.width.saturating_sub(4).min(104);
    let height = area.height.saturating_sub(4).min(30);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let overlay = app.overlay.as_mut().unwrap();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            format!(" {} ", overlay.title),
            p.accent().add_modifier(Modifier::BOLD),
        ))
        .border_style(p.accent())
        .style(Style::default().bg(p.panel).fg(p.text));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(overlay.subtitle.as_str())
            .style(p.muted())
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    let search = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(p.line));
    let search_inner = search.inner(rows[1]);
    frame.render_widget(search, rows[1]);
    frame.render_widget(
        Paragraph::new(" / ").style(p.accent()),
        Rect {
            width: 3.min(search_inner.width),
            ..search_inner
        },
    );
    let editor_area = Rect {
        x: search_inner.x + 3,
        width: search_inner.width.saturating_sub(4),
        height: 1,
        ..search_inner
    };
    draw_editor(frame, &overlay.filter, editor_area, true, p, "筛选…");
    let filtered = overlay.filtered();
    overlay.selected = overlay.selected.min(filtered.len().saturating_sub(1));
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new("\n  没有匹配项目。Esc 返回。").style(p.muted()),
            rows[2],
        );
    } else {
        let items: Vec<_> = filtered
            .iter()
            .map(|&index| {
                let choice = &overlay.choices[index];
                ListItem::new(vec![
                    Line::styled(choice.title.clone(), p.text()),
                    Line::styled(format!("   {}", choice.detail), p.muted()),
                ])
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(overlay.selected));
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("› ")
                .highlight_style(Style::default().bg(p.selected).fg(p.accent)),
            rows[2],
            &mut state,
        );
    }
    frame.render_widget(
        Paragraph::new(format!(
            " {} 项  ·  ↑↓ 选择  ·  Enter 确认  ·  Esc 返回",
            filtered.len()
        ))
        .style(p.muted()),
        rows[3],
    );
}
