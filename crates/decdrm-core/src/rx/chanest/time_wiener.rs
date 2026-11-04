//! Wiener interpolation of the channel in time direction at the gain-reference
//! grid (port of Dream's `CTimeWiener`), including the Doppler-spread (σ) estimate.
//!
//! The channel is assumed to have a Gaussian Doppler spectrum:
//! r(τ) = exp(−2π²·σ²·Ts²·τ²), τ in OFDM symbols.

use crate::cellmap::CellMap;
use crate::dsp::{iir1_c, iir1_lambda, levinson, linear_regression_slope};
use crate::params::RobustnessMode;
use crate::{Cplx, Real};
use std::collections::VecDeque;
use std::f64::consts::PI;

const SIGMA_TAPS: usize = 3;
const TICONST_TI_CORREL_EST: Real = 60.0;
const SIGMA_OVERESTIMATION: Real = 3.0;
const LOW_BOUND_SIGMA: Real = 0.1 / 2.0;
const INIT_SNR_DB: Real = 25.0;

fn params(mode: RobustnessMode) -> (usize, Real) {
    // (filter length in pilots, maximum σ in Hz)
    match mode {
        RobustnessMode::A => (5, 1.6 / 2.0),
        RobustnessMode::B => (7, 2.7 / 2.0),
        RobustnessMode::C => (9, 5.7 / 2.0),
        RobustnessMode::D => (9, 4.5 / 2.0),
        // Mode E (DRM+): measured, not inherited. A car on Band III at 100 km/h sees 18.5 Hz
        // and at 200 km/h 37 Hz of Doppler, an order of magnitude above the modes C/D weights
        // this used to borrow (σ = 2.85 Hz). Swept against COST 207 TU6 at 200 MHz
        // (`examples/modee_sweep.rs`, 50-frame runs at 20/10/6 dB): the 3-tap/12 Hz pair
        // decodes every multiplex frame at 100 km/h and 200 km/h, where the placeholder got
        // 22/36 and 0/36 at 20 dB, and it also beats it parked (48 FAC against 47), so there
        // is no static-reception trade. The short filter is what tracks a 37 Hz fade; σ = 12
        // is the widest the sweep tried and still improved, so a faster receiver may want
        // more (see the sweep for the next cell).
        RobustnessMode::E => (3, 12.0),
    }
}

#[derive(Debug, Clone, Copy)]
struct PilotSample {
    h: Cplx,
    /// Cumulative timing shift at the time the pilot was received.
    cum_shift: i64,
}

#[derive(Debug)]
pub struct TimeWiener {
    len: usize,
    t: usize,
    /// Output delay in symbols.
    pub delay: usize,
    ts: Real,
    sigma_max: Real,
    sigma: Real,
    /// Per grid carrier, the last `len` pilot estimates (front = newest).
    hist: Vec<VecDeque<PilotSample>>,
    /// Whether a grid carrier has received its first pilot yet.
    seeded: Vec<bool>,
    /// Filter taps per phase.
    filters: Vec<Vec<Real>>,
    mmse: Real,
    ticorr: [Cplx; SIGMA_TAPS],
    lambda_ticorr: Real,
    pub tracking: bool,
    update_cnt: usize,
    snr_acc: Real,
    snr_cnt: usize,
    ns: usize,
    /// First frame symbol with a gain reference on carrier offset 0.
    s0: usize,
    /// FFT size at the working rate: the denominator of the timing-shift phase rotation.
    fft_size: usize,
}

impl TimeWiener {
    pub fn new(map: &CellMap) -> Self {
        Self::new_at(map, crate::params::WorkingRate::R48)
    }

    /// [`Self::new`] at an explicit working sample rate (mode E, DRM+, needs 120 or 192 kHz).
    ///
    /// The rate matters because a timing shift (`dshift`, in working-rate samples) is turned
    /// into a per-carrier phase rotation with the **FFT size at that rate** as the
    /// denominator: using the 48 kHz size at 192 kHz rotates every estimate 4x too far.
    pub fn new_at(map: &CellMap, rate: crate::params::WorkingRate) -> Self {
        Self::new_with(map, rate, super::ChanEstTuning::default())
    }

