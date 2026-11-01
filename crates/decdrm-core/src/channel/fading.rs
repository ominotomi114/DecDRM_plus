//! Rayleigh-fading tap gains, with either the Gaussian Doppler spectrum of the Watterson
//! model (ES 201 980 annex B) or the classical Jakes/Clarke spectrum of a moving
//! receiver (the VHF models; see [`super::vhf`]).
//!
//! Same construction as Dream's `CTapgain` (`drmchannel/ChannelSimulation.cpp`):
//! complex white Gaussian noise at a low update rate is shaped by a Gaussian FIR
//! filter and interpolated up to the sample rate. Dream picks one of several
//! polyphase/linear interpolation factors; here the update rate is simply fixed at
//! [`OVERSAMPLING`]·σ and the gain is linearly interpolated between updates.

use super::rng::Rng;
use super::wrap;
use crate::{Cplx, Real};
use std::f64::consts::PI;

/// Update rate of the filtered noise in units of the Doppler σ. At 32σ the Gaussian
/// spectrum is negligible (−500 dB) at the update Nyquist frequency and linear
/// interpolation between updates is essentially exact.
pub const OVERSAMPLING: Real = 32.0;

/// Gaussian filter half-length in units of its time-domain standard deviation.
const HALF_LENGTH_SIGMAS: Real = 4.5;

/// Generator of one fading tap: a circularly symmetric complex Gaussian process with
/// unit mean power and Doppler power spectrum `S(f) ∝ exp(−f²/(2σ²))`, where the
/// Doppler spread (the annex B "fd" column) is `2σ`.
#[derive(Debug, Clone)]
pub struct GaussianFading {
    /// Gaussian shaping filter at the update rate, normalised to Σh² = 1.
    taps: Vec<Real>,
    /// Ring buffer of white noise at the update rate (same length as `taps`).
    noise: Vec<Cplx>,
    pos: usize,
    /// Samples between filter updates.
    step: usize,
    /// Samples elapsed since the last update.
    phase: usize,
    prev: Cplx,
    next: Cplx,
    rng: Rng,
}

impl GaussianFading {
    /// `spread_hz`: Doppler spread 2σ in Hz (must be > 0); `fs`: sample rate.
    pub fn new(spread_hz: Real, fs: Real, rng: Rng) -> Self {
        assert!(spread_hz > 0.0, "a fading tap needs a positive Doppler spread");
        let sigma = spread_hz / 2.0;
        let step = ((fs / (OVERSAMPLING * sigma)).round() as usize).max(1);
        let update_rate = fs / step as Real;
        // A Gaussian impulse response exp(−n²/(2τ²)) has the power response
        // exp(−ν²/(2σₙ²)) with σₙ = 1/(2√2·π·τ) (ν in cycles per update).
        let sigma_n = sigma / update_rate;
        let tau = 1.0 / (2.0 * std::f64::consts::SQRT_2 * PI * sigma_n);
        let half = (HALF_LENGTH_SIGMAS * tau).ceil() as usize;
        let mut taps: Vec<Real> = (0..=2 * half)
            .map(|i| {
                let t = i as Real - half as Real;
                (-t * t / (2.0 * tau * tau)).exp()
            })
            .collect();
        let norm = taps.iter().map(|h| h * h).sum::<Real>().sqrt();
        for h in &mut taps {
            *h /= norm;
        }
        let mut rng = rng;
        // Pre-fill the filter memory so the process is stationary from sample 0.
        let noise: Vec<Cplx> = (0..taps.len()).map(|_| rng.complex_gaussian()).collect();
        let mut g = Self { taps, noise, pos: 0, step, phase: 0, prev: Cplx::new(0.0, 0.0), next: Cplx::new(0.0, 0.0), rng };
        g.next = g.filter_output();
        g.advance();
        g
    }

    /// Samples between two updates of the underlying filtered-noise process.
    pub fn update_interval(&self) -> usize {
        self.step
    }

