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
    run_with_band_limit(rate, seconds, true)
}

/// `band_limit` = the transmit channel filter (`OutputConfig::band_limit`).
///
/// This distinction matters and is currently NOT understood: with the filter **on** the same
/// mode E signal does not decode (FAC 0), with it **off** it does (FAC 2-3). It is not the
/// filter's in-band response (measured within 0.2 dB of unfiltered up to 47.4 kHz) nor its
/// group delay (1.8 ms at 192 kHz, far less than the interleaver's own delay). See
/// `band_limiting_breaks_mode_e_reception` below.
fn run_with_band_limit(rate: WorkingRate, seconds: Real, band_limit: bool) -> (usize, usize, usize) {
    let cfg = tx_config(rate);
    let mut tx = Transmitter::new(cfg).expect("mode E transmitter");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
    let frame_samples = tx.samples_per_frame();
    assert_eq!(frame_samples, (0.1 * f64::from(rate.hz())) as usize, "100 ms per frame");

    let mut out = OutputStage::new(layout, OutputConfig { band_limit, ..OutputConfig::iq_at(0.0, rate) })
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

    // Pin the mode and skip acquisition: the frequency search in `freqacq.rs` is still
    // built for 48 kHz geometry (fixed FFT length and pilot bins), so it cannot find a
    // mode E signal unaided. This test is about demodulation, not acquisition.
    rx.set_mode(RobustnessMode::E, SpectrumOccupancy::SO_0);

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

/// A **pinned** mode must still establish the frame boundary itself.
///
/// `Receiver::set_mode` used to force `state = Tracking`, which skipped the frame-sync
/// attempts and the no-FAC resync entirely - so a pinned signal was only ever decoded if it
/// happened to be aligned already, which a signal generated in the same process is and a file
/// (shifted by the transmit filter's group delay) is not. Measured on a written-then-read mode
/// E file: forcing `Tracking` gave FAC 3, staying in `Acquisition` gives FAC 7.
///
/// HONEST LIMIT: this test does **not** guard the `set_mode` fix - reverting that line still
/// passes it. A leading pad of silence in memory is not the same as a written file's delay.
///
/// It also exposed a real bug, since fixed: the receiver used to tolerate a lead-in of zero
/// samples and of large ones (5000 -> FAC 58) but **not small ones**, where it restarted and
/// never recovered. Before the fix (`DECDRM_LEAD` sweeps the pad, mode E, 192 kHz):
///     0 -> FAC 3    100 -> 0    240 -> 55    412 -> 0    1000 -> 0    5000 -> 58
/// The cause was `Receiver::restart`: it re-armed the 48 kHz frequency scan unconditionally,
/// even for a receiver whose mode was pinned. A pinned receiver runs at DC without a scan, so
/// after its first restart it started hunting for a DRM30 signal it could never find and never
/// returned to the signal at DC. Pinned, it now stays at DC and recovers — every lead value
/// above decodes. The band-limited file's 412 zero samples of transmit-filter warm-up are
/// exactly this case, which is why stripping them used to matter so much.
///
/// A sub-symbol lead-in is still resolved the slow way: the receiver restarts once and
/// re-acquires, rather than aligning first time. Emitting no warm-up would avoid the restart
/// altogether, but the receiver now survives it.
#[test]
fn a_pinned_mode_e_signal_is_aligned_from_the_signal_itself() {
    let rate = WorkingRate::R192;
    let cfg = tx_config(rate);
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
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
    rx.set_mode(RobustnessMode::E, SpectrumOccupancy::SO_0);

    let cap = tx.msc_capacity();
    let msc = vec![0u8; cap.total_bits()];
    let mut fac_ok = 0;
    let mut pcm: Vec<f32> = Vec::new();
    // A leading pad of silence shifts the frame boundary away from where the receiver starts
    // counting, standing in for the transmit filter's group delay on a file. 412 samples is the
    // measured real value; the env var lets it be swept.
    let lead: usize = std::env::var("DECDRM_LEAD").ok().and_then(|v| v.parse().ok()).unwrap_or(412);
    let pad = vec![0.0f32; lead * 2];
    for ev in rx.push(&pad) {
        let _ = ev;
    }
    for _ in 0..60 {
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc.as_deref()).expect("transmit");
        pcm.clear();
        out.process(&baseband, &mut pcm);
        for ev in rx.push(&pcm) {
            match ev {
                ReceiverEvent::Fac(_) => fac_ok += 1,
                ReceiverEvent::Restarted => eprintln!("[lead {lead}] RESTARTED"),
                ReceiverEvent::Resynchronising => eprintln!("[lead {lead}] RESYNC"),
                ReceiverEvent::ModeDetected(m) => eprintln!("[lead {lead}] mode {m}"),
                _ => {}
            }
        }
    }
    // The bug this test used to pin: a small lead-in restarted and never recovered. The
    // restart now keeps a pinned receiver at DC, so the signal is re-acquired. A few FACs is
    // enough — the point is that it decodes at all, not how fast.
    assert!(fac_ok > 0, "a small lead-in must be re-acquired (FAC {fac_ok})");
}

