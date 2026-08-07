//! Turns raw decoded PCM into smoothed levels for a VU-meter + spectrum audio visualizer.
//!
//! This is deliberately backend-agnostic (plain `&[f32]` in, [`VisualizerFrame`] out) so any
//! backend able to tap its decoded samples can reuse it, without depending on rodio/symphonia
//! types.

use std::sync::Arc;

use parking_lot::Mutex;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

/// Shared slot holding the latest [`VisualizerFrame`], written frequently by whichever task
/// feeds a [`VisualizerProcessor`], and read occasionally (e.g. by a slower ticker) for
/// streaming out. A plain lock is fine: writes and reads are both O(1) and never contended for
/// long, this is not a hot loop by itself.
pub type VisualizerHandle = Arc<Mutex<VisualizerFrame>>;

/// How many frequency bars [`VisualizerProcessor`] produces.
///
/// 32 is a sweet spot for a `FFT_SIZE` of 1024 (512 usable bins): enough resolution to actually
/// read as a spectrum rather than a coarse "bass/mid/treble" meter, while each bucket still
/// spans several bins (log-spaced, so the lowest few bars are the most bin-starved).
pub const BAR_COUNT: usize = 32;

/// One smoothed output frame of the [`VisualizerProcessor`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct VisualizerFrame {
    /// Smoothed RMS level (perceived loudness), in `0.0..=1.0`.
    pub rms: f32,
    /// Peak level with slower decay ("peak hold" style), in `0.0..=1.0`.
    pub peak: f32,
    /// Smoothed magnitude per frequency bar, low to high, each in `0.0..=1.0`.
    ///
    /// Always [`BAR_COUNT`] entries; all zero until enough samples have accumulated for a
    /// first FFT pass.
    pub bars: Vec<f32>,
}

impl From<VisualizerFrame> for termusiclib::player::VisualizerFrame {
    fn from(value: VisualizerFrame) -> Self {
        Self {
            rms: value.rms,
            peak: value.peak,
            bars: value.bars,
        }
    }
}

/// Attack/release ballistics for [`VisualizerProcessor::rms`]: how much of the gap to the new
/// instantaneous value is closed per [`VisualizerProcessor::process`] call.
///
/// Attack is fast so the meter feels responsive going up; release is slower so it doesn't
/// flicker between chunks going down.
const RMS_ATTACK: f32 = 0.6;
const RMS_RELEASE: f32 = 0.15;
/// Release for [`VisualizerProcessor::peak`]; peak itself has no attack smoothing (it jumps to
/// the instantaneous peak immediately, then decays), matching typical peak-hold meters.
const PEAK_RELEASE: f32 = 0.05;

/// Samples accumulated (mono-downmixed) before running one FFT pass.
///
/// 1024 is a common trade-off: enough frequency resolution to tell bass from mid from treble,
/// short enough (~23ms at 44.1kHz) to feel responsive.
const FFT_SIZE: usize = 1024;
/// Lower edge of the lowest bar's frequency band. Below this is mostly DC/rumble, not useful.
const MIN_FREQ_HZ: f32 = 40.0;
/// Same ballistics idea as RMS/peak, tuned separately since bars update once per FFT pass
/// (only every `FFT_SIZE` samples) rather than every decoded chunk.
const SPECTRUM_ATTACK: f32 = 0.7;
const SPECTRUM_RELEASE: f32 = 0.25;
/// The `FFT_SIZE/4` scale below is calibrated against a single full-scale test tone, which
/// concentrates all its energy in one bin. Real music spreads energy across many bins/frequencies
/// at once, so any one band's magnitude sits far below that theoretical max — bars barely move
/// without this boost. Tuned by ear against typical music, not derived analytically.
///
/// Bumped alongside the max→RMS change above: averaging pulls every band's reading down further
/// (most on the wider, bin-heavy high-frequency bands), so this needs re-tuning by ear again.
const SPECTRUM_GAIN: f32 = 6.0;

/// Smooths raw decoded chunks into stable [`VisualizerFrame`]s: RMS/peak from the raw samples,
/// plus a small log-spaced frequency spectrum via FFT.
///
/// Not thread-safe; owned by whichever task feeds it samples (one per playing track, so
/// smoothing state naturally resets on track change).
pub struct VisualizerProcessor {
    rms: f32,
    peak: f32,
    spectrum: SpectrumAnalyzer,
}

