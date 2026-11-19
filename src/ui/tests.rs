use super::*;
use crate::storage::Store;
use ratatui::{backend::TestBackend, Terminal};
use unicode_width::UnicodeWidthStr;

#[test]
fn home_and_overlay_render_at_multiple_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    for (width, height) in [(120, 40), (80, 24), (40, 16), (20, 8)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }
    app.execute("/help");
    for (width, height) in [(120, 40), (80, 24), (40, 16)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }
}

#[test]
fn chapter_renders_with_cjk_wrapping_and_light_theme() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut app = App::new(store.clone()).unwrap();
    let library = crate::library::Library::new(store, app.settings.clone()).unwrap();
    let book = library.demo().unwrap();
    app.content = library.chapter(&book, 0).unwrap();
    app.current = Some(book);
    app.command_focus = false;
    for light in [false, true] {
        app.settings.light_theme = light;
        for (width, height) in [(120, 40), (80, 24), (40, 16), (30, 12)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
            assert!(rendered.contains("FANQIE"));
            assert!(app
                .lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= app.wrap_width));
        }
    }
}
