//! Does the Mode E (DRM+) transmitter produce a signal matching real DRM+?
//!
//! The assertions are the measurements taken from ground-truth DRM+ recordings (in `refs/`),
//! so this test fails if the transmitter drifts from reality:
//!   * occupied span -47.11 .. +47.11 kHz (Kmin/Kmax = -106..+106 at 4000/9 Hz)
//!   * 100 ms frame, 40 OFDM symbols, Ts = 2.5 ms (Tu 2.25, Tg 0.25)
//!   * gain-reference grid: carrier remainder (2 + 4*(s mod 4)) mod 16, 13-14 per symbol

use decdrm_core::cellmap::{CellMap, CellType};
use decdrm_core::dsp::fft::Fft;
use decdrm_core::fac::{ChannelParams, Fac, Interleaving, MscMode, SdcMode, ServiceParams};
use decdrm_core::fec::mlc::MscProtection;
use decdrm_core::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::tx::output::{OutputConfig, OutputStage};
use decdrm_core::tx::{Transmitter, TxConfig};
use decdrm_core::Cplx;

fn fac() -> Fac {
    Fac {
        channel: ChannelParams {
            enhancement: false,
            frame_index: 0,
            afs_valid: false,
            occupancy: SpectrumOccupancy::SO_0,
            interleaving: Interleaving::Long,
            msc_mode: MscMode::Qam16Sm,
            sdc_mode: SdcMode::Qam4,
            num_audio: 1,
            num_data: 0,
            reconfiguration_index: 0,
            toggle: false,
        },
        service: ServiceParams {
            service_id: 1,
            short_id: 0,
            audio_ca: false,
            language: 0,
            is_data: false,
            descriptor: 0,
            data_ca: false,
        },
    }
}

fn cfg(rate: WorkingRate) -> TxConfig {
    TxConfig {
        mode: RobustnessMode::E,
        occupancy: SpectrumOccupancy::SO_0,
        msc_mode: MscMode::Qam16Sm,
        sdc_mode: SdcMode::Qam4,
        interleaving: Interleaving::Long,
        protection: MscProtection { part_a: 0, part_b: 0, hierarchical: 0 },
        part_a_bytes: 0,
        afs_index: 0,
        working_rate: rate,
    }
}

/// Mode E is refused below its minimum rate, and accepted above it.
#[test]
fn mode_e_is_refused_below_100_khz() {
    for rate in [WorkingRate::R48, WorkingRate::R96] {
        let e = Transmitter::new(cfg(rate)).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("100 kHz"), "rate {rate}: {msg}");
    }
    for rate in [WorkingRate::R120, WorkingRate::R192] {
        assert!(Transmitter::new(cfg(rate)).is_ok(), "rate {rate} should be accepted");
    }
}

#[test]
fn fac_bit_count_is_116_for_mode_e() {
    let f = fac();
    assert_eq!(f.to_bits_for(RobustnessMode::A).len(), 72, "A-D");
    assert_eq!(f.to_bits_for(RobustnessMode::E).len(), 116, "mode E");
}

/// A frame is 100 ms at the working rate.
#[test]
fn a_mode_e_frame_is_100_ms() {
    for (rate, want) in [(WorkingRate::R120, 12_000usize), (WorkingRate::R192, 19_200)] {
        let mut tx = Transmitter::new(cfg(rate)).unwrap();
        assert_eq!(tx.samples_per_frame(), want, "{rate}");
        let bits = vec![0u8; tx.msc_capacity().total_bits()];
        let frame = tx.transmit_frame(&fac(), &bits, Some(&[])).unwrap();
        assert_eq!(frame.len(), want, "{rate}: frame length");
    }
}

/// 213 carriers, Kmin/Kmax -106/+106, no unused carriers, top carrier at +47.11 kHz.
#[test]
fn carrier_layout_matches_reality() {
    let map = CellMap::new(RobustnessMode::E, SpectrumOccupancy::SO_0).unwrap();
    assert_eq!((map.kmin, map.kmax), (-106, 106));
    assert_eq!(map.num_carriers, 213);

    // No carrier along Kmin..=Kmax is typed DC (the "unused" marker).
    for k in map.kmin..=map.kmax {
        let c = map.cell(0, (k - map.kmin) as usize);
        assert!(!c.is_dc(), "carrier {k} is unused, but mode E leaves none unused");
    }

    let top = f64::from(map.kmax) * RobustnessMode::E.carrier_spacing_at(WorkingRate::R192);
    assert!(top > 47_000.0 && top < 47_200.0, "top carrier {top} Hz");
}

/// Every gain-reference cell follows the verified grid, 13-14 per symbol.
#[test]
fn gain_reference_grid_matches_reality() {
    let map = CellMap::new(RobustnessMode::E, SpectrumOccupancy::SO_0).unwrap();
    let nsym = RobustnessMode::E.symbols_per_frame();

    for s in 0..nsym {
        let want_rem = ((2 + 4 * (s % 4)) % 16) as i32;
        let mut n = 0;
        for k in map.kmin..=map.kmax {
            let c = map.cell(s, (k - map.kmin) as usize);
            if c.contains(CellType::SCAT_PILOT) {
                assert_eq!(
                    k.rem_euclid(16),
                    want_rem,
                    "symbol {s}: carrier {k} is a gain reference but has remainder {}",
                    k.rem_euclid(16)
                );
                n += 1;
            }
        }
        assert!(n == 13 || n == 14, "symbol {s} has {n} gain references, expected 13 or 14");
    }
}

/// The generated signal occupies the band the recordings occupy: energy at the outermost
/// carrier (±47.11 kHz) and none above it, at 192 kHz.
#[test]
fn generated_signal_occupies_the_expected_band() {
    let rate = WorkingRate::R192;
    let mut tx = Transmitter::new(cfg(rate)).unwrap();
    let layout = tx.layout();
    let mut out = OutputStage::new(layout, OutputConfig::iq_at(0.0, rate)).unwrap();

    let bits = vec![0u8; tx.msc_capacity().total_bits()];
    let frame = tx.transmit_frame(&fac(), &bits, Some(&[])).unwrap();
    let mut sig: Vec<f32> = Vec::new();
    out.process(&frame, &mut sig);

    let n = sig.len() / 2;
    let iq: Vec<Cplx> = (0..n)
        .map(|j| Cplx::new(f64::from(sig[2 * j]), f64::from(sig[2 * j + 1])))
        .collect();
    let mut fft = Fft::new(n);
    let mut spec = iq;
    fft.forward(&mut spec);
    let p: Vec<f64> =
        spec.iter().map(|c| 10.0 * (c.norm_sqr() / n as f64 + 1e-30).log10()).collect();
    let peak = p.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    let fs = f64::from(rate.hz());
    let df_bin = fs / n as f64;
    let df = RobustnessMode::E.carrier_spacing_at(rate);

    // Energy present at the outermost carrier (+106 => +47.11 kHz).
    let top_bin = (106.0 * df / df_bin).round() as usize;
    let around = p[top_bin.saturating_sub(4)..=(top_bin + 4).min(n / 2)]
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        around > peak - 20.0,
        "no carrier energy near +47.11 kHz: {around:.1} dB vs peak {peak:.1}"
    );

    // Nothing well above the band (60 kHz): the comb stops at 47.11 kHz.
    let out_bin = (60_000.0 / df_bin).round() as usize;
    let outside = p[out_bin.saturating_sub(3)..=(out_bin + 3).min(n / 2)]
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        outside < peak - 40.0,
        "energy above the band at 60 kHz: {outside:.1} dB vs peak {peak:.1}"
    );
}