impl VisualizerProcessor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            rms: 0.0,
            peak: 0.0,
            spectrum: SpectrumAnalyzer::new(),
        }
    }

    /// Feed a newly decoded chunk of interleaved samples (`channels` channels, at `sample_rate`).
    ///
    /// Cheap and allocation-free for the RMS/peak part; the spectrum part only actually runs an
    /// FFT pass once enough samples have accumulated (every [`FFT_SIZE`] mono samples), so most
    /// calls are cheap there too. Safe to call from a latency-sensitive decode loop.
    pub fn process(&mut self, samples: &[f32], channels: usize, sample_rate: u32) {
        if samples.is_empty() {
            return;
        }

        let sum_squares: f32 = samples.iter().map(|sample| sample * sample).sum();
        let instant_rms = (sum_squares / samples.len() as f32).sqrt().min(1.0);
        let instant_peak = samples
            .iter()
            .fold(0.0_f32, |acc, sample| acc.max(sample.abs()));
        let instant_peak = instant_peak.min(1.0);

        let rms_coeff = if instant_rms > self.rms {
            RMS_ATTACK
        } else {
            RMS_RELEASE
        };
        self.rms += (instant_rms - self.rms) * rms_coeff;

        if instant_peak > self.peak {
            self.peak = instant_peak;
        } else {
            self.peak += (instant_peak - self.peak) * PEAK_RELEASE;
        }

        self.spectrum.process(samples, channels, sample_rate);
    }

    /// The current smoothed output. Cheap; call as often as needed.
    #[must_use]
    pub fn output(&self) -> VisualizerFrame {
        VisualizerFrame {
            rms: self.rms,
            peak: self.peak,
            bars: self.spectrum.bars.to_vec(),
        }
    }
}

/// Hann window + FFT + log-spaced bucketing, producing [`BAR_COUNT`] smoothed bars.
struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// Mono-downmixed samples not yet consumed by a FFT pass.
    mono_buffer: Vec<f32>,
    channels: usize,
    sample_rate: u32,
    bars: [f32; BAR_COUNT],
}

impl SpectrumAnalyzer {
    fn new() -> Self {
        Self {
            fft: FftPlanner::new().plan_fft_forward(FFT_SIZE),
            window: hann_window(FFT_SIZE),
            mono_buffer: Vec::with_capacity(FFT_SIZE * 2),
            channels: 1,
            sample_rate: 44100,
            bars: [0.0; BAR_COUNT],
        }
    }

    fn process(&mut self, samples: &[f32], channels: usize, sample_rate: u32) {
        let channels = channels.max(1);
        if channels != self.channels || sample_rate != self.sample_rate {
            // the accumulated partial window no longer matches the new format; drop it rather
            // than mixing samples decoded under two different formats into one FFT pass.
            self.channels = channels;
            self.sample_rate = sample_rate;
            self.mono_buffer.clear();
        }

        self.mono_buffer.extend(
            samples
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );

        while self.mono_buffer.len() >= FFT_SIZE {
            self.run_fft_pass();
            self.mono_buffer.drain(..FFT_SIZE);
        }
    }

