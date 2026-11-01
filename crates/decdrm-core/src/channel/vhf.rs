//! VHF propagation models for DRM+ (mode E).
//!
//! ES 201 980 annex B's channel models are HF ones: delays of milliseconds and Doppler
//! spreads of a few hertz, sized for 10–20 kHz shortwave signals. DRM+ runs at VHF — Band
//! I (47–68 MHz), Band II (87.5–108 MHz), Band III (174–230 MHz) — where the same physics
//! gives *much* larger Doppler shifts (a car at 100 km/h sees 9.3 Hz at 100 MHz and
//! 18.5 Hz at 200 MHz) and different delay profiles, and where mode E's guard interval
//! (Tg = 3T = 250 µs) decides what can be equalised at all.
//!
//! The delay profiles here are the COST 207 ones, the common choice in DAB/DVB VHF work:
//! **RA** (rural area, four taps within 0.6 µs) for a receiver in open country and **TU6**
//! (typical urban, six taps over 5 µs) for one in town. Both are simulated with a
//! classical (Jakes) Doppler spectrum per tap, as COST 207 prescribes for mobile
//! reception; pass `speed_kmh = 0` for the static case, where the taps stop fading but the
//! multipath remains.
//!
//! Not included: HT (hilly terrain) and BU (bad urban). Their tables are longer than
//! these two and have not been checked against the source; a measured or standard profile
//! can be passed to [`static_multipath`] in the meantime.
//!
//! Long echoes — the second transmitter of an SFN, or a mountain — are not COST 207 at
//! all: they can reach beyond mode E's guard interval, and [`sfn_echo`] models exactly
//! that case (inside the guard it is ordinary multipath; beyond it, ISI the receiver
//! cannot equalise).

use super::{ChannelModel, DopplerSpectrum, Path};
use crate::Real;

/// Speed of light, m/s (CODATA).
const SPEED_OF_LIGHT: Real = 299_792_458.0;

/// Maximum Doppler shift `f_d = v·f_c/c` of a receiver moving at `speed_kmh` towards the
/// transmitter, Hz.
///
/// Unlike annex B's absolute hertz, a VHF Doppler depends on the carrier frequency: the
/// same speed gives 9.3 Hz at 100 MHz and 18.5 Hz at 200 MHz.
pub fn max_doppler_hz(speed_kmh: Real, carrier_hz: Real) -> Real {
    speed_kmh / 3.6 * carrier_hz / SPEED_OF_LIGHT
}

/// COST 207 rural area (RA): four taps, all within 0.6 µs, so the whole profile sits deep
/// inside mode E's 250 µs guard.
///
/// Taps (delay µs, level dB): (0, 0), (0.2, −2), (0.4, −10), (0.6, −20).
pub fn cost207_ra(speed_kmh: Real, carrier_hz: Real) -> ChannelModel {
    const TAPS: [(Real, Real); 4] = [(0.0, 0.0), (0.2, -2.0), (0.4, -10.0), (0.6, -20.0)];
    profile(&TAPS, speed_kmh, carrier_hz)
}

/// COST 207 typical urban, six taps (TU6): the classic mobile profile, delays out to
/// 5 µs, still only 2 % of mode E's guard.
///
/// Taps (delay µs, level dB): (0, −3), (0.2, 0), (0.5, −2), (1.6, −6), (2.3, −8),
/// (5.0, −10).
pub fn cost207_tu6(speed_kmh: Real, carrier_hz: Real) -> ChannelModel {
    const TAPS: [(Real, Real); 6] =
        [(0.0, -3.0), (0.2, 0.0), (0.5, -2.0), (1.6, -6.0), (2.3, -8.0), (5.0, -10.0)];
    profile(&TAPS, speed_kmh, carrier_hz)
}

/// A static multipath profile given as (delay µs, level dB) pairs — a measured channel
/// response, a hilly-terrain profile, or any COST 207 table not provided above.
///
/// Static means the taps do not fade (their gains stay constant); the delay comb still
/// shapes the spectrum. Include the main path as `(0.0, 0.0)` or any level you like — the
/// model is normalised to unit mean power gain.
pub fn static_multipath(taps: &[(Real, Real)]) -> ChannelModel {
    profile(taps, 0.0, 0.0)
}

/// Two paths, the second delayed by `delay_us` at `level_db`: an SFN transmitter or one
/// strong reflection. Static, and combined with mode E's 250 µs guard it splits into the
/// two regimes that matter — inside the guard (flat fading, equalisable) and beyond it
/// (ISI).
pub fn sfn_echo(delay_us: Real, level_db: Real) -> ChannelModel {
    static_multipath(&[(0.0, 0.0), (delay_us, level_db)])
}

