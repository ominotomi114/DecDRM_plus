//! Soft-input Viterbi decoder for the punctured DRM convolutional code
//! (maximum-likelihood sequence estimation, as Dream's `CViterbiDecoder`).
//!
//! The encoder starts and ends in the all-zero state (six zero tail bits), so the
//! trellis is initialised in state 0 and traced back from state 0.

use super::BitMetric;
use super::conv::mother_output;
use crate::tables::{CONSTRAINT_LENGTH, NUM_MOTHER_OUTPUTS, NUM_STATES, PunctureMask};
#[cfg(test)]
use crate::fec::puncture::coded_len;
#[cfg(test)]
use crate::tables::{CODE_RATES, GENERATORS, RATE_1_6};

/// Number of distinct mother-code output words: one bit per mother output.
const NUM_WORDS: usize = 1 << NUM_MOTHER_OUTPUTS;

/// Reusable decoder; keeps its decision memory between calls to avoid reallocating.
#[derive(Debug, Default)]
pub struct ViterbiDecoder {
    /// One bit per state per trellis step: which predecessor won.
    decisions: Vec<u64>,
}

/// For new state `s` and predecessor choice `c` (0 ⇒ `s>>1`, 1 ⇒ `(s>>1)|32`), the
/// mother-code output bits packed as `b0 | b1<<1 | ... | b5<<5`.
fn output_table() -> [[u8; 2]; NUM_STATES] {
    let mut t = [[0u8; 2]; NUM_STATES];
    for (s, entry) in t.iter_mut().enumerate() {
        for c in 0..2 {
            let pred = (s >> 1) | (c << 5);
            let reg = (((pred << 1) | (s & 1)) & 0x7F) as u8;
            let mut w = 0u8;
            for j in 0..NUM_MOTHER_OUTPUTS {
                w |= mother_output(reg, j) << j;
            }
            entry[c] = w;
        }
    }
    t
}

/// Decode. `metrics` holds one entry per *transmitted* coded bit, in encoder
/// output order; `masks` is the puncturing table (one per input bit incl. tail).
/// Returns the decoded information bits (tail removed) in `out` and the final
/// path metric normalised by the number of coded bits (a reliability measure —
/// lower is better).
impl ViterbiDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decode(&mut self, metrics: &[BitMetric], masks: &[PunctureMask], out: &mut Vec<u8>) -> f64 {
        let steps = masks.len();
        let num_bits = steps.saturating_sub(CONSTRAINT_LENGTH - 1);
        let outputs = output_table();

        self.decisions.clear();
        self.decisions.resize(steps, 0);

        const INF: f64 = 1e30;
        let mut old = [INF; NUM_STATES];
        old[0] = 0.0;
        let mut new = [0.0f64; NUM_STATES];
        let mut pos = 0usize;
        // Branch metric of each of the 2^6 possible output words for this step.
        let mut word_metric = [0.0f64; NUM_WORDS];

        for (step, &mask) in masks.iter().enumerate() {
            // Gather the soft values of the outputs actually transmitted.
            let mut m = [BitMetric::ERASURE; NUM_MOTHER_OUTPUTS];
            for (j, mj) in m.iter_mut().enumerate() {
                if mask & (1 << j) != 0 {
                    *mj = metrics.get(pos).copied().unwrap_or(BitMetric::ERASURE);
                    pos += 1;
                }
            }
            for (w, wm) in word_metric.iter_mut().enumerate() {
                let mut acc = 0.0;
                for (j, mj) in m.iter().enumerate() {
                    acc += if (w >> j) & 1 == 0 { mj.to0 } else { mj.to1 };
                }
                *wm = acc;
            }

            let mut dec = 0u64;
            for s in 0..NUM_STATES {
                let p0 = s >> 1;
                let p1 = p0 | 32;
                let m0 = old[p0] + word_metric[outputs[s][0] as usize];
                let m1 = old[p1] + word_metric[outputs[s][1] as usize];
                if m1 < m0 {
                    new[s] = m1;
                    dec |= 1u64 << s;
                } else {
                    new[s] = m0;
                }
            }
            self.decisions[step] = dec;
            std::mem::swap(&mut old, &mut new);
        }