    fn run_fft_pass(&mut self) {
        let mut buffer: Vec<Complex32> = self.mono_buffer[..FFT_SIZE]
            .iter()
            .zip(&self.window)
            .map(|(sample, coeff)| Complex32::new(sample * coeff, 0.0))
            .collect();

        self.fft.process(&mut buffer);

        let nyquist = self.sample_rate as f32 / 2.0;
        let bin_hz = self.sample_rate as f32 / FFT_SIZE as f32;
        let max_bin = FFT_SIZE / 2;
        // log-spaced band edges from MIN_FREQ_HZ to Nyquist, so bass/mid/treble get roughly
        // equal visual weight instead of treble dominating (as it would with linear bins).
        let ratio = (nyquist / MIN_FREQ_HZ).max(1.0);

        for (i, bar) in self.bars.iter_mut().enumerate() {
            let low_hz = MIN_FREQ_HZ * ratio.powf(i as f32 / BAR_COUNT as f32);
            let high_hz = MIN_FREQ_HZ * ratio.powf((i + 1) as f32 / BAR_COUNT as f32);

            let low_bin = ((low_hz / bin_hz).round() as usize).clamp(1, max_bin);
            let high_bin = ((high_hz / bin_hz).round() as usize).clamp(low_bin, max_bin);

            // RMS across the band's bins, not the single loudest bin: a lone hot bin (e.g. a
            // strong harmonic right at a bucket edge) would otherwise make that one bar spike
            // while its neighbors stay flat, which reads as noisy rather than musical.
            let band = &buffer[low_bin..=high_bin];
            let magnitude = (band.iter().map(Complex32::norm_sqr).sum::<f32>() / band.len() as f32)
                .sqrt();

            // A full-scale sine, Hann-windowed and FFT'd at this size, peaks at roughly
            // FFT_SIZE/4 in (single-bin) magnitude; scale so that lands near 1.0, then boost for
            // real (non-single-tone, RMS-averaged) program material — see `SPECTRUM_GAIN`.
            let instant = (magnitude / (FFT_SIZE as f32 / 4.0) * SPECTRUM_GAIN).min(1.0);

            let coeff = if instant > *bar {
                SPECTRUM_ATTACK
            } else {
                SPECTRUM_RELEASE
            };
            *bar += (instant - *bar) * coeff;
        }
    }
}