    /// [`Self::new_at`] with the filter length and Doppler σ overridden (for tuning).
    pub fn new_with(
        map: &CellMap,
        rate: crate::params::WorkingRate,
        tuning: super::ChanEstTuning,
    ) -> Self {
        let mode = map.mode();
        let (default_len, default_sigma) = params(mode);
        let len = tuning.time_len.unwrap_or(default_len);
        let sigma_max = tuning.time_sigma_hz.unwrap_or(default_sigma);
        let t = map.scattered.time_int;
        let delay = ((len * t - t + 1) as Real / 2.0).ceil() as usize + t / 2 - 1;
        let ts = mode.symbol_len_at(rate) as Real / Real::from(rate.hz());
        let fft_size = mode.fft_size_at(rate);
        let grid_len = (map.num_carriers - 1) / map.scattered.freq_int + 1;
        let s0 = (0..map.symbols_per_frame).find(|&s| map.cell(s, 0).is_scattered()).unwrap_or(0);
        let num_pil_one_sym = grid_len.div_ceil(t);
        let lambda_ticorr = iir1_lambda(TICONST_TI_CORREL_EST * num_pil_one_sym as Real, 1.0 / ts);
        let init = PilotSample { h: Cplx::new(1.0, 0.0), cum_shift: 0 };
        let mut w = Self {
            len,
            t,
            delay,
            ts,
            fft_size,
            sigma_max,
            sigma: sigma_max,
            hist: vec![VecDeque::from(vec![init; len]); grid_len],
            seeded: vec![false; grid_len],
            filters: vec![vec![0.0; len]; t],
            mmse: 1.0,
            ticorr: [Cplx::new(0.0, 0.0); SIGMA_TAPS],
            lambda_ticorr,
            tracking: false,
            update_cnt: map.symbols_per_frame,
            snr_acc: 0.0,
            snr_cnt: 0,
            ns: map.symbols_per_frame,
            s0,
        };
        w.mmse = w.update_filters(10f64.powf(INIT_SNR_DB / 10.0), sigma_max);
        w
    }

    /// Doppler spread estimate σ in Hz (two-sided spread is 2σ).
    pub fn sigma(&self) -> Real {
        self.sigma
    }

    fn update_filters(&mut self, snr: Real, sigma: Real) -> Real {
        let fac = -2.0 * PI * PI * self.ts * self.ts * sigma * sigma;
        let r = |tau: i64| (fac * (tau * tau) as Real).exp();
        let mut mmse = 0.0;
        for phase in 0..self.t {
            // Pilot j sits at (delay − phase − j·t) symbols from the output symbol.
            let rhp: Vec<Real> = (0..self.len)
                .map(|j| r(self.delay as i64 - phase as i64 - (j * self.t) as i64))
                .collect();
            let mut rpp: Vec<Real> = (0..self.len).map(|j| r((j * self.t) as i64)).collect();
            rpp[0] += 1.0 / snr;
            let taps = levinson(&rpp, &rhp);
            mmse += 1.0 - rhp.iter().zip(&taps).map(|(a, b)| a * b).sum::<Real>();
            self.filters[phase] = taps;
        }
        mmse / self.t as Real
    }

