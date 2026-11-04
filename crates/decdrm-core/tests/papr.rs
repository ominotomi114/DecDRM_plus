//! The transmit PAPR limiter (`tx::output::PeakLimit`): what it does to a real DRM signal
//! and that the receiver still decodes it.
//!
//! The limiter is a memoryless `tanh` soft limiter on the baseband, before the transmit
//! channel filter. The numbers here are the ones `examples/papr` prints; the thresholds
//! are loose so that a different seed or an extra carrier does not decide the test.

use decdrm_core::cellmap::CellMap;
use decdrm_core::channel::Rng;
use decdrm_core::dsp::fft::Fft;
use decdrm_core::fac::{ChannelParams, Fac, Interleaving, MscMode, SdcMode, ServiceParams};
use decdrm_core::fec::mlc::MscProtection;
use decdrm_core::params::{ChannelLayout, RobustnessMode, SpectrumOccupancy};
use decdrm_core::rx::{InputFormat, MscConfig, Receiver, ReceiverEvent, ReceiverConfig};
use decdrm_core::tx::output::{OutputConfig, OutputStage, PeakLaw, PeakLimit, apply_peak_limit};
use decdrm_core::tx::{Transmitter, TxConfig};
use decdrm_core::{Cplx, Real};

fn tx_config() -> TxConfig {
    // Mode B, 10 kHz, 64-QAM: the most distortion-sensitive DRM30 configuration.
    TxConfig {
        mode: RobustnessMode::B,
        occupancy: SpectrumOccupancy::SO_3,
        msc_mode: MscMode::Qam64Sm,
        sdc_mode: SdcMode::Qam16,
        interleaving: Interleaving::Short,
        protection: MscProtection { part_a: 0, part_b: 1, hierarchical: 0 },
        part_a_bytes: 0,
        afs_index: 0,
        working_rate: decdrm_core::params::WorkingRate::R48,
    }
}

fn test_fac() -> Fac {
    Fac {
        channel: ChannelParams {
            enhancement: false,
            frame_index: 0,
            afs_valid: false,
            occupancy: SpectrumOccupancy::SO_3,
            interleaving: Interleaving::Short,
            msc_mode: MscMode::Qam64Sm,
            sdc_mode: SdcMode::Qam16,
            num_audio: 1,
            num_data: 0,
            reconfiguration_index: 0,
            toggle: false,
        },
        service: ServiceParams {
            service_id: 0xABCDE,
            short_id: 0,
            audio_ca: false,
            language: 5,
            is_data: false,
            descriptor: 10,
            data_ca: false,
        },
    }
}

/// `frames` frames of baseband, the layout, and the map's mean power per sample.
fn signal(frames: usize) -> (Vec<Cplx>, ChannelLayout, Real) {
    let cfg = tx_config();
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    let layout = tx.layout();
    let mut rng = Rng::new(0x5EED_1234);
    let cap = tx.msc_capacity();
    let mut band = Vec::with_capacity(frames * tx.samples_per_frame());
    for _ in 0..frames {
        let msc = rng.bits(cap.total_bits());
        let sdc = (tx.frame_index() == 0).then(|| rng.bytes(tx.sdc_capacity_bytes()));
        tx.transmit_frame_into(&test_fac(), &msc, sdc.as_deref(), &mut band).expect("transmit");
    }
    let mean_power = CellMap::new(layout.mode, layout.occupancy).expect("layout").avg_power_per_symbol;
    (band, layout, mean_power)
}

fn power(x: &[Cplx]) -> Real {
    x.iter().map(|v| v.norm_sqr()).sum::<Real>() / x.len() as Real
}

/// Peak-to-average ratio, dB.
fn papr_db(x: &[Cplx]) -> Real {
    20.0 * (x.iter().map(|v| v.norm_sqr()).fold(0.0, Real::max).sqrt() / power(x).sqrt()).log10()
}

