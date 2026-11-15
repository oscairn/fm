// SPDX-License-Identifier: AGPL-3.0-or-later
//! The small vector type the kernels are written against: four `f32` or `u32` lanes, one voice a
//! lane. Three back ends implement it: NEON (aarch64), SSE2 (x86_64) and scalar (any target, and
//! the reference the other two are tested against; generic over the lane count, so it also serves
//! a lane holding one voice, one lane wide). Every operation here is exact IEEE single
//! precision or exact integer arithmetic, and `mul_add` is a multiply then an add (never fused),
//! so the three back ends give bit-identical results; `tests` checks that.
//!
//! State lives in memory as plain `[f32; 4]` / `[u32; 4]` arrays, whatever the back end, so a voice
//! layout does not depend on the target.

use crate::tables::{Table, Table16, BITS, N};
use core::ops::{Add, Mul, Sub};

pub mod scalar;

#[cfg(target_arch = "aarch64")]
pub mod neon;

#[cfg(target_arch = "x86_64")]
pub mod sse2;

/// Lanes per vector: voices processed together (a lane of voices; narrower vectors serve lanes
/// holding one or two).
pub const W: usize = 4;

/// The first `L` of four values.
#[inline(always)]
fn first<T: Copy + Default, const L: usize>(a: &[T; W]) -> [T; L] {
    let mut r = [T::default(); L];
    r.iter_mut().zip(a).for_each(|(r, x)| *r = *x);
    r
}

/// The table index mask: every gather reads within the table whatever the lanes hold.
const MASK: u32 = (N as u32).wrapping_sub(1);
/// A 32-bit phase's table index is its top `BITS` bits.
const SHIFT: i32 = 32 - BITS as i32;

/// Four `f32` lanes.
/// `L` lanes (four, a lane of voices, unless narrower).
pub trait F4<const L: usize = W>: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> {
    /// The matching `u32` lanes.
    type U: U4<L, F = Self>;
    fn splat(x: f32) -> Self;
    fn load(a: [f32; L]) -> Self;
    fn store(self) -> [f32; L];
    /// The first `L` of four lanes kept in memory.
    #[inline(always)]
    fn load4(a: &[f32; W]) -> Self {
        Self::load(first(a))
    }
    /// Writes the first `L` of four lanes kept in memory.
    #[inline(always)]
    fn store4(self, a: &mut [f32; W]) {
        a.iter_mut().zip(self.store()).for_each(|(a, x)| *a = x);
    }
    fn min(self, o: Self) -> Self;
    fn max(self, o: Self) -> Self;
    /// All ones in each lane where `self >= o`.
    fn ge(self, o: Self) -> Self::U;
    /// `a` where `m` is all ones, `b` where it is zero.
    fn select(m: Self::U, a: Self, b: Self) -> Self;
    /// Truncating conversion to `i32` bits. The caller keeps `self` within (-2^31, 2^31): outside it
    /// the back ends disagree.
    fn to_i32(self) -> Self::U;
    /// The bit pattern as `u32` lanes.
    fn to_bits(self) -> Self::U;
    /// `t[k]` and `t[k + 1]` for each lane's phase, `k` its top `BITS` bits: a table read and its
    /// neighbour.
    fn gather2(t: &Table, ph: Self::U) -> (Self, Self);
    /// [`F4::gather2`] of a Q14 table, as values (`t[k] / 16384`, exact).
    fn gather2_q14(t: &Table16, ph: Self::U) -> (Self, Self);

    /// `self * b + c`, rounded twice (identical on every back end).
    #[inline(always)]
    fn mul_add(self, b: Self, c: Self) -> Self {
        self * b + c
    }

    /// 2^x for x in [-30, 0] (clamped): degree-5 polynomial on the fraction, 1.6e-4 relative. Control
    /// rate only.
    #[inline(always)]
    fn exp2(self) -> Self {
        let x = self.min(Self::splat(0.0)).max(Self::splat(-30.0));
        // floor(x) for negative x: truncation of x + 30 is its floor, as x + 30 >= 0
        let k = (x + Self::splat(30.0)).to_i32();
        let n = k.i32_to_f32() - Self::splat(30.0);
        let f = x - n;
        let mut p = f.mul_add(Self::splat(1.333_355_8e-3), Self::splat(9.618_129e-3));
        p = p.mul_add(f, Self::splat(5.550_411e-2));
        p = p.mul_add(f, Self::splat(2.402_265e-1));
        p = p.mul_add(f, Self::splat(6.931_472e-1));
        p = p.mul_add(f, Self::splat(1.0));
        // p in [1, 2); add (k - 30) to its exponent field
        p.to_bits()
            .add(k.add(<Self::U as U4<L>>::splat(30u32.wrapping_neg())).shl::<23>())
            .bits_to_f32()
    }
}

