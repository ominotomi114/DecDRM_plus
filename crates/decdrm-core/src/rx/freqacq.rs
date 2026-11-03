//! Coarse frequency acquisition: find the DRM DC carrier in the input spectrum from
//! the three continuous frequency reference pilots, which sit 750, 2250 and 3000 Hz
//! above DC in every robustness mode (port of Dream's `CFreqSyncAcq`).
//!
//! Improvement over Dream: the mirrored pilot pattern is searched as well, so a
//! spectrally inverted signal (e.g. LSB reception) is detected automatically.
//!
//! Above 48 kHz the pilots are not enough: mode E (DRM+) defines no frequency reference
//! cells at all (§8.4.2), so there the search matches the signal's **spectrum plateau**
//! instead (see [`FreqAcquisition::detect_plateau`]).

use crate::dsp::fft::Fft;
use crate::dsp::hamming;
use crate::params::{RobustnessMode, SAMPLE_RATE, WorkingRate};
use crate::{Cplx, Real};
use std::collections::VecDeque;

/// FFT size at 48 kHz: 6 × the mode-B FFT → 7.8125 Hz resolution.
const FFT_LEN_48: usize = 6 * 1024;
/// [`FFT_LEN_48`] scaled to the working rate, keeping the bin width (and so the noise
/// bandwidth the threshold was set for). Mode E runs at 120 or 192 kHz, where the fixed
/// 48 kHz length would stretch the bin to 31 kHz.
fn fft_len_at(rate: WorkingRate) -> usize {
    FFT_LEN_48 * (rate.hz() as usize / SAMPLE_RATE as usize).max(1)
}
/// Pilot offsets from DC in FFT bins (750, 2250, 3000 Hz).
const PILOT_BINS: [usize; 3] = [96, 288, 384];
/// Number of periodograms averaged (Dream: NUM_FFT_RES_AV_BLOCKS).
const NUM_AVERAGE: usize = 13;
/// Hop between periodograms in samples (one mode-B symbol).
const HOP: usize = 1280;
/// Samples spanned by the averaged periodograms _at 48 kHz_; a rate-scaled search looks at
/// proportionally more (`fft_len_at`, `HOP` and this all scale with the rate).
pub const ANALYSIS_SPAN: usize = FFT_LEN_48 + (NUM_AVERAGE - 1) * HOP;
/// [`ANALYSIS_SPAN`] at an explicit rate.
pub fn analysis_span_at(rate: WorkingRate) -> usize {
    fft_len_at(rate) + (NUM_AVERAGE - 1) * hop_at(rate)
}
/// Hop between periodograms in samples: one symbol. [`HOP`] is the mode-B length at 48 kHz.
fn hop_at(rate: WorkingRate) -> usize {
    HOP * (rate.hz() as usize / SAMPLE_RATE as usize).max(1)
}
/// Noise-floor smoother (IIR over frequency).
const LAMBDA_FLOOR: Real = 0.87;
/// Detection threshold on the sum of the three noise-normalised pilot powers.
const PEAK_BOUND: Real = 9.0;
/// Sinusoid rejection ratios (second-highest/highest, lowest/highest).
const MAX_RATIO_HIGH: Real = 0.99;
const MAX_RATIO_LOW: Real = 0.8;

/// Where to search for the DC carrier.
#[derive(Debug, Clone, Copy)]
pub struct SearchWindow {
    pub min_hz: Real,
    pub max_hz: Real,
}

/// Smallest ratio of mean power inside the mode E band to mean power outside it that counts
/// as a signal. A mode E band 3 dB above the noise reads about 2 and empty spectrum about
/// 1, so this sits between them with room for the band's own ripple.
const MIN_BAND_RATIO: Real = 1.5;
/// Fraction of the best ratio that still counts as the plateau: the carrier is taken at the
/// centre of the region scoring at least this much (see `detect_plateau`).
const CREST: Real = 0.9;
/// A band narrower than this many bins is noise, not a signal.
const MIN_BAND_BINS: usize = 8;


/// Successful acquisition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Acquisition {
    /// Frequency of the DRM DC carrier in the (complex) input spectrum, Hz.
    pub dc_hz: Real,
    /// The pilots were found in mirrored order: the spectrum is inverted.
    pub inverted: bool,
    /// Detection metric (sum of normalised pilot powers).
    pub score: Real,
    /// Mean input power over the analysed span (window-weighted), |sample|².
    pub power: Real,
}

