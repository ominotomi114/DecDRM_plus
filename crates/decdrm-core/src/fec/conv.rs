//! Punctured rate-1/6 convolutional encoder (ES 201 980 §7.3.2).

use crate::tables::{CONSTRAINT_LENGTH, GENERATORS, NUM_MOTHER_OUTPUTS, PunctureMask};

/// Parity of the tap-masked 7-bit register for mother-code output `j`.
#[inline]
pub fn mother_output(reg: u8, j: usize) -> u8 {
    ((reg & GENERATORS[j]).count_ones() & 1) as u8
}

/// Encode `bits` (values 0/1) followed by six zero tail bits, keeping the outputs
/// selected by `masks` (one mask per input bit incl. tail, from
/// [`super::puncture::mask_table`]). Outputs of one input bit are emitted in order
/// b₀, b₁, … — six of them for the unpunctured 1/6 rate, fewer as the mask punctures.
pub fn encode(bits: &[u8], masks: &[PunctureMask], out: &mut Vec<u8>) {
    debug_assert_eq!(masks.len(), bits.len() + CONSTRAINT_LENGTH - 1);
    out.clear();
    let mut reg: u8 = 0;
    for (i, &mask) in masks.iter().enumerate() {
        let input = bits.get(i).copied().unwrap_or(0) & 1;
        reg = ((reg << 1) | input) & 0x7F;
        for j in 0..NUM_MOTHER_OUTPUTS {
            if mask & (1 << j) != 0 {
                out.push(mother_output(reg, j));
            }
        }
    }
}