/// `L` `u32` lanes; arithmetic wraps.
pub trait U4<const L: usize = W>: Copy {
    type F: F4<L, U = Self>;
    fn splat(x: u32) -> Self;
    fn load(a: [u32; L]) -> Self;
    fn store(self) -> [u32; L];
    /// The first `L` of four lanes kept in memory.
    #[inline(always)]
    fn load4(a: &[u32; W]) -> Self {
        Self::load(first(a))
    }
    /// Writes the first `L` of four lanes kept in memory.
    #[inline(always)]
    fn store4(self, a: &mut [u32; W]) {
        a.iter_mut().zip(self.store()).for_each(|(a, x)| *a = x);
    }
    fn add(self, o: Self) -> Self;
    fn mul(self, o: Self) -> Self;
    fn and(self, o: Self) -> Self;
    fn shr<const N: i32>(self) -> Self;
    fn shl<const N: i32>(self) -> Self;
    /// Exact for lanes below 2^24 (the callers' range).
    fn small_to_f32(self) -> Self::F;
    /// The low 16 bits as a fraction of 2^16 (exact).
    #[inline(always)]
    fn fraction16(self) -> Self::F {
        self.and(Self::splat(0xffff)).small_to_f32() * Self::F::splat(1.0 / 65536.0)
    }
    /// The lanes read as `i32`, converted (exact below 2^24 in magnitude).
    fn i32_to_f32(self) -> Self::F;
    fn bits_to_f32(self) -> Self::F;
}

/// The native back end: NEON on aarch64, SSE2 on x86_64, scalar elsewhere or with the `scalar`
/// feature.
#[cfg(all(target_arch = "aarch64", not(feature = "scalar")))]
pub type Native = neon::F;
#[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
pub type Native = sse2::F;
#[cfg(any(feature = "scalar", not(any(target_arch = "aarch64", target_arch = "x86_64"))))]
pub type Native = scalar::F;

#[cfg(test)]
mod tests {
    use super::*;

    fn probe<V: F4>() -> [[u32; W]; 12] {
        let a = V::load([0.25, -1.5, 3.0e-7, 1000.0]);
        let b = V::load([-0.75, 2.0, 0.333, -0.001]);
        let u = <V::U as U4>::load([0xdead_beef, 7, 0x8000_0001, u32::MAX]);
        let w = <V::U as U4>::load([3, 0xffff_0000, 12345, 2]);
        [
            (a.mul_add(b, a) - b).to_bits().store(),
            a.min(b).max(V::splat(-1.0)).to_bits().store(),
            V::select(a.ge(b), a, b).to_bits().store(),
            (a * V::splat(1000.0)).to_i32().store(),
            V::load([-0.5, -7.25, -29.9, 0.0]).exp2().to_bits().store(),
            u.add(w).mul(w).store(),
            u.shr::<9>().and(w).shl::<3>().store(),
            u.shr::<16>().small_to_f32().to_bits().store(),
            u.fraction16().to_bits().store(),
            <V::U as U4>::load([5, (-9i32) as u32, 0, (-123456i32) as u32])
                .i32_to_f32()
                .to_bits()
                .store(),
            {
                // every phase reads inside the table
                let (lo, hi) = V::gather2(&crate::tables::SINE, u);
                (lo * V::splat(3.0) + hi).to_bits().store()
            },
            {
                let (lo, hi) = V::gather2_q14(&crate::forms::FORMS[37], u);
                (lo * V::splat(3.0) + hi).to_bits().store()
            },
        ]
    }

    #[test]
    fn native_matches_scalar() {
        assert_eq!(probe::<Native>(), probe::<scalar::F>());
    }

    #[test]
    fn exp2_accuracy() {
        for i in 0..=300 {
            let x = -(i as f32) * 0.1;
            let got = <scalar::F>::splat(x).exp2().store()[0];
            let want = 2f32.powf(x);
            assert!(((got - want) / want).abs() < 2e-4, "{x}: {got} against {want}");
        }
    }
}
