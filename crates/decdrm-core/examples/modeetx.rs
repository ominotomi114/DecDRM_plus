//! Which mode E MSC combinations does the transmitter actually accept and encode?
use decdrm_core::cellmap::CellMap;
use decdrm_core::fac::{Interleaving, MscMode, SdcMode};
use decdrm_core::fec::mlc::MscProtection;
use decdrm_core::params::{RobustnessMode, SpectrumOccupancy, WorkingRate};
use decdrm_core::tx::{Transmitter, TxConfig};
fn main() {
    let rate = WorkingRate::R192;
    let map = CellMap::new(RobustnessMode::E, SpectrumOccupancy::SO_0).unwrap();
    for msc in [MscMode::Qam4, MscMode::Qam16Sm] {
        for p in 0..4usize {
            let cfg = TxConfig {
                mode: RobustnessMode::E, occupancy: SpectrumOccupancy::SO_0,
                msc_mode: msc, sdc_mode: SdcMode::Qam4, interleaving: Interleaving::Long,
                protection: MscProtection { part_a: p, part_b: p, hierarchical: 0 },
                part_a_bytes: 0, afs_index: 0, working_rate: rate,
            };
            match Transmitter::new(cfg) {
                Ok(tx) => {
                    let n = tx.msc_capacity().total_bits();
                    println!("{msc:?} prot {p}: OK, {} bytes ({n} bits), cells {}", n / 8, map.msc_cells_per_frame);
                }
                Err(e) => println!("{msc:?} prot {p}: REFUSED - {e}"),
            }
        }
    }
}
