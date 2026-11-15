// SPDX-License-Identifier: AGPL-3.0-or-later
//! Single-cycle tables, computed at compile time (read-only data, shared by every instance and
//! process through the page cache). `LEN` is one cycle plus a guard point for interpolation.

use crate::simd::{F4, U4};
use crate::turns::sin_turns;

/// Index bits of a table: 1024 points a cycle.
pub const BITS: u32 = 10;
/// Points a cycle.
pub const N: usize = 1 << BITS;
/// Stored points: a cycle and the guard point (equal to the first).
pub const LEN: usize = N + 1;

pub type Table = [f32; LEN];

/// A table stored as Q14 (`i16`, value / 16384): half the bytes, for the operator forms.
pub type Table16 = [i16; LEN];

#[allow(clippy::indexing_slicing)] // compile-time evaluation: an out-of-range index fails the build
const fn build(form: Form) -> Table {
    let mut t = [0.0f32; LEN];
    let mut i = 0;
    while i < LEN {
        let x = (i % N) as f64 / N as f64;
        t[i] = match form {
            Form::Sine => sin_turns(x),
            Form::Saw16 => saw16(x),
            Form::Window => window(x),
        } as f32;
        i += 1;
    }
    t
}

// const fn cannot call through a function pointer, so the table kinds are named here
enum Form {
    Sine,
    Saw16,
    Window,
}

/// Placeholder spectral form for the spike: harmonics 1-16 at 1/k (a band-limited saw), scaled to a
/// peak of about 1. Not the reference's law: it has the cost of any single-table form.
const fn saw16(t: f64) -> f64 {
    let mut s = 0.0;
    let mut k = 1;
    while k <= 16 {
        s += sin_turns(t * k as f64) / k as f64;
        k += 1;
    }
    s * 0.55
}

/// A raised-cosine window peaking half a cycle in, the shape the resonant forms multiply a sine by.
/// Placeholder width (the real window depends on the skirt).
const fn window(t: f64) -> f64 {
    0.5 - 0.5 * sin_turns(t + 0.25)
}

/// The tables the engine reads at audio rate, the sine and the operator forms, in one block
/// aligned to the first-level cache's way (8 KB of the device's 32 KB 4-way cache): the sets
/// their lines fall in do not depend on where the linker places them (tools/cost/cg.sh).
#[repr(C, align(8192))]
pub struct Block {
    pub sine: Table,
    pub forms: [Table16; 48],
}

pub static BLOCK: Block = Block {
    sine: build(Form::Sine),
    forms: crate::forms::FORMS_DATA,
};
pub static SINE: &Table = &BLOCK.sine;
pub static SAW16: Table = build(Form::Saw16);
pub static WINDOW: Table = build(Form::Window);

/// Linear interpolation in `t` at 32-bit phase `p`: the top `BITS` bits index, the next 16 are the
/// fraction.
#[inline(always)]
pub fn read<const L: usize, V: F4<L>>(t: &Table, p: V::U) -> V {
    let f = p.shr::<{ 16 - BITS as i32 }>().fraction16();
    let (a, b) = V::gather2(t, p);
    (b - a).mul_add(f, a)
}

/// [`read`] of a Q14 table.
#[inline(always)]
pub fn read16<const L: usize, V: F4<L>>(t: &Table16, p: V::U) -> V {
    let f = p.shr::<{ 16 - BITS as i32 }>().fraction16();
    let (a, b) = V::gather2_q14(t, p);
    (b - a).mul_add(f, a)
}

/// A modulation input in cycles as a 32-bit phase offset (2^-24 cycle resolution; inputs clamped to
/// +-127 cycles so the conversion is exact on every back end).
#[inline(always)]
pub fn offset<const L: usize, V: F4<L>>(m: V) -> V::U {
    (m.min(V::splat(127.0)).max(V::splat(-127.0)) * V::splat(16_777_216.0))
        .to_i32()
        .shl::<8>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_what_they_say() {
        for (i, &v) in SINE.iter().enumerate() {
            let want = (2.0 * std::f64::consts::PI * (i % N) as f64 / N as f64).sin();
            assert!((v as f64 - want).abs() < 1e-7, "sine {i}");
        }
        assert_eq!(SINE[N], SINE[0]);
        assert_eq!(SAW16[N], SAW16[0]);
        let peak = SAW16.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.9 && peak < 1.05, "saw peak {peak}");
        assert!(WINDOW[0].abs() < 1e-7 && (WINDOW[N / 2] - 1.0).abs() < 1e-7);
    }
}
