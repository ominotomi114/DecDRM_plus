//! Coarse frequency acquisition: find the DRM DC carrier in the input spectrum from
//! the three continuous frequency reference pilots, which sit 750, 2250 and 3000 Hz
//! above DC in every robustness mode (port of Dream's `CFreqSyncAcq`).
//!
//! Improvement over Dream: the mirrored pilot pattern is searched as well, so a
//! spectrally inverted signal (e.g. LSB reception) is detected automatically.

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

/// Fraction of the in-band mean power a bin must exceed to count as occupied.
const OCCUPIED_OVER_FLOOR: Real = 6.0;
/// Percentile of the PSD taken as the noise floor.
const FLOOR_PERCENTILE: Real = 0.2;
/// A run of occupied bins shorter than this is noise, not a signal band.
const MIN_BAND_BINS: usize = 8;
/// Occupied bins of guard beyond the signal's own span that a plateau must show on each
/// side. With the ±47.11 kHz span a mode E signal can sit anywhere in a 192 kHz input, so
/// an edge is only recognisable when there is clear spectrum next to it.
const MIN_OUTSIDE_BINS: usize = 4;

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

    /// Find a mode E (DRM+) signal by its **spectrum plateau**: a wide band of flat
    /// occupied spectrum with sharp edges, and a notch where its DC carrier is.
    ///
    /// The pilot search cannot find DRM+ at all — mode E defines no frequency reference
    /// cells (§8.4.2), so the three bins it looks in are empty in a DRM+ signal. What a
    /// mode E signal does have is 213 carriers over a flat ±47.11 kHz, i.e. `n_carriers`
    /// times the carrier spacing at this rate.
    ///
    /// This only runs on the **unconjugated** spectrum: mirroring swaps the two edges, and
    /// an edge-position test cannot tell that a mirrored plateau is mirrored, so an inverted
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
        if hi <= lo {
            return None;
        }
        // Noise floor: a low percentile of the PSD over the searched span (the one-pole
        // smoother cannot serve: it tracks a 95 kHz plateau as if it were the floor).
        let mut sorted: Vec<Real> = psd[lo..=hi].to_vec();
        sorted.sort_by(Real::total_cmp);
        let floor = sorted[((sorted.len() - 1) as Real * FLOOR_PERCENTILE) as usize];
        if floor <= 0.0 {
            return None;
        }
        let occupied: Vec<bool> = psd[lo..=hi].iter().map(|&v| v > OCCUPIED_OVER_FLOOR * floor).collect();
        if std::env::var_os("DECDRM_ACQDBG").is_some() {
            eprintln!("ACQ n {n} bin {bin_hz:.2} Hz lo {lo} hi {hi} floor {floor:.3e} occ {}/{}",
                      occupied.iter().filter(|&&b| b).count(), occupied.len());
        }
        // Longest run of occupied bins, with the band of guard around it clear.
        let (mut best_len, mut best_start) = (0usize, lo);
        let mut run_start = lo;
        for i in 0..=occupied.len() {
            let here = i < occupied.len() && occupied[i];
            if here {
                if i == 0 || !occupied[i - 1] {
                    run_start = lo + i;
                }
            } else if i > 0 && occupied[i - 1] {
                let len = lo + i - run_start;
                let clear = run_start.saturating_sub(lo) >= MIN_OUTSIDE_BINS
                    && hi + 1 - (lo + i) >= MIN_OUTSIDE_BINS;
                if clear && len > best_len {
                    (best_len, best_start) = (len, run_start);
                }
            }
        }
        let (len, start_bin) = (best_len, best_start);
        if len < MIN_BAND_BINS {
            return None;
        }
        // The span of a mode E signal at this rate, in bins.
        // The ±47.11 kHz span: 212 carrier spacings either side of the DC carrier.
        let span_hz = 2.0 * 106.0 * RobustnessMode::E.carrier_spacing_at(self.rate);
        let want_bins = (span_hz / bin_hz).round() as usize;
        // Accept a plateau that is the signal's own width (within a bin or two of trimming)
        // or wider (a real signal in a wide input, edges partly outside the search span).
        if want_bins == 0 || len + 2 < want_bins {
            return None;
        }
        // The DC carrier is the **middle of the band's edges**, not the left edge plus half
        // the nominal span: both edges smear outward by the same amount (the window and the
        // outermost carriers' own skirts), so their midpoint cancels that where an edge
        // does not. The mode E spectrum is symmetric about its DC carrier by construction.
        //
        // The edges are taken at **half the band's own median power**, which is where a
        // flat plateau's skirt actually starts; the threshold used to find the run is set
        // low so nothing is missed, and that alone puts the midpoint ~1 kHz off.
        let (band_lo, band_hi) = (start_bin, start_bin + len - 1);
        let mut inside: Vec<Real> = psd[band_lo..=band_hi].to_vec();
        inside.sort_by(Real::total_cmp);
        let half_power = 0.5 * inside[inside.len() / 2];
        let left = (band_lo..=band_hi).find(|&i| psd[i] >= half_power).unwrap_or(band_lo);
        let right = (band_lo..=band_hi).rev().find(|&i| psd[i] >= half_power).unwrap_or(band_hi);
        let center_bin = (left + right) as Real / 2.0;
        let dc = (center_bin - half as Real) * bin_hz;
        // The DC carrier must sit inside the search window, or the mixer would be handed a
        // frequency the receiver may not use.
        if dc < self.search.min_hz || dc > self.search.max_hz {
            return None;
        }
        let score = psd[start_bin..start_bin + len].iter().sum::<Real>() / len as Real;
        Some(Acquisition { dc_hz: dc, inverted: false, score, power })
    }
}
