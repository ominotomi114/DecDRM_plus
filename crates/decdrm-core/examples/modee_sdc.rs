//! Diagnostic: does a mode E SDC block demodulate and decode back to what the transmitter
//! encoded? Uses perfect timing, like `modee_fac`, so it isolates the SDC coding and the SDC
//! cell map from the receiver's synchronisation.
//!
//! Usage: cargo run --release -p decdrm-core --example modee_sdc -- [192]

use decdrm_core::cellmap::CellMap;
use decdrm_core::fec::mlc::{MlcDecoder, MlcParams};
use decdrm_core::fec::qam::EqCell;
use decdrm_core::fac::SdcMode;
use decdrm_core::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::rx::ofdm::OfdmDemod;
use decdrm_core::tx::{Transmitter, TxConfig};

fn main() {
    let khz: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(192);
    let rate = WorkingRate::new(khz * 1000).expect("supported rate");
    let mode = RobustnessMode::E;
    let cfg = TxConfig {
        mode,
        occupancy: SpectrumOccupancy::SO_0,
        msc_mode: decdrm_core::fac::MscMode::Qam16Sm,
        sdc_mode: SdcMode::Qam4, // mode E SDC is 4-QAM (8.5.3.2)
        interleaving: decdrm_core::fac::Interleaving::Long,
        protection: decdrm_core::fec::mlc::MscProtection { part_a: 0, part_b: 0, hierarchical: 0 },
        part_a_bytes: 0,
        afs_index: 0,
        working_rate: rate,
    };
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    let map = CellMap::new(mode, SpectrumOccupancy::SO_0).expect("map");

    let n_sdc = map.sdc_cells_per_superframe;
    let params = MlcParams::sdc(SdcMode::Qam4.mapping(), n_sdc);
    println!("mode E: {n_sdc} SDC cells per super frame, {} SDC symbols, {} information bits",
             mode.sdc_symbols(), params.total_bits());

    // Build a known SDC data field and encode it as the transmitter does.
    let mut enc = decdrm_core::fec::mlc::MlcEncoder::new(params.clone());
    let capacity = tx.sdc_capacity_bytes();
    let data: Vec<u8> = (0..capacity).map(|i| (i * 7 + 3) as u8).collect();
    let block = decdrm_core::mux::sdc::sdc_block_bits(0, &data, params.total_bits());
    println!("encoded SDC block: {} bits for {} data bytes", block.len(), data.len());
    let mut sdc_cells = Vec::new();
    enc.encode(&block, &mut sdc_cells);
    println!("-> {} SDC cells", sdc_cells.len());

    // Now the receive side: transmit a frame and demodulate its SDC symbols with perfect timing.
    let cap = tx.msc_capacity();
    let baseband = tx
        .transmit_frame(
            &decdrm_core::fac::Fac {
                channel: decdrm_core::fac::ChannelParams {
                    enhancement: false, frame_index: 0, afs_valid: false,
                    occupancy: SpectrumOccupancy::SO_0,
                    interleaving: decdrm_core::fac::Interleaving::Long,
                    msc_mode: decdrm_core::fac::MscMode::Qam16Sm,
                    sdc_mode: SdcMode::Qam4,
                    num_audio: 1, num_data: 0, reconfiguration_index: 0, toggle: false,
                },
                service: decdrm_core::fac::ServiceParams {
                    service_id: 0, short_id: 0, audio_ca: false, language: 0,
                    is_data: false, descriptor: 0, data_ca: false,
                },
            },
            &vec![0u8; cap.total_bits()],
            Some(&data),
        )
        .expect("transmit_frame");

    let guard = mode.guard_len_at(rate);
    let nu = mode.fft_size_at(rate);
    let ts = mode.symbol_len_at(rate);
    let ns = mode.symbols_per_frame();
    let mut demod = OfdmDemod::new_at(&map, rate);
    let mut symbols: Vec<Vec<decdrm_core::Cplx>> = Vec::new();
    for s in 0..ns {
        let start = s * ts + guard;
        let mut cells = Vec::new();
        demod.demodulate(&baseband[start..start + nu], &mut cells);
        symbols.push(cells);
    }

    // Collect the SDC cells in the map's transmission order (super-frame symbols 0..=4).
    let mut got: Vec<EqCell> = Vec::new();
    for s in 0..mode.sdc_symbols() {
        for &c in map.sdc_carriers(s) {
            got.push(EqCell { sig: symbols[s][c as usize], chan: 1.0 });
        }
    }
    println!("collected {} SDC cells (expect {n_sdc})", got.len());

    let mut dec = MlcDecoder::new(params, 1);
    let mut bits = Vec::new();
    dec.decode(&got, &mut bits);
    println!("decoded {} bits (expect {})", bits.len(), block.len());

    let n = bits.len().min(block.len());
    let diff: Vec<usize> = (0..n).filter(|&i| bits[i] != block[i]).collect();
    println!("bit positions differing: {} of {n}", diff.len());
    if diff.is_empty() && bits.len() == block.len() {
        println!("OK: the SDC round-trips exactly. Any SDC failure in the receiver is therefore");
        println!("    in its synchronisation or equalisation, not in the SDC coding or cell map.");
    } else {
        println!("FAIL: the SDC does not round-trip -> the SDC cell map or coding is wrong.");
        println!("  first differences at {:?}", &diff[..diff.len().min(12)]);
    }
}