    fn filter_output(&self) -> Cplx {
        let n = self.noise.len();
        // Symmetric taps, so the direction in which the ring buffer is read does not
        // matter.
        self.taps.iter().enumerate().map(|(k, &h)| self.noise[(self.pos + k) % n] * h).sum()
    }

    fn advance(&mut self) {
        self.prev = self.next;
        self.noise[self.pos] = self.rng.complex_gaussian();
        self.pos = (self.pos + 1) % self.noise.len();
        self.next = self.filter_output();
    }

    /// Gain for the next sample.
    #[inline]
    pub fn next_gain(&mut self) -> Cplx {
        let t = self.phase as Real / self.step as Real;
        let g = self.prev + (self.next - self.prev) * t;
        self.phase += 1;
        if self.phase == self.step {
            self.phase = 0;
            self.advance();
        }
        g
    }
}

/// Classical (Jakes/Clarke) Rayleigh fading: the tap of a receiver moving through a field
/// of scattered waves, with maximum Doppler shift `f_d` on the direction of travel.
///
/// Jakes' sum-of-sinusoids construction: sinusoid `n` arrives from the angle
/// `θ_n = 2πn/N` and so carries the frequency `f_d·cos θ_n`. With random phases this is a
/// circularly symmetric complex Gaussian process of unit mean power whose Doppler
/// spectrum is U-shaped, `S(f) = 1/(π f_d √(1 − (f/f_d)²))`, and whose autocorrelation is
/// `J₀(2π f_d τ)` — the model a car's receiver is normally simulated with (COST 207 uses
/// it for every tap of its mobile profiles).
///
/// As in [`GaussianFading`], the process is evaluated at [`OVERSAMPLING`]·`f_d` and
/// linearly interpolated between updates, so the per-sample cost does not grow with the
/// number of sinusoids (only the per-update cost does, and updates are rare).
#[derive(Debug, Clone)]
pub struct JakesFading {
    /// Phase increment of each sinusoid per update, radians.
    steps: Vec<Real>,
    /// Phase of each sinusoid at the current update.
    phases: Vec<Real>,
    /// 1/√N, so N unit phasors have mean power 1.
    norm: Real,
    /// Samples between process updates.
    step: usize,
    /// Samples elapsed since the last update.
    phase: usize,
    prev: Cplx,
    next: Cplx,
}

impl JakesFading {
    /// Sinusoids in the sum. Jakes' original construction uses 8 plus one at the maximum
    /// shift; 32 random angles give a smoother spectrum at negligible cost here, since
    /// they are only evaluated once per update.
    const SINUSOIDS: usize = 32;

    /// `max_doppler_hz` is `f_d = v·f_c/c` (must be > 0); `fs`: sample rate.
    pub fn new(max_doppler_hz: Real, fs: Real, mut rng: Rng) -> Self {
        assert!(max_doppler_hz > 0.0, "a Jakes tap needs a positive maximum Doppler shift");
        let step = ((fs / (OVERSAMPLING * max_doppler_hz)).round() as usize).max(1);
        let update_rate = fs / step as Real;
        let n = Self::SINUSOIDS as Real;
        // Angles of arrival drawn at random rather than on a fixed grid: a grid repeats
        // `cos θ` for θ and 2π−θ, and the two equal-frequency sinusoids would beat
        // against each other instead of averaging out, biasing the tap's power. Random
        // angles average to the classical spectrum (E[cos(2π f_d cos θ τ)] = J₀(2π f_d τ))
        // with no repeated frequency.
        let steps: Vec<Real> = (0..Self::SINUSOIDS)
            .map(|_| 2.0 * PI * max_doppler_hz * (2.0 * PI * rng.uniform()).cos() / update_rate)
            .collect();
        let phases: Vec<Real> = (0..Self::SINUSOIDS).map(|_| 2.0 * PI * rng.uniform()).collect();
        let mut f = Self { steps, phases, norm: n.sqrt().recip(), step, phase: 0, prev: Cplx::new(0.0, 0.0), next: Cplx::new(0.0, 0.0) };
        f.next = f.sum();
        f.advance();
        f
    }

