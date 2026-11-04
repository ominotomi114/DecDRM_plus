//! Diagnostic: **upsample 120 kHz to 192 kHz and decode there.**
//!
//! The receiver's 120 kHz path fails at the SDC while 192 kHz passes end to end on the
//! same waveform, and the cause is still unknown. A way to sidestep it entirely is to
//! convert a 120 kHz input to 192 kHz *before* the receiver and let the known-good path
//! do the work. This measures whether that works, and what it costs.
//!
//! It also measures the thing that decides whether the trick is safe: the image. A 1.6×
//! interpolation leaves a copy of the spectrum mirrored about 0.5·(input rate) = 60 kHz.
//! Mode E fills to 47.11 kHz, so the image of the band edge lands at
//! 120 − 47.11 = 72.89 kHz — outside the 96 kHz Nyquist of the *output*, so the
//! interpolation filter can remove it. This checks the arithmetic and prints the numbers.
//!
//! Usage: cargo run --release -p decdrm-core --example upsprobe -- <file.wav> <in-khz> [out-khz]

use decdrm_core::channel::resample::Resampler;
use decdrm_core::rx::{InputFormat, Receiver, ReceiverConfig, ReceiverEvent};
use decdrm_core::Cplx;
use decdrm_core::params::RobustnessMode;
use std::io::Read;

fn read_wav(path: &str) -> (u32, usize, Vec<f32>) {
    let mut f = std::fs::File::open(path).expect("open wav");
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).expect("read wav");
    assert_eq!(&buf[0..4], b"RIFF");
    let mut pos = 12;
    let (mut rate, mut ch, mut bits) = (0u32, 0usize, 0u16);
    let mut data: Option<(usize, usize)> = None;
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let len = u32::from_le_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]]) as usize;
        if id == b"fmt " {
            rate = u32::from_le_bytes([buf[pos + 12], buf[pos + 13], buf[pos + 14], buf[pos + 15]]);
            ch = u16::from_le_bytes([buf[pos + 10], buf[pos + 11]]) as usize;
            bits = u16::from_le_bytes([buf[pos + 22], buf[pos + 23]]);
        } else if id == b"data" {
            data = Some((pos + 8, len));
        }
        pos += 8 + len + (len & 1);
    }
    assert_eq!(bits, 16, "expected 16-bit samples");
    let (off, len) = data.expect("data chunk");
    let mut out = Vec::with_capacity(len / 2);
    let mut i = off;
    while i + 1 < off + len {
        out.push(f32::from(i16::from_le_bytes([buf[i], buf[i + 1]])) / 32768.0);
        i += 2;
    }
    (rate, ch, out)
}

/// Generate DecDRM's own mode E signal at `in_khz`, upsample it, and decode at `out_khz`.
///
/// This is the measurement that matters for the idea: the real recordings fail at 192 kHz
/// natively as well (they fail for a different, still-unknown reason), so they cannot say
/// whether upsampling dodges the **120 kHz** fault. Our own loopback can: it decodes at
/// 192 kHz and not at 120 kHz, so if the upsampled version decodes, the trick is sound.
fn loopback(in_khz: u32, out_khz: u32, seconds: f64) {
    use decdrm_core::fac::{ChannelParams, Fac, Interleaving, MscMode, SdcMode, ServiceParams};
    use decdrm_core::fec::mlc::MscProtection;
    use decdrm_core::params::{SpectrumOccupancy, WorkingRate};
    use decdrm_core::tx::output::{OutputConfig, OutputStage};
    use decdrm_core::tx::{Transmitter, TxConfig};

    let in_rate = WorkingRate::new(in_khz * 1000).expect("in rate");
    let out_rate = WorkingRate::new(out_khz * 1000).expect("out rate");
    let cfg = TxConfig {
        mode: RobustnessMode::E,
        occupancy: SpectrumOccupancy::SO_0,
        msc_mode: MscMode::Qam16Sm,
        sdc_mode: SdcMode::Qam4,
        interleaving: Interleaving::Long,
        protection: MscProtection { part_a: 0, part_b: 0, hierarchical: 0 },
        part_a_bytes: 0,
        afs_index: 0,
        working_rate: in_rate,
    };
    let mut tx = Transmitter::new(cfg).expect("tx");
    tx.set_fixed_fac_identity(Some(3));
    let layout = tx.layout();
    let mut out = OutputStage::new(layout, OutputConfig::iq_at(0.0, in_rate)).expect("output");
    let mut rx = Receiver::new(ReceiverConfig {
        input: InputFormat::Iq { swap: false },
        channels: 2,
        working_rate: out_rate,
        ..Default::default()
    });
    rx.set_mode(RobustnessMode::E, SpectrumOccupancy::SO_0);

    let fac = Fac {
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
    };
    let cap = tx.msc_capacity();
    let msc = vec![0u8; cap.total_bits()];
    let mut rs = Resampler::new(out_khz as f64 / in_khz as f64);
    let frames = (seconds / 0.1).ceil() as usize;
    let (mut fac_ok, mut sdc_ok, mut msc_ok) = (0, 0, 0);
    let mut pcm: Vec<f32> = Vec::new();
    let mut up: Vec<Cplx> = Vec::new();
    for _ in 0..frames {
        let sdc = (tx.frame_index() == 0).then(|| vec![0u8; tx.sdc_capacity_bytes()]);
        let baseband = tx.transmit_frame(&fac, &msc, sdc.as_deref()).expect("transmit");
        pcm.clear();
        out.process(&baseband, &mut pcm);
        let iq: Vec<Cplx> = pcm.chunks(2).map(|c| Cplx::new(f64::from(c[0]), f64::from(c[1]))).collect();
        up.clear();
        rs.process(&iq, &mut up);
        let mut f32v = Vec::with_capacity(up.len() * 2);
        for z in &up {
            f32v.push(z.re as f32);
            f32v.push(z.im as f32);
        }
        for ev in rx.push(&f32v) {
            match ev {
                ReceiverEvent::Fac(_) => fac_ok += 1,
                ReceiverEvent::Sdc(s) if s.crc_ok => sdc_ok += 1,
                ReceiverEvent::Msc(f) if f.complete => msc_ok += 1,
                _ => {}
            }
        }
    }
    println!("LOOPBACK generated at {in_khz} kHz, upsampled to {out_khz} kHz: FAC {fac_ok}, SDC {sdc_ok}, MSC {msc_ok}");
}

