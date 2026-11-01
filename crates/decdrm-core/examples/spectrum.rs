//! Print a coarse averaged spectrum of a recording, and its image rejection.
//!
//! For a two-channel (I/Q) recording this is the quickest way to tell a receiver problem
//! from a source one: a complex signal has energy at +f and (near) none at −f, while a
//! real signal — or I/Q whose Q channel is missing, zero, or a copy of I — has the same
//! energy at both. That mirror image is exactly what a DRM+ signal looks like when the
//! input is read with the wrong channel format.
//!
//! Usage: `cargo run --release -p decdrm-core --example spectrum -- FILE [START_S]`
//! (WAV or FLAC; START_S skips that many seconds).

use decdrm_core::dsp::fft::Fft;
use decdrm_core::Cplx;

fn main() {
    let path = std::env::args().nth(1).expect("usage: spectrum FILE [START_S]");
    let start_s: f64 = std::env::args().nth(2).map(|s| s.parse().expect("START_S")).unwrap_or(0.0);
    let (samples, ch, fs) = load(&path);

    let n = 8192;
    let mut fft = Fft::new(n);
    let mut acc = vec![0.0; n];
    let start = (start_s * fs) as usize * ch;
    let mut blocks = 0;
    for b in 0..40 {
        let off = start + b * n * ch;
        if off + n * ch > samples.len() {
            break;
        }
        let mut v: Vec<Cplx> = (0..n)
            .map(|i| {
                let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
                if ch == 2 {
                    Cplx::new(samples[off + 2 * i], samples[off + 2 * i + 1]) * w
                } else {
                    Cplx::new(samples[off + i] * w, 0.0)
                }
            })
            .collect();
        fft.forward(&mut v);
        for (a, x) in acc.iter_mut().zip(&v) {
            *a += x.norm_sqr();
        }
        blocks += 1;
    }
    if blocks == 0 {
        eprintln!("no complete {n}-sample block from {start_s} s");
        std::process::exit(1);
    }

    let bin = fs / n as f64;
    println!("fs={fs:.0} Hz, {ch} channel(s), {blocks} blocks, bin {bin:.2} Hz");
    if ch == 2 {
        let half = n / 2;
        // Strongest positive-frequency bin and the same bin mirrored to negative. Note
        // that a DRM+ signal is itself symmetric about its carrier, so the number is only
        // meaningful when the two humps are at ±something rather than ±0.
        let (peak_bin, peak) = (1..half)
            .map(|k| (k, acc[(k + half) % n]))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .expect("some positive bins");
        let mirror = acc[(n - peak_bin + half) % n];
        println!(
            "strongest bin {:+.0} Hz; the mirrored bin {:+.0} Hz is {:+.1} dB relative",
            peak_bin as f64 * bin,
            -(peak_bin as f64 * bin),
            10.0 * (mirror / peak.max(1e-300)).log10()
        );
        println!("(a complex signal is ONE hump, symmetric about its carrier; a real signal — or");
        println!(" I/Q with Q missing or copied from I — shows a SECOND hump mirrored about 0 Hz)");
    } else {
        println!("one channel: a real signal, so its spectrum is symmetric by construction");
    }

    // Print in 500 Hz bands, from −fs/2 to +fs/2 (only 0..fs/2 for a real recording).
    let band = (500.0 / bin) as usize;
    let lo = if ch == 2 { -(n as isize) / 2 } else { 0 };
    let hi = (n / 2) as isize;
    let mut k = lo;
    while k < hi {
        let mut p = 0.0;
        for j in 0..band {
            let idx = (k + j as isize).rem_euclid(n as isize) as usize;
            p += acc[idx];
        }
        let db = 10.0 * (p / band as f64 / blocks as f64 + 1e-30).log10();
        let bar = ((db + 120.0).max(0.0) / 2.0) as usize;
        println!("{:7.0} Hz {:7.1} dB {}", k as f64 * bin, db, "#".repeat(bar.min(80)));
        k += band as isize;
    }
}

/// `(samples, channels, sample_rate)`, normalised to ±1.
fn load(path: &str) -> (Vec<f64>, usize, f64) {
    if path.to_ascii_lowercase().ends_with(".wav") {
        let mut r = hound::WavReader::open(path).expect("open WAV");
        let spec = r.spec();
        let ch = usize::from(spec.channels);
        let fs = f64::from(spec.sample_rate);
        let scale = match spec.sample_format {
            hound::SampleFormat::Float => 1.0,
            hound::SampleFormat::Int => 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f64,
        };
        let samples = match spec.sample_format {
            hound::SampleFormat::Float => {
                r.samples::<f32>().map(|v| f64::from(v.expect("sample")) * scale).collect()
            }
            hound::SampleFormat::Int => {
                r.samples::<i32>().map(|v| f64::from(v.expect("sample")) * scale).collect()
            }
        };
        (samples, ch, fs)
    } else {
        let mut r = claxon::FlacReader::open(path).expect("open FLAC");
        let info = r.streaminfo();
        let ch = info.channels as usize;
        let fs = f64::from(info.sample_rate);
        let scale = 1.0 / (1u64 << (info.bits_per_sample - 1)) as f64;
        let samples = r.samples().map(|v| f64::from(v.expect("sample")) * scale).collect();
        (samples, ch, fs)
    }
}
