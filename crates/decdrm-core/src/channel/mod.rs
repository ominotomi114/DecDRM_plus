//! Channel simulator: multipath propagation with Rayleigh fading (the DRM channel
//! models of ES 201 980 annex B, table B.1, as implemented by Dream's `CDRMChannel`
//! in `drmchannel/ChannelSimulation.cpp`, and the VHF models of [`vhf`]), a constant
//! frequency offset, a sample-rate offset and additive white Gaussian noise (AWGN).
//!
//! The simulator works on the complex baseband signal produced by
//! [`crate::tx::Transmitter`], at the **working rate** the chain runs at: 48 kHz for
//! modes A–D, 120 or 192 kHz for mode E (DRM+). [`ChannelSimulator::new`] is the 48 kHz
//! case; [`ChannelSimulator::new_at`] takes the rate. Convert the output with
//! [`crate::tx::output::OutputStage`] to real IF or I/Q samples for the receiver.
//! Processing order: multipath/fading → frequency offset → sample-rate offset → AWGN.
//!
//! # SNR definition
//!
//! `snr_db` is the ratio of the mean received signal power to the noise power in the
//! **nominal channel bandwidth** of the spectrum occupancy (4.5, 5, 9, 10, 18 or
//! 20 kHz). This is how Dream's simulations specify the SNR
//! (`CParameter::SetNominalSNRdB`) and what the receiver's SNR estimate reports
//! (`RxStatus::snr_db`). The channel model is normalised to unit mean power gain
//! (Σ gain² = 1, Dream's `rGainCorr`), so the mean received signal power equals the
//! transmitted power P, i.e. Dream's `rAvPowPerSymbol` = [`CellMap::avg_power_per_symbol`]
//! (data cells 1, pilots 2, boosted pilots 4, averaged over a super frame), which is
//! also the mean power of the transmitter's baseband output. The complex noise has
//! variance
//!
//! ```text
//! σ² = E|w|² = P · fs / (SNR · B_nom)
//! ```
//!
//! over the full ±fs/2 band. Inside `CDRMChannel` Dream works with the equivalent
//! "system" SNR relative to the bandwidth spanned by the carriers,
//! B_sys = (Kmax − Kmin + 1)·fs/N: SNR_sys = SNR · B_nom / B_sys
//! (see [`ChannelSimulator::system_snr_db`]).

pub mod fading;
pub mod resample;
pub mod rng;
pub mod vhf;

pub use rng::Rng;

use crate::cellmap::CellMap;
use crate::params::{ChannelLayout, WorkingRate, SAMPLE_RATE};
use crate::{Cplx, Real};
use fading::{GaussianFading, JakesFading};
use resample::Resampler;
use std::f64::consts::PI;

/// Doppler spectrum of a fading tap.
///
/// Annex B's models (modes A–D, HF) all use the Gaussian/Watterson spectrum; the VHF
/// models of [`vhf`] use the classical one, which is what a receiver moving through
/// scattered waves sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DopplerSpectrum {
    /// Gaussian, `S(f) ∝ exp(−f²/(2σ²))` with `σ = spread/2` (ES 201 980 annex B).
    #[default]
    Gaussian,
    /// Classical Jakes/Clarke, U-shaped over the spread (a moving receiver).
    Jakes,
}

/// One propagation path of a channel model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Path {
    /// Delay relative to the channel input, ms (rounded to whole samples; Dream
    /// truncates).
    pub delay_ms: Real,
    /// Amplitude gain before the model is normalised to unit power.
    pub gain: Real,
    /// Frequency (Doppler) shift of the path, Hz.
    pub doppler_shift_hz: Real,
    /// Doppler spread of the fading spectrum, Hz; 0 means the path does not fade
    /// (constant gain, as in Dream). For [`DopplerSpectrum::Gaussian`] this is `2σ` (the
    /// annex B `fd` column); for [`DopplerSpectrum::Jakes`] it is the total width
    /// `2·f_d`.
    pub doppler_spread_hz: Real,
    /// Which Doppler spectrum the fading tap uses (ignored when the spread is 0).
    pub spectrum: DopplerSpectrum,
}

impl Path {
    /// A path with the given parameters (same order as the columns of table B.1).
    pub const fn new(delay_ms: Real, gain: Real, doppler_shift_hz: Real, doppler_spread_hz: Real) -> Self {
        Self { delay_ms, gain, doppler_shift_hz, doppler_spread_hz, spectrum: DopplerSpectrum::Gaussian }
    }

    /// The same path, fading with `spectrum` instead of the annex B Gaussian one.
    pub const fn with_spectrum(mut self, spectrum: DopplerSpectrum) -> Self {
        self.spectrum = spectrum;
        self
    }
}

