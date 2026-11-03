//! What a `tanh` PAPR limiter buys a DRM transmitter, and what it costs.
//!
//! `cargo run -p decdrm-core --release --example papr`
//!
//! For a mode B (DRM30) and a mode E (DRM+) transmission it prints
//! * the peak-to-average ratio of the baseband and of the real part (the real-IF output),
//! * how much of the output reaches full scale at a few RMS levels, with and without the
//!   limiter — the reason to have one,
//! * for each limiter knee: the resulting peak (which the receiver sees as the maximum
//!   level the transmitter can use), the mean-power change, and the in-band **MER** —
//!   the error-vector magnitude of every cell against the unlimited signal, measured
//!   with a perfect receiver. Also the MER of a **hard** clip at the same peak, which is
//!   what the ±1 clip at the end of the chain does, for comparison.
//!
//! MER is the honest cost: the limiter is a memoryless nonlinearity, so it distorts the
//! constellation. The out-of-band part of that distortion is removed by the transmit
//! channel filter (the limiter works *before* it); the in-band part is not, and it is
//! what a receiver has to demodulate through.

use decdrm_core::cellmap::CellMap;
use decdrm_core::channel::Rng;
use decdrm_core::dsp::fft::Fft;
use decdrm_core::fac::{ChannelParams, Fac, Interleaving, MscMode, SdcMode, ServiceParams};
use decdrm_core::fec::mlc::MscProtection;
use decdrm_core::params::{ChannelLayout, RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::tx::output::{OutputConfig, OutputStage, PeakLimit, apply_peak_limit};
use decdrm_core::tx::{Transmitter, TxConfig};
use decdrm_core::{Cplx, Real};

/// Limiter knees to report, dB above the RMS (6 dB = "compress above 2σ").
const KNEES_DB: [Real; 8] = [12.0, 10.0, 9.0, 8.0, 7.5, 6.0, 4.5, 3.0];
/// RMS levels to report full-scale use at, dBFS.
const LEVELS_DBFS: [Real; 4] = [-18.0, -15.0, -12.0, -9.0];

fn test_fac(occupancy: SpectrumOccupancy, msc: MscMode, sdc: SdcMode) -> Fac {
    Fac {
        channel: ChannelParams {
            enhancement: false,
            frame_index: 0,
            afs_valid: false,
            occupancy,
            interleaving: Interleaving::Long,
            msc_mode: msc,
            sdc_mode: sdc,
            num_audio: 1,
            num_data: 0,
            reconfiguration_index: 0,
            toggle: false,
        },
        service: ServiceParams {
            service_id: 0xABCDE,
            short_id: 0,
            audio_ca: false,
            language: 5,
            is_data: false,
            descriptor: 10,
            data_ca: false,
        },
    }
}

/// The complex baseband of `frames` transmission frames of `cfg`.
fn baseband(cfg: TxConfig, frames: usize) -> (Vec<Cplx>, ChannelLayout) {
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    let layout = tx.layout();
    let mut rng = Rng::new(0x9E37_79B9);
    let cap = tx.msc_capacity();
    let fac = test_fac(cfg.occupancy, cfg.msc_mode, cfg.sdc_mode);
    let mut out = Vec::with_capacity(frames * tx.samples_per_frame());
    for _ in 0..frames {
        let msc = rng.bits(cap.total_bits());
        let sdc = (tx.frame_index() == 0).then(|| rng.bytes(tx.sdc_capacity_bytes()));
        tx.transmit_frame_into(&fac, &msc, sdc.as_deref(), &mut out).expect("transmit");
    }
    (out, layout)
}

/// The signal the output stage produces for `cfg` at `level_dbfs`, as interleaved
/// floats, plus the stage (for its statistics).
fn output(sig: &[Cplx], layout: ChannelLayout, cfg: OutputConfig) -> (Vec<f32>, OutputStage) {
    let mut st = OutputStage::new(layout, cfg).expect("output stage");
    let mut out = Vec::new();
    st.process(sig, &mut out);
    (out, st)
}

/// The limited baseband, and the number of samples the limiter found above its knee.
fn limited_baseband(sig: &[Cplx], mean_power: Real, knee_db: Real) -> (Vec<Cplx>, u64) {
    let mut y = sig.to_vec();
    let peaks = apply_peak_limit(&mut y, mean_power, PeakLimit::tanh(knee_db));
    (y, peaks)
}

/// Mean power of a sample sequence.
fn power(x: &[Cplx]) -> Real {
    x.iter().map(|v| v.norm_sqr()).sum::<Real>() / x.len() as Real
}

/// Peak-to-average ratio, dB: `max|x| / √E|x|²`.
fn papr_db(x: &[Cplx]) -> Real {
    20.0 * (x.iter().map(|v| v.norm_sqr()).fold(0.0, Real::max).sqrt() / power(x).sqrt()).log10()
}

/// PAPR of the real part, dB: `max|Re x| / √E[Re² x]`.
fn real_papr_db(x: &[Cplx]) -> Real {
    let p = x.iter().map(|v| v.re * v.re).sum::<Real>() / x.len() as Real;
    20.0 * (x.iter().map(|v| v.re.abs()).fold(0.0, Real::max) / p.sqrt()).log10()
}

/// MER (dB) of `tested` against `reference`: the power in all cells of a symbol over the
/// power of their difference, with a perfect FFT receiver (the limiter is memoryless and
/// adds no delay, so the symbol grid is the reference's).
fn mer_db(reference: &[Cplx], tested: &[Cplx], layout: ChannelLayout, n: usize, guard: usize) -> Real {
    let mut fft = Fft::new(n);
    let (kmin, kmax) = layout.carrier_range();
    let (mut sig, mut err) = (0.0, 0.0);
    for (a, b) in reference.chunks(n + guard).zip(tested.chunks(n + guard)) {
        // Symbol length not a multiple of the FFT: the last partial symbol is ignored.
        if a.len() < n + guard || b.len() < n + guard {
            continue;
        }
        let (mut ra, mut rb) = (a[guard..].to_vec(), b[guard..].to_vec());
        fft.forward(&mut ra);
        fft.forward(&mut rb);
        for k in kmin..=kmax {
            let i = k.rem_euclid(n as i32) as usize;
            let ra = ra[i] / n as Real;
            let rb = rb[i] / n as Real;
            sig += ra.norm_sqr();
            err += (rb - ra).norm_sqr();
        }
    }
    10.0 * (sig / err).log10()
}

/// A hard clip at `ceiling` (baseband units) followed by a power normalisation, i.e. the
/// final ±1 clip when the output level puts `ceiling` at full scale.
fn hard_clipped(reference: &[Cplx], ceiling: Real) -> Vec<Cplx> {
    let mut y: Vec<Cplx> = reference
        .iter()
        .map(|v| {
            let r = v.norm();
            if r > ceiling { v * (ceiling / r) } else { *v }
        })
        .collect();
    let g = (power(reference) / power(&y)).sqrt();
    for v in &mut y {
        v.re *= g;
        v.im *= g;
    }
    y
}

fn report(name: &str, cfg: TxConfig, frames: usize) {
    let rate = cfg.working_rate;
    let (band, layout) = baseband(cfg, frames);
    let mode = layout.mode;
    let (n, guard) = (mode.fft_size_at(rate), mode.guard_len_at(rate));
    println!(
        "\n{name}: {} × {} ms, {} carriers, {:.0} kHz",
        frames,
        mode.frame_ms(),
        layout.num_carriers(),
        f64::from(rate.hz()) / 1000.0
    );
    println!("  baseband PAPR {:.1} dB (complex), {:.1} dB (real part)", papr_db(&band), real_papr_db(&band));

    // Full-scale use of the real-IF output, which is what a sound card receives.
    println!("\n  real-IF output, samples clipped at full scale (one channel):");
    println!("     RMS level     no limiter     6 dB knee");
    let if_hz = decdrm_core::tx::output::suggested_if_hz_at(layout, rate);
    for level in LEVELS_DBFS {
        let (_, st_off) = output(&band, layout, OutputConfig { level_dbfs: level, ..OutputConfig::real_at(if_hz, rate) });
        let (_, st_on) = output(
            &band,
            layout,
            OutputConfig { level_dbfs: level, peak_limit: Some(PeakLimit::tanh(6.0)), ..OutputConfig::real_at(if_hz, rate) },
        );
        println!(
            "    {level:>6.1} dBFS   {:>10}   {:>10}",
            st_off.clipped_samples(), st_on.clipped_samples()
        );
    }

    let mean_power = CellMap::new(layout.mode, layout.occupancy).expect("layout").avg_power_per_symbol;
    println!("\n  knee   peak/RMS   RMS change   peaks > knee        MER   hard clip, same peak");
    println!("   [dB]       [dB]         [dB]        [%]         [dB]        [dB]");
    for knee in KNEES_DB {
        let (lim, peaks) = limited_baseband(&band, mean_power, knee);
        let peak_db = papr_db(&lim);
        let rms_change = 10.0 * (power(&lim) / power(&band)).log10();
        let mer = mer_db(&band, &lim, layout, n, guard);
        // A hard clip at the peak the limiter reached, renormalised to the same power.
        let ceiling = lim.iter().map(|v| v.norm()).fold(0.0, Real::max) * (power(&band) / power(&lim)).sqrt();
        let hard = hard_clipped(&band, ceiling);
        let hard_mer = mer_db(&band, &hard, layout, n, guard);
        let pct = 100.0 * peaks as Real / band.len() as Real;
        println!(
            "   {knee:>4.1}     {peak_db:>5.2}      {rms_change:>+5.2}      {pct:>6.3}     {mer:>6.1}         {hard_mer:>6.1}"
        );
    }
}

fn main() {
    // Mode B, 10 kHz, 64-QAM SM: the DRM30 configuration most sensitive to in-band
    // distortion, and 12 frames is 4.8 s — enough for the statistics to settle.
    report(
        "mode B SO_3 64-QAM",
        TxConfig {
            mode: RobustnessMode::B,
            occupancy: SpectrumOccupancy::SO_3,
            msc_mode: MscMode::Qam64Sm,
            sdc_mode: SdcMode::Qam16,
            interleaving: Interleaving::Short,
            protection: MscProtection { part_a: 0, part_b: 1, hierarchical: 0 },
            part_a_bytes: 0,
            afs_index: 0,
            working_rate: WorkingRate::R48,
        },
        12,
    );
    // Mode E, DRM+, 16-QAM (its strongest MSC mode): 20 frames are 2 s.
    report(
        "mode E SO_0 16-QAM",
        TxConfig {
            mode: RobustnessMode::E,
            occupancy: SpectrumOccupancy::SO_0,
            msc_mode: MscMode::Qam16Sm,
            sdc_mode: SdcMode::Qam4,
            interleaving: Interleaving::Short,
            protection: MscProtection { part_a: 0, part_b: 0, hierarchical: 0 },
            part_a_bytes: 0,
            afs_index: 0,
            working_rate: WorkingRate::R192,
        },
        20,
    );
}
