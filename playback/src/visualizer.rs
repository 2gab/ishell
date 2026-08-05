//! Turns raw decoded PCM into smoothed levels for a VU-meter-style audio visualizer.
//!
//! This is deliberately backend-agnostic (plain `&[f32]` in, [`VisualizerFrame`] out) so any
//! backend able to tap its decoded samples can reuse it, without depending on rodio/symphonia
//! types.

/// One smoothed output frame of the [`VisualizerProcessor`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisualizerFrame {
    /// Smoothed RMS level (perceived loudness), in `0.0..=1.0`.
    pub rms: f32,
    /// Peak level with slower decay ("peak hold" style), in `0.0..=1.0`.
    pub peak: f32,
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

/// Smooths raw decoded chunks into stable [`VisualizerFrame`]s.
///
/// Not thread-safe; owned by whichever task feeds it samples (one per playing track, so
/// smoothing state naturally resets on track change).
#[derive(Debug, Default, Clone, Copy)]
pub struct VisualizerProcessor {
    rms: f32,
    peak: f32,
}

impl VisualizerProcessor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a newly decoded chunk of (possibly interleaved multi-channel) samples.
    ///
    /// Cheap and allocation-free; safe to call from a latency-sensitive decode loop.
    pub fn process(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }

        let sum_squares: f32 = samples.iter().map(|sample| sample * sample).sum();
        let instant_rms = (sum_squares / samples.len() as f32).sqrt().min(1.0);
        let instant_peak = samples
            .iter()
            .fold(0.0_f32, |acc, sample| acc.max(sample.abs()))
            .min(1.0);

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
    }

    /// The current smoothed output. Cheap; call as often as needed.
    #[must_use]
    pub fn output(&self) -> VisualizerFrame {
        VisualizerFrame {
            rms: self.rms,
            peak: self.peak,
        }
    }
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

    #[test]
    fn starts_at_zero() {
        let proc = VisualizerProcessor::new();
        assert_eq!(
            proc.output(),
            VisualizerFrame {
                rms: 0.0,
                peak: 0.0
            }
        );
    }

    #[test]
    fn empty_chunk_is_a_noop() {
        let mut proc = VisualizerProcessor::new();
        proc.process(&full_scale_square_wave(64));
        let before = proc.output();
        proc.process(&[]);
        assert_eq!(proc.output(), before);
    }

    #[test]
    fn full_scale_signal_converges_towards_one() {
        let mut proc = VisualizerProcessor::new();
        for _ in 0..64 {
            proc.process(&full_scale_square_wave(256));
        }
        let frame = proc.output();
        assert!(frame.rms > 0.99, "rms should converge near 1.0: {frame:?}");
        assert!(frame.peak > 0.99, "peak should converge near 1.0: {frame:?}");
    }

    #[test]
    fn rms_attacks_faster_than_it_releases() {
        let mut proc = VisualizerProcessor::new();
        proc.process(&full_scale_square_wave(256));
        let after_attack = proc.output().rms;

        proc.process(&silence(256));
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
        proc.process(&full_scale_square_wave(256));
        let peak_after_signal = proc.output().peak;
        assert!(peak_after_signal > 0.99);

        proc.process(&silence(256));
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
        proc.process(&[5.0, -5.0, 5.0, -5.0]);
        let frame = proc.output();
        assert!((0.0..=1.0).contains(&frame.rms), "{frame:?}");
        assert!((0.0..=1.0).contains(&frame.peak), "{frame:?}");
    }
}
