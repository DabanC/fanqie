use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::{source_label, App, COMMANDS};

use super::theme::Palette;

pub(super) fn header(frame: &mut Frame, app: &App, area: Rect, p: Palette) {
    let columns = Layout::horizontal([
        Constraint::Min(25),
        Constraint::Length(if area.width >= 70 { 29 } else { 0 }),
    ])
    .split(area);
    let brand = vec![
        Line::from(vec![
            Span::styled("  *  ", p.accent()),
            Span::styled("FANQIE", p.text().add_modifier(Modifier::BOLD)),
            Span::styled("  v0.1.0", p.muted()),
        ]),
        Line::from(vec![
            Span::styled(" *#* ", p.accent()),
            Span::styled("把故事，留在终端里。", p.text()),
        ]),
        Line::from(vec![
            Span::styled("  *  ", p.accent()),
            Span::styled("Terminal Novel Reader · Rust", p.muted()),
        ]),
    ];
    frame.render_widget(Paragraph::new(brand), columns[0]);
    let mode = if let Some(book) = &app.current {
        source_label(&book.source)
    } else {
        "番茄在线"
    };
    let api = if app.settings.content_api.is_empty() {
        "官方直连 / 自动缓存"
    } else {
        "已配置正文服务"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!("● {mode}"),
                Style::default().fg(p.green),
            )),
            Line::from(Span::styled(api, p.muted())),
            Line::from(Span::styled("/help 查看使用说明", p.muted())),
        ])
        .alignment(ratatui::layout::Alignment::Right),
        columns[1],
    );
    let line = Rect {
        y: area.bottom().saturating_sub(1),
        height: 1,
        ..area
    };
    frame.render_widget(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(p.line)),
        line,
    );
}

pub(super) fn status(frame: &mut Frame, app: &App, area: Rect, p: Palette) {
    let (text, style) = if let Some(label) = &app.busy {
        let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][app.tick % 10];
        (
            format!("{spinner} {label}…  Ctrl+X 取消  Ctrl+Q 退出"),
            p.accent(),
        )
    } else {
        (
            format!("{} {}", if app.is_error { "!" } else { "·" }, app.status),
            if app.is_error {
                Style::default().fg(p.red)
            } else {
                p.muted()
            },
        )
    };
    frame.render_widget(
        Paragraph::new(text).style(style).wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn hint(frame: &mut Frame, app: &App, area: Rect, p: Palette) {
    let hint = if app.command_focus && app.input.text.starts_with('/') {
        let prefix = app.input.text.split_whitespace().next().unwrap_or("/");
        let commands: Vec<_> = COMMANDS
            .iter()
            .filter(|(command, _)| command.starts_with(prefix))
            .take(3)
            .map(|(command, detail)| format!("{} {detail}", command.trim()))
            .collect();
        if commands.is_empty() {
            "未知命令 · /help 查看说明".into()
        } else {
            commands.join("   ·   ")
        }
    } else if app.command_focus {
        "输入 /命令、小说路径、番茄链接或书名…".into()
    } else {
        "阅读模式 · Esc 或 / 进入命令输入".into()
    };
    frame.render_widget(Paragraph::new(format!("  {hint}")).style(p.muted()), area);
}