/// A multipath channel model: a list of paths.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelModel {
    pub paths: Vec<Path>,
}

impl ChannelModel {
    /// A single non-fading path (DRM channel 1; AWGN only).
    pub fn awgn() -> Self {
        Self { paths: vec![Path::new(0.0, 1.0, 0.0, 0.0)] }
    }

    /// DRM channel model `number` (1..=6) of ES 201 980 annex B, table B.1, with the
    /// parameters Dream uses:
    ///
    /// | # | name | paths (delay ms, gain, shift Hz, spread Hz) |
    /// |---|------|------|
    /// | 1 | AWGN | (0, 1, 0, 0) |
    /// | 2 | Rice with delay | (0, 1, 0, 0), (1, 0.5, 0, 0.1) |
    /// | 3 | US Consortium | (0, 1, 0.1, 0.1), (0.7, 0.7, 0.2, 0.5), (1.5, 0.5, 0.5, 1), (2.2, 0.25, 1, 2) |
    /// | 4 | CCIR Poor | (0, 1, 0, 1), (2, 1, 0, 1) |
    /// | 5 | — | (0, 1, 0, 2), (4, 1, 0, 2) |
    /// | 6 | — | (0, 0.5, 0, 0.1), (2, 1, 1.2, 2.4), (4, 0.25, 2.4, 4.8), (6, 0.0625, 3.6, 7.2) |
    pub fn drm(number: u8) -> Option<Self> {
        let p = Path::new;
        let paths = match number {
            1 => vec![p(0.0, 1.0, 0.0, 0.0)],
            2 => vec![p(0.0, 1.0, 0.0, 0.0), p(1.0, 0.5, 0.0, 0.1)],
            3 => vec![p(0.0, 1.0, 0.1, 0.1), p(0.7, 0.7, 0.2, 0.5), p(1.5, 0.5, 0.5, 1.0), p(2.2, 0.25, 1.0, 2.0)],
            4 => vec![p(0.0, 1.0, 0.0, 1.0), p(2.0, 1.0, 0.0, 1.0)],
            5 => vec![p(0.0, 1.0, 0.0, 2.0), p(4.0, 1.0, 0.0, 2.0)],
            6 => vec![p(0.0, 0.5, 0.0, 0.1), p(2.0, 1.0, 1.2, 2.4), p(4.0, 0.25, 2.4, 4.8), p(6.0, 0.0625, 3.6, 7.2)],
            _ => return None,
        };
        Some(Self { paths })
    }

    /// Short name of DRM channel model `number` (for reports).
    pub fn drm_name(number: u8) -> &'static str {
        match number {
            1 => "AWGN",
            2 => "Rice with delay",
            3 => "US Consortium",
            4 => "CCIR Poor",
            5 => "Channel 5",
            6 => "Channel 6",
            _ => "unknown",
        }
    }

    /// A user-defined model.
    pub fn custom(paths: Vec<Path>) -> Self {
        Self { paths }
    }

    /// Σ gain² before normalisation.
    pub fn power(&self) -> Real {
        self.paths.iter().map(|p| p.gain * p.gain).sum()
    }
}

/// Channel simulator settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelConfig {
    pub model: ChannelModel,
    /// SNR in the nominal channel bandwidth, dB (see the module docs); `None` adds no
    /// noise.
    pub snr_db: Option<Real>,
    /// Constant frequency offset added to the signal, Hz.
    pub freq_offset_hz: Real,
    /// Sample-rate offset of the simulated receiver clock relative to the
    /// transmitter, in ppm. Positive: the receiver samples faster, so it gets
    /// `1 + ppm·10⁻⁶` samples per transmitted sample.
    pub sample_rate_offset_ppm: Real,
    /// Seed of the fading and noise generators (same seed ⇒ identical output).
    pub seed: u64,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self { model: ChannelModel::awgn(), snr_db: None, freq_offset_hz: 0.0, sample_rate_offset_ppm: 0.0, seed: 1 }
    }
}

impl ChannelConfig {
    /// AWGN channel at `snr_db`.
    pub fn awgn(snr_db: Real) -> Self {
        Self { snr_db: Some(snr_db), ..Self::default() }
    }

    /// DRM channel model `number` (1..=6) at `snr_db`.
    pub fn drm(number: u8, snr_db: Real) -> Option<Self> {
        Some(Self { model: ChannelModel::drm(number)?, snr_db: Some(snr_db), ..Self::default() })
    }
}

/// Fading process of one tap: the spectrum decides which generator drives it.
#[derive(Debug, Clone)]
enum TapFading {
    Gaussian(GaussianFading),
    Jakes(JakesFading),
}