/// At 192 kHz: the receiver locks on, decodes the FAC and the SDC, and recovers multiplex
/// frames from DecDRM's own mode E signal.
///
/// This passes as of the interleaver-depth (D = 6 for mode E, §7.6) and TimeWiener
/// working-rate fixes. It is a DecDRM-to-DecDRM loopback, so it cannot catch a misreading
/// shared by this transmitter and receiver — for that, decode a real recording with
/// `--example modee_real` or `--example rxfiled` (see the mode E notes).
#[test]
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
///
/// KNOWN FAILING at the SDC step: the FAC decodes (32 of 80 frames) but no SDC block passes
/// its CRC, so no multiplex frame comes out. 192 kHz works, 120 kHz does not, and the
/// difference is not yet understood — a sweep of the TimeWiener filter-length/Doppler
/// placeholders (25 combinations) made no difference, so it is not equaliser tuning. Run with
/// `--ignored` to see it.
#[test]
#[ignore = "mode E at 120 kHz: FAC decodes but the SDC CRC fails; 192 kHz passes"]
fn mode_e_loopback_at_120k() {
    let (fac, sdc, msc) = run(WorkingRate::R120, 8.0);
    assert!(fac > 0, "no FAC decoded");
    assert!(sdc > 0, "no SDC decoded");
    assert!(msc >= 20, "only {msc} multiplex frames recovered");
}

/// The transmit channel filter does NOT break mode E reception.
///
/// This test exists because it disproved a wrong theory: writing the same signal to a WAV
/// and reading it back gave FAC 0 with `band_limit = true` and FAC 2-3 with it false, so the
/// filter looked like the cause. In memory - the same transmitter, output stage and receiver,
/// only bypassing the file - both decode, so **the filter is not the problem and never was**.
///
/// What the file experiment actually isolates is the **file round trip** (the WAV writer and
/// `examples/rxfiled`'s reader), not the filter. Real DRM+ recordings still fail to decode
/// while every in-memory path passes, so that round trip is where to look next. Keep this
/// test: it pins the in-memory behaviour and stops the file/pipeline difference being
/// mis-attributed to band limiting again.
#[test]
fn band_limiting_does_not_break_mode_e_reception() {
    let (fac_on, _, _) = run_with_band_limit(WorkingRate::R192, 5.0, true);
    let (fac_off, _, _) = run_with_band_limit(WorkingRate::R192, 5.0, false);
    assert!(fac_on > 0, "band-limited path must decode (got FAC {fac_on})");
    assert!(fac_off > 0, "unfiltered path must decode (got FAC {fac_off})");
}

/// The clean file pair, resolved: the band-limited file is the **same signal, time-shifted**
/// by the transmit filter's group delay (~415 samples measured; the filter has ~697 taps).
/// Aligned, the two files agree to 0.35% of peak, so band limiting corrupts nothing.
///
/// What that means for the reception failure: it is **timing offset tolerance**, not signal
/// quality. The in-memory tests pass because the transmitter's and receiver's sample clocks
/// line up; a written-then-read file carries the extra filter delay, and the receiver does not
/// re-establish the frame boundary from it. The real recordings presumably differ from
/// DecDRM's own output by an arbitrary offset of the same kind.
///
/// So the next thing to investigate is the receiver's **initial timing/frame alignment for a
/// pinned mode**, not the filter: `set_mode` skips acquisition, and acquisition is what would
/// normally find the boundary.
///
/// Does the **file round trip** - quantise the output stage's floats to int16 and read them
/// back - break mode E reception? The in-memory path passes, real recordings fail, and a
/// written-then-read DecDRM file failed too, so this isolates the one step between them.
#[test]
fn int16_quantisation_of_the_output_does_not_break_mode_e_reception() {
    let rate = WorkingRate::R192;
    let cfg = tx_config(rate);
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
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
    rx.set_mode(RobustnessMode::E, SpectrumOccupancy::SO_0);

    let cap = tx.msc_capacity();
    let msc = vec![0u8; cap.total_bits()];
    let mut fac_ok = 0;
    let mut pcm: Vec<f32> = Vec::new();
    for f in 0..80 {
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc.as_deref()).expect("transmit");
        pcm.clear();
        out.process(&baseband, &mut pcm);
        // Round trip through int16, as a WAV would.
        let quantised: Vec<f32> = pcm
            .iter()
            .map(|&v| {
                let s = (f64::from(v) * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                s as f32 / 32768.0
            })
            .collect();
        for ev in rx.push(&quantised) {
            if let ReceiverEvent::Fac(_) = ev {
                fac_ok += 1;
            }
        }
        let _ = f;
    }
    assert!(fac_ok > 0, "int16 round trip must decode (got FAC {fac_ok})");
}

