use termusiclib::config::SharedTuiSettings;
use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::Event;
use tuirealm::props::{AttrValue, Attribute, Color, QueryResult, Style};
use tuirealm::ratatui::Frame;
use tuirealm::ratatui::buffer::Buffer;
use tuirealm::ratatui::layout::Rect;
use tuirealm::ratatui::widgets::{Block, BorderType, Borders};
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
    /// Per-bar peak-hold cap, scaled `0..=100`, parallel to `bars`.
    peaks: Vec<u64>,
    foreground: Color,
    background: Color,
    border_color: Color,
}

impl Visualizer {
    pub fn new(config: &SharedTuiSettings, bars: Vec<u64>, peaks: Vec<u64>) -> Self {
        let config = config.read_recursive();
        Self {
            bars,
            peaks,
            foreground: config.settings.theme.library_highlight(),
            background: config.settings.theme.library_background(),
            border_color: config.settings.theme.library_border(),
        }
    }
}

/// Gap between bars, in columns.
const GAP: u16 = 2;

/// Small fixed inset from the panel's left/right border. Deliberately not centering or
/// stretching bars to consume every last column: 32 bands read left-to-right as low→high
/// frequency, so a small, constant margin keeps that spatial mapping stable and reads as a
/// spectrum strip rather than an object being centered in the panel.
const MARGIN: u16 = 2;

/// Bottom-aligned eighth-block glyphs, indexed by how many eighths of the cell are filled
/// (`glyph_for_eighths(0)` is blank, `glyph_for_eighths(8)` is a full block).
fn glyph_for_eighths(eighths: u32) -> &'static str {
    [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"][eighths as usize]
}

/// Draw one band: a bar `value` (`0..=100`) tall plus its peak-hold cap, against `height` rows,
/// `width` columns wide starting at `(x, y)`.
///
/// The bar uses eighth-cell precision on its topmost partial row — the same value→row math
/// ratatui-widgets' `BarChart` uses internally (`value * height * 8 / max`), just applied by
/// hand so the cap (see below) can be placed relative to the bar's own drawn pixels instead of
/// through a separate, uncoordinated pass.
///
/// The cap is a single-column marker centered over the bar, sitting at the band's recent max
/// (see playback's `BAND_PEAK_RELEASE`) so a short transient is still visible a moment after it
/// passes — but always at least one row *above* whatever row the bar itself last painted. Both
/// are computed from the same `height*8` tick scale, so without that floor a bar and a
/// near-equal peak reading would round to the same row and the cap would overwrite the bar's
/// own top pixels with its (differently colored, mostly-empty) glyph, punching a visible notch
/// in an otherwise solid bar. If the bar already reaches the very top row, there's no row left
/// to put the cap above it, so it's simply not drawn that frame rather than notching the bar.
fn draw_band(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    value: u64,
    peak: u64,
    bar_style: Style,
    peak_style: Style,
) {
    if width == 0 || height == 0 {
        return;
    }
    let height32 = u32::from(height);

    let bar_ticks = u32::try_from(value.min(100)).unwrap_or(100) * height32 * 8 / 100;
    let bar_full_rows = (bar_ticks / 8).min(height32);
    let bar_partial = bar_ticks % 8;
    for row in 0..height {
        let row32 = u32::from(row);
        let symbol = if row32 < bar_full_rows {
            "█"
        } else if row32 == bar_full_rows && bar_partial > 0 {
            glyph_for_eighths(bar_partial)
        } else {
            continue;
        };
        let row_y = y + height - 1 - row;
        for xi in x..x + width {
            buf[(xi, row_y)].set_symbol(symbol).set_style(bar_style);
        }
    }

    let bar_top_rows_from_bottom = bar_full_rows + u32::from(bar_partial > 0);
    let peak_ticks = u32::try_from(peak.min(100)).unwrap_or(100) * height32 * 8 / 100;
    let peak_rows_from_bottom = (peak_ticks / 8).max(bar_top_rows_from_bottom + 1);
    if peak_rows_from_bottom > height32 {
        return;
    }
    let cap_y = y + height - peak_rows_from_bottom as u16;
    let cap_x = x + width / 2;
    buf[(cap_x, cap_y)].set_symbol("▔").set_style(peak_style);
}