impl TapFading {
    /// `None` when `path` does not fade (zero spread).
    fn new(path: &Path, fs: Real, rng: Rng) -> Option<Self> {
        if path.doppler_spread_hz <= 0.0 {
            return None;
        }
        Some(match path.spectrum {
            DopplerSpectrum::Gaussian => Self::Gaussian(GaussianFading::new(path.doppler_spread_hz, fs, rng)),
            // The spread is the full width 2·f_d, so the maximum Doppler shift is half.
            DopplerSpectrum::Jakes => Self::Jakes(JakesFading::new(path.doppler_spread_hz / 2.0, fs, rng)),
        })
    }

    #[inline]
    fn next_gain(&mut self) -> Cplx {
        match self {
            Self::Gaussian(f) => f.next_gain(),
            Self::Jakes(f) => f.next_gain(),
        }
    }
}

/// Taps of the fractional-delay interpolator (a Hamming-windowed sinc).
///
/// Whole-sample delays are exact and cheap, but VHF profiles have taps well below one
/// sample at DRM+'s 192 kHz: COST 207's 0.2 µs is 0.04 samples. Rounding those to the
/// nearest sample would merge taps and move every null in the response, so the sub-sample
/// part of each delay is interpolated instead.
pub const FRACTIONAL_TAPS: usize = 16;

/// Group delay of the interpolator, samples. Every simulated path carries it, so relative
/// delays stay exact; the receiver only sees the whole signal shifted by this much.
pub const FRACTIONAL_LATENCY: usize = FRACTIONAL_TAPS / 2 - 1;

/// Windowed-sinc taps approximating a delay of `frac` (0 ≤ frac < 1) samples, with the
/// peak at [`FRACTIONAL_LATENCY`]. Normalised for unit DC gain.
fn fractional_taps(frac: Real) -> Vec<Real> {
    let mut taps: Vec<Real> = (0..FRACTIONAL_TAPS)
        .map(|k| {
            let x = k as Real - frac - FRACTIONAL_LATENCY as Real;
            let sinc = if x.abs() < 1e-12 { 1.0 } else { (PI * x).sin() / (PI * x) };
            let window = 0.54 - 0.46 * (2.0 * PI * k as Real / (FRACTIONAL_TAPS - 1) as Real).cos();
            sinc * window
        })
        .collect();
    let sum: Real = taps.iter().sum();
    for t in &mut taps {
        *t /= sum;
    }
    taps
}

/// Parse a channel specification, as the CLI's `--channel` and the GUI's field take it.
///
/// ```text
/// awgn                              noise only (no SNR: noise off)
/// ra:<speed>@<mhz>                  COST 207 rural, e.g. ra:0@200 (static) or ra:100@200
/// tu6:<speed>@<mhz>                 COST 207 typical urban
/// ht:<speed>@<mhz>                  COST 207 hilly terrain
/// sfn:<delay_us>@<level_db>         one echo, e.g. sfn:300@-6 (beyond mode E's guard)
/// static:<delay>/<db>;<delay>/<db>  measured profile, e.g. static:0/0;5/-6;15/-12
/// ```
///
/// followed by any of `,snr=<db>`, `,seed=<n>`, `,foff=<hz>`, `,sro=<ppm>`.
pub fn parse_spec(text: &str) -> Result<ChannelConfig, String> {
    let mut cfg = ChannelConfig::default();
    let mut parts = text.split(',');
    let head = parts.next().unwrap_or("").trim();
    let (name, args) = match head.split_once(':') {
        Some((n, a)) => (n.trim(), Some(a.trim())),
        None => (head, None),
    };
    cfg.model = match name {
        "awgn" | "" => ChannelModel::awgn(),
        "ra" | "tu6" | "ht" => {
            let (speed, mhz) = pair(args, name)?;
            let carrier_hz = mhz * 1e6;
            match name {
                "ra" => vhf::cost207_ra(speed, carrier_hz),
                "tu6" => vhf::cost207_tu6(speed, carrier_hz),
                _ => vhf::cost207_ht(speed, carrier_hz),
            }
        }
        "sfn" => {
            let (delay_us, level_db) = pair(args, name)?;
            vhf::sfn_echo(delay_us, level_db)
        }
        "static" => {
            let list = args.ok_or("static needs delay_us/level_db pairs, e.g. static:0/0;5/-6")?;
            let mut taps = Vec::new();
            for tap in list.split(';').filter(|t| !t.trim().is_empty()) {
                let (delay, level) = tap.split_once('/').ok_or_else(|| format!("bad tap {tap:?} (delay/dB)"))?;
                taps.push((parse_num(delay, "delay")?, parse_num(level, "level")?));
            }
            if taps.is_empty() {
                return Err("static needs at least one tap".into());
            }
            vhf::static_multipath(&taps)
        }
        other => {
            return Err(format!("unknown channel profile {other:?} (awgn, ra, tu6, ht, sfn, static)"));
        }
    };
    for option in parts {
        let (key, value) = option.split_once('=').ok_or_else(|| format!("bad option {option:?} (key=value)"))?;
        match key.trim() {
            "snr" => cfg.snr_db = Some(parse_num(value, "snr")?),
            "seed" => cfg.seed = parse_num(value, "seed")? as u64,
            "foff" => cfg.freq_offset_hz = parse_num(value, "foff")?,
            "sro" => cfg.sample_rate_offset_ppm = parse_num(value, "sro")?,
            other => return Err(format!("unknown channel option {other:?}")),
        }
    }
    Ok(cfg)
}

