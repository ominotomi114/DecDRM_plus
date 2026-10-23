//! Masking of audio dropouts by decaying the last good audio into the gap.
//!
//! When the receiver loses synchronisation, or a run of audio frames is lost (or
//! concealed by the codec), the decoded audio simply stops: the output has a hole, heard
//! as an abrupt cut to silence and then an abrupt return. [`DropoutConcealer`] fills the
//! hole with the audio that came before it, at a falling level, so a short gap is heard
//! as a dip rather than a cut.
//!
//! 1. The last [`LOOP_PERIOD_MS`]·2 of *clean* audio is kept as a loop. While a gap lasts,
//!    the loop is played back continuously; its seam is removed by cross-fading its second
//!    half into its first (overlap-add), so the repetition itself does not click.
//! 2. The level falls exponentially, with time constant [`DECAY_TAU_MS`], so the gap starts
//!    at the level the audio had and is inaudible well before [`MAX_CONCEAL_MS`]. A short
//!    taper in the last [`TAPER_MS`] reaches exactly zero, so the end is not a step either.
//! 3. The first [`ONSET_RAMP_MS`] of the gap ramps up from zero: the loop starts one period
//!    behind the live audio, and without this the junction would step.
//! 4. Live audio returns through a [`RECOVERY_MS`] fade-in, so the end of the gap is not a
//!    step either.
//!
//! The whole thing is deliberately codec-agnostic: it works on the decoded `f32` stream, so
//! it hides a dropout whether the audio was lost in the RF, in the multiplex, or by a
//! codec's own concealment.
//!
//! # Limitations
//!
//! * **Only frames that arrive.** The concealer acts on the frames it is given, so it fills
//!   a gap only where the engine still produces audio and marks it concealed — a run of lost
//!   or CRC-failed MSC frames, a super frame in error, or a codec's own concealment. When
//!   synchronisation is lost outright the engine stops producing audio frames altogether, and
//!   the sound card simply runs dry: this concealer has nothing to act on. Covering that case
//!   needs a hold-and-fade at the player (on its underrun counter), which is a different
//!   change again.
//! * **No pitch detection.** The loop repeats a fixed [`LOOP_PERIOD_MS`], which is one or two
//!   periods of a low-pitched voice but not of most music, so a long gap can sound buzzy. That
//!   is deliberate for now — the decay makes the gap much shorter than the point where it
//!   becomes objectionable — and the natural improvement is to estimate the pitch period and
//!   loop that instead.

use std::collections::VecDeque;

/// Length of the loop period, milliseconds. The history kept is twice this.
pub const LOOP_PERIOD_MS: f64 = 20.0;
/// Ramp up from zero at the start of a gap, milliseconds (see step 3 of the module docs).
pub const ONSET_RAMP_MS: f64 = 2.0;
/// Time constant of the level's exponential fall, milliseconds.
pub const DECAY_TAU_MS: f64 = 110.0;
/// Beyond this the output is silence, milliseconds.
pub const MAX_CONCEAL_MS: f64 = 400.0;
/// Length of the taper to exactly zero at [`MAX_CONCEAL_MS`], milliseconds.
pub const TAPER_MS: f64 = 60.0;
/// Fade-in of the live audio when the gap ends, milliseconds.
pub const RECOVERY_MS: f64 = 12.0;

/// Which stretch of the concealment a frame belongs to, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcealState {
    /// Nothing concealed yet.
    Idle,
    /// Playing the loop, decaying.
    Concealing,
    /// Live audio again, fading in.
    Recovering,
}

/// Fills audio dropouts with decaying repeats of the last good audio (see the module docs).
#[derive(Debug, Clone)]
pub struct DropoutConcealer {
    sample_rate: u32,
    channels: usize,
    /// Frames per loop period ([`LOOP_PERIOD_MS`]).
    period: usize,
    /// Frames of ramp-up at the start of a gap.
    onset_frames: usize,
    /// Frames of fade-in when the audio returns.
    recovery_frames: usize,
    /// Total frames a gap may last before silence ([`MAX_CONCEAL_MS`]).
    max_frames: usize,
    /// The last `2 * period` frames of clean audio, interleaved, oldest first.
    history: VecDeque<f32>,
    /// A snapshot of [`Self::history`] taken when the gap started (the loop).
    loop_buf: Vec<f32>,
    /// Position in [`Self::loop_buf`], in frames, within one period.
    phase: usize,
    /// Frames of the current gap generated so far.
    elapsed: usize,
    /// Frames left of the recovery fade-in.
    recovering: usize,
    /// The previous frame was a gap, so this one is the first live audio after it.
    in_gap: bool,
}