impl Component for Visualizer {
    fn view(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(self.border_color))
            .title(" Visualizer ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let bar_count = self.bars.len();
        if bar_count == 0 || inner.width == 0 || inner.height == 0 {
            return;
        }

        let bar_style = Style::new().fg(self.foreground).bg(self.background);
        let peak_style = Style::new().fg(Color::White).bg(self.background);
        let buf = frame.buffer_mut();

        // Fixed left-aligned layout: bars sit at `MARGIN + i * cell_width`, all the same width,
        // with a constant `GAP` between them — never centered or stretched to reach the right
        // border exactly. 32 bands read left-to-right as low→high frequency, so a fixed position
        // per band matters more here than using every last column; the spectrum may end a few
        // columns short of the right border on odd widths, and that's fine.
        let usable_width = inner.width.saturating_sub(MARGIN * 2);
        let bar_count_u16 = bar_count as u16;
        let cell_width = (usable_width / bar_count_u16).max(1);
        let bar_width = cell_width.saturating_sub(GAP).max(1);
        let start_x = inner.x + MARGIN;

        for (i, (&value, &peak)) in self.bars.iter().zip(&self.peaks).enumerate() {
            let x = start_x + i as u16 * cell_width;
            if x + bar_width > inner.x + inner.width {
                break;
            }

            draw_band(buf, x, inner.y, bar_width, inner.height, value, peak, bar_style, peak_style);
        }
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
        let allowed: &str = " ╭╮╰╯─│▁▂▃▄▅▆▇█▔";

        for width in [40u16, 80, 120, 160, 220, 300] {
            for height in [3u16, 4, 5, 6] {
                for seed in 0..50u64 {
                    let bars_values: Vec<u64> = (0..32)
                        .map(|i| (seed.wrapping_mul(37).wrapping_add(i * 13)) % 101)
                        .collect();
                    // peaks at-or-above their bar, like real peak-hold: instant jump, slow decay
                    let peaks_values: Vec<u64> = bars_values
                        .iter()
                        .enumerate()
                        .map(|(i, &v)| (v + (seed.wrapping_add(i as u64) * 7) % 30).min(100))
                        .collect();

                    let mut widget = Visualizer {
                        bars: bars_values.clone(),
                        peaks: peaks_values,
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

    /// The peak cap must never land on a row the bar itself painted — regression test for a
    /// real report of the cap punching a visible notch into near-full-height bars (bar and
    /// peak rounding to the same row, cap glyph overwriting the bar's solid top block).
    #[test]
    fn peak_never_overwrites_the_bars_own_rows() {
        const HEIGHT: u16 = 10;
        let bars_values: Vec<u64> = (0..32).map(|i| 40 + (i * 3) % 61).collect();
        // peaks at-or-just-above their bar, the tightest case for the cap colliding with the bar.
        let peaks_values: Vec<u64> =
            bars_values.iter().map(|&v| (v + 1).min(100)).collect();

        let mut widget = Visualizer {
            bars: bars_values,
            peaks: peaks_values,
            foreground: Color::Yellow,
            background: Color::Black,
            border_color: Color::Blue,
        };

        let width = 160;
        let mut terminal = Terminal::new(TestBackend::new(width, HEIGHT + 2)).unwrap();
        terminal
            .draw(|f| {
                widget.view(f, Rect::new(0, 0, width, HEIGHT + 2));
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        for y in 1..HEIGHT + 1 {
            for x in 0..width {
                if buf[(x, y)].symbol() == "▔" {
                    // Every cell directly below a peak marker, down to the panel's bottom
                    // border, must still show bar fill (a block glyph or blank gap column) —
                    // never a gap left by the cap having eaten into the bar's own pixels.
                    for below_y in (y + 1)..HEIGHT + 1 {
                        let below = buf[(x, below_y)].symbol();
                        assert_ne!(
                            below, " ",
                            "peak at ({x},{y}) sits above a blank cell at ({x},{below_y}), \
                             meaning it overwrote what should've been bar fill"
                        );
                    }
                }
            }
        }
    }

    /// All 32 bars are laid out left-to-right, non-overlapping, evenly spaced, and never past
    /// the panel's right border. Deliberately does *not* assert the spectrum reaches the right
    /// edge exactly — the layout intentionally favors a fixed, consistent left-aligned position
    /// per band over consuming every last column (see `Component::view`'s layout comment).
    #[test]
    fn bars_are_left_aligned_evenly_spaced_and_non_overlapping() {
        // 32 bars need at least ~2 columns each (1 bar + a sliver of gap) to render as visually
        // distinct runs at all; narrower than that and bars legitimately touch (see the
        // `no_stray_text_across_many_configs` test above for that degraded-but-still-valid case).
        for width in [80u16, 97, 160, 223, 300] {
            let mut widget = Visualizer {
                bars: vec![100; 32],
                peaks: vec![0; 32],
                foreground: Color::Yellow,
                background: Color::Black,
                border_color: Color::Blue,
            };

            let mut terminal = Terminal::new(TestBackend::new(width, 6)).unwrap();
            terminal
                .draw(|f| {
                    widget.view(f, Rect::new(0, 0, width, 6));
                })
                .unwrap();

            let buf = terminal.backend().buffer();
            // At full-scale values every bar is a solid run of full blocks on this row; find
            // each contiguous run and treat it as one bar.
            let row = 2;
            let mut runs = Vec::new();
            let mut run_start = None;
            for x in 0..width {
                let filled = buf[(x, row)].symbol() == "█";
                match (filled, run_start) {
                    (true, None) => run_start = Some(x),
                    (false, Some(s)) => {
                        runs.push((s, x - s));
                        run_start = None;
                    }
                    _ => {}
                }
            }
            if let Some(s) = run_start {
                runs.push((s, width - s));
            }

            assert_eq!(runs.len(), 32, "expected all 32 bars to render at width={width}, got {runs:?}");

            // Left-aligned: the first bar should sit close to the left border, not centered.
            let (first_x, _) = runs[0];
            assert!(first_x <= 4, "first bar should hug the left edge at width={width}, got x={first_x}");

            // Evenly spaced: every bar has the same width and the same start-to-start step.
            let bar_width = runs[0].1;
            let step = runs[1].0 - runs[0].0;
            for (i, &(x, w)) in runs.iter().enumerate() {
                assert_eq!(w, bar_width, "bar {i} width differs at width={width}");
                if i > 0 {
                    assert_eq!(x - runs[i - 1].0, step, "bar {i} spacing differs at width={width}");
                }
            }

            // Never past the panel's right border.
            let (last_x, last_w) = *runs.last().unwrap();
            assert!(
                last_x + last_w <= width - 1,
                "last bar overruns the right border at width={width}"
            );
        }
    }
}

/// Floor of the dB range mapped onto the widget's height; a `level` this many dB (or more)
/// below full scale renders as zero height. Real per-band FFT magnitude spends almost all its
/// time well under 0dB (only loud bass hits get close), so a *linear* 0..100 mapping left most
/// of the spectrum below the widget's 1-tick visibility threshold — dB is how ears (and every
/// other spectrum analyzer) actually perceive that range, so it reads as "the music breathing"
/// instead of a couple of bars twitching.
const MIN_DB: f32 = -50.0;

/// Map a `0.0..=1.0` linear level to a `0..=100` display scale via dB, so quiet levels get
/// meaningfully more height than a linear mapping would give them, without clipping loud ones
/// (`1.0` still maps to `100`).
fn level_to_display_scale(level: f32) -> u64 {
    let level = level.clamp(0.0, 1.0);
    let db = 20.0 * level.max(1e-5).log10();
    (((db - MIN_DB) / -MIN_DB).clamp(0.0, 1.0) * 100.0).round() as u64
}

impl Model {
    /// (Re)mount the [`Visualizer`] widget with [`Model::visualizer_frame`]'s current bars, if
    /// the panel is currently mounted (skips the work entirely when it is hidden).
    pub fn visualizer_update(&mut self) {
        if !self.app.mounted(&Id::Visualizer) {
            return;
        }

        let (bars, peaks) = self
            .visualizer_frame
            .as_ref()
            .map(|frame| {
                let bars = frame.bars.iter().copied().map(level_to_display_scale).collect();
                let peaks = frame.band_peaks.iter().copied().map(level_to_display_scale).collect();
                (bars, peaks)
            })
            .unwrap_or_default();

        self.app
            .remount(
                Id::Visualizer,
                Box::new(Visualizer::new(&self.config_tui, bars, peaks)),
                Vec::new(),
            )
            .ok();
    }
}
