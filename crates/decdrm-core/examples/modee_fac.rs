//! Diagnostic: decode a mode E frame the way the RECEIVER does, from the transmitter's own
//! baseband, with perfect timing (no acquisition, no frame sync, no equalisation).
//!
//! If the FAC still fails here, the bug is in the cell map or the FAC coding, not in the
//! receiver's synchronisation. If it passes, the bug is in sync or equalisation.
//!
//! Usage: cargo run --release -p decdrm-core --example modee_fac -- [192]

use decdrm_core::cellmap::CellMap;
use decdrm_core::fac::{ChannelParams, Fac, Interleaving, MscMode, SdcMode, ServiceParams};
use decdrm_core::fec::mlc::{MlcDecoder, MlcParams, MscProtection};
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
        msc_mode: MscMode::Qam16Sm,
        sdc_mode: SdcMode::Qam4,
        interleaving: Interleaving::Long,
        protection: MscProtection { part_a: 0, part_b: 0, hierarchical: 0 },
        part_a_bytes: 0,
        afs_index: 0,
        working_rate: rate,
    };
    let mut tx = Transmitter::new(cfg).expect("transmitter");
    let map = CellMap::new(mode, SpectrumOccupancy::SO_0).expect("map");
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
    let sent = tx.fac_for_next_frame(&fac);
    println!("sent FAC: occupancy {} services {} service id {:#x}",
             sent.channel.occupancy.value(), sent.channel.num_audio, sent.service.service_id);

    let cap = tx.msc_capacity();
    let msc = vec![0u8; cap.total_bits()];
    let baseband = tx.transmit_frame(&fac, &msc, Some(&[])).expect("transmit_frame");
    let ns = map.symbols_per_frame;
    let guard = mode.guard_len_at(rate);
    let nu = mode.fft_size_at(rate);
    let ts = mode.symbol_len_at(rate);
    println!("frame {} samples = {} symbols of {}", baseband.len(), baseband.len() / ts, ts);

    // Demodulate every symbol with perfect timing: window = guard..guard+nu.
    let mut demod = OfdmDemod::new_at(&map, rate);
    let mut frames: Vec<Vec<decdrm_core::Cplx>> = Vec::new();
    for s in 0..ns {
        let start = s * ts + guard;
        let mut cells = Vec::new();
        demod.demodulate(&baseband[start..start + nu], &mut cells);
        frames.push(cells);
    }

    // Collect the FAC cells exactly as the receiver's demap does, in the map's order.
    let mut fac_cells = Vec::new();
    for s in 0..ns {
        for &c in map.fac_carriers(s) {
            fac_cells.push(decdrm_core::fec::qam::EqCell { sig: frames[s][c as usize], chan: 1.0 });
        }
    }
    println!("collected {} FAC cells (expect {})", fac_cells.len(),
             decdrm_core::tables::num_fac_cells(mode));

    // What the encoder made, for comparison.
    let want_bits = sent.to_bits_for(mode);
    println!("encoder produced {} bits; CRC over the first 108", want_bits.len());

    let mut dec = MlcDecoder::new(MlcParams::fac_for(mode), 0);
    let mut bits = Vec::new();
    dec.decode(&fac_cells, &mut bits);
    println!("decoded {} bits (expect {})", bits.len(), 116);
    let n = bits.len().min(want_bits.len());
    let diff: Vec<usize> = (0..n).filter(|&i| bits[i] != want_bits[i]).collect();
    println!("bit positions differing: {} of {n} -> {:?}", diff.len(),
             &diff[..diff.len().min(20)]);
    match Fac::parse(&bits) {
        Some(got) => {
            println!("OK: FAC decoded");
            println!("  occupancy {} (want {})", got.channel.occupancy.value(), sent.channel.occupancy.value());
            println!("  num_audio {} (want {})", got.channel.num_audio, sent.channel.num_audio);
            println!("  service id {:#x} (want {:#x})", got.service.service_id, sent.service.service_id);
            println!("  language {} (want {})", got.service.language, sent.service.language);
            println!("  descriptor {} (want {})", got.service.descriptor, sent.service.descriptor);
        }
        None => println!("FAIL: FAC did not parse — the cell map or the FAC coding is wrong"),
    }
}