impl DropoutConcealer {
    /// A concealer for `channels` interleaved channels at `sample_rate` Hz.
    ///
    /// `channels` is clamped to at least 1, so a malformed format cannot make this panic.
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let rate = f64::from(sample_rate.max(1));
        let period = ((LOOP_PERIOD_MS / 1000.0) * rate).round() as usize;
        let period = period.max(1);
        Self {
            sample_rate: sample_rate.max(1),
            channels,
            period,
            onset_frames: frames_of(ONSET_RAMP_MS, rate).max(1),
            recovery_frames: frames_of(RECOVERY_MS, rate),
            max_frames: frames_of(MAX_CONCEAL_MS, rate).max(1),
            history: VecDeque::with_capacity(2 * period * channels),
            loop_buf: Vec::new(),
            phase: 0,
            elapsed: 0,
            recovering: 0,
            in_gap: false,
        }
    }

    /// Sampling rate this concealer was built for, Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channels this concealer was built for.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// What the concealer is doing (see [`ConcealState`]).
    pub fn state(&self) -> ConcealState {
        if self.in_gap {
            ConcealState::Concealing
        } else if self.recovering > 0 {
            ConcealState::Recovering
        } else {
            ConcealState::Idle
        }
    }

    /// Forget the history and any gap in progress.
    pub fn reset(&mut self) {
        self.history.clear();
        self.loop_buf.clear();
        self.phase = 0;
        self.elapsed = 0;
        self.recovering = 0;
        self.in_gap = false;
    }

    /// One frame of audio on its way out: `samples` is interleaved and `concealed` says
    /// whether it is a real decode or already a gap (a lost frame, or a codec's concealment).
    ///
    /// Returns `samples.len()` values either way: the input itself, faded back in if a gap
    /// just ended, or the decaying loop while the gap lasts. Never panics on an empty or
    /// ragged slice.
    pub fn process(&mut self, samples: &[f32], concealed: bool) -> Vec<f32> {
        let ch = self.channels;
        let frames = samples.len() / ch;

        if !concealed {
            // Real audio: it becomes history, and it fades in if a gap just ended.
            if self.in_gap {
                // The first live audio after a gap: arm (or re-arm) the fade-in so the
                // return is not a step, even if the gap ran past the point of silence.
                self.recovering = self.recovery_frames;
                self.in_gap = false;
            }
            self.elapsed = 0;
            self.phase = 0;
            self.push_history(samples);

            let mut out = samples.to_vec();
            if self.recovering > 0 {
                let left = self.recovering;
                fade_in(&mut out, ch, frames, self.recovery_frames - left, self.recovery_frames);
                self.recovering = self.recovering.saturating_sub(frames);
            }
            return out;
        }

        // A gap: start the loop if this is its first frame.
        if !self.in_gap {
            self.start_gap();
            self.in_gap = true;
        }

        let mut out = vec![0.0f32; frames * ch];
        let rate = f64::from(self.sample_rate);
        for f in 0..frames {
            let gain = self.gap_gain(self.elapsed + f, rate);
            let j = (self.phase + f) % self.period;
            let w = j as f32 / self.period as f32;
            for c in 0..ch {
                // Overlap-add: the second half of the loop cross-fades into the first, so
                // the seam between repeats is a pair of adjacent samples.
                let a = self.loop_sample(self.period + j, c);
                let b = self.loop_sample(j, c);
                out[f * ch + c] = (a * (1.0 - w) + b * w) * gain;
            }
        }
        self.phase = (self.phase + frames) % self.period;
        self.elapsed += frames;
        out
    }

    /// Arm a recovery ramp and snapshot the loop, at the first frame of a gap.
    fn start_gap(&mut self) {
        self.recovering = self.recovery_frames;
        self.phase = 0;
        self.loop_buf = self.history.iter().copied().collect();
        // A short history (the stream just started) would read past the end: repeat what we
        // have until the loop is full, so the index arithmetic below always holds.
        let want = 2 * self.period * self.channels;
        if self.loop_buf.len() < want {
            if self.loop_buf.is_empty() {
                self.loop_buf = vec![0.0; want];
            } else {
                let have = self.loop_buf.len();
                let mut i = 0;
                while self.loop_buf.len() < want {
                    self.loop_buf.push(self.loop_buf[i % have]);
                    i += 1;
                }
            }
        }
    }

    /// The loop's value at `frame`, channel `c`, clamped into the buffer.
    fn loop_sample(&self, frame: usize, c: usize) -> f32 {
        let idx = frame * self.channels + c;
        self.loop_buf.get(idx).copied().unwrap_or(0.0)
    }

    /// Envelope of the gap at frame `n` from its start (steps 2 and 3 of the module docs).
    fn gap_gain(&self, n: usize, rate: f64) -> f32 {
        if n >= self.max_frames {
            return 0.0;
        }
        let t_ms = n as f64 / rate * 1000.0;
        let decay = (-t_ms / DECAY_TAU_MS).exp();
        let left = (MAX_CONCEAL_MS - t_ms).max(0.0);
        let taper = (left / TAPER_MS).min(1.0);
        let onset = ((n + 1) as f64 / self.onset_frames as f64).min(1.0);
        (decay * taper * onset) as f32
    }

    /// Keep the last `2 * period` frames of `samples`.
    fn push_history(&mut self, samples: &[f32]) {
        let cap = 2 * self.period * self.channels;
        for &s in samples {
            if self.history.len() == cap {
                self.history.pop_front();
            }
            self.history.push_back(s);
        }
    }
}