    /// Process one OFDM symbol.
    ///
    /// * `sym` — received carriers of the current symbol (frame symbol `s`).
    /// * `cum_shift` / `out_cum_shift` — cumulative timing shift of the current
    ///   symbol and of the (delayed) output symbol.
    /// * `snr_pilots` — current SNR estimate on the pilots (linear).
    ///
    /// Writes the channel estimate at every grid carrier for the output symbol and
    /// returns the SNR improvement factor of the interpolation (1/MMSE).
    #[allow(clippy::too_many_arguments)]
    pub fn estimate(
        &mut self,
        map: &CellMap,
        sym: &[Cplx],
        sf_sym: usize,
        cum_shift: i64,
        out_cum_shift: i64,
        snr_pilots: Real,
        out: &mut [Cplx],
    ) -> Real {
        let x = map.scattered.freq_int;
        // The timing shift is in working-rate samples, so the rotation denominator is the
        // FFT size at that rate (NOT the mode's 48 kHz size).
        let n = self.fft_size as Real;
        let cells = map.symbol_cells(sf_sym);
        let pilots = map.symbol_pilots(sf_sym);
        let rot = |h: Cplx, c: usize, dshift: i64| -> Cplx {
            // Every mode rotates by the timing shift. Mode E used to skip it, because the
            // shift the loop produced for it was not a real sub-guard drift but the residue
            // of a timing search coarser than its guard interval (`timesync::step_for`);
            // with that fixed the rotation is what A-D have always used it for.
            if dshift == 0 {
                h
            } else {
                let k = (map.kmin + c as i32) as Real;
                h * Cplx::from_polar(1.0, 2.0 * PI * k * dshift as Real / n)
            }
        };
        // The scattered grid repeats with period `t` **symbols within a frame**, and `s0`
        // is a frame-relative symbol, so the filter phase must use the frame-relative
        // symbol. `sf_sym` is a super-frame index (only the map rows need that).
        let s = sf_sym % self.ns;
        for (p, hist) in self.hist.iter_mut().enumerate() {
            let c = p * x;
            if c >= map.num_carriers {
                break;
            }
            if cells[c].is_scattered() {
                let sample = PilotSample { h: sym[c] / pilots[c], cum_shift };
                if self.seeded[p] {
                    hist.pop_back();
                    hist.push_front(sample);
                } else {
                    // Dream starts from h = 1, which is harmless with its raw 16-bit
                    // scale but dominates with our normalised input; start from the
                    // first measurement instead.
                    hist.iter_mut().for_each(|h| *h = sample);
                    self.seeded[p] = true;
                }
                // Time correlation estimate for the Doppler estimate.
                let newest = hist[0].h;
                for j in 0..SIGMA_TAPS.min(self.len) {
                    let old = rot(hist[j].h, c, cum_shift - hist[j].cum_shift);
                    iir1_c(&mut self.ticorr[j], newest.conj() * old, self.lambda_ticorr);
                }
            }
            if cells[c].is_dc() {
                out[p] = Cplx::new(0.0, 0.0);
                continue;
            }
            let phase = (s + self.t * self.ns - self.s0 - p % self.t) % self.t;
            let taps = &self.filters[phase];
            let mut acc = Cplx::new(0.0, 0.0);
            for (j, tap) in taps.iter().enumerate() {
                let ps = hist[j];
                acc += rot(ps.h, c, out_cum_shift - ps.cum_shift) * *tap;
            }
            out[p] = acc;
        }

        if self.tracking {
            if self.update_cnt > 0 {
                self.update_cnt -= 1;
                self.snr_acc += snr_pilots;
                self.snr_cnt += 1;
            } else {
                self.sigma = self.estimate_sigma();
                let s_over = (self.sigma * SIGMA_OVERESTIMATION).min(self.sigma_max);
                let snr = if self.snr_cnt > 0 { self.snr_acc / self.snr_cnt as Real } else { snr_pilots };
                self.mmse = self.update_filters(snr.max(1.0), s_over);
                if 1.0 / self.mmse < snr_pilots {
                    self.mmse = 1.0 / snr_pilots.max(1e-3);
                }
                self.update_cnt = self.ns;
                self.snr_acc = 0.0;
                self.snr_cnt = 0;
            }
        }
        1.0 / self.mmse
    }

    /// Fit |R(τ)| = a·exp(−b·τ²) to the averaged time correlation (Dream's
    /// "modified linear regression") and convert to σ.
    fn estimate_sigma(&self) -> Real {
        let m = SIGMA_TAPS.min(self.len);
        let w: Vec<Real> = (0..m).map(|i| ((i * self.t) as Real).powi(2)).collect();
        let z: Vec<Real> = self.ticorr[..m].iter().map(|c| c.norm().max(1e-30).ln()).collect();
        let a1 = linear_regression_slope(&w, &z);
        let sigma = 0.5 / PI * (-2.0 * a1).max(0.0).sqrt() / self.ts;
        sigma.clamp(LOW_BOUND_SIGMA, self.sigma_max)
    }
}