fn pair(args: Option<&str>, name: &str) -> Result<(Real, Real), String> {
    let text = args.ok_or_else(|| format!("{name} needs <a>@<b>, e.g. {name}:100@200"))?;
    let (a, b) = text.split_once('@').ok_or_else(|| format!("{name} needs <a>@<b>, got {text:?}"))?;
    Ok((parse_num(a, name)?, parse_num(b, name)?))
}

fn parse_num(text: &str, what: &str) -> Result<Real, String> {
    text.trim().parse::<Real>().map_err(|e| format!("{what} {text:?}: {e}"))
}

/// Run-time state of one path.
#[derive(Debug, Clone)]
struct PathState {
    /// Delay in samples, exact (for reporting).
    samples: Real,
    /// Integer part of the path delay: index into the history line.
    delay: usize,
    /// Sub-sample interpolation taps (peak at `FRACTIONAL_LATENCY`).
    taps: Vec<Real>,
    /// Normalised amplitude.
    gain: Real,
    /// Doppler shift in radians per sample, and its accumulated phase.
    shift_step: Real,
    shift_phase: Real,
    fading: Option<TapFading>,
}

impl PathState {
    #[inline]
    fn next_gain(&mut self) -> Cplx {
        let g = match self.fading.as_mut() {
            Some(f) => f.next_gain() * self.gain,
            None => Cplx::new(self.gain, 0.0),
        };
        if self.shift_step == 0.0 {
            return g;
        }
        let rot = Cplx::from_polar(1.0, self.shift_phase);
        self.shift_phase = wrap(self.shift_phase + self.shift_step);
        g * rot
    }
}

fn wrap(p: Real) -> Real {
    if p > PI {
        p - 2.0 * PI
    } else if p < -PI {
        p + 2.0 * PI
    } else {
        p
    }
}

/// Streaming channel simulator (see the module docs).
#[derive(Debug, Clone)]
pub struct ChannelSimulator {
    cfg: ChannelConfig,
    paths: Vec<PathState>,
    /// True when the multipath stage is a plain copy (single static unit path).
    identity: bool,
    /// Samples of history the delay line holds: the longest integer delay plus the
    /// interpolator's length.
    history: usize,
    /// Delay line: `history` samples of past input followed by the current block.
    line: Vec<Cplx>,
    freq_step: Real,
    freq_phase: Real,
    resampler: Option<Resampler>,
    signal_power: Real,
    nominal_bw_hz: Real,
    system_bw_hz: Real,
    /// Noise standard deviation per complex sample (E|w|² = noise_std²).
    noise_std: Real,
    noise_rng: Rng,
    work: Vec<Cplx>,
    /// Working sample rate the simulator was built for, Hz.
    fs: Real,
}

impl ChannelSimulator {
    /// Simulator for the signal of a transmitter configured with `layout` (which
    /// defines the reference signal power and the nominal bandwidth of the SNR), at the
    /// rate modes A–D run at (48 kHz).
    pub fn new(layout: ChannelLayout, cfg: ChannelConfig) -> Self {
        Self::new_at(layout, WorkingRate::R48, cfg)
    }

    /// Simulator for a signal generated at `rate` — 120 or 192 kHz for mode E, where the
    /// delays, the Doppler frequencies and the noise reference are all per working-rate
    /// sample rather than per 48 kHz one.
    pub fn new_at(layout: ChannelLayout, rate: WorkingRate, cfg: ChannelConfig) -> Self {
        let map = CellMap::new(layout.mode, layout.occupancy).expect("a ChannelLayout is always valid");
        let fs = Real::from(rate.hz());
        let system_bw = map.num_carriers as Real * fs / layout.mode.fft_size_at(rate) as Real;
        Self::with_reference_at(map.avg_power_per_symbol, layout.bandwidth_khz() * 1000.0, system_bw, fs, cfg)
    }