/// MER (dB) per carrier against `reference`, with a perfect FFT receiver.
fn mer_db(reference: &[Cplx], tested: &[Cplx], layout: ChannelLayout) -> Real {
    let rate = decdrm_core::params::WorkingRate::R48;
    let (n, g) = (layout.mode.fft_size_at(rate), layout.mode.guard_len_at(rate));
    let mut fft = Fft::new(n);
    let (kmin, kmax) = layout.carrier_range();
    let (mut sig, mut err) = (0.0, 0.0);
    for (a, b) in reference.chunks(n + g).zip(tested.chunks(n + g)) {
        if a.len() < n + g || b.len() < n + g {
            continue;
        }
        let (mut ra, mut rb) = (a[g..].to_vec(), b[g..].to_vec());
        fft.forward(&mut ra);
        fft.forward(&mut rb);
        for k in kmin..=kmax {
            let i = k.rem_euclid(n as i32) as usize;
            sig += ra[i].norm_sqr();
            err += (rb[i] - ra[i]).norm_sqr();
        }
    }
    10.0 * (sig / err).log10()
}

/// Both laws: the peak is capped near the knee, the power is restored, and the fraction of
/// samples the limiter touches is the Gaussian tail it predicts (`e^{−γ²}` = 1.8 % at 2σ).
#[test]
fn both_limiter_laws_cap_the_peaks_and_keep_the_power() {
    let (band, _, mean_power) = signal(12);
    let sigma = mean_power.sqrt();
    for limit in [PeakLimit::tanh(6.0), PeakLimit::tanh(9.0), PeakLimit::clip(6.0), PeakLimit::clip(9.0)] {
        let knee = limit.threshold_db;
        let mut y = band.clone();
        let peaks = apply_peak_limit(&mut y, mean_power, limit);
        let gamma = 10f64.powf(knee / 20.0);
        // Power is restored by the Gaussian model: no more than 0.1 dB of error.
        assert!((10.0 * (power(&y) / power(&band)).log10()).abs() < 0.1, "{limit:?}: power changed");
        // No sample exceeds the knee's asymptote (the knee itself scaled by the power
        // normalisation: within 1 % for a clip, a little more for tanh's).
        let peak = y.iter().map(|v| v.norm()).fold(0.0, Real::max);
        let bound = if limit.law == PeakLaw::Tanh { 1.08 } else { 1.02 };
        assert!(peak <= gamma * sigma * bound, "{limit:?}: peak {peak:.1} vs knee {:.1}", gamma * sigma);
        assert!(papr_db(&y) < papr_db(&band) - 1.0, "{limit:?}: no PAPR reduction");
        // The tail above γσ is e^{−γ²} of the samples; a real frame with pilots is close.
        let want = (-gamma * gamma).exp();
        let got = peaks as Real / band.len() as Real;
        assert!(
            (got - want).abs() < (0.005 as Real).max(0.25 * want),
            "{limit:?}: {got:.4} of samples above the knee, want ~{want:.4}"
        );
    }
    // A 6 dB knee is the aggressive end: 4-5 dB off the peak, 1-2 % of samples touched.
    let crest = |limit| {
        let mut y = band.clone();
        apply_peak_limit(&mut y, mean_power, limit);
        papr_db(&y)
    };
    let (tanh, clip) = (crest(PeakLimit::tanh(6.0)), crest(PeakLimit::clip(6.0)));
    assert!((6.5..7.2).contains(&tanh), "tanh 6 dB knee: {tanh:.2} dB crest");
    assert!((5.7..6.4).contains(&clip), "clip 6 dB knee: {clip:.2} dB crest");
}

/// What the reduction costs: in-band distortion, i.e. MER. Measured on the same frame:
/// ~28 dB at a 9 dB knee, ~24 dB at 6 dB with `tanh`; a hard clip at the same knee is
/// ~11 dB better, because it leaves everything below the knee exactly alone.
#[test]
fn tanh_costs_in_band_distortion_and_clip_costs_less() {
    let (band, layout, mean_power) = signal(6);
    let mer = |limit| {
        let mut y = band.clone();
        apply_peak_limit(&mut y, mean_power, limit);
        mer_db(&band, &y, layout)
    };
    let (t9, t6) = (mer(PeakLimit::tanh(9.0)), mer(PeakLimit::tanh(6.0)));
    let (c9, c6) = (mer(PeakLimit::clip(9.0)), mer(PeakLimit::clip(6.0)));
    eprintln!("[papr] MER at a 9 dB / 6 dB knee: tanh {t9:.1} / {t6:.1} dB, clip {c9:.1} / {c6:.1} dB");
    assert!(t9 > 27.0, "9 dB knee: tanh MER {t9:.1} dB");
    assert!(t6 > 22.0, "6 dB knee: tanh MER {t6:.1} dB");
    // More limiting, less MER — the trade is monotone for both laws.
    assert!(t6 < t9 && c6 < c9);
    // The same peak costs much less in band when the limiter is a clip (~8 dB at this knee).
    assert!(c6 > t6 + 6.0, "clip {c6:.1} dB vs tanh {t6:.1} dB at a 6 dB knee");
    assert!(c9 > t9 + 8.0, "clip {c9:.1} dB vs tanh {t9:.1} dB at a 9 dB knee");
}

