// SPDX-License-Identifier: AGPL-3.0-or-later
//! SSE2 back end (x86_64, where SSE2 is always present).

use super::{F4, MASK, U4, W};
use crate::tables::Table;
use core::arch::x86_64::*;
use core::ops::{Add, Mul, Sub};

#[derive(Clone, Copy)]
pub struct F(__m128);

#[derive(Clone, Copy)]
pub struct U(__m128i);

impl Add for F {
    type Output = F;
    #[inline(always)]
    fn add(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_add_ps(self.0, o.0)) }
    }
}

impl Sub for F {
    type Output = F;
    #[inline(always)]
    fn sub(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_sub_ps(self.0, o.0)) }
    }
}

impl Mul for F {
    type Output = F;
    #[inline(always)]
    fn mul(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_mul_ps(self.0, o.0)) }
    }
}

impl F4 for F {
    type U = U;
    #[inline(always)]
    fn splat(x: f32) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_set1_ps(x)) }
    }
    #[inline(always)]
    fn load(a: [f32; W]) -> F {
        // SAFETY: reads the four floats of `a` (unaligned load).
        F(unsafe { _mm_loadu_ps(a.as_ptr()) })
    }
    #[inline(always)]
    fn store(self) -> [f32; W] {
        let mut a = [0.0; W];
        // SAFETY: writes the four floats of `a` (unaligned store).
        unsafe { _mm_storeu_ps(a.as_mut_ptr(), self.0) };
        a
    }
    #[inline(always)]
    fn min(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_min_ps(self.0, o.0)) }
    }
    #[inline(always)]
    fn max(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_max_ps(self.0, o.0)) }
    }
    #[inline(always)]
    fn ge(self, o: F) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { U(_mm_castps_si128(_mm_cmpge_ps(self.0, o.0))) }
    }
    #[inline(always)]
    fn select(m: U, a: F, b: F) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe {
            let m = _mm_castsi128_ps(m.0);
            F(_mm_or_ps(_mm_and_ps(m, a.0), _mm_andnot_ps(m, b.0)))
        }
    }
    #[inline(always)]
    fn to_i32(self) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { U(_mm_cvttps_epi32(self.0)) }
    }
    #[inline(always)]
    fn to_bits(self) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { U(_mm_castps_si128(self.0)) }
    }
    #[inline(always)]
    fn gather2(t: &Table, i: U) -> (F, F) {
        let p = t.as_ptr();
        let mut k = [0u32; W];
        // SAFETY: each index is masked below N, and the table holds N + 1 floats, so every 8-byte
        // load (t[k], t[k + 1]) is inside it.
        unsafe {
            _mm_storeu_si128(k.as_mut_ptr().cast(), _mm_and_si128(i.0, _mm_set1_epi32(MASK as i32)));
            // one unaligned 8-byte load a lane: (t[k], t[k + 1])
            let pair = |n: u32| _mm_loadl_epi64(p.add(n as usize).cast::<__m128i>());
            let ab = _mm_castsi128_ps(_mm_unpacklo_epi64(pair(k[0]), pair(k[1])));
            let cd = _mm_castsi128_ps(_mm_unpacklo_epi64(pair(k[2]), pair(k[3])));
            (
                F(_mm_shuffle_ps::<0b10_00_10_00>(ab, cd)),
                F(_mm_shuffle_ps::<0b11_01_11_01>(ab, cd)),
            )
        }
    }
}

impl U4 for U {
    type F = F;
    #[inline(always)]
    fn splat(x: u32) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { U(_mm_set1_epi32(x as i32)) }
    }
    #[inline(always)]
    fn load(a: [u32; W]) -> U {
        // SAFETY: reads the 16 bytes of `a` (unaligned load).
        U(unsafe { _mm_loadu_si128(a.as_ptr().cast()) })
    }
    #[inline(always)]
    fn store(self) -> [u32; W] {
        let mut a = [0; W];
        // SAFETY: writes the 16 bytes of `a` (unaligned store).
        unsafe { _mm_storeu_si128(a.as_mut_ptr().cast(), self.0) };
        a
    }
    #[inline(always)]
    fn add(self, o: U) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { U(_mm_add_epi32(self.0, o.0)) }
    }
    #[inline(always)]
    fn mul(self, o: U) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe {
            // SSE2 has no 32-bit low multiply: lanes 0 and 2, then 1 and 3, and interleave the low halves
            let even = _mm_mul_epu32(self.0, o.0);
            let odd = _mm_mul_epu32(_mm_srli_si128::<4>(self.0), _mm_srli_si128::<4>(o.0));
            let lo = |x| _mm_shuffle_epi32::<0b00_00_10_00>(x);
            U(_mm_unpacklo_epi32(lo(even), lo(odd)))
        }
    }
    #[inline(always)]
    fn and(self, o: U) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { U(_mm_and_si128(self.0, o.0)) }
    }
    #[inline(always)]
    fn shr<const N: i32>(self) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { U(_mm_srli_epi32::<N>(self.0)) }
    }
    #[inline(always)]
    fn shl<const N: i32>(self) -> U {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { U(_mm_slli_epi32::<N>(self.0)) }
    }
    #[inline(always)]
    fn small_to_f32(self) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_cvtepi32_ps(self.0)) }
    }
    #[inline(always)]
    fn i32_to_f32(self) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_cvtepi32_ps(self.0)) }
    }
    #[inline(always)]
    fn bits_to_f32(self) -> F {
        // SAFETY: a register-only intrinsic; SSE2 is part of the target's baseline.
        unsafe { F(_mm_castsi128_ps(self.0)) }
    }
}
