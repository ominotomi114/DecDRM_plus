//! Mode E (DRM+) loopback: DecDRM's own transmitter → I/Q → its own receiver, at 192 kHz.
//!
//! The modes A–D equivalent is `loopback.rs`; this covers the part that differs, because
//! mode E runs at a higher working sample rate and has its own frame geometry (100 ms
//! frames, 40 symbols, 4 frames per super frame, 244 FAC cells, a 116-bit two-service FAC).
//!
//! The receiver has to be told the working rate (there is no way to infer it from the
//! signal), and mode E is only accepted at 120/192 kHz.

use decdrm_core::fac::{ChannelParams, Fac, Interleaving, MscMode, SdcMode, ServiceParams};
use decdrm_core::fec::mlc::MscProtection;
use decdrm_core::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::rx::{InputFormat, MscConfig, Receiver, ReceiverConfig, ReceiverEvent};
use decdrm_core::tx::output::{OutputConfig, OutputStage};
use decdrm_core::tx::{Transmitter, TxConfig};
use decdrm_core::Real;

/// A mode E transmission configuration at `rate`, one 16-QAM stream.
fn tx_config(rate: WorkingRate) -> TxConfig {
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

fn test_fac() -> Fac {
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
            service_id: 0xD0D001,
            short_id: 0,
            audio_ca: false,
            language: 9,
            is_data: false,
            descriptor: 5,
            data_ca: false,
        },
    }
}

/// Decode `seconds` of self-generated mode E signal at `rate`; returns
/// (FACs decoded, SDC CRCs that passed, multiplex frames recovered).
fn run(rate: WorkingRate, seconds: Real) -> (usize, usize, usize) {
    let cfg = tx_config(rate);
    let mut tx = Transmitter::new(cfg).expect("mode E transmitter");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
    let frame_samples = tx.samples_per_frame();
    assert_eq!(frame_samples, (0.1 * f64::from(rate.hz())) as usize, "100 ms per frame");

    let mut out = OutputStage::new(layout, OutputConfig::iq_at(0.0, rate)).expect("output stage");
    let mut rx = Receiver::new(ReceiverConfig {
        input: InputFormat::Iq { swap: false },
        channels: 2,
        working_rate: rate,
        ..Default::default()
    });
    rx.set_msc_config(Some(MscConfig {
        mode: cfg.msc_mode,
        protection: cfg.protection,
        part_a_bytes: 0,
        interleaving: cfg.interleaving,
    }));

    let cap = tx.msc_capacity();
    let frames = (seconds / 0.1).ceil() as usize;
    // A plain counter stands in for the MSC payload; the test only needs the container.
    let mut msc = vec![0u8; cap.total_bits()];
    let (mut fac_ok, mut sdc_ok, mut msc_ok) = (0, 0, 0);
    let mut pcm: Vec<f32> = Vec::new();
    for f in 0..frames {
        for (i, b) in msc.iter_mut().enumerate() {
            *b = (((f * 7 + i) >> 3) & 1) as u8;
        }
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc.as_deref()).expect("transmit");
        pcm.clear();
        out.process(&baseband, &mut pcm);
        for ev in rx.push(&pcm) {
            eprintln!("ev: {ev:?}");
            match ev {
                ReceiverEvent::Fac(_) => fac_ok += 1,
                ReceiverEvent::Sdc(s) if s.crc_ok => sdc_ok += 1,
                ReceiverEvent::Msc(frame) if frame.complete => msc_ok += 1,
                _ => {}
            }
        }
    }
    (fac_ok, sdc_ok, msc_ok)
}

/// At 192 kHz: the receiver locks on, decodes the FAC and the SDC, and recovers multiplex
/// frames from DecDRM's own mode E signal.
///
/// KNOWN FAILING: mode E reception is not finished. Acquisition and symbol sync work — the
/// receiver finds the signal and its own detector identifies the mode — but the demodulated
/// cells are wrong, so every FAC CRC fails. Run with `--ignored` to see the progress:
/// `cargo test -p decdrm-core --test mode_e_rx -- --ignored --nocapture`
#[test]
#[ignore = "mode E reception incomplete: cells demodulate wrong, FAC CRC fails"]
fn mode_e_loopback_at_192k() {
    let (fac, sdc, msc) = run(WorkingRate::R192, 8.0);
    assert!(fac > 0, "no FAC decoded");
    assert!(sdc > 0, "no SDC decoded");
    assert!(msc > 0, "no multiplex frame recovered");
    // With long interleaving the first frames are held back; 8 s of 100 ms frames is 80,
    // so a healthy receiver recovers a good share.
    assert!(msc >= 20, "only {msc} multiplex frames recovered");
}

/// The same at 120 kHz, the other rate mode E runs at.
#[test]
#[ignore = "mode E reception incomplete: see mode_e_loopback_at_192k"]
fn mode_e_loopback_at_120k() {
    let (fac, sdc, msc) = run(WorkingRate::R120, 8.0);
    assert!(fac > 0, "no FAC decoded");
    assert!(sdc > 0, "no SDC decoded");
    assert!(msc >= 20, "only {msc} multiplex frames recovered");
}

/// The mode detector must not choose mode E at a rate that cannot carry it, and must not
/// panic when the receiver is at 48 kHz (where mode E has no geometry).
#[test]
fn mode_e_is_never_detected_below_its_rate() {
    // A mode A signal at 48 kHz: the detector may pick A or B (they share a symbol rate),
    // but never E.
    let cfg = TxConfig { mode: RobustnessMode::A, occupancy: SpectrumOccupancy::SO_3, ..Default::default() };
    let mut tx = Transmitter::new(cfg).unwrap();
    let layout = tx.layout();
    let mut out = OutputStage::new(layout, OutputConfig::iq(0.0)).unwrap();
    let mut rx = Receiver::new(ReceiverConfig {
        input: InputFormat::Iq { swap: false },
        channels: 2,
        ..Default::default() // 48 kHz
    });
    let cap = tx.msc_capacity();
    let msc = vec![0u8; cap.total_bits()];
    let mut pcm: Vec<f32> = Vec::new();
    for _ in 0..20 {
        let sdc = None;
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc).unwrap();
        pcm.clear();
        out.process(&baseband, &mut pcm);
        for ev in rx.push(&pcm) {
            if let ReceiverEvent::ModeDetected(m) = ev {
                assert_ne!(m, RobustnessMode::E, "mode E must not be detected at 48 kHz");
            }
        }
    }
}