/// The end-to-end check: a 64-QAM signal through a 6 dB knee still decodes. The limit is
/// set so that the unlimited signal would hit full scale at -12 dBFS RMS (its peaks are
/// ~10 dB above the RMS) while the limited one stays clear of it.
#[test]
fn a_soft_limited_signal_still_decodes() {
    let cfg = tx_config();
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    let layout = tx.layout();
    let mut out = OutputStage::new(
        layout,
        OutputConfig { level_dbfs: -12.0, peak_limit: Some(PeakLimit::tanh(6.0)), ..OutputConfig::iq(0.0) },
    )
    .expect("output stage");
    let mut rx = Receiver::new(ReceiverConfig {
        input: InputFormat::Iq { swap: false },
        channels: 2,
        ..Default::default()
    });
    rx.set_msc_config(Some(MscConfig {
        mode: cfg.msc_mode,
        protection: cfg.protection,
        part_a_bytes: 0,
        interleaving: cfg.interleaving,
    }));

    let cap = tx.msc_capacity();
    let mut msc = vec![0u8; cap.total_bits()];
    let (mut fac_ok, mut msc_ok) = (0usize, 0usize);
    let mut pcm = Vec::new();
    for f in 0..25 {
        for (i, b) in msc.iter_mut().enumerate() {
            *b = (((f * 7 + i) >> 3) & 1) as u8;
        }
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc.as_deref()).expect("transmit");
        pcm.clear();
        out.process(&baseband, &mut pcm);
        for ev in rx.push(&pcm) {
            match ev {
                ReceiverEvent::Fac(_) => fac_ok += 1,
                ReceiverEvent::Msc(frame) if frame.complete => msc_ok += 1,
                _ => {}
            }
        }
    }
    eprintln!("[papr] limited 64-QAM: {fac_ok} FACs, {msc_ok} multiplex frames, {} clipped", out.clipped_samples());
    assert_eq!(out.clipped_samples(), 0, "a -12 dBFS level must fit under the limiter's ceiling");
    assert!(fac_ok > 0, "no FAC decoded through the limiter");
    assert!(msc_ok > 0, "no multiplex frame decoded through the limiter");
}

/// A knee outside the supported range is refused rather than silently doing something
/// violent to the signal.
#[test]
fn a_knee_outside_the_range_is_rejected() {
    let (_, layout, _) = signal(1);
    for db in [1.0, 25.0] {
        let cfg = OutputConfig { peak_limit: Some(PeakLimit::tanh(db)), ..OutputConfig::iq(0.0) };
        assert!(OutputStage::new(layout, cfg).is_err(), "{db} dB must be rejected");
    }
    // The tanh asymptote sits a little above the knee (the power normalisation scales it
    // up, ~7.2 dB for a 6 dB knee); a clip's is the knee itself. So a clip at 6 dB lets the
    // level be 6 dB higher than the raw crest allows, plus the 3 dB of the real part.
    let tanh = PeakLimit::tanh(6.0).ceiling_db();
    let clip = PeakLimit::clip(6.0).ceiling_db();
    assert!((6.0..7.5).contains(&tanh), "tanh ceiling {tanh:.2} dB for a 6 dB knee");
    assert!((5.9..6.3).contains(&clip), "clip ceiling {clip:.2} dB for a 6 dB knee");
    assert_eq!(PeakLimit::clip(6.0).law, PeakLaw::Clip);
}