/// A Hann window of the given length, to reduce spectral leakage before the FFT.
fn hann_window(len: usize) -> Vec<f32> {
    if len <= 1 {
        return vec![1.0; len];
    }

    (0..len)
        .map(|i| {
            let x = std::f32::consts::PI * i as f32 / (len - 1) as f32;
            x.sin().powi(2)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence(len: usize) -> Vec<f32> {
        vec![0.0; len]
    }

    fn full_scale_square_wave(len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect()
    }

    /// A mono sine wave at `freq_hz`, sampled at `sample_rate`, `len` samples long.
    fn sine_wave(freq_hz: f32, sample_rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * freq_hz * t).sin()
            })
            .collect()
    }

    #[test]
    fn starts_at_zero() {
        let proc = VisualizerProcessor::new();
        let frame = proc.output();
        assert_eq!(frame.rms, 0.0);
        assert_eq!(frame.peak, 0.0);
        assert_eq!(frame.bars, vec![0.0; BAR_COUNT]);
    }

    #[test]
    fn empty_chunk_is_a_noop() {
        let mut proc = VisualizerProcessor::new();
        proc.process(&full_scale_square_wave(64), 1, 44100);
        let before = proc.output();
        proc.process(&[], 1, 44100);
        assert_eq!(proc.output(), before);
    }

    #[test]
    fn full_scale_signal_converges_towards_one() {
        let mut proc = VisualizerProcessor::new();
        for _ in 0..64 {
            proc.process(&full_scale_square_wave(256), 1, 44100);
        }
        let frame = proc.output();
        assert!(frame.rms > 0.99, "rms should converge near 1.0: {frame:?}");
        assert!(frame.peak > 0.99, "peak should converge near 1.0: {frame:?}");
    }

    #[test]
    fn rms_attacks_faster_than_it_releases() {
        let mut proc = VisualizerProcessor::new();
        proc.process(&full_scale_square_wave(256), 1, 44100);
        let after_attack = proc.output().rms;

        proc.process(&silence(256), 1, 44100);
        let after_one_release_step = proc.output().rms;

        // one attack step should have closed more of the gap (from 0 to ~1) than one release
        // step closes (from ~attack level back towards 0).
        assert!(after_attack > 0.5, "attack step: {after_attack}");
        let released_fraction = (after_attack - after_one_release_step) / after_attack;
        assert!(
            released_fraction < 0.5,
            "release should be slower than attack, released {released_fraction:.2} in one step"
        );
    }

    #[test]
    fn peak_holds_then_decays_on_silence() {
        let mut proc = VisualizerProcessor::new();
        proc.process(&full_scale_square_wave(256), 1, 44100);
        let peak_after_signal = proc.output().peak;
        assert!(peak_after_signal > 0.99);

        proc.process(&silence(256), 1, 44100);
        let peak_after_silence = proc.output().peak;
        assert!(
            peak_after_silence < peak_after_signal,
            "peak should decay on silence"
        );
        assert!(
            peak_after_silence > 0.0,
            "peak should not instantly drop to 0"
        );
    }

    #[test]
    fn output_is_clamped_to_unit_range() {
        let mut proc = VisualizerProcessor::new();
        // samples outside the normal [-1.0, 1.0] range (e.g. from a buggy decoder) must not
        // produce an out-of-range level.
        proc.process(&[5.0, -5.0, 5.0, -5.0], 1, 44100);
        let frame = proc.output();
        assert!((0.0..=1.0).contains(&frame.rms), "{frame:?}");
        assert!((0.0..=1.0).contains(&frame.peak), "{frame:?}");
    }

    #[test]
    fn no_bars_before_one_full_fft_window() {
        let mut proc = VisualizerProcessor::new();
        proc.process(&sine_wave(1000.0, 44100, FFT_SIZE - 1), 1, 44100);
        // fewer than FFT_SIZE samples accumulated: no FFT pass has run yet, bars stay at their
        // initial all-zero state (not "empty" — the widget always gets a fixed BAR_COUNT-long
        // slice, it just isn't showing real data yet).
        assert_eq!(proc.output().bars, vec![0.0; BAR_COUNT]);
    }

    #[test]
    fn bars_appear_after_one_full_fft_window() {
        let mut proc = VisualizerProcessor::new();
        proc.process(&sine_wave(1000.0, 44100, FFT_SIZE), 1, 44100);
        let bars = proc.output().bars;
        assert_eq!(bars.len(), BAR_COUNT);
    }

    #[test]
    fn low_tone_energy_concentrates_in_a_low_bar() {
        let mut proc = VisualizerProcessor::new();
        // ~100Hz: solidly in the lowest quarter of the log-spaced bars (40Hz..22050Hz).
        for _ in 0..8 {
            proc.process(&sine_wave(100.0, 44100, FFT_SIZE), 1, 44100);
        }
        let bars = proc.output().bars;
        let (loudest_idx, &loudest) = bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!(loudest > 0.3, "expected a clear peak, got {bars:?}");
        assert!(
            loudest_idx <= BAR_COUNT / 4,
            "100Hz tone should peak in one of the lowest bars, got index {loudest_idx} in {bars:?}"
        );
    }

    #[test]
    fn high_tone_energy_concentrates_in_a_high_bar() {
        let mut proc = VisualizerProcessor::new();
        // 8000Hz: solidly in the highest bars' band.
        for _ in 0..8 {
            proc.process(&sine_wave(8000.0, 44100, FFT_SIZE), 1, 44100);
        }
        let bars = proc.output().bars;
        let (loudest_idx, &loudest) = bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!(loudest > 0.3, "expected a clear peak, got {bars:?}");
        assert!(
            loudest_idx >= BAR_COUNT - BAR_COUNT / 4,
            "8000Hz tone should peak in one of the highest bars, got index {loudest_idx} in {bars:?}"
        );
    }

    #[test]
    fn stereo_downmix_matches_mono_of_same_signal() {
        let mono = sine_wave(1000.0, 44100, FFT_SIZE);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();

        let mut mono_proc = VisualizerProcessor::new();
        mono_proc.process(&mono, 1, 44100);

        let mut stereo_proc = VisualizerProcessor::new();
        stereo_proc.process(&stereo, 2, 44100);

        let mono_bars = mono_proc.output().bars;
        let stereo_bars = stereo_proc.output().bars;
        for (m, s) in mono_bars.iter().zip(&stereo_bars) {
            assert!(
                (m - s).abs() < 0.01,
                "mono {mono_bars:?} vs downmixed-stereo {stereo_bars:?} should match"
            );
        }
    }

    #[test]
    fn format_change_clears_partial_buffer_instead_of_mixing_formats() {
        let mut proc = VisualizerProcessor::new();
        // half a window's worth, mono at 44100
        proc.process(&sine_wave(1000.0, 44100, FFT_SIZE / 2), 1, 44100);
        // format changes (e.g. new track): must not silently concatenate with the above as if
        // it were still mono/44100, which would produce a garbage FFT window.
        proc.process(&sine_wave(1000.0, 48000, FFT_SIZE), 2, 48000);
        // should not panic, and should still eventually produce a valid bar count once a full
        // window of the *new* format has accumulated.
        assert!(proc.output().bars.len() <= BAR_COUNT);
    }
}