    /// Sum of the sinusoids at their current phases.
    fn sum(&self) -> Cplx {
        let mut acc = Cplx::new(0.0, 0.0);
        for &p in &self.phases {
            acc += Cplx::from_polar(1.0, p);
        }
        acc * self.norm
    }

    fn advance(&mut self) {
        self.prev = self.next;
        for (p, s) in self.phases.iter_mut().zip(&self.steps) {
            *p = wrap(*p + *s);
        }
        self.next = self.sum();
    }

    /// Samples between two updates of the underlying sinusoid phases.
    pub fn update_interval(&self) -> usize {
        self.step
    }

    /// Gain for the next sample.
    #[inline]
    pub fn next_gain(&mut self) -> Cplx {
        let t = self.phase as Real / self.step as Real;
        let g = self.prev + (self.next - self.prev) * t;
        self.phase += 1;
        if self.phase == self.step {
            self.phase = 0;
            self.advance();
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean power ≈ 1 and the autocorrelation of a Gaussian Doppler spectrum with
    /// σ = spread/2: R(t) = exp(−2π²σ²t²).
    #[test]
    fn unit_power_and_gaussian_doppler_width() {
        let fs = 400.0; // low rate so a long observation stays cheap
        let spread = 2.0; // Hz → σ = 1 Hz
        let mut f = GaussianFading::new(spread, fs, Rng::new(5));
        let n = 400_000;
        let x: Vec<Cplx> = (0..n).map(|_| f.next_gain()).collect();
        let r = |lag: usize| -> Real {
            let acc: Cplx = (lag..n).map(|i| x[i] * x[i - lag].conj()).sum();
            acc.re / (n - lag) as Real
        };
        let r0 = r(0);
        assert!((r0 - 1.0).abs() < 0.1, "mean power {r0}");
        for lag in [20usize, 50, 100] {
            let t = lag as Real / fs;
            let want = (-2.0 * PI * PI * t * t).exp();
            let got = r(lag) / r0;
            assert!((got - want).abs() < 0.05, "lag {t} s: correlation {got:.3}, expected {want:.3}");
        }
    }

    #[test]
    fn slow_fading_uses_long_update_interval() {
        let f = GaussianFading::new(0.1, 48_000.0, Rng::new(1));
        assert_eq!(f.update_interval(), 30_000);
    }

    /// Bessel J₀ by its power series, accurate for the arguments used below.
    fn bessel_j0(x: Real) -> Real {
        let (mut term, mut sum, mut k) = (1.0, 0.0, 0.0);
        loop {
            sum += term;
            k += 1.0;
            term *= -(x * x / 4.0) / (k * k);
            if term.abs() < 1e-12 {
                return sum;
            }
        }
    }

    /// A Jakes tap has unit mean power and the classical autocorrelation
    /// `J₀(2π f_d τ)`, not the Gaussian one — that is what makes a moving receiver fade
    /// the way a moving receiver does.
    #[test]
    fn jakes_power_and_classical_correlation() {
        let fs = 400.0;
        let fd = 2.0;
        let mut f = JakesFading::new(fd, fs, Rng::new(7));
        let n = 400_000;
        let x: Vec<Cplx> = (0..n).map(|_| f.next_gain()).collect();
        let r = |lag: usize| -> Real {
            let acc: Cplx = (lag..n).map(|i| x[i] * x[i - lag].conj()).sum();
            acc.re / (n - lag) as Real
        };
        let r0 = r(0);
        assert!((r0 - 1.0).abs() < 0.1, "mean power {r0}");
        for lag in [10usize, 50, 100] {
            let t = lag as Real / fs;
            let want = bessel_j0(2.0 * PI * fd * t);
            let got = r(lag) / r0;
            assert!((got - want).abs() < 0.15, "lag {t} s: correlation {got:.3}, expected J0 {want:.3}");
        }
    }
}