#[derive(Debug)]
pub struct FreqAcquisition {
    fft: Fft,
    window: Vec<Real>,
    /// Σ window², for the input power.
    window_energy: Real,
    /// Working sample rate: the analysis geometry and whether the mode E detector applies.
    rate: WorkingRate,
    /// Most recent `fft_len` samples.
    history: VecDeque<Cplx>,
    since_last: usize,
    psds: VecDeque<Vec<Real>>,
    psd_sum: Vec<Real>,
    search: SearchWindow,
    allow_inverted: bool,
    work: Vec<Cplx>,
    /// Last averaged, noise-normalised spectrum (for diagnostics), centred on 0 Hz.
    pub last_normalised: Vec<Real>,
}

impl FreqAcquisition {
    /// `real_input`: the signal came from a real source, so only positive
    /// frequencies can hold the DRM signal.
    pub fn new(real_input: bool, allow_inverted: bool) -> Self {
        Self::new_at(real_input, allow_inverted, WorkingRate::R48)
    }

    /// [`Self::new`] at an explicit working sample rate.
    ///
    /// The rate sets the analysis resolution (the FFT length scales with it, keeping the
    /// 7.8125 Hz bin) and enables the **mode E** detector: above 48 kHz the pilot search
    /// cannot find a DRM+ signal, which carries no frequency reference cells at all
    /// (§8.4.2), so the spectrum plateau is searched instead (see [`Self::detect_plateau`]).
    pub fn new_at(real_input: bool, allow_inverted: bool, rate: WorkingRate) -> Self {
        let fs = f64::from(rate.hz());
        let top = 3000.0 + 100.0;
        let search = if real_input {
            SearchWindow { min_hz: 0.0, max_hz: fs / 2.0 - top }
        } else {
            SearchWindow { min_hz: -fs / 2.0 + top, max_hz: fs / 2.0 - top }
        };
        let fft_len = fft_len_at(rate);
        let window = hamming(fft_len);
        Self {
            fft: Fft::new(fft_len),
            rate,
            window_energy: window.iter().map(|w| w * w).sum(),
            window,
            history: VecDeque::with_capacity(fft_len),
            since_last: 0,
            psds: VecDeque::with_capacity(NUM_AVERAGE),
            psd_sum: vec![0.0; fft_len],
            search,
            allow_inverted,
            work: vec![Cplx::new(0.0, 0.0); fft_len],
            last_normalised: Vec::new(),
        }
    }

    /// Restrict the search (e.g. to a user-given IF ± tolerance).
    pub fn set_search_window(&mut self, w: SearchWindow) {
        self.search = w;
    }

