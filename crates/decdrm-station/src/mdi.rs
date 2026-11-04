//! MDI (TS 102 820) and the station: the content of a transmission frame as an MDI
//! frame — what a content server sends a modulator — for tests and tools
//! ([`crate::Station::capture_mdi`]).

use crate::plan::MultiplexPlan;
use decdrm_core::fac::Fac;
use decdrm_core::params::RobustnessMode;
use decdrm_mdi::mdi::{MdiFac, MdiSdc};
use decdrm_mdi::{MdiFrame, Protocol, Sdci};

/// The `sdci` item of a multiplex: protection levels and stream lengths (with
/// hierarchical modulation stream 0 is the hierarchical stream: its protection level
/// and length).
pub fn sdci(plan: &MultiplexPlan) -> Sdci {
    let hp = plan.tx.protection.hierarchical as u16;
    Sdci {
        protection_a: plan.multiplex.protection_a,
        protection_b: plan.multiplex.protection_b,
        streams: plan
            .streams
            .iter()
            .map(|s| if s.hierarchical { (hp << 10, s.lengths.part_b as u16) } else { (s.lengths.part_a as u16, s.lengths.part_b as u16) })
            .collect(),
    }
}

/// One frame as MDI: `fac` as sent (frame index included), the SDC data field of the
/// super frame it starts (padded to `sdc_capacity` bytes), the logical frames of the
/// streams.
pub fn frame(plan: &MultiplexPlan, dlfc: u32, fac: &Fac, sdc: Option<&[u8]>, sdc_capacity: usize, streams: &[Vec<u8>]) -> MdiFrame {
    let sdc = sdc.map(|d| {
        let mut data = d.to_vec();
        data.resize(sdc_capacity.max(d.len()), 0);
        MdiSdc::new(plan.tx.afs_index, data)
    });
    let mut out = MdiFrame {
        protocol: Some(Protocol::MDI),
        dlfc: Some(dlfc),
        // `robm` names A–D only: a mode E frame is marked by its 116-bit FAC instead
        // (see `decdrm_mdi::mdi`).
        robustness: (plan.tx.mode != RobustnessMode::E).then_some(plan.tx.mode.index() as u8),
        fac: Some(MdiFac::from_bits(&fac.to_bits_for(plan.tx.mode))),
        sdc,
        sdci: Some(sdci(plan)),
        ..MdiFrame::default()
    };
    for (slot, data) in out.streams.iter_mut().zip(streams) {
        *slot = Some(data.clone());
    }
    out
}