fn main() {
    if std::env::args().any(|a| a == "--loop") {
        let a: Vec<String> = std::env::args().collect();
        let in_khz: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(120);
        let out_khz: u32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(192);
        loopback(in_khz, out_khz, 20.0);
        return;
    }
    let path = std::env::args().nth(1).expect("wav path");
    let in_khz: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(120);
    let out_khz: u32 = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(192);

    let (rate, ch, pcm) = read_wav(&path);
    println!("{path}: {rate} Hz, {ch} ch, {} samples ({:.1} s)", pcm.len() / ch, pcm.len() as f64 / ch as f64 / rate as f64);
    assert_eq!(rate, in_khz * 1000, "file rate does not match the rate given");

    // Interleaved f32 -> complex baseband (I/Q, I left).
    let iq: Vec<Cplx> = pcm.chunks(2).map(|c| Cplx::new(f64::from(c[0]), f64::from(*c.get(1).unwrap_or(&0.0)))).collect();

    let ratio = out_khz as f64 / in_khz as f64;
    println!("upsampling by {ratio} ({} -> {} kHz)", in_khz, out_khz);
    // The image of the band edge, and where it has to sit for the filter to remove it.
    let nyq_in = in_khz as f64 * 1000.0 / 2.0;
    let nyq_out = out_khz as f64 * 1000.0 / 2.0;
    println!("  input Nyquist {nyq_in:.0} Hz; output Nyquist {nyq_out:.0} Hz");
    println!("  mode E band edge 47110 Hz -> its image at {:.0} Hz ({:.0} Hz clear of the output Nyquist)",
        (2.0 * nyq_in) - 47110.0, nyq_out - ((2.0 * nyq_in) - 47110.0));
    println!("  the interpolation filter must pass 47110 Hz and reject {:.0} Hz",
        (2.0 * nyq_in) - 47110.0);

    let mut rs = Resampler::new(ratio);
    let mut up: Vec<Cplx> = Vec::new();
    rs.process(&iq, &mut up);
    println!("resampled to {} complex samples", up.len());

    // Feed the upsampled stream to the receiver pinned to mode E at the output rate.
    let mut rx = Receiver::new(ReceiverConfig {
        input: InputFormat::Iq { swap: false },
        channels: 2,
        working_rate: decdrm_core::params::WorkingRate::new(out_khz * 1000).expect("rate"),
        ..Default::default()
    });
    rx.set_mode(RobustnessMode::E, decdrm_core::params::SpectrumOccupancy::SO_0);

    let mut fac_ok = 0;
    let mut sdc_ok = 0;
    let mut msc_ok = 0;
    let mut chunk = Vec::with_capacity(4096);
    for (i, z) in up.iter().enumerate() {
        chunk.push(z.re as f32);
        chunk.push(z.im as f32);
        if chunk.len() >= 8192 || i + 1 == up.len() {
            for ev in rx.push(&chunk) {
                match ev {
                    ReceiverEvent::Fac(_) => fac_ok += 1,
                    ReceiverEvent::Sdc(s) if s.crc_ok => sdc_ok += 1,
                    ReceiverEvent::Msc(f) if f.complete => msc_ok += 1,
                    _ => {}
                }
            }
            chunk.clear();
        }
    }
    println!("RESULT at {out_khz} kHz (upsampled from {in_khz}): FAC {fac_ok}, SDC {sdc_ok}, MSC {msc_ok}");
}
