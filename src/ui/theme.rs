use ratatui::style::{Color, Style};

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub(super) bg: Color,
    pub(super) panel: Color,
    pub(super) text: Color,
    pub(super) muted: Color,
    pub(super) accent: Color,
    pub(super) line: Color,
    pub(super) selected: Color,
    pub(super) green: Color,
    pub(super) red: Color,
}
impl Palette {
    pub(super) fn new(light: bool) -> Self {
        if light {
            Self {
                bg: Color::Rgb(250, 248, 243),
                panel: Color::Rgb(243, 238, 229),
                text: Color::Rgb(55, 51, 46),
                muted: Color::Rgb(124, 117, 107),
                accent: Color::Rgb(177, 84, 54),
                line: Color::Rgb(211, 202, 188),
                selected: Color::Rgb(232, 219, 199),
                green: Color::Rgb(74, 115, 70),
                red: Color::Rgb(179, 57, 44),
            }
        } else {
            Self {
                bg: Color::Rgb(28, 27, 25),
                panel: Color::Rgb(35, 33, 30),
                text: Color::Rgb(223, 216, 204),
                muted: Color::Rgb(143, 137, 126),
                accent: Color::Rgb(218, 142, 111),
                line: Color::Rgb(72, 67, 60),
                selected: Color::Rgb(60, 48, 40),
                green: Color::Rgb(153, 180, 133),
                red: Color::Rgb(234, 126, 113),
            }
        }
    }
    pub(super) fn text(self) -> Style {
        Style::default().fg(self.text)
    }
    pub(super) fn muted(self) -> Style {
        Style::default().fg(self.muted)
    }
    pub(super) fn accent(self) -> Style {
        Style::default().fg(self.accent)
    }
}