/// Shared construction: COST 207-style taps, every one fading with a Jakes spectrum at
/// `f_d = v·f_c/c` (none at zero speed).
fn profile(taps: &[(Real, Real)], speed_kmh: Real, carrier_hz: Real) -> ChannelModel {
    let spread = 2.0 * max_doppler_hz(speed_kmh, carrier_hz);
    ChannelModel {
        paths: taps
            .iter()
            .map(|&(delay_us, level_db)| Path {
                delay_ms: delay_us / 1000.0,
                gain: 10f64.powf(level_db / 20.0),
                doppler_shift_hz: 0.0,
                doppler_spread_hz: spread,
                spectrum: DopplerSpectrum::Jakes,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ChannelLayout, RobustnessMode, SpectrumOccupancy, WorkingRate};
    use crate::channel::{ChannelConfig, ChannelSimulator, Rng};

    /// Mode E at 192 kHz, its usual working rate.
    fn mode_e_192() -> ChannelLayout {
        ChannelLayout::new(RobustnessMode::E, SpectrumOccupancy::SO_0).unwrap()
    }

    fn sim(model: ChannelModel, rate: WorkingRate) -> ChannelSimulator {
        ChannelSimulator::new_at(mode_e_192(), rate, ChannelConfig { model, ..Default::default() })
    }

    /// A car's Doppler shift is a fraction of a hertz per km/h at Band II and grows with
    /// the band: 100 km/h is 9.3 Hz at 100 MHz, 18.5 Hz at 200 MHz, 37 Hz at 400 MHz.
    #[test]
    fn max_doppler_scales_with_speed_and_band() {
        assert!((max_doppler_hz(0.0, 200e6) - 0.0).abs() < 1e-12);
        assert!((max_doppler_hz(100.0, 100e6) - 9.265_7).abs() < 1e-3, "{}", max_doppler_hz(100.0, 100e6));
        assert!((max_doppler_hz(100.0, 200e6) - 18.531).abs() < 1e-2);
        assert!((max_doppler_hz(200.0, 200e6) - 37.063).abs() < 1e-2);
        // Linear in speed: twice the speed, twice the shift.
        assert!((max_doppler_hz(200.0, 100e6) - 2.0 * max_doppler_hz(100.0, 100e6)).abs() < 1e-9);
    }

    /// TU6's taps are *sub-sample* at DRM+ rates — 0.2 µs is 0.04 samples at 192 kHz — so
    /// they are kept exactly by the fractional-delay interpolator rather than rounded to
    /// nothing. At 120 kHz every delay is 5/8 of its 192 kHz value.
    #[test]
    fn tu6_taps_keep_their_sub_sample_delays() {
        let s = sim(cost207_tu6(0.0, 200e6), WorkingRate::R192);
        let exact = s.path_delays_exact();
        let want = [0.0, 0.0384, 0.0960, 0.3072, 0.4416, 0.9600];
        for (got, want) in exact.iter().zip(want) {
            assert!((got - want).abs() < 1e-9, "{exact:?} vs {want:?}");
        }
        // Rounded for reporting, the first five taps are below one sample: this is why
        // rounding the delay instead of interpolating it would erase the profile.
        assert_eq!(s.path_delays(), vec![0, 0, 0, 0, 0, 1]);
        let s120 = sim(cost207_tu6(0.0, 200e6), WorkingRate::R120);
        let exact120 = s120.path_delays_exact();
        let want120 = [0.0, 0.024, 0.06, 0.192, 0.276, 0.6];
        for (got, want) in exact120.iter().zip(want120) {
            assert!((got - want).abs() < 1e-9, "{exact120:?}");
        }
    }

    /// At zero speed nothing fades: a static profile has constant taps, so its gain is
    /// the same on every sample (the delay comb, not Rayleigh).
    #[test]
    fn static_profiles_do_not_fade() {
        for model in [cost207_ra(0.0, 200e6), cost207_tu6(0.0, 200e6), sfn_echo(300.0, 0.0)] {
            assert!(model.paths.iter().all(|p| p.doppler_spread_hz == 0.0), "{model:?}");
            assert!(model.paths.iter().all(|p| p.doppler_shift_hz == 0.0));
        }
    }

    /// In motion the taps do fade, with unit mean power gain (the model is normalised by
    /// Σgain²), and the power varies from block to block — that is Rayleigh.
    #[test]
    fn tu6_fades_when_moving() {
        let cfg = ChannelConfig { model: cost207_tu6(100.0, 200e6), seed: 11, ..Default::default() };
        let mut s = ChannelSimulator::new_at(mode_e_192(), WorkingRate::R192, cfg);
        let mut rng = Rng::new(5);
        let x: Vec<_> = (0..192_000 * 4).map(|_| rng.complex_gaussian()).collect();
        let mut y = Vec::new();
        s.process(&x, &mut y);
        let p = y.iter().map(|v| v.norm_sqr()).sum::<Real>() / y.len() as Real;
        assert!((p - 1.0).abs() < 0.15, "mean power gain {p}");
        let blocks: Vec<Real> =
            y.chunks(19_200).map(|b| b.iter().map(|v| v.norm_sqr()).sum::<Real>() / b.len() as Real).collect();
        let (min, max) = blocks.iter().fold((Real::INFINITY, 0.0_f64), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(max / min > 1.5, "block powers do not fade: {min}..{max}");
    }

    /// An SFN echo is a single delayed copy; 300 µs is beyond mode E's 250 µs guard, which
    /// is the case this profile exists to test.
    #[test]
    fn sfn_echo_models_beyond_guard_delays() {
        let m = sfn_echo(300.0, -6.0);
        assert_eq!(m.paths.len(), 2);
        assert!((m.paths[1].delay_ms - 0.3).abs() < 1e-12);
        assert!((m.paths[1].gain - 10f64.powf(-6.0 / 20.0)).abs() < 1e-12);
        let s = sim(m, WorkingRate::R192);
        assert_eq!(s.path_delays_exact()[1], 57.6, "300 µs at 192 kHz");
        assert_eq!(s.path_delays(), vec![0, 58]);
    }
}