    /// Simulator for an arbitrary complex signal of mean power `signal_power` whose
    /// SNR is referred to `nominal_bw_hz` (and reported as "system" SNR relative to
    /// `system_bw_hz`), at 48 kHz.
    pub fn with_reference(signal_power: Real, nominal_bw_hz: Real, system_bw_hz: Real, cfg: ChannelConfig) -> Self {
        Self::with_reference_at(signal_power, nominal_bw_hz, system_bw_hz, Real::from(SAMPLE_RATE), cfg)
    }

    /// [`Self::with_reference`] at an explicit sample rate: the delays are rounded to
    /// `fs`-rate samples and the noise power is `P·fs/(SNR·B_nom)`.
    pub fn with_reference_at(
        signal_power: Real,
        nominal_bw_hz: Real,
        system_bw_hz: Real,
        fs: Real,
        cfg: ChannelConfig,
    ) -> Self {
        let mut master = Rng::new(cfg.seed);
        let total = cfg.model.power();
        let norm = if total > 0.0 { total.sqrt().recip() } else { 0.0 };
        let paths: Vec<PathState> = cfg
            .model
            .paths
            .iter()
            .map(|p| {
                let samples = (p.delay_ms * fs / 1000.0).max(0.0);
                let delay = samples.floor() as usize;
                PathState {
                    samples,
                    delay,
                    taps: fractional_taps(samples - delay as Real),
                    gain: p.gain * norm,
                    shift_step: 2.0 * PI * p.doppler_shift_hz / fs,
                    shift_phase: 0.0,
                    fading: TapFading::new(p, fs, master.fork()),
                }
            })
            .collect();
        let max_delay = paths.iter().map(|p| p.delay).max().unwrap_or(0);
        let history = max_delay + FRACTIONAL_TAPS;
        let identity = paths.len() == 1
            && paths[0].delay == 0
            && paths[0].fading.is_none()
            && paths[0].shift_step == 0.0
            && (paths[0].samples - 0.0).abs() < 1e-15
            && (paths[0].gain - 1.0).abs() < 1e-15;
        let noise_std = match cfg.snr_db {
            Some(snr_db) => {
                let snr = 10f64.powf(snr_db / 10.0);
                (signal_power * fs / (snr * nominal_bw_hz)).sqrt()
            }
            None => 0.0,
        };
        let resampler =
            (cfg.sample_rate_offset_ppm != 0.0).then(|| Resampler::new(1.0 + cfg.sample_rate_offset_ppm * 1e-6));
        Self {
            freq_step: 2.0 * PI * cfg.freq_offset_hz / fs,
            freq_phase: 0.0,
            paths,
            identity,
            history,
            line: vec![Cplx::new(0.0, 0.0); history],
            resampler,
            signal_power,
            nominal_bw_hz,
            system_bw_hz,
            noise_std,
            noise_rng: master.fork(),
            work: Vec::new(),
            fs,
            cfg,
        }
    }

    pub fn config(&self) -> &ChannelConfig {
        &self.cfg
    }

    /// Working sample rate the simulator converts delays and Doppler at, Hz.
    pub fn sample_rate_hz(&self) -> Real {
        self.fs
    }

    /// Reference signal power P used for the SNR.
    pub fn signal_power(&self) -> Real {
        self.signal_power
    }

    /// Noise variance E|w|² per complex sample (0 without noise).
    pub fn noise_power(&self) -> Real {
        self.noise_std * self.noise_std
    }

    /// SNR relative to the bandwidth spanned by the carriers (Dream's internal
    /// "system" SNR), dB.
    pub fn system_snr_db(&self) -> Option<Real> {
        self.cfg.snr_db.map(|s| s + 10.0 * (self.nominal_bw_hz / self.system_bw_hz).log10())
    }

    /// Path delays in samples, rounded for reporting (the simulated delays are exact; see
    /// [`Self::path_delays_exact`]).
    pub fn path_delays(&self) -> Vec<usize> {
        self.paths.iter().map(|p| p.samples.round() as usize).collect()
    }

    /// Path delays in samples exactly, including the sub-sample part the interpolator
    /// realises (no rounding to whole samples anywhere).
    pub fn path_delays_exact(&self) -> Vec<Real> {
        self.paths.iter().map(|p| p.samples).collect()
    }