        // Trace back from state 0. The decision at step t is the bit shifted out of
        // the register, i.e. the input bit of step t − 6.
        out.clear();
        out.resize(num_bits, 0);
        let mut state = 0usize;
        for i in 0..num_bits {
            let step = steps - 1 - i;
            let bit = ((self.decisions[step] >> state) & 1) as usize;
            state = (state >> 1) | (bit << 5);
            out[num_bits - 1 - i] = bit as u8;
        }
        old[0] / pos.max(1) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fec::conv::encode;
    use crate::fec::puncture::{PunctureSpec, TailRule, mask_table};

    fn hard_metrics(bits: &[u8]) -> Vec<BitMetric> {
        bits.iter()
            .map(|&b| if b == 0 { BitMetric { to0: 0.0, to1: 1.0 } } else { BitMetric { to0: 1.0, to1: 0.0 } })
            .collect()
    }

    fn spec(bits_a: usize, bits_b: usize, rate_a: usize, rate_b: usize) -> PunctureSpec {
        PunctureSpec {
            n1: 100,
            n2: 400,
            bits_a,
            bits_b,
            rate_a,
            rate_b,
            level: 0,
            tail_rule: TailRule::Standard,
            is_fac: false,
        }
    }

    #[test]
    fn decodes_every_code_rate_error_free() {
        // All 15 rates, including mode E's 1/6 and 2/5. 1/6 is the unpunctured mother
        // code, so it is the one rate that exercises mother outputs b₄ and b₅.
        for rate in 0..CODE_RATES.len() {
            let s = spec(96, 500, rate, rate);
            let masks = mask_table(&s);
            let bits: Vec<u8> = (0..596u32).map(|i| (i.wrapping_mul(2654435761) >> 7) as u8 & 1).collect();
            let mut coded = Vec::new();
            encode(&bits, &masks, &mut coded);
            let mut dec = ViterbiDecoder::new();
            let mut out = Vec::new();
            dec.decode(&hard_metrics(&coded), &masks, &mut out);
            assert_eq!(out, bits, "rate index {rate}");
        }
    }

    /// The six-output mother code, checked against the standard's own generator
    /// polynomials rather than against the encoder that uses it: b₀..b₂ repeat as
    /// b₃..b₅, and b₄/b₅ are the only outputs the 1/6 rate adds (§7.3.2).
    #[test]
    fn the_mother_code_is_rate_one_sixth() {
        assert_eq!(GENERATORS, [0o155, 0o117, 0o123, 0o155, 0o117, 0o123]);
        assert_eq!(NUM_MOTHER_OUTPUTS, 6);
        for reg in 0..128u8 {
            for j in 0..3 {
                assert_eq!(mother_output(reg, j), mother_output(reg, j + 3), "reg {reg}, output {j}");
            }
        }
        // 1/6 keeps every output for the information bits. The six tail bits are
        // punctured by table 28, so the total is 6·(bits + 6) minus what the tail
        // pattern drops.
        let bits = 100usize;
        let s = spec(0, bits, RATE_1_6, RATE_1_6);
        let masks = mask_table(&s);
        let info_ones: usize = masks[..bits].iter().map(|m| m.count_ones() as usize).sum();
        assert_eq!(info_ones, 6 * bits, "the 1/6 rate must keep all six outputs");
        assert!(coded_len(&masks) <= 6 * masks.len());
    }

    #[test]
    fn corrects_scattered_errors_at_rate_half() {
        let s = spec(0, 1000, 4, 4);
        let masks = mask_table(&s);
        let bits: Vec<u8> = (0..1000u32).map(|i| ((i * 40503) >> 3) as u8 & 1).collect();
        let mut coded = Vec::new();
        encode(&bits, &masks, &mut coded);
        for i in (5..coded.len()).step_by(29) {
            coded[i] ^= 1;
        }
        let mut out = Vec::new();
        ViterbiDecoder::new().decode(&hard_metrics(&coded), &masks, &mut out);
        assert_eq!(out, bits);
    }
}
