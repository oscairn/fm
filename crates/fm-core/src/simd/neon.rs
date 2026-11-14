// SPDX-License-Identifier: AGPL-3.0-or-later
//! NEON back end (aarch64, where NEON is always present).

use super::{F4, MASK, U4, W};
use crate::tables::Table;
use core::arch::aarch64::*;
use core::ops::{Add, Mul, Sub};

#[derive(Clone, Copy)]
pub struct F(float32x4_t);

#[derive(Clone, Copy)]
pub struct U(uint32x4_t);

impl Add for F {
    type Output = F;
    #[inline(always)]
    fn add(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vaddq_f32(self.0, o.0)) }
    }
}

impl Sub for F {
    type Output = F;
    #[inline(always)]
    fn sub(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vsubq_f32(self.0, o.0)) }
    }
}

impl Mul for F {
    type Output = F;
    #[inline(always)]
    fn mul(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vmulq_f32(self.0, o.0)) }
    }
}

impl F4 for F {
    type U = U;
    #[inline(always)]
    fn splat(x: f32) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vdupq_n_f32(x)) }
    }
    #[inline(always)]
    fn load(a: [f32; W]) -> F {
        // SAFETY: reads the four floats of `a`.
        F(unsafe { vld1q_f32(a.as_ptr()) })
    }
    #[inline(always)]
    fn store(self) -> [f32; W] {
        let mut a = [0.0; W];
        // SAFETY: writes the four floats of `a`.
        unsafe { vst1q_f32(a.as_mut_ptr(), self.0) };
        a
    }
    // compare and select rather than vminq/vmaxq: the scalar and SSE2 rule for signed zeros
    #[inline(always)]
    fn min(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vbslq_f32(vcltq_f32(self.0, o.0), self.0, o.0)) }
    }
    #[inline(always)]
    fn max(self, o: F) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vbslq_f32(vcgtq_f32(self.0, o.0), self.0, o.0)) }
    }
    #[inline(always)]
    fn ge(self, o: F) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vcgeq_f32(self.0, o.0)) }
    }
    #[inline(always)]
    fn select(m: U, a: F, b: F) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vbslq_f32(m.0, a.0, b.0)) }
    }
    #[inline(always)]
    fn to_i32(self) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vreinterpretq_u32_s32(vcvtq_s32_f32(self.0))) }
    }
    #[inline(always)]
    fn to_bits(self) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vreinterpretq_u32_f32(self.0)) }
    }
    #[inline(always)]
    fn gather2(t: &Table, i: U) -> (F, F) {
        let p = t.as_ptr();
        // SAFETY: each index is masked below N, and the table holds N + 1 floats, so every 8-byte
        // load (t[k], t[k + 1]) is inside it.
        unsafe {
            let k = vandq_u32(i.0, vdupq_n_u32(MASK));
            let a = vld1_f32(p.add(vgetq_lane_u32::<0>(k) as usize));
            let b = vld1_f32(p.add(vgetq_lane_u32::<1>(k) as usize));
            let c = vld1_f32(p.add(vgetq_lane_u32::<2>(k) as usize));
            let d = vld1_f32(p.add(vgetq_lane_u32::<3>(k) as usize));
            let u = vuzpq_f32(vcombine_f32(a, b), vcombine_f32(c, d));
            (F(u.0), F(u.1))
        }
    }
}

impl U4 for U {
    type F = F;
    #[inline(always)]
    fn splat(x: u32) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vdupq_n_u32(x)) }
    }
    #[inline(always)]
    fn load(a: [u32; W]) -> U {
        // SAFETY: reads the four words of `a`.
        U(unsafe { vld1q_u32(a.as_ptr()) })
    }
    #[inline(always)]
    fn store(self) -> [u32; W] {
        let mut a = [0; W];
        // SAFETY: writes the four words of `a`.
        unsafe { vst1q_u32(a.as_mut_ptr(), self.0) };
        a
    }
    #[inline(always)]
    fn add(self, o: U) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vaddq_u32(self.0, o.0)) }
    }
    #[inline(always)]
    fn mul(self, o: U) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vmulq_u32(self.0, o.0)) }
    }
    #[inline(always)]
    fn and(self, o: U) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vandq_u32(self.0, o.0)) }
    }
    #[inline(always)]
    fn shr<const N: i32>(self) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vshrq_n_u32::<N>(self.0)) }
    }
    #[inline(always)]
    fn shl<const N: i32>(self) -> U {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { U(vshlq_n_u32::<N>(self.0)) }
    }
    #[inline(always)]
    fn small_to_f32(self) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vcvtq_f32_u32(self.0)) }
    }
    #[inline(always)]
    fn i32_to_f32(self) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vcvtq_f32_s32(vreinterpretq_s32_u32(self.0))) }
    }
    #[inline(always)]
    fn bits_to_f32(self) -> F {
        // SAFETY: a register-only intrinsic; NEON is part of the target's baseline.
        unsafe { F(vreinterpretq_f32_u32(self.0)) }
    }
}
