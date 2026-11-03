//! Sweep mode E's channel-estimator parameters over a synthesised VHF channel.
//!
//! The receiver's TimeWiener length/Doppler and frequency-Wiener length are per-mode
//! constants; mode E's were copied from modes C/D, which see a few Hz of Doppler, while a
//! car on Band III sees ten times that. This decodes a self-generated mode E signal through
//! COST 207's TU6 at a given speed, for one set of parameters, and counts what came out.
//!
//! Three phases: the time filter's length and maximum Doppler at 100 and 200 km/h; the
//! frequency filter's length at the best of those; and the winner against mode E's default
//! when parked and at 10 dB, the cases a wide Doppler setting could hurt.
//!
//! Usage: `cargo run --release -p decdrm-core --example modee_sweep [SECONDS]`
//! (SECONDS default 6; the signal is 100 ms frames at 192 kHz, so 6 s is 60 frames).

use decdrm_core::channel::{ChannelConfig, ChannelSimulator, vhf};
use decdrm_core::fac::{ChannelParams, Fac, Interleaving, MscMode, SdcMode, ServiceParams};
use decdrm_core::fec::mlc::MscProtection;
use decdrm_core::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::rx::{ChanEstTuning, InputFormat, MscConfig, Receiver, ReceiverConfig, ReceiverEvent};
use decdrm_core::tx::output::{OutputConfig, OutputStage};
use decdrm_core::tx::{Transmitter, TxConfig};
use decdrm_core::{Cplx, Real};

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

/// Decode `seconds` of mode E at 192 kHz through TU6 at `speed` and `snr`, with the
/// estimator parameters in `tuning`; returns (FACs, valid SDCs, complete MSC frames).
fn run(tuning: ChanEstTuning, speed: Real, snr: Real, seconds: Real) -> (usize, usize, usize) {
    let rate = WorkingRate::R192;
    let cfg = tx_config(rate);
    let mut tx = Transmitter::new(cfg).expect("mode E transmitter");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
    let mut chan = ChannelSimulator::new_at(
        layout,
        rate,
        ChannelConfig {
            model: vhf::cost207_tu6(speed, 200e6),
            snr_db: Some(snr),
            seed: 3,
            ..Default::default()
        },
    );
    let mut out = OutputStage::new(layout, OutputConfig::iq_at(0.0, rate)).expect("output stage");
    let mut rx = Receiver::new(ReceiverConfig {
        input: InputFormat::Iq { swap: false },
        channels: 2,
        working_rate: rate,
        chanest: tuning,
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
    let frames = (seconds / 0.1).ceil() as usize;
    let mut msc = vec![0u8; cap.total_bits()];
    let (mut fac_ok, mut sdc_ok, mut msc_ok) = (0usize, 0usize, 0usize);
    let mut pcm: Vec<f32> = Vec::new();
    let mut impaired: Vec<Cplx> = Vec::new();
    for f in 0..frames {
        for (i, b) in msc.iter_mut().enumerate() {
            *b = (((f * 7 + i) >> 3) & 1) as u8;
        }
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&test_fac(), &msc, sdc.as_deref()).expect("transmit");
        impaired.clear();
        chan.process(&baseband, &mut impaired);
        pcm.clear();
        out.process(&impaired, &mut pcm);
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

fn main() {
    let seconds: Real = std::env::args().nth(1).map(|a| a.parse().expect("SECONDS")).unwrap_or(6.0);
    let snr = 20.0;
    let frames = (seconds / 0.1) as usize;
    println!("mode E, 192 kHz, TU6 at 200 MHz, {snr} dB, {seconds} s ({frames} frames)\n");

    // Phase 1: the time Wiener's length and maximum Doppler. Ranked by multiplex frames at
    // the two speeds together (a frame needs both timing and equalisation to work).
    println!("len  sigma  speed  FAC  SDC  MSC");
    let mut best = (0usize, 0.0, 0usize);
    for len in [3usize, 5, 7, 9] {
        for sigma in [4.0, 8.0, 12.0] {
            let tuning = ChanEstTuning { time_len: Some(len), time_sigma_hz: Some(sigma), freq_len: None };
            let mut total = 0;
            for speed in [100.0, 200.0] {
                let (fac, sdc, msc) = run(tuning, speed, snr, seconds);
                println!("{len:3}  {sigma:5.2}  {speed:5.0}  {fac:3}  {sdc:3}  {msc:3}");
                total += msc;
            }
            if total > best.2 {
                best = (len, sigma, total);
            }
        }
    }

    // Phase 2: the frequency Wiener's length, at the best time parameters.
    println!("\nfreq  speed  FAC  SDC  MSC   (time len {}, sigma {:.2})", best.0, best.1);
    for freq in [7usize, 9, 11, 13, 15] {
        let tuning = ChanEstTuning { time_len: Some(best.0), time_sigma_hz: Some(best.1), freq_len: Some(freq) };
        for speed in [100.0, 200.0] {
            let (fac, sdc, msc) = run(tuning, speed, snr, seconds);
            println!("{freq:4}  {speed:5.0}  {fac:3}  {sdc:3}  {msc:3}");
        }
    }

    // Phase 3: the winner against mode E's default, where a wide Doppler setting could
    // cost: parked, and at 10 dB.
    println!("\ncandidate        speed  snr   FAC  SDC  MSC   (default is len 9, sigma 2.85)");
    let winner = ChanEstTuning { time_len: Some(best.0), time_sigma_hz: Some(best.1), freq_len: None };
    for (name, tuning) in [("default", ChanEstTuning::default()), ("winner", winner)] {
        for speed in [0.0, 100.0, 200.0] {
            for snr in [10.0, 20.0] {
                let (fac, sdc, msc) = run(tuning, speed, snr, seconds);
                println!("{name:8}  {speed:5.0}  {snr:4.0}  {fac:4}  {sdc:3}  {msc:3}");
            }
        }
    }
    println!("\nbest time parameters: len {} sigma {:.2}", best.0, best.1);
}
