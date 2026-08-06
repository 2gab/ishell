use termusiclib::config::SharedTuiSettings;
use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::Event;
use tuirealm::props::{AttrValue, Attribute, Color, QueryResult, Style};
use tuirealm::ratatui::Frame;
use tuirealm::ratatui::layout::Rect;
use tuirealm::ratatui::widgets::{Block, BorderType, Borders, Sparkline};
use tuirealm::state::State;

use crate::ui::ids::Id;
use crate::ui::model::{Model, UserEvent, VISUALIZER_HISTORY_LEN};
use crate::ui::msg::Msg;

/// Live RMS-level history, rendered as a scrolling bar graph.
///
/// A read-only display, like [`NowPlaying`](super::NowPlaying) — no keyboard handling, no
/// `Attribute`-based updates (rebuilt via [`Model::visualizer_update`] on every new frame
/// instead, since that is the pattern proven to actually redraw reliably in this codebase).
pub struct Visualizer {
    data: Vec<u64>,
    foreground: Color,
    background: Color,
    border_color: Color,
}

impl Visualizer {
    pub fn new(config: &SharedTuiSettings, data: Vec<u64>) -> Self {
        let config = config.read_recursive();
        Self {
            data,
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

        let sparkline = Sparkline::default()
            .block(block)
            .style(Style::new().fg(self.foreground).bg(self.background))
            .max(100)
            .data(&self.data);

        frame.render_widget(sparkline, area);
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
    /// Push a new RMS level (`0.0..=1.0`) onto [`Model::visualizer_history`], then redraw the
    /// `Visualizer` panel if it is currently mounted.
    pub fn visualizer_push_level(&mut self, rms: f32) {
        let level = (rms.clamp(0.0, 1.0) * 100.0).round() as u64;

        if self.visualizer_history.len() >= VISUALIZER_HISTORY_LEN {
            self.visualizer_history.pop_front();
        }
        self.visualizer_history.push_back(level);

        self.visualizer_update();
    }

    /// (Re)mount the [`Visualizer`] widget with the current history, if mounted.
    fn visualizer_update(&mut self) {
        if !self.app.mounted(&Id::Visualizer) {
            return;
        }

        let data: Vec<u64> = self.visualizer_history.iter().copied().collect();
        self.app
            .remount(
                Id::Visualizer,
                Box::new(Visualizer::new(&self.config_tui, data)),
                Vec::new(),
            )
            .ok();
    }
}
