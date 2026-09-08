//! Faithful Rust port of Sebastian Gesemann's dsd2pcm (BSD-2-Clause).
//!
//! Original: "DSD2PCM conversion engine", (c) 2009 Sebastian Gesemann,
//! distributed under the BSD 2-Clause license; the `HTAPS` coefficient table
//! and the FIFO/table-fold algorithm below are ported verbatim from
//! dsd2pcm.c. A symmetric 96-tap FIR low-pass runs at the DSD rate,
//! evaluated bytewise through 6×256 precomputed partial-sum tables, emitting
//! one float sample per input byte (8:1 decimation).

use std::sync::OnceLock;

const HTAPS_LEN: usize = 48;
const FIFO_SIZE: usize = 16;
const FIFO_MASK: usize = FIFO_SIZE - 1;
const CTABLES: usize = 6;

/// Half of the symmetric 96-tap filter (dsd2pcm.c `htaps`, verbatim).
#[rustfmt::skip]
const HTAPS: [f64; HTAPS_LEN] = [
    0.09950731974056658, 0.09562845727714668, 0.08819647126516944,
    0.07782552527068175, 0.06534876523171299, 0.05172629311427257,
    0.0379429484910187, 0.02490921351762261, 0.0133774746265897,
    0.003883043418804416, -0.003284703416210726, -0.008080250212687497,
    -0.01067241812471033, -0.01139427235000863, -0.0106813877974587,
    -0.009007905078766049, -0.006828859761015335, -0.004535184322001496,
    -0.002425035959059578, -0.0006922187080790708, 0.0005700762133516592,
    0.001353838005269448, 0.001713709169690937, 0.001742046839472948,
    0.001545601648013235, 0.001226696225277855, 0.0008704322683580222,
    0.0005381636200535649, 0.000266446345425276, 7.002968738383528e-05,
    -5.279407053811266e-05, -0.0001140625650874684, -0.0001304796361231895,
    -0.0001189970287491285, -9.396247155265073e-05, -6.577634378272832e-05,
    -4.07492895872535e-05, -2.17407957554587e-05, -9.163058931391722e-06,
    -2.017460145032201e-06, 1.249721855219005e-06, 2.166655190537392e-06,
    1.930520892991082e-06, 1.319400334374195e-06, 7.410039764949091e-07,
    3.423230509967409e-07, 1.244182214744588e-07, 3.130441005359396e-08,
];

struct Tables {
    bitreverse: [u8; 256],
    ctables: [[f32; 256]; CTABLES],
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        // Bit-reverse table (dsd2pcm.c precalc, verbatim logic).
        let mut bitreverse = [0u8; 256];
        let mut e: u32 = 0;
        for t in 0..256usize {
            bitreverse[t] = e as u8;
            let mut m: u32 = 128;
            while m != 0 {
                e ^= m;
                if e & m != 0 {
                    break;
                }
                m >>= 1;
            }
        }
        // Partial-sum tables: ctables[CTABLES-1-t][e] = Σ ±htaps[t*8+m].
        let mut ctables = [[0.0f32; 256]; CTABLES];
        for t in 0..CTABLES {
            let k = (HTAPS_LEN - t * 8).min(8);
            for e in 0..256usize {
                let mut acc = 0.0f64;
                for m in 0..k {
                    let sign = (((e >> (7 - m)) & 1) as i32 * 2 - 1) as f64;
                    acc += sign * HTAPS[t * 8 + m];
                }
                ctables[CTABLES - 1 - t][e] = acc as f32;
            }
        }
        Tables { bitreverse, ctables }
    })
}

/// Reverse the bit order of a DSD byte (LSB-first ↔ MSB-first).
pub(crate) fn bit_reverse(b: u8) -> u8 {
    tables().bitreverse[b as usize]
}

/// Streaming DSD→PCM decimator, one instance per channel. Emits one f32
/// sample (at dsd_rate/8) per input DSD byte.
pub struct Dsd2Pcm {
    fifo: [u8; FIFO_SIZE],
    pos: usize,
}

impl Dsd2Pcm {
    pub fn new() -> Self {
        // 0x69 is DSD silence; priming the FIFO with it avoids a start click.
        Self { fifo: [0x69; FIFO_SIZE], pos: 0 }
    }

    /// Translate DSD bytes to float samples, appending to `dst`.
    /// `lsb_first` = bit order within each source byte.
    pub fn translate(&mut self, src: &[u8], lsb_first: bool, dst: &mut Vec<f32>) {
        let tb = tables();
        let mut ffp = self.pos;
        dst.reserve(src.len());
        for &byte in src {
            let bite1 = if lsb_first { tb.bitreverse[byte as usize] } else { byte };
            self.fifo[ffp] = bite1;
            // Pre-reverse the byte that just crossed the filter midpoint so
            // the mirrored half reads straight from the table (dsd2pcm.c).
            let p = (ffp.wrapping_sub(CTABLES)) & FIFO_MASK;
            self.fifo[p] = tb.bitreverse[self.fifo[p] as usize];
            let mut acc = 0.0f64;
            for i in 0..CTABLES {
                let a = self.fifo[(ffp.wrapping_sub(i)) & FIFO_MASK] as usize;
                let b = self.fifo[(ffp.wrapping_sub(CTABLES * 2 - 1).wrapping_add(i)) & FIFO_MASK]
                    as usize;
                acc += (tb.ctables[i][a] + tb.ctables[i][b]) as f64;
            }
            dst.push(acc as f32);
            ffp = (ffp + 1) & FIFO_MASK;
        }
        self.pos = ffp;
    }
}

impl Default for Dsd2Pcm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_in_near_zero_out() {
        let mut d = Dsd2Pcm::new();
        let mut out = Vec::new();
        // 0x69 = 01101001: the classic DSD silence pattern (DC-balanced).
        d.translate(&[0x69; 4096], false, &mut out);
        assert_eq!(out.len(), 4096);
        // Skip the filter warm-up, then expect (near) silence.
        for &s in &out[64..] {
            assert!(s.abs() < 1e-3, "sample {s} not near zero");
        }
    }

    #[test]
    fn all_ones_approaches_full_scale_positive() {
        let mut d = Dsd2Pcm::new();
        let mut out = Vec::new();
        d.translate(&[0xFF; 4096], false, &mut out);
        let tail = &out[512..];
        let avg: f32 = tail.iter().sum::<f32>() / tail.len() as f32;
        // DC gain of the filter is ~1.0 for the all-ones (+1) stream.
        assert!(avg > 0.8, "avg {avg} too low for all-ones input");
    }

    #[test]
    fn lsb_msb_orders_differ_only_by_bit_reversal() {
        let mut a = Dsd2Pcm::new();
        let mut b = Dsd2Pcm::new();
        let (mut oa, mut ob) = (Vec::new(), Vec::new());
        let pattern: Vec<u8> = (0..=255u8).cycle().take(2048).collect();
        let reversed: Vec<u8> = pattern.iter().map(|&x| tables().bitreverse[x as usize]).collect();
        a.translate(&pattern, false, &mut oa);
        b.translate(&reversed, true, &mut ob);
        assert_eq!(oa, ob);
    }
}