    pub fn reset(&mut self) {
        self.history.clear();
        self.since_last = 0;
        self.psds.clear();
        self.psd_sum.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Feed samples; returns an acquisition as soon as the pilots are found.
    pub fn push(&mut self, samples: &[Cplx]) -> Option<Acquisition> {
        let mut result = None;
        let fft_len = self.fft.len();
        for &s in samples {
            if self.history.len() == fft_len {
                self.history.pop_front();
            }
            self.history.push_back(s);
            self.since_last += 1;
            if self.history.len() == fft_len && self.since_last >= hop_at(self.rate) {
                self.since_last = 0;
                self.add_periodogram();
                if self.psds.len() == NUM_AVERAGE
                    && let Some(r) = self.detect()
                {
                    result = Some(r);
                }
            }
        }
        result
    }

    fn add_periodogram(&mut self) {
        for (i, (w, s)) in self.window.iter().zip(&self.history).enumerate() {
            self.work[i] = s * *w;
        }
        self.fft.forward(&mut self.work);
        // Store centred: index j ↔ frequency (j − N/2)·fs/N.
        let n = self.fft.len();
        let half = n / 2;
        let psd: Vec<Real> = (0..n).map(|j| self.work[(j + half) % n].norm_sqr()).collect();
        if self.psds.len() == NUM_AVERAGE
            && let Some(old) = self.psds.pop_front()
        {
            for (acc, v) in self.psd_sum.iter_mut().zip(&old) {
                *acc -= v;
            }
        }
        for (acc, v) in self.psd_sum.iter_mut().zip(&psd) {
            *acc += v;
        }
        self.psds.push_back(psd);
    }

    fn detect(&mut self) -> Option<Acquisition> {
        let n = self.fft.len();
        let psd: Vec<Real> = self.psd_sum.iter().map(|v| v / NUM_AVERAGE as Real).collect();
        // Noise floor: forward and backward one-pole smoothing, averaged.
        let mut lr = vec![0.0; n];
        let mut rl = vec![0.0; n];
        lr[0] = psd[0];
        for i in 1..n {
            lr[i] = (lr[i - 1] - psd[i]) * LAMBDA_FLOOR + psd[i];
        }
        rl[n - 1] = psd[n - 1];
        for i in (0..n - 1).rev() {
            rl[i] = (rl[i + 1] - psd[i]) * LAMBDA_FLOOR + psd[i];
        }
        let norm: Vec<Real> = (0..n)
            .map(|i| {
                let floor = 0.5 * (lr[i] + rl[i]);
                if floor > 0.0 { psd[i] / floor } else { 0.0 }
            })
            .collect();

        let fs = f64::from(self.rate.hz());
        let bin_hz = fs / n as Real;
        let half = (n / 2) as isize;
        let lo = ((self.search.min_hz / bin_hz).floor() as isize + half).max(0) as usize;
        let hi = ((self.search.max_hz / bin_hz).ceil() as isize + half).min(n as isize - 1) as usize;

        let mut best: Option<(usize, bool, Real)> = None;
        let orientations: &[bool] = if self.allow_inverted { &[false, true] } else { &[false] };
        for &inv in orientations {
            for d in lo..=hi {
                let idx = |off: usize| -> Option<usize> {
                    if inv { d.checked_sub(off) } else { (d + off < n).then_some(d + off) }
                };
                let (Some(a), Some(b), Some(c)) = (idx(PILOT_BINS[0]), idx(PILOT_BINS[1]), idx(PILOT_BINS[2])) else {
                    continue;
                };
                let score = norm[a] + norm[b] + norm[c];
                if score <= PEAK_BOUND {
                    continue;
                }
                // Reject single sinusoids: the three pilots must have similar power.
                let mut v = [norm[a], norm[b], norm[c]];
                v.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
                if v[1] / v[2] < MAX_RATIO_HIGH && v[0] / v[2] < MAX_RATIO_LOW {
                    continue;
                }
                if best.is_none_or(|(_, _, s)| score > s) {
                    best = Some((d, inv, score));
                }
            }
        }
        self.last_normalised = norm.clone();
        // Parseval: Σ|X|² = N·Σ|x·w|².
        let power = psd.iter().sum::<Real>() / (n as Real * self.window_energy);
        // Above 48 kHz, also look for a mode E plateau. It is a much stronger feature than
        // the pilot triplet, and it cannot be mistaken for a DRM30 signal (which would have
        // to be 95 kHz wide at this rate, where the widest DRM30 band is 20 kHz).
        if let Some(a) = self.detect_plateau(&psd, power) {
            return Some(a);
        }
        best.map(|(d, inv, score)| Acquisition {
            dc_hz: (d as isize - half) as Real * bin_hz,
            inverted: inv,
            score,
            power,
        })
    }

    /// Find a mode E (DRM+) signal by matching its **spectrum plateau**.
    ///
    /// The pilot search cannot find DRM+ at all — mode E defines no frequency reference
    /// cells (§8.4.2), so the three bins it looks in are empty in a DRM+ signal. What a
    /// mode E signal does have is 213 carriers over a flat ±47.11 kHz with sharp edges, so
    /// this slides a window of exactly that width along the spectrum and keeps the position
    /// where the power inside most exceeds the power outside. That is the "sharp ascend and
    /// descend about 98 kHz apart" of the signal, and being a matched filter it needs only
    /// that the band be *relatively* stronger than its surroundings — no absolute
    /// threshold, which is what an edge test cannot avoid and what made the noisiest
    /// recordings unfindable.
    ///
    /// The window is the mode's own width, so the peak of the ratio is the carrier
    /// position directly; the spectrum can sit anywhere in a 192 kHz input, which is the
    /// case every real recording here is in.
    ///
    /// This only runs on the **unconjugated** spectrum: mirroring swaps the two edges, and
    /// a symmetric window cannot tell that a mirrored plateau is mirrored, so an inverted
    /// mode E signal is not found here (the pinned path and `flip` cover it).
    fn detect_plateau(&self, psd: &[Real], power: Real) -> Option<Acquisition> {
        let n = psd.len();
        if self.rate == WorkingRate::R48 {
            return None;
        }
        let fs = f64::from(self.rate.hz());
        let bin_hz = fs / n as Real;
        let half = (n / 2) as isize;
        let lo = (((self.search.min_hz / bin_hz).floor() as isize) + half).max(0) as usize;
        let hi = (((self.search.max_hz / bin_hz).ceil() as isize) + half).min(n as isize - 1) as usize;
        // The ±47.11 kHz span: 212 carrier spacings either side of the DC carrier.
        let span_hz = 2.0 * 106.0 * RobustnessMode::E.carrier_spacing_at(self.rate);
        let w = (span_hz / bin_hz).round() as usize;
        if hi <= lo || w < MIN_BAND_BINS || w >= hi - lo {
            return None;
        }
        // Prefix sums: the band's power at every window position in one pass.
        let mut prefix = vec![0.0; hi - lo + 2];
        for i in lo..=hi {
            prefix[i - lo + 1] = prefix[i - lo] + psd[i];
        }
        let total = prefix[hi - lo + 1];
        let inside_at = |start: usize| -> Real {
            prefix[start - lo + w] - prefix[start - lo]
        };
        // The window slides over the searched span, computing the ratio of mean power
        // inside it to mean power outside.
        let score_at = |start: usize| -> Real {
            let inside = inside_at(start);
            let outside = total - inside;
            if outside > 0.0 { (inside / w as Real) / (outside / (hi - lo + 1 - w) as Real) } else { 0.0 }
        };
        let mut best: Real = 0.0;
        for start in lo..=(hi + 1 - w) {
            best = best.max(score_at(start));
        }
        if best < MIN_BAND_RATIO {
            return None;
        }
        // Where the band is, to sub-bin precision. The ratio's own peak is flat — the band
        // is as wide as the window — so its argmax wanders over tens of bins; the band's two
        // *edges* are far better conditioned. Fall back to the crest centroid below if the
        // edges cannot be found (a search span too narrow for the kernel's margins).
        let (mut first, mut last) = (usize::MAX, 0usize);
        for start in lo..=(hi + 1 - w) {
            if score_at(start) >= CREST * best {
                first = first.min(start);
                last = last.max(start);
            }
        }
        let centroid = if first != usize::MAX {
            let (mut num, mut den) = (0.0 as Real, 0.0 as Real);
            for start in first..=last {
                let weight = score_at(start) - CREST * best;
                num += weight * start as Real;
                den += weight;
            }
            if den > 0.0 { num / den } else { (first + last) as Real / 2.0 }
        } else {
            (lo + hi) as Real / 2.0
        };
        let best_start = centroid as usize;
        let center_bin = match self.edge_centre(psd, lo, hi, bin_hz) {
            Some(c) => c,
            None => centroid + w as Real / 2.0 - 0.5,
        };
        if std::env::var_os("DECDRM_ACQDBG").is_some() {
            eprintln!(
                "ACQ w {w} best {best:.2} span {first}..{last} centroid {centroid:.1} \
                 edge-centre {center_bin:.2}"
            );
        }
        let dc = (center_bin - half as Real) * bin_hz;
        if dc < self.search.min_hz || dc > self.search.max_hz {
            return None;
        }
        let score = inside_at(best_start) / w as Real;
        Some(Acquisition { dc_hz: dc, inverted: false, score, power })
    }

    /// The band's centre from its two edges, as a fractional bin index into `psd`.
    ///
    /// A band edge is a step in the spectrum, and an antisymmetric kernel (the derivative of
    /// a Gaussian) turns a step into an extremum whose slope is as steep as the smoothing
    /// allows — the position is then well conditioned, unlike the flat top of the band-ratio
    /// the detector maximises. The kernel is the same at both edges, so a symmetric
    /// transmit-filter rolloff cancels in their midpoint, and the midpoint is the carrier.
    ///
    /// Measured on the 13 reference recordings (10 windows each, 0.47 s): a standard
    /// deviation of 4.5–14 Hz and a bias under 5 Hz, against 14–62 Hz and up to 70 Hz for
    /// the crest centroid this replaces.
    fn edge_centre(&self, psd: &[Real], lo: usize, hi: usize, bin_hz: Real) -> Option<Real> {
        // Smooth over one carrier spacing (57 bins at the 7.8125 Hz bin every rate keeps):
        // the subcarrier ripple has that period, so this averages it out and leaves the band
        // edge, which is a few tens of Hz wide.
        let spacing_bins = (RobustnessMode::E.carrier_spacing_at(self.rate) / bin_hz).round() as usize;
        let smooth = spacing_bins.max(3) | 1;
        let sigma = (smooth as Real / 4.0).max(2.0);
        let radius = (4.0 * sigma).ceil() as usize;
        let margin = radius + smooth / 2 + 1;
        if hi <= lo + 2 * margin {
            return None;
        }
        let mut prefix = vec![0.0; psd.len() + 1];
        for (i, &v) in psd.iter().enumerate() {
            prefix[i + 1] = prefix[i] + v;
        }
        let mean_at = |i: usize| -> Real {
            let a = i.saturating_sub(smooth / 2);
            let b = (i + smooth / 2 + 1).min(psd.len());
            let sum = prefix[b] - prefix[a];
            if b > a { sum / (b - a) as Real } else { 0.0 }
        };
        let smoothed: Vec<Real> = (lo..=hi).map(mean_at).collect();
        // Antisymmetric kernel: positive where the power rises, negative where it falls. Its
        // normalisation does not affect the position (both extrema use the same kernel).
        let kern: Vec<Real> = (0..=2 * radius)
            .map(|j| {
                let x = j as Real - radius as Real;
                x * (-x * x / (2.0 * sigma * sigma)).exp()
            })
            .collect();
        let len = smoothed.len();
        let mut d = vec![0.0; len];
        let (mut kr, mut kv) = (0usize, Real::NEG_INFINITY);
        let (mut kf, mut low) = (0usize, Real::INFINITY);
        for k in margin..len - margin {
            let mut v = 0.0;
            for (j, &kj) in kern.iter().enumerate() {
                v += kj * smoothed[k + j - radius];
            }
            d[k] = v;
            if v > kv {
                (kr, kv) = (k, v);
            }
            if v < low {
                (kf, low) = (k, v);
            }
        }
        if kr >= kf || kv <= 0.0 || low >= 0.0 {
            return None;
        }
        // Three-point parabola through each extremum: a fraction of a bin, which at 7.8 Hz a
        // bin is a few percent of a carrier spacing.
        let refine = |k: usize| -> Real {
            let (y0, y1, y2) = (d[k - 1], d[k], d[k + 1]);
            let den = y0 - 2.0 * y1 + y2;
            if den != 0.0 { k as Real + 0.5 * (y0 - y2) / den } else { k as Real }
        };
        Some(0.5 * (refine(kr) + refine(kf)) + lo as Real)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mode E band `width_hz` wide at `centre_hz` on a floor 20 dB lower, with a linear
    /// rolloff `roll_hz` at each edge and a deterministic ±10 % ripple, as a stand-in for
    /// the subcarrier ripple of a real signal.
    fn band_psd(n: usize, rate: WorkingRate, centre_hz: Real, width_hz: Real, roll_hz: Real) -> Vec<Real> {
        let bin = f64::from(rate.hz()) / n as Real;
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut ripple = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            0.9 + 0.2 * ((seed >> 11) as Real / (1u64 << 53) as Real)
        };
        (0..n)
            .map(|i| {
                let f = (i as isize - (n / 2) as isize) as Real * bin;
                let d = (f - centre_hz).abs();
                let half = width_hz / 2.0;
                let inside = if d <= half - roll_hz {
                    1.0
                } else if d >= half {
                    0.0
                } else {
                    1.0 - (d - (half - roll_hz)) / roll_hz
                };
                ripple() * (0.01 + inside)
            })
            .collect()
    }

    /// The band's two edges locate the carrier to a few Hz — a band narrower by 100 Hz or a
    /// centre off by 100 Hz is a different, wrong answer, and would cost mode E most of its
    /// multiplex frames (it has no pilots to refine the estimate with).
    #[test]
    fn the_edge_centre_locates_the_band_within_a_few_hz() {
        let rate = WorkingRate::R192;
        let acq = FreqAcquisition::new_at(false, false, rate);
        let n = fft_len_at(rate);
        let bin = f64::from(rate.hz()) / n as Real;
        let half = (n / 2) as isize;
        let (lo, hi) = ((half - 12_000) as usize, (half + 12_000) as usize);
        for centre in [0.0, 20_000.0, -15_500.0, 41_000.0] {
            let psd = band_psd(n, rate, centre, 94_700.0, 250.0);
            let c = acq.edge_centre(&psd, lo, hi, bin).expect("the band edges");
            let est = (c - half as Real) * bin;
            assert!((est - centre).abs() < 10.0, "band at {centre}: got {est:.1} Hz");
        }
    }
}
