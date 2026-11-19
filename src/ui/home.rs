use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::app::{source_label, App};

use super::theme::Palette;

pub(super) fn home(frame: &mut Frame, app: &App, area: Rect, p: Palette) {
    let compact = area.width < 70 || area.height < 17;
    let card_height = if compact { 7 } else { 13 };
    let rows = Layout::vertical([
        Constraint::Length(card_height),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .split(area);
    let card = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(p.accent())
        .title(Span::styled(" 欢迎回来 ", p.accent()));
    let inner = card.inner(rows[0]);
    frame.render_widget(card, rows[0]);
    if compact {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  /search 十日终焉", p.accent()),
                    Span::styled("  在线搜书", p.text()),
                ]),
                Line::from(vec![
                    Span::styled("  /import <TXT>", p.accent()),
                    Span::styled("  导入你的小说", p.text()),
                ]),
                Line::from(vec![Span::styled("  /open <书籍ID或链接>", p.accent())]),
                Line::from(Span::styled(
                    "  /resume 继续阅读 · /shelf 我的书架",
                    p.muted(),
                )),
            ]),
            inner,
        );
    } else {
        let cols = Layout::horizontal([Constraint::Percentage(41), Constraint::Percentage(59)])
            .split(inner);
        let welcome = vec![
            Line::from(""),
            Line::from(Span::styled(
                "一个安静的阅读角落",
                p.text().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled("       _  _       ", p.accent())),
            Line::from(Span::styled("      ( .. )      ", p.accent())),
            Line::from(Span::styled("     /|____|\\     ", p.accent())),
            Line::from(""),
            Line::from(Span::styled("没有浏览器。只有下一章。", p.muted())),
            Line::from(Span::styled("本地书架 · 自动续读 · 按需缓存", p.muted())),
        ];
        frame.render_widget(
            Paragraph::new(welcome).alignment(ratatui::layout::Alignment::Center),
            cols[0],
        );
        let tips = vec![
            Line::from(""),
            Line::from(Span::styled(
                "从这里开始",
                p.accent().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("/search 十日终焉   ", p.accent()),
                Span::styled("直接在线搜索", p.text()),
            ]),
            Line::from(vec![
                Span::styled("/open 书籍链接    ", p.accent()),
                Span::styled("直接读取正文", p.text()),
            ]),
            Line::from(vec![
                Span::styled("/import 路径      ", p.accent()),
                Span::styled("也支持本地 TXT", p.text()),
            ]),
            Line::from(vec![
                Span::styled("/resume           ", p.accent()),
                Span::styled("回到上次读到的地方", p.text()),
            ]),
            Line::from(""),
            Line::from(Span::styled("内置番茄在线书源，无需自备 API。", p.muted())),
            Line::from(Span::styled(
                "按 / 查看命令，Ctrl+K 打开命令面板。",
                p.muted(),
            )),
        ];
        frame.render_widget(Paragraph::new(tips), cols[1]);
    }
    if rows[2].height < 2 {
        return;
    }
    let block = Block::default().title(Span::styled(
        format!(" 最近阅读  /  {} 本 ", app.shelf.len()),
        p.muted(),
    ));
    let inner = block.inner(rows[2]);
    frame.render_widget(block, rows[2]);
    if app.shelf.is_empty() {
        frame.render_widget(
            Paragraph::new("\n  书架还是空的。输入 /demo，或者导入你的第一本小说。")
                .style(p.muted())
                .wrap(Wrap { trim: false }),
            inner,
        );
    } else {
        let books: Vec<_> = app
            .shelf
            .iter()
            .map(|book| {
                let author = if book.author.is_empty() {
                    "佚名"
                } else {
                    &book.author
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{}  ", book.title), p.text()),
                    Span::styled(
                        format!(
                            "{author} · {} 章 · {}",
                            book.chapters.len(),
                            source_label(&book.source)
                        ),
                        p.muted(),
                    ),
                ]))
            })
            .collect();
        let list = List::new(books)
            .highlight_symbol("› ")
            .highlight_style(Style::default().bg(p.selected).fg(p.accent));
        let mut state = ListState::default().with_selected(Some(app.home_selection));
        frame.render_stateful_widget(list, inner, &mut state);
    }
}
