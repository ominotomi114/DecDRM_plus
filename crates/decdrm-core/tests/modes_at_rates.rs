//! Modes A–D at every working sample rate, and mode detection behind a quiet lead-in.
//!
//! Modes A–D are *specified* at 48 kHz, but the chains take the working rate as a parameter
//! and a recording carries its own rate (`WorkingRate::closest`, so a 192 kHz DRM30 capture is
//! demodulated at 192 kHz rather than resampled). Nothing tested that until this file did:
//! `loopback.rs` runs everything at the 48 kHz default and `mode_e_rx.rs` only mode E.
//!
//! Both tests here are loopbacks through the real transmit chain (the output stage's channel
//! filter included), received **unpinned** — the receiver has to find the mode itself, which
//! is the part that is rate-sensitive:
//!
//! * [`drm30_modes_decode_at_every_working_rate`] — the four DRM30 modes at 48, 96, 120 and
//!   192 kHz. 96 and 120 kHz are not what a DRM30 recording would normally carry, but they are
//!   what `WorkingRate::closest` picks for a 88.2/96/100/110 kHz stream, so they must work.
//! * [`a_quiet_lead_in_is_not_detected_as_a_mode`] — **the regression test for a
//!   real bug**: silence gives every mode a guard-correlation score of exactly zero, the ratio
//!   between the best and the second-best score is then 0/0 = ∞, and `max_by` returns the last
//!   maximum on a tie — which is mode E. A 192 kHz recording that starts with a fraction of a
//!   second of silence (every real recording does) was therefore "detected" as DRM+, which
//!   threw away the acquisition and cost several seconds of audio before the right mode came
//!   back. Mode E's 100 ms frames made its detection window short enough (16 symbols = 41 ms)
//!   to fire inside the pre-signal buffer; at 48 kHz the window is 16 mode-B symbols = 0.43 s
//!   and the buffer only holds 0.45 s, which is why the bug only ever showed above 48 kHz.

use decdrm_core::channel::{ChannelConfig, ChannelSimulator, Rng};
use decdrm_core::fac::{ChannelParams, Fac, Interleaving, MscMode, SdcMode, ServiceParams};
use decdrm_core::fec::mlc::MscProtection;
use decdrm_core::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::rx::{InputFormat, MscConfig, Receiver, ReceiverConfig, ReceiverEvent};
use decdrm_core::tx::output::{OutputConfig, OutputStage};
use decdrm_core::tx::{Transmitter, TxConfig};
use decdrm_core::{Cplx, Real};

/// A 10 kHz (SO 3) transmission in `mode` at `rate`, one 16-QAM stream.
fn tx_config(mode: RobustnessMode, rate: WorkingRate) -> TxConfig {
    TxConfig {
        mode,
        occupancy: SpectrumOccupancy::SO_3,
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
            occupancy: SpectrumOccupancy::SO_3,
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

/// What the receiver made of the transmission.
#[derive(Default, Debug)]
struct Result {
    /// FAC blocks with a good CRC.
    fac: usize,
    /// SDC blocks with a good CRC.
    sdc: usize,
    /// Modes reported by mode detection, in order.
    modes: Vec<RobustnessMode>,
}

impl Result {
    fn collect(&mut self, rx: &mut Receiver, samples: &[f32]) {
        for ev in rx.push(samples) {
            match ev {
                ReceiverEvent::Fac(_) => self.fac += 1,
                ReceiverEvent::Sdc(s) if s.crc_ok => self.sdc += 1,
                ReceiverEvent::ModeDetected(m) => self.modes.push(m),
                _ => {}
            }
        }
    }
}

/// Decode `seconds` of `mode` at `rate` over an I/Q link, behind `lead_s` seconds of a low
/// noise floor. `channel`, if given, is the impairment between the transmitter and the
/// output stage.
fn run(
    mode: RobustnessMode,
    rate: WorkingRate,
    seconds: Real,
    lead_s: Real,
    lead_noise: Real,
    channel: Option<ChannelConfig>,
) -> Result {
    let cfg = tx_config(mode, rate);
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
    let mut channel = channel.map(|c| ChannelSimulator::new_at(layout, rate, c));
    let mut out = OutputStage::new(layout, OutputConfig { band_limit: true, ..OutputConfig::iq_at(0.0, rate) })
        .expect("output stage");
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

    let mut res = Result::default();
    // The lead-in: a noise floor 50 dB below full scale, the way a recording of an empty
    // channel or of the seconds before the transmitter keys up starts.
    let mut rng = Rng::new(7);
    let mut lead = Vec::new();
    for _ in 0..(lead_s * f64::from(rate.hz())) as usize {
        let z = if lead_noise > 0.0 { rng.complex_gaussian() * lead_noise } else { Cplx::new(0.0, 0.0) };
        lead.push(z.re as f32);
        lead.push(z.im as f32);
    }
    res.collect(&mut rx, &lead);
    assert!(res.modes.is_empty(), "the quiet lead-in alone reported {:?}", res.modes);

    let frame_s = tx.samples_per_frame() as f64 / f64::from(rate.hz());
    let frames = (seconds / frame_s).ceil() as usize;
    let cap = tx.msc_capacity();
    let mut msc = vec![0u8; cap.total_bits()];
    let mut pcm: Vec<f32> = Vec::new();
    let mut impaired: Vec<Cplx> = Vec::new();
    for f in 0..frames {
        for (i, b) in msc.iter_mut().enumerate() {
            *b = (((f * 7 + i) >> 3) & 1) as u8;
        }
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc.as_deref()).expect("transmit");
        pcm.clear();
        match channel.as_mut() {
            Some(ch) => {
                impaired.clear();
                ch.process(&baseband, &mut impaired);
                out.process(&impaired, &mut pcm);
            }
            None => out.process(&baseband, &mut pcm),
        }
        res.collect(&mut rx, &pcm);
    }
    res
}

/// Every DRM30 mode decodes at every working rate the receiver accepts.
#[test]
fn drm30_modes_decode_at_every_working_rate() {
    for rate in WorkingRate::ALL {
        for mode in RobustnessMode::DRM30 {
            let r = run(mode, rate, 6.0, 0.0, 0.0, Some(ChannelConfig::awgn(20.0)));
            eprintln!(
                "mode {mode} at {rate}: FAC {}, SDC {}, detected {:?}",
                r.fac, r.sdc, r.modes
            );
            assert_eq!(r.modes.first().copied(), Some(mode), "mode {mode} at {rate}: detected {:?}", r.modes);
            assert!(r.fac >= 8, "mode {mode} at {rate}: only {} FAC blocks", r.fac);
            assert!(r.sdc >= 1, "mode {mode} at {rate}: no SDC block");
        }
    }
}

/// A quiet lead-in must not be detected as a mode — least of all mode E, whose short
/// detection window (16 symbols = 41 ms) and last position in `RobustnessMode::ALL` made it
/// the default answer for silence at any rate above mode E's 100 kHz minimum.
///
/// Digital silence is the sharp case: the frequency search hands time sync the ~0.45 s of
/// buffered samples that preceded the acquisition, and if that buffer begins inside the
/// silence, mode detection decides on zeros alone. Which lead lengths land there depends on
/// where the acquisition fires (0.5–0.55 s does at 192 kHz), so this sweeps a few.
#[test]
fn a_quiet_lead_in_is_not_detected_as_a_mode() {
    for lead in [0.45, 0.5, 0.55, 0.6] {
        let r = run(RobustnessMode::A, WorkingRate::R192, 6.0, lead, 0.0, None);
        eprintln!("mode A at 192 kHz behind {lead} s of silence: detected {:?}, FAC {}", r.modes, r.fac);
        assert_eq!(
            r.modes.first().copied(),
            Some(RobustnessMode::A),
            "mode A at 192 kHz behind {lead} s of silence: detected {:?}",
            r.modes
        );
        assert!(r.fac > 0, "mode A at 192 kHz behind {lead} s of silence: no FAC");
    }
    // The same with a noise floor instead of digital silence, and every DRM30 mode at both
    // rates mode E can run at (below 100 kHz, mode E is not a candidate at all).
    for rate in [WorkingRate::R120, WorkingRate::R192] {
        for mode in RobustnessMode::DRM30 {
            let r = run(mode, rate, 6.0, 1.0, 0.003, None);
            eprintln!("mode {mode} at {rate} behind 1 s of quiet: detected {:?}, FAC {}", r.modes, r.fac);
            assert_eq!(
                r.modes.first().copied(),
                Some(mode),
                "mode {mode} at {rate} behind a quiet lead-in: detected {:?}",
                r.modes
            );
            assert!(r.fac > 0, "mode {mode} at {rate}: no FAC");
        }
    }
}