    /// Pass `input` through the channel, appending the result to `out`. With a
    /// sample-rate offset the number of output samples differs from the input
    /// (by `ppm·10⁻⁶` on average).
    pub fn process(&mut self, input: &[Cplx], out: &mut Vec<Cplx>) {
        // `mem::take` moves the scratch buffer out of `self` (leaving an empty Vec) so
        // it can be filled while other fields of `self` are borrowed mutably; it is
        // put back at the end, keeping its allocation for the next call.
        let mut sig = std::mem::take(&mut self.work);
        sig.clear();
        self.multipath(input, &mut sig);

        if self.freq_step != 0.0 {
            for s in &mut sig {
                *s *= Cplx::from_polar(1.0, self.freq_phase);
                self.freq_phase = wrap(self.freq_phase + self.freq_step);
            }
        }

        let start = out.len();
        match self.resampler.as_mut() {
            Some(r) => r.process(&sig, out),
            None => out.extend_from_slice(&sig),
        }

        if self.noise_std > 0.0 {
            for s in &mut out[start..] {
                *s += self.noise_rng.complex_gaussian() * self.noise_std;
            }
        }
        self.work = sig;
    }

    fn multipath(&mut self, input: &[Cplx], out: &mut Vec<Cplx>) {
        if self.identity {
            out.extend_from_slice(input);
            return;
        }
        let h = self.history;
        self.line.extend_from_slice(input);
        out.reserve(input.len());
        for i in 0..input.len() {
            let mut acc = Cplx::new(0.0, 0.0);
            // Borrowing `self.paths` mutably while indexing `self.line` is fine: the
            // compiler tracks borrows of distinct struct fields separately.
            for p in &mut self.paths {
                let g = p.next_gain();
                // Each path contributes its integer delay and then the windowed-sinc
                // interpolation of the sub-sample part, `FRACTIONAL_TAPS` taps wide.
                let mut v = Cplx::new(0.0, 0.0);
                let base = h + i - p.delay;
                for (k, &tap) in p.taps.iter().enumerate() {
                    v += self.line[base - k] * tap;
                }
                acc += v * g;
            }
            out.push(acc);
        }
        let consumed = self.line.len() - h;
        self.line.drain(..consumed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};

    fn layout() -> ChannelLayout {
        ChannelLayout::new(RobustnessMode::B, SpectrumOccupancy::SO_3).unwrap()
    }

    fn tone(f_hz: Real, n: usize) -> Vec<Cplx> {
        let fs = Real::from(SAMPLE_RATE);
        (0..n).map(|i| Cplx::from_polar(1.0, 2.0 * PI * f_hz * i as Real / fs)).collect()
    }

    #[test]
    fn drm_models_have_expected_paths() {
        for n in 1..=6 {
            assert!(ChannelModel::drm(n).is_some(), "model {n}");
        }
        assert!(ChannelModel::drm(0).is_none() && ChannelModel::drm(7).is_none());
        let m = ChannelModel::drm(3).unwrap();
        assert_eq!(m.paths.len(), 4);
        let sim = ChannelSimulator::new(layout(), ChannelConfig { model: m, ..Default::default() });
        assert_eq!(sim.path_delays(), vec![0, 34, 72, 106]);
    }

    /// Noise power follows σ² = P·fs/(SNR·B_nom).
    #[test]
    fn awgn_power_matches_definition() {
        let snr_db = 10.0;
        let mut sim = ChannelSimulator::new(layout(), ChannelConfig::awgn(snr_db));
        let n = 200_000;
        let mut out = Vec::new();
        sim.process(&vec![Cplx::new(0.0, 0.0); n], &mut out);
        let measured = out.iter().map(|v| v.norm_sqr()).sum::<Real>() / n as Real;
        let map = CellMap::new(RobustnessMode::B, SpectrumOccupancy::SO_3).unwrap();
        let want = map.avg_power_per_symbol * 48_000.0 / (10.0 * 10_000.0);
        assert!((measured / want - 1.0).abs() < 0.02, "noise power {measured} vs {want}");
        // Mode B / SO3: 207 carriers of 46.875 Hz = 9703 Hz system bandwidth.
        let sys = sim.system_snr_db().unwrap();
        assert!((sys - (10.0 + 10.0 * (10_000.0f64 / 9703.125).log10())).abs() < 1e-9);
    }

    #[test]
    fn same_seed_same_output() {
        let cfg = ChannelConfig { seed: 77, ..ChannelConfig::drm(3, 15.0).unwrap() };
        let x = tone(1000.0, 5000);
        let run = || {
            let mut s = ChannelSimulator::new(layout(), cfg.clone());
            let mut y = Vec::new();
            for c in x.chunks(999) {
                s.process(c, &mut y);
            }
            y
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn frequency_offset_shifts_a_tone() {
        let cfg = ChannelConfig { freq_offset_hz: 40.0, ..Default::default() };
        let mut sim = ChannelSimulator::new(layout(), cfg);
        let x = tone(1000.0, 48_000);
        let mut y = Vec::new();
        sim.process(&x, &mut y);
        let mut acc = Cplx::new(0.0, 0.0);
        for n in 1..y.len() {
            acc += y[n] * y[n - 1].conj();
        }
        let f = acc.arg() / (2.0 * PI) * 48_000.0;
        assert!((f - 1040.0).abs() < 1e-6, "{f}");
    }

    #[test]
    fn sample_rate_offset_scales_tone_frequency() {
        let ppm = 50.0;
        let cfg = ChannelConfig { sample_rate_offset_ppm: ppm, ..Default::default() };
        let mut sim = ChannelSimulator::new(layout(), cfg);
        let x = tone(3000.0, 96_000);
        let mut y = Vec::new();
        for c in x.chunks(4096) {
            sim.process(c, &mut y);
        }
        let expect_len = 96_000.0 * (1.0 + ppm * 1e-6);
        assert!((y.len() as Real - expect_len).abs() < 70.0, "{} samples", y.len());
        let mut acc = Cplx::new(0.0, 0.0);
        for n in 1000..y.len() - 1000 {
            acc += y[n] * y[n - 1].conj();
        }
        let f = acc.arg() / (2.0 * PI) * 48_000.0;
        let want = 3000.0 / (1.0 + ppm * 1e-6);
        assert!((f - want).abs() < 1e-4, "{f} vs {want}");
    }

    /// A static two-path channel has the expected frequency response.
    #[test]
    fn static_multipath_frequency_response() {
        let model = ChannelModel::custom(vec![Path::new(0.0, 1.0, 0.0, 0.0), Path::new(1.0, 0.5, 0.0, 0.0)]);
        let mut sim = ChannelSimulator::new(layout(), ChannelConfig { model, ..Default::default() });
        let n = 4800;
        let f = 700.0;
        let x = tone(f, n);
        let mut y = Vec::new();
        sim.process(&x, &mut y);
        let norm = 1.25f64.sqrt().recip();
        let h = (Cplx::new(1.0, 0.0) + Cplx::from_polar(0.5, -2.0 * PI * f * 48.0 / 48_000.0)) * norm;
        // The interpolator's fixed latency shifts the whole signal; relative delays are
        // exact.
        for i in 100..n {
            assert!((y[i] - x[i - FRACTIONAL_LATENCY] * h).norm() < 1e-9);
        }
    }

    /// Fading channels have unit mean power gain on average.
    #[test]
    fn fading_channel_mean_power_is_one() {
        let cfg = ChannelConfig { seed: 3, ..ChannelConfig::drm(5, 100.0).unwrap() };
        let mut sim = ChannelSimulator::new(layout(), ChannelConfig { snr_db: None, ..cfg });
        // Wide-band input: white noise, so the channel's frequency selectivity averages out.
        let mut rng = Rng::new(9);
        let x: Vec<Cplx> = (0..48_000 * 60).map(|_| rng.complex_gaussian()).collect();
        let mut y = Vec::new();
        sim.process(&x, &mut y);
        let p = y.iter().map(|v| v.norm_sqr()).sum::<Real>() / y.len() as Real;
        assert!((p - 1.0).abs() < 0.15, "mean power gain {p}");
        // And the fading actually varies over time.
        let blocks: Vec<Real> =
            y.chunks(48_000).map(|b| b.iter().map(|v| v.norm_sqr()).sum::<Real>() / b.len() as Real).collect();
        let max = blocks.iter().copied().fold(0.0, Real::max);
        let min = blocks.iter().copied().fold(Real::INFINITY, Real::min);
        assert!(max / min > 2.0, "block powers do not fade: {min}..{max}");
    }

    /// The CLI/GUI channel string maps to the expected models, and bad text is rejected
    /// with an explanation rather than silently ignored.
    #[test]
    fn channel_spec_covers_the_profiles() {
        let tu6 = parse_spec("tu6:100@200,snr=20").unwrap();
        assert_eq!(tu6.snr_db, Some(20.0));
        assert_eq!(tu6.model.paths.len(), 6);
        assert!((tu6.model.paths[3].delay_ms - 0.0016).abs() < 1e-12, "TU6's fourth tap is 1.6 µs");
        assert!(tu6.model.paths.iter().all(|p| p.doppler_spread_hz > 0.0), "100 km/h fades");

        let parked = parse_spec("ra:0@200").unwrap();
        assert_eq!(parked.snr_db, None);
        assert!(parked.model.paths.iter().all(|p| p.doppler_spread_hz == 0.0), "0 km/h is static");

        let sfn = parse_spec("sfn:300@-6,seed=7,foff=12.5,sro=3").unwrap();
        assert_eq!((sfn.seed, sfn.freq_offset_hz, sfn.sample_rate_offset_ppm), (7, 12.5, 3.0));
        assert!((sfn.model.paths[1].delay_ms - 0.3).abs() < 1e-12);

        let custom = parse_spec("static:0/0;5/-6;15/-12").unwrap();
        assert_eq!(custom.model.paths.len(), 3);
        assert!((custom.model.paths[2].gain - 10f64.powf(-12.0 / 20.0)).abs() < 1e-12);

        assert!(parse_spec("awgn,snr=10").is_ok());
        assert!(parse_spec("tu6:100").unwrap_err().contains('@'));
        assert!(parse_spec("nope:1@2").unwrap_err().contains("unknown channel profile"));
        assert!(parse_spec("tu6:100@200,snr=x").is_err());
        assert!(parse_spec("tu6:100@200,wat=1").unwrap_err().contains("unknown channel option"));
    }

    /// Mode E at 192 kHz: the SNR is referred to its 100 kHz channel (table 48), and the
    /// system bandwidth spans the 213 carriers over the 432-sample FFT.
    #[test]
    fn mode_e_noise_uses_its_own_bandwidth_at_192k() {
        let layout = ChannelLayout::new(RobustnessMode::E, SpectrumOccupancy::SO_0).unwrap();
        let s = ChannelSimulator::new_at(layout, WorkingRate::R192, ChannelConfig::awgn(10.0));
        let sys_bw = 213.0 * 192_000.0 / 432.0;
        let want = s.signal_power() * 192_000.0 / (10.0 * 100_000.0);
        assert!((s.noise_power() / want - 1.0).abs() < 1e-12, "noise {} vs {want}", s.noise_power());
        let sys = s.system_snr_db().unwrap();
        assert!((sys - (10.0 + 10.0 * (100_000.0f64 / sys_bw).log10())).abs() < 1e-9);
    }

    /// A static two-path channel at 192 kHz has the same frequency response as at 48 kHz,
    /// with the delay in samples scaled by four (0.5 ms = 96 samples, not 24).
    #[test]
    fn static_multipath_at_mode_e_rate() {
        let layout = ChannelLayout::new(RobustnessMode::E, SpectrumOccupancy::SO_0).unwrap();
        let model = ChannelModel::custom(vec![Path::new(0.0, 1.0, 0.0, 0.0), Path::new(0.5, 0.5, 0.0, 0.0)]);
        let mut sim =
            ChannelSimulator::new_at(layout, WorkingRate::R192, ChannelConfig { model, ..Default::default() });
        assert_eq!(sim.path_delays(), vec![0, 96]);
        let fs = 192_000.0;
        let n = 19_200;
        let f = 3000.0;
        let x: Vec<Cplx> = (0..n).map(|i| Cplx::from_polar(1.0, 2.0 * PI * f * i as Real / fs)).collect();
        let mut y = Vec::new();
        sim.process(&x, &mut y);
        let norm = 1.25f64.sqrt().recip();
        let h = (Cplx::new(1.0, 0.0) + Cplx::from_polar(0.5, -2.0 * PI * f * 96.0 / fs)) * norm;
        for i in 200..n {
            assert!((y[i] - x[i - FRACTIONAL_LATENCY] * h).norm() < 1e-9, "sample {i}");
        }
    }

    /// The sub-sample half of a delay is interpolated, not rounded away: half a sample at
    /// 48 kHz must still delay the signal by half a sample, with near-unit gain at 3 kHz.
    /// (This is what lets a 2 µs VHF tap survive at 192 kHz, where it is 0.38 samples.)
    #[test]
    fn sub_sample_delays_are_interpolated() {
        let half_sample_ms = 0.5 * 1000.0 / 48_000.0;
        let model = ChannelModel::custom(vec![Path::new(half_sample_ms, 1.0, 0.0, 0.0)]);
        let mut sim = ChannelSimulator::new(layout(), ChannelConfig { model, ..Default::default() });
        let exact = sim.path_delays_exact();
        assert!((exact[0] - 0.5).abs() < 1e-12, "delay {exact:?}");
        let n = 4800;
        let f = 3000.0;
        let fs = 48_000.0;
        let x = tone(f, n);
        let mut y = Vec::new();
        sim.process(&x, &mut y);
        // y[n] = x(n − 0.5 − latency): the tone delayed by half a sample, through the
        // interpolator's fixed latency.
        for i in 100..n {
            let want = x[i - FRACTIONAL_LATENCY] * Cplx::from_polar(1.0, -2.0 * PI * f * 0.5 / fs);
            assert!((y[i] - want).norm() < 2e-2, "sample {i}: |error| {}", (y[i] - want).norm());
        }
    }
}
