//! Diagnostic: demodulate a REAL DRM+ recording with perfect timing and decode its FAC.
//!
//! The self-loopback diagnostics (`modee_fac`, `modee_sdc`) prove DecDRM's transmitter and
//! receiver agree with *each other*; they cannot catch a shared misreading of the standard.
//! This one works on a third-party recording instead, so a pass means DecDRM agrees with a
//! real DRM+ transmitter.
//!
//! It finds the frame start by cyclic-prefix correlation (no acquisition), lays out the
//! 40 symbols of a 100 ms frame from it, demodulates each with the mode E geometry, collects
//! the FAC cells exactly as the receiver's demap order does, and decodes.
//!
//! Usage: cargo run --release -p decdrm-core --example modee_real -- <file.wav> [khz]

use decdrm_core::cellmap::CellMap;
use decdrm_core::fac::Fac;
use decdrm_core::fec::mlc::{MlcDecoder, MlcParams};
use decdrm_core::fec::qam::EqCell;
use decdrm_core::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::rx::ofdm::OfdmDemod;
use decdrm_core::Cplx;

/// Minimal 16-bit stereo WAV reader (the recordings are 16-bit I/Q WAV).
fn read_iq(path: &str) -> (u32, Vec<Cplx>) {
    let b = std::fs::read(path).expect("read wav");
    assert_eq!(&b[0..4], b"RIFF");
    let mut pos = 12;
    let (mut rate, mut ch, mut bits) = (0u32, 0usize, 0u16);
    let mut data = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes([b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]]) as usize;
        let body = pos + 8;
        match id {
            b"fmt " => {
                ch = u16::from_le_bytes([b[body + 2], b[body + 3]]) as usize;
                rate = u32::from_le_bytes([b[body + 4], b[body + 5], b[body + 6], b[body + 7]]);
                bits = u16::from_le_bytes([b[body + 14], b[body + 15]]);
            }
            b"data" => data = Some((body, len.min(b.len() - body))),
            _ => {}
        }
        pos = body + len + (len & 1);
    }
    assert_eq!(bits, 16, "expected 16-bit samples");
    let (body, len) = data.expect("no data chunk");
    let n = len / 2;
    let v: Vec<f32> =
        (0..n).map(|i| i16::from_le_bytes([b[body + 2 * i], b[body + 2 * i + 1]]) as f32 / 32768.0).collect();
    let frames = v.len() / ch;
    let z = (0..frames).map(|i| Cplx::new(f64::from(v[i * ch]), f64::from(v[i * ch + 1]))).collect();
    (rate, z)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: modee_real <file.wav> [khz]");
    let khz: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(192);
    let rate = WorkingRate::new(khz * 1000).expect("supported rate");
    let mode = RobustnessMode::E;
    let map = CellMap::new(mode, SpectrumOccupancy::SO_0).expect("map");

    let (fs, z) = read_iq(&path);
    println!("{path}: {fs} Hz, {} samples ({:.1}s)", z.len(), z.len() as f64 / f64::from(fs));
    assert_eq!(fs, rate.hz(), "pass the file's own rate in kHz");

    let tu = mode.fft_size_at(rate);
    let tg = mode.guard_len_at(rate);
    let ts = mode.symbol_len_at(rate);
    let ns = mode.symbols_per_frame();
    let frame = ns * ts;
    println!("mode E geometry: Tu={tu} Tg={tg} Ts={ts}, {ns} symbols/frame ({frame} samples)");

    // Frame start: best cyclic-prefix correlation repeating every `frame` samples.
    let n = (z.len() - tu - 1).min(6 * frame);
    let p: Vec<Cplx> = (0..n).map(|i| z[i] * z[i + tu].conj()).collect();
    let (mut cs, mut mag) = (Cplx::new(0.0, 0.0), Vec::with_capacity(n));
    for i in 0..n {
        cs += p[i];
        if i >= tg {
            cs -= p[i - tg];
        }
        if i + 1 >= tg {
            mag.push(cs.norm() / tg as f64);
        }
    }
    let mut best = 0usize;
    let mut bestv = -1.0;
    for off in 0..frame.min(mag.len().saturating_sub(1)) {
        let mut acc = 0.0;
        let mut k = 1;
        while off + k * frame < mag.len() && k <= 6 {
            acc += mag[off + k * frame];
            k += 1;
        }
        if acc > bestv {
            bestv = acc;
            best = off;
        }
    }
    println!("frame start offset = {best}");

    // Demodulate every symbol of one frame with perfect timing (window = Tu after Tg).
    let mut demod = OfdmDemod::new_at(&map, rate);
    let mut symbols: Vec<Vec<Cplx>> = Vec::new();
    for s in 0..ns {
        let start = best + s * ts + tg;
        if start + tu > z.len() {
            println!("!! recording too short for a full frame");
            return;
        }
        let mut cells = Vec::new();
        demod.demodulate(&z[start..start + tu], &mut cells);
        symbols.push(cells);
    }

    // The frame start from correlation is only accurate to one symbol boundary, and the
    // symbol phase within the frame is unknown (the FAC lives at symbols 5..=26, so which
    // detected symbol is spec symbol 0 matters). Try every rotation and report which decode.
    if std::env::var("DECDRM_ROTATE").is_ok() {
        for sh in 0..ns {
            let mut cells = Vec::new();
            for s in 0..ns {
                let spec = (s + sh) % ns;
                for &c in map.fac_carriers(spec) {
                    cells.push(EqCell { sig: symbols[s][c as usize], chan: 1.0 });
                }
            }
            let mut d = MlcDecoder::new(MlcParams::fac_for(mode), 0);
            let mut b = Vec::new();
            d.decode(&cells, &mut b);
            if let Some(f) = Fac::parse(&b) {
                println!("ROTATION {sh}: DECODED occupancy {} services {} id {:#x}",
                         f.channel.occupancy.value(), f.channel.num_audio, f.service.service_id);
            }
        }
        return;
    }

    // Collect the FAC cells in the receiver's order and decode.
    let mut fac_cells = Vec::new();
    for s in 0..ns {
        for &c in map.fac_carriers(s) {
            fac_cells.push(EqCell { sig: symbols[s][c as usize], chan: 1.0 });
        }
    }
    println!("collected {} FAC cells (expect {})", fac_cells.len(),
             decdrm_core::tables::num_fac_cells(mode));
    let mut dec = MlcDecoder::new(MlcParams::fac_for(mode), 0);
    let mut bits = Vec::new();
    dec.decode(&fac_cells, &mut bits);
    println!("decoded {} bits", bits.len());
    // Are the FAC cells on the 4-QAM constellation at all? A real signal puts them at four
    // phases 90 degrees apart; noise scatters. Print how many land near one of four ideal
    // points, which tells us whether the cell values are being extracted correctly.
    {
        use std::f64::consts::PI;
        let mags: Vec<f64> = fac_cells.iter().map(|c| c.sig.norm()).collect();
        let mean = mags.iter().sum::<f64>() / mags.len() as f64;
        let mut hits = [0usize; 4];
        let mut worst = 0.0f64;
        for c in &fac_cells {
            let ph = c.sig.arg();
            // Ideal 4-QAM phases for DRM are pi/4 + k*pi/2.
            let mut best = f64::MAX;
            let mut bi = 0;
            for k in 0..4 {
                let ideal = PI / 4.0 + k as f64 * PI / 2.0;
                let d = (ph - ideal).rem_euclid(2.0 * PI);
                let d = d.min(2.0 * PI - d);
                if d < best {
                    best = d;
                    bi = k;
                }
            }
            hits[bi] += 1;
            worst = worst.max(best);
        }
        println!("phase check: mean |.| {mean:.5}; nearest-ideal-phase counts {:?}", hits);
        println!("  (a real 4-QAM signal spreads them over all four; worst phase error {:.1} deg)",
                 worst.to_degrees());
    }

    match Fac::parse(&bits) {
        Some(f) => {
            println!("OK: real DRM+ FAC decoded");
            println!("  occupancy {} services {} service id {:#x}",
                     f.channel.occupancy.value(), f.channel.num_audio, f.service.service_id);
        }
        None => {
            println!("FAIL: real DRM+ FAC did not parse");
            println!("  raw bits (first 64): {:?}", &bits[..bits.len().min(64)]);
            // How many of the 244 cells carry a plausible 4-QAM point vs noise?
            let mags: Vec<f64> = fac_cells.iter().map(|c| c.sig.norm()).collect();
            let mean = mags.iter().sum::<f64>() / mags.len() as f64;
            println!("  FAC cell |.| mean {mean:.4}, min {:.4}, max {:.4}",
                     mags.iter().cloned().fold(f64::MAX, f64::min),
                     mags.iter().cloned().fold(0.0, f64::max));
        }
    }
}