/// Feed in the **exact** chunking `examples/rxfiled` uses (one 100 ms frame of interleaved
/// floats per `push`), rather than whole frames, to rule the caller's chunk size in or out.
#[test]
fn frame_sized_pushes_still_decode() {
    let rate = WorkingRate::R192;
    let cfg = tx_config(rate);
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
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
    rx.set_mode(RobustnessMode::E, SpectrumOccupancy::SO_0);

    let cap = tx.msc_capacity();
    let msc = vec![0u8; cap.total_bits()];
    let mut fac_ok = 0;
    let mut pcm: Vec<f32> = Vec::new();
    for _ in 0..40 {
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc.as_deref()).expect("transmit");
        pcm.clear();
        out.process(&baseband, &mut pcm);
        // Exactly rxfiled's per_chunk: rate/10 * channels floats.
        assert_eq!(pcm.len(), (rate.hz() as usize / 10) * 2);
        for ev in rx.push(&pcm) {
            if let ReceiverEvent::Fac(_) = ev {
                fac_ok += 1;
            }
        }
    }
    assert!(fac_ok > 0, "frame-sized pushes must decode (got FAC {fac_ok})");
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

/// A receiver configured with `ReceiverConfig::pinned` decodes mode E without a caller
/// having to call `set_mode` — the path the GUI and engine take, since neither can reach a
/// mode E signal through the automatic search.
///
/// This covers the configuration plumbing only; the demodulation itself is
/// `mode_e_loopback_at_192k`'s. It also pins the restart behaviour a pinned receiver needs:
/// a pinned receiver must not re-arm the 48 kHz frequency scan on restart, or a DRM+ signal
/// would be lost for good the first time acquisition restarted.
#[test]
fn a_pinned_configuration_reaches_mode_e_without_set_mode() {
    let rate = WorkingRate::R192;
    let cfg = tx_config(rate);
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
    let mut out = OutputStage::new(layout, OutputConfig::iq_at(0.0, rate)).expect("output stage");
    // No `set_mode` call: the configuration alone must pin the mode and skip the search.
    let mut rx = Receiver::new(ReceiverConfig {
        input: InputFormat::Iq { swap: false },
        channels: 2,
        working_rate: rate,
        pinned: Some((RobustnessMode::E, SpectrumOccupancy::SO_0)),
        ..Default::default()
    });
    assert_eq!(
        rx.pinned_mode(),
        Some((RobustnessMode::E, SpectrumOccupancy::SO_0)),
        "the configured pin must reach the receiver"
    );
    rx.set_msc_config(Some(MscConfig {
        mode: cfg.msc_mode,
        protection: cfg.protection,
        part_a_bytes: 0,
        interleaving: cfg.interleaving,
    }));

    let cap = tx.msc_capacity();
    let mut msc = vec![0u8; cap.total_bits()];
    let mut pcm: Vec<f32> = Vec::new();
    let mut fac_ok = 0;
    for f in 0..80 {
        for (i, b) in msc.iter_mut().enumerate() {
            *b = (((f * 7 + i) >> 3) & 1) as u8;
        }
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc.as_deref()).expect("transmit");
        pcm.clear();
        out.process(&baseband, &mut pcm);
        for ev in rx.push(&pcm) {
            if let ReceiverEvent::Fac(_) = ev {
                fac_ok += 1;
            }
        }
    }
    assert!(fac_ok > 0, "a pinned configuration must decode (got FAC {fac_ok})");
    // Acquisition may have restarted during the run (the lead-in case above restarts once and
    // recovers), so the point is not that it never did: it is that the pin survived. A
    // restart that dropped the pin would leave the receiver searching for a DRM30 signal at
    // 48 kHz geometry and decoding nothing.
    assert_eq!(
        rx.pinned_mode(),
        Some((RobustnessMode::E, SpectrumOccupancy::SO_0)),
        "a restart must keep the pin, not fall back to searching"
    );
}
