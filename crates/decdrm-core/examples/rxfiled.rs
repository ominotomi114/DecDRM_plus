//! Decode a WAV recording with the receiver, at an explicit working sample rate.
//!
//! `rxfile` handles FLAC and assumes 48 kHz; this handles the 16-bit WAV that the mode E
//! reference recordings are, and takes the rate and signal format on the command line.
//!
//! Usage:
//!   cargo run --release -p decdrm-core --example rxfiled -- <file.wav> <rate-khz> [mode]
//!
//! `mode` (A–E) pins the robustness mode instead of searching for it, which is how a
//! mode E recording is decoded: its mode cannot be inferred from the 2-bit FAC field, and
//! the acquisition search is still 48 kHz-bound.

use decdrm_core::params::{RobustnessMode, WorkingRate};
use decdrm_core::rx::{InputFormat, Receiver, ReceiverConfig, ReceiverEvent};
use std::io::Read;

/// Minimal 16/24/32-bit PCM WAV reader: skips unknown chunks (the 192 kHz files carry a
/// `JUNK` chunk before `data`), returns (rate, channels, interleaved f32).
fn read_wav(path: &str) -> (u32, usize, Vec<f32>) {
    let mut f = std::fs::File::open(path).expect("open wav");
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).expect("read wav");
    assert_eq!(&buf[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&buf[8..12], b"WAVE", "not a WAVE file");
    let mut pos = 12;
    let (mut rate, mut ch, mut bits) = (0u32, 0usize, 0u16);
    let mut data: Option<(usize, usize)> = None;
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let len = u32::from_le_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]]) as usize;
        let body = pos + 8;
        match id {
            b"fmt " => {
                ch = u16::from_le_bytes([buf[body + 2], buf[body + 3]]) as usize;
                rate = u32::from_le_bytes([buf[body + 4], buf[body + 5], buf[body + 6], buf[body + 7]]);
                bits = u16::from_le_bytes([buf[body + 14], buf[body + 15]]);
            }
            b"data" => data = Some((body, len.min(buf.len() - body))),
            _ => {} // JUNK, LIST, ...
        }
        pos = body + len + (len & 1); // chunks are word-aligned
    }
    let (body, len) = data.expect("no data chunk");
    let bytes = bits as usize / 8;
    let n = len / bytes;
    let mut out = Vec::with_capacity(n);
    match bits {
        16 => {
            for i in 0..n {
                let v = i16::from_le_bytes([buf[body + 2 * i], buf[body + 2 * i + 1]]);
                out.push(v as f32 / 32768.0);
            }
        }
        24 => {
            for i in 0..n {
                let b = &buf[body + 3 * i..body + 3 * i + 3];
                let v = ((b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16)) << 8 >> 8;
                out.push(v as f32 / 8388608.0);
            }
        }
        32 => {
            for i in 0..n {
                let v = f32::from_le_bytes([
                    buf[body + 4 * i],
                    buf[body + 4 * i + 1],
                    buf[body + 4 * i + 2],
                    buf[body + 4 * i + 3],
                ]);
                out.push(v);
            }
        }
        b => panic!("unsupported bit depth {b}"),
    }
    (rate, ch, out)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let khz: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(192);
    let forced = args.get(3).and_then(|s| match s.to_uppercase().as_str() {
        "A" => Some(RobustnessMode::A),
        "B" => Some(RobustnessMode::B),
        "C" => Some(RobustnessMode::C),
        "D" => Some(RobustnessMode::D),
        "E" => Some(RobustnessMode::E),
        _ => None,
    });
    let Some(rate) = WorkingRate::new(khz * 1000) else {
        panic!("unsupported rate {khz} kHz (use 48, 96, 120 or 192)");
    };

    let (wav_rate, ch, samples) = read_wav(path);
    println!("{path}: {wav_rate} Hz, {ch} ch, {} frames ({:.1} s)",
             samples.len() / ch, samples.len() as f64 / ch as f64 / wav_rate as f64);
    if wav_rate != rate.hz() {
        panic!("file is {wav_rate} Hz but --rate says {khz} kHz; give the file's own rate");
    }

    let mut cfg = ReceiverConfig {
        input: InputFormat::Iq { swap: false },
        channels: ch,
        working_rate: rate,
        ..Default::default()
    };
    if forced.is_some() {
        // Pin the mode: the acquisition search cannot be trusted for mode E yet.
        cfg.auto_flip = false;
    }
    let mut rx = Receiver::new(cfg);
    if let Some(m) = forced {
        rx.set_mode(m, decdrm_core::params::SpectrumOccupancy::SO_0);
    }

    let mut rms = 0.0f64;
    for &s in samples.iter().step_by(97) {
        rms += f64::from(s) * f64::from(s);
    }
    rms = (rms / (samples.len() / 97).max(1) as f64).sqrt();
    println!("input rms {rms:.4}");

    let per_chunk = (rate.hz() / 10) as usize * ch; // 100 ms
    let (mut fac_ok, mut fac_bad, mut sdc_ok, mut msc_ok) = (0, 0, 0, 0);
    let mut modes = Vec::new();
    let t0 = std::time::Instant::now();
    let chunks = samples.chunks(per_chunk).enumerate();
    let total = samples.len() / per_chunk.max(1);
    for (i, chunk) in chunks {
        let t = i as f64 / 10.0;
        for ev in rx.push(chunk) {
            match ev {
                ReceiverEvent::ModeDetected(m) => {
                    modes.push(m);
                    println!("{t:7.2}s mode detected {m}");
                }
                ReceiverEvent::SignalFound { dc_hz, inverted } => {
                    println!("{t:7.2}s signal found dc={dc_hz:.0} Hz inverted={inverted}");
                }
                ReceiverEvent::Fac(f) => {
                    fac_ok += 1;
                    if fac_ok <= 2 || fac_ok % 50 == 0 {
                        println!("{t:7.2}s FAC ok: {:?} occupancy {} services {}",
                                 f.channel, f.channel.occupancy.value(), f.channel.num_audio);
                        println!("          service id {:#x} short {} \"{}\"", f.service.service_id,
                                 f.service.short_id, f.service.short_id);
                    }
                }
                ReceiverEvent::FacError => fac_bad += 1,
                ReceiverEvent::Sdc(b) => {
                    if b.crc_ok {
                        sdc_ok += 1;
                        if sdc_ok <= 2 {
                            println!("{t:7.2}s SDC ok: {} bytes, afs index {}", b.data.len(), b.afs_index);
                        }
                    }
                }
                ReceiverEvent::Msc(m) if m.complete => {
                    msc_ok += 1;
                    if msc_ok <= 2 {
                        println!("{t:7.2}s MSC complete: {} bits (hpp {})", m.bits.len(), m.hpp_bits);
                    }
                }
                _ => {}
            }
        }
        if i % 50 == 0 {
            eprintln!("  [{i}/{total}] {t:.1}s");
        }
    }
    println!("\n--- {path} ---");
    println!("modes detected : {:?}", modes);
    println!("FAC ok {fac_ok}, FAC bad {fac_bad}");
    println!("SDC ok {sdc_ok}");
    println!("MSC frames complete {msc_ok}");
    println!("elapsed {:.1} s ({:.0}x realtime)",
             t0.elapsed().as_secs_f64(),
             samples.len() as f64 / ch as f64 / rate.hz() as f64 / t0.elapsed().as_secs_f64());
}