/// Frames in `ms` at `rate` Hz.
fn frames_of(ms: f64, rate: f64) -> usize {
    ((ms / 1000.0) * rate).round().max(0.0) as usize
}

/// Fade the first `frames` of `out` in from zero, given that `done` frames of the fade are
/// already behind us out of `total`.
fn fade_in(out: &mut [f32], channels: usize, frames: usize, done: usize, total: usize) {
    if total == 0 {
        return;
    }
    for f in 0..frames {
        let g = ((done + f + 1) as f32 / total as f32).min(1.0);
        for c in 0..channels {
            if let Some(v) = out.get_mut(f * channels + c) {
                *v *= g;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 24_000;

    /// A sine of `frames` frames, `channels` interleaved, at `freq` Hz.
    fn tone(frames: usize, channels: usize, freq: f64, from: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            let t = (from + i) as f64 / f64::from(RATE);
            let s = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    fn rms(v: &[f32]) -> f64 {
        if v.is_empty() {
            return 0.0;
        }
        (v.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / v.len() as f64).sqrt()
    }

    #[test]
    fn clean_audio_passes_through_unchanged() {
        let mut c = DropoutConcealer::new(RATE, 1);
        let mut n = 0;
        for _ in 0..20 {
            let input = tone(480, 1, 440.0, n);
            n += 480;
            // No gap ever: the output is the input, sample for sample.
            assert_eq!(c.process(&input, false), input);
        }
        assert_eq!(c.state(), ConcealState::Idle);
    }

    #[test]
    fn concealment_decays_towards_silence() {
        let mut c = DropoutConcealer::new(RATE, 1);
        let mut n = 0;
        // Fill the history with a healthy tone.
        for _ in 0..20 {
            let input = tone(480, 1, 440.0, n);
            n += 480;
            c.process(&input, false);
        }
        // Now a long gap: the level must fall.
        let mut seen = Vec::new();
        for _ in 0..10 {
            let out = c.process(&vec![0.0; 480], true);
            seen.push(rms(&out));
        }
        assert!(seen[0] > 0.0, "the gap starts audible: {seen:?}");
        for w in seen.windows(2) {
            assert!(w[1] <= w[0] + 1e-6, "level must not rise during a gap: {seen:?}");
        }
        assert!(seen[9] < seen[0] * 0.7, "level must fall clearly: {seen:?}");
    }

    #[test]
    fn concealment_is_silent_after_the_limit() {
        let mut c = DropoutConcealer::new(RATE, 1);
        let mut n = 0;
        for _ in 0..20 {
            let input = tone(480, 1, 440.0, n);
            n += 480;
            c.process(&input, false);
        }
        let max = frames_of(MAX_CONCEAL_MS, f64::from(RATE));
        let mut done = 0;
        while done < max + 480 {
            let out = c.process(&vec![0.0; 480], true);
            done += 480;
            if done > max {
                assert_eq!(rms(&out), 0.0, "silence after {MAX_CONCEAL_MS} ms");
            }
        }
    }

    #[test]
    fn output_keeps_length_and_channels() {
        let mut c = DropoutConcealer::new(RATE, 2);
        let input = tone(480, 2, 440.0, 0);
        assert_eq!(c.process(&input, false).len(), input.len());
        assert_eq!(c.process(&input, true).len(), input.len());
        // An empty frame is not a panic.
        assert!(c.process(&[], true).is_empty());
        assert!(c.process(&[], false).is_empty());
    }

    #[test]
    fn empty_input_does_not_panic() {
        let mut c = DropoutConcealer::new(0, 0);
        assert!(c.process(&[], true).is_empty());
        assert!(c.process(&[], false).is_empty());
        assert!(c.process(&[1.0], true).len() == 1);
    }

    #[test]
    fn a_gap_never_starts_with_a_step() {
        let mut c = DropoutConcealer::new(RATE, 1);
        let mut n = 0;
        for _ in 0..20 {
            let input = tone(480, 1, 440.0, n);
            n += 480;
            c.process(&input, false);
        }
        // The first sample of the gap is ramped up from zero, so it cannot step away from
        // the live audio that preceded it.
        let out = c.process(&vec![0.0; 480], true);
        assert!(out[0].abs() < 0.2, "the gap starts near zero: {}", out[0]);
    }

    #[test]
    fn live_audio_fades_back_in() {
        let mut c = DropoutConcealer::new(RATE, 1);
        let mut n = 0;
        for _ in 0..20 {
            let input = tone(480, 1, 440.0, n);
            n += 480;
            c.process(&input, false);
        }
        c.process(&vec![0.0; 480], true);
        assert_eq!(c.state(), ConcealState::Concealing);
        // The return is faded in, so its first sample is well below full scale.
        let back = tone(480, 1, 440.0, n);
        let out = c.process(&back, false);
        assert_eq!(out.len(), back.len());
        assert!(
            out[0].abs() < 0.2,
            "the first sample back does not step: {}",
            out[0]
        );
        // ... and once the ramp is over it is the input again, sample for sample.
        let mut n2 = n + 480;
        let mut after = Vec::new();
        for _ in 0..4 {
            let input = tone(480, 1, 440.0, n2);
            n2 += 480;
            after = c.process(&input, false);
        }
        assert_eq!(c.state(), ConcealState::Idle);
        assert!(rms(&after) > 0.5, "back to full level: {}", rms(&after));
    }

    #[test]
    fn the_loop_seam_is_not_a_step() {
        // A gap played from a clean tone must not produce a bigger sample-to-sample jump
        // than the tone itself does (that is what the overlap-add is for).
        let mut c = DropoutConcealer::new(RATE, 1);
        let mut n = 0;
        for _ in 0..40 {
            let input = tone(480, 1, 300.0, n);
            n += 480;
            c.process(&input, false);
        }
        let tone_step = {
            let t = tone(2400, 1, 300.0, 0);
            t.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max)
        };
        let mut worst = 0.0f32;
        for _ in 0..8 {
            let out = c.process(&vec![0.0; 480], true);
            if let Some(m) = out.windows(2).map(|w| (w[1] - w[0]).abs()).reduce(f32::max) {
                worst = worst.max(m);
            }
        }
        assert!(
            worst <= tone_step * 1.5,
            "loop seam steps by {worst} against a tone step of {tone_step}"
        );
    }

    #[test]
    fn a_brief_gap_stays_audible() {
        // The point of the feature: masking a short dropout. At 40 ms the level must still
        // be most of the way up, not already gone.
        let mut c = DropoutConcealer::new(RATE, 1);
        let mut n = 0;
        for _ in 0..20 {
            let input = tone(480, 1, 440.0, n);
            n += 480;
            c.process(&input, false);
        }
        let out = c.process(&vec![0.0; 960], true); // exactly 40 ms
        assert!(rms(&out) > 0.3, "a 40 ms gap stays audible: {}", rms(&out));
    }

    #[test]
    fn channels_stay_independent() {
        let mut c = DropoutConcealer::new(RATE, 2);
        // Left is a tone, right is silence: the loop must not mix them.
        let mut input = Vec::new();
        for i in 0..4800 {
            let t = i as f64 / f64::from(RATE);
            input.push((2.0 * std::f64::consts::PI * 440.0 * t).sin() as f32);
            input.push(0.0);
        }
        c.process(&input, false);
        let out = c.process(&vec![0.0; 480], true);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert!(rms(&left) > 0.0, "the left channel is concealed");
        assert_eq!(rms(&right), 0.0, "the silent right channel stays silent");
    }

    #[test]
    fn a_gap_before_any_audio_is_silence_not_a_panic() {
        let mut c = DropoutConcealer::new(RATE, 1);
        let out = c.process(&vec![0.0; 480], true);
        assert_eq!(out.len(), 480);
        assert_eq!(rms(&out), 0.0, "nothing was heard yet, so nothing is invented");
    }
}
