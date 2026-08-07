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

        // Keep each bar exactly as thin as before — spread them across the widget's full width
        // by growing the *gap* between bars instead of the bars themselves, so the spectrum
        // reaches edge to edge without turning into a handful of oversized blocks.
        const BAR_WIDTH: u16 = 3;
        const MAX_BAR_GAP: u16 = 8;
        let bar_count = bars.len().max(1) as u16;
        let inner_width = area.width.saturating_sub(2); // account for the left/right border
        let total_bar_width = BAR_WIDTH.saturating_mul(bar_count);
        let bar_gap = if bar_count > 1 {
            inner_width
                .saturating_sub(total_bar_width)
                .checked_div(bar_count - 1)
                .unwrap_or(1)
                .clamp(1, MAX_BAR_GAP)
        } else {
            1
        };

        let chart = BarChart::default()
            .block(block)
            .data(BarGroup::default().bars(&bars))
            .max(100)
            .bar_width(BAR_WIDTH)
            .bar_gap(bar_gap);

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

#[cfg(test)]
mod tests {
    use tuirealm::component::Component;
    use tuirealm::props::Color;
    use tuirealm::ratatui::Terminal;
    use tuirealm::ratatui::backend::TestBackend;
    use tuirealm::ratatui::layout::Rect;

    use super::Visualizer;

    /// Sweep many value/width/height combinations, flagging any rendered cell whose symbol is
    /// not a space, a border char, or one of the barchart block-drawing glyphs. Bars must never
    /// show numbers/letters — regression test for a real report of stray text appearing mid-bar.
    #[test]
    fn no_stray_text_across_many_configs() {
        let allowed: &str = " ╭╮╰╯─│▁▂▃▄▅▆▇█";

        for width in [40u16, 80, 120, 160, 220, 300] {
            for height in [3u16, 4, 5, 6] {
                for seed in 0..50u64 {
                    let bars_values: Vec<u64> = (0..32)
                        .map(|i| (seed.wrapping_mul(37).wrapping_add(i * 13)) % 101)
                        .collect();

                    let mut widget = Visualizer {
                        bars: bars_values.clone(),
                        foreground: Color::Yellow,
                        background: Color::Black,
                        border_color: Color::Blue,
                    };

                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal
                        .draw(|f| {
                            widget.view(f, Rect::new(0, 0, width, height));
                        })
                        .unwrap();

                    let buf = terminal.backend().buffer();
                    // rows 0 and height-1 are the block's border/title row — only check the
                    // inner rows where bars actually render.
                    for y in 1..height.saturating_sub(1) {
                        for x in 0..width {
                            let symbol = buf[(x, y)].symbol();
                            if !allowed.contains(symbol) {
                                panic!(
                                    "stray glyph {symbol:?} at ({x},{y}) width={width} height={height} bars={bars_values:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
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
