//! Diagnostic: does a mode E symbol demodulate back to the cells it was built from?
//!
//! Bypasses the whole receiver: build one symbol's cells with the transmitter's cell map,
//! modulate them with the OFDM modulator, demodulate with the OFDM demodulator at the same
//! rate, and compare cell for cell. If this fails, the OFDM pair is wrong for mode E and
//! nothing downstream can work. If it passes, the bug is in the receiver's framing or
//! equalisation.
//!
//! Usage: cargo run --release -p decdrm-core --example modee_diag -- [192]

use decdrm_core::cellmap::CellMap;
use decdrm_core::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::tx::ofdm::OfdmModulator;
use decdrm_core::rx::ofdm::OfdmDemod;
use decdrm_core::Cplx;

fn main() {
    let khz: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(192);
    let rate = WorkingRate::new(khz * 1000).expect("supported rate");
    let mode = RobustnessMode::E;
    let map = CellMap::new(mode, SpectrumOccupancy::SO_0).expect("mode E map");
    println!("mode {mode} at {rate}: {} carriers, fft {}, guard {}, symbol {}",
             map.num_carriers, mode.fft_size_at(rate), mode.guard_len_at(rate),
             mode.symbol_len_at(rate));

    // Cells: a distinct value per carrier, so a mis-mapping is obvious.
    let cells: Vec<Cplx> = (0..map.num_carriers)
        .map(|c| Cplx::new((c % 13) as f64 - 6.0, (c % 7) as f64 - 3.0) * 0.25)
        .collect();

    // Modulate then demodulate.
    let mut modu = OfdmModulator::new_at(&map, rate);
    let mut baseband = Vec::new();
    modu.modulate(&cells, &mut baseband);
    let expect = mode.symbol_len_at(rate);
    println!("modulated {} samples (expect {expect})", baseband.len());

    let mut demod = OfdmDemod::new_at(&map, rate);
    // The receiver's window is the useful part *after* the guard.
    let guard = mode.guard_len_at(rate);
    let window = &baseband[guard..guard + mode.fft_size_at(rate)];
    let mut got = Vec::new();
    demod.demodulate(window, &mut got);
    println!("demodulated {} cells", got.len());

    let mut worst: f64 = 0.0;
    let mut bad = 0;
    for (c, (a, b)) in cells.iter().zip(&got).enumerate() {
        let d = (*a - *b).norm();
        if d > 1e-9 {
            bad += 1;
            if bad <= 6 {
                println!("  carrier {c} (k = {}): want {a:?} got {b:?}  err {d:.3e}",
                         map.kmin + c as i32);
            }
        }
        worst = worst.max(d);
    }
    println!("\ncarriers off by >1e-9: {bad} of {}; worst error {worst:.3e}", cells.len());
    if bad == 0 {
        println!("OK: the OFDM modulator/demodulator pair round-trips exactly for mode E.");
    } else {
        println!("FAIL: the OFDM pair does not round-trip for mode E — fix this first.");
    }
}