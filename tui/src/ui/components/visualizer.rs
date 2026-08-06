use termusiclib::config::SharedTuiSettings;
use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::Event;
use tuirealm::props::{AttrValue, Attribute, Color, QueryResult, Style};
use tuirealm::ratatui::Frame;
use tuirealm::ratatui::layout::Rect;
use tuirealm::ratatui::widgets::{Bar, BarChart, BarGroup, Block, BorderType, Borders};
use tuirealm::state::State;

use crate::ui::ids::Id;
use crate::ui::model::{Model, UserEvent};
use crate::ui::msg::Msg;

/// Live frequency spectrum (low to high bar), from the server's FFT.
///
/// A read-only display, like [`NowPlaying`](super::NowPlaying) — no keyboard handling, no
/// `Attribute`-based updates (rebuilt via [`Model::visualizer_update`] on every new frame
/// instead, since that is the pattern proven to actually redraw reliably in this codebase).
pub struct Visualizer {
    /// Bar levels scaled `0..=100`.
    bars: Vec<u64>,
    foreground: Color,
    background: Color,
    border_color: Color,
}

impl Visualizer {
    pub fn new(config: &SharedTuiSettings, bars: Vec<u64>) -> Self {
        let config = config.read_recursive();
        Self {
            bars,
            foreground: config.settings.theme.library_highlight(),
            background: config.settings.theme.library_background(),
            border_color: config.settings.theme.library_border(),
        }
    }
}

impl Component for Visualizer {
    fn view(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(self.border_color))
            .title(" Visualizer ");

        let bar_style = Style::new().fg(self.foreground).bg(self.background);
        let bars: Vec<Bar<'_>> = self
            .bars
            .iter()
            .map(|&value| Bar::default().value(value).style(bar_style).text_value(String::new()))
            .collect();

        let chart = BarChart::default()
            .block(block)
            .data(BarGroup::default().bars(&bars))
            .max(100)
            .bar_width(3)
            .bar_gap(1);

        frame.render_widget(chart, area);
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }

    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}

    fn state(&self) -> State {
        State::None
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}

impl AppComponent<Msg, UserEvent> for Visualizer {
    fn on(&mut self, _ev: &Event<UserEvent>) -> Option<Msg> {
        None
    }
}

impl Model {
    /// (Re)mount the [`Visualizer`] widget with [`Model::visualizer_frame`]'s current bars, if
    /// the panel is currently mounted (skips the work entirely when it is hidden).
    pub fn visualizer_update(&mut self) {
        if !self.app.mounted(&Id::Visualizer) {
            return;
        }

        let bars = self
            .visualizer_frame
            .as_ref()
            .map(|frame| {
                frame
                    .bars
                    .iter()
                    .map(|level| (level.clamp(0.0, 1.0) * 100.0).round() as u64)
                    .collect()
            })
            .unwrap_or_default();

        self.app
            .remount(
                Id::Visualizer,
                Box::new(Visualizer::new(&self.config_tui, bars)),
                Vec::new(),
            )
            .ok();
    }
}
