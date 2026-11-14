// SPDX-License-Identifier: AGPL-3.0-or-later
//! NEON back end (aarch64, where NEON is always present).

use super::{F4, MASK, SHIFT, U4, W};
use crate::tables::{Table, Table16};
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
    fn gather2(t: &Table, ph: U) -> (F, F) {
        let p = t.as_ptr().cast::<u8>();
        // SAFETY: each index is masked below N, and the table holds N + 1 floats, so every 8-byte
        // load (t[k], t[k + 1]) is inside it. The byte offsets are formed in the vector, so each
        // lane's load takes its offset as it is extracted.
        unsafe {
            let k = vandq_u32(vshrq_n_u32::<{ SHIFT - 2 }>(ph.0), vdupq_n_u32(MASK << 2));
            let at = |o: u32| vld1_f32(p.add(o as usize).cast::<f32>());
            let a = at(vgetq_lane_u32::<0>(k));
            let b = at(vgetq_lane_u32::<1>(k));
            let c = at(vgetq_lane_u32::<2>(k));
            let d = at(vgetq_lane_u32::<3>(k));
            let u = vuzpq_f32(vcombine_f32(a, b), vcombine_f32(c, d));
            (F(u.0), F(u.1))
        }
    }
    #[inline(always)]
    fn gather2_q14(t: &Table16, ph: U) -> (F, F) {
        let p = t.as_ptr().cast::<u8>();
        // SAFETY: each index is masked below N, and the table holds N + 1 values, so every 4-byte
        // load (t[k], t[k + 1]) is inside it; unaligned loads are allowed. Little-endian: t[k] is
        // the low half. The conversions are exact (16 significant bits, power-of-two scales).
        unsafe {
            let k = vandq_u32(vshrq_n_u32::<{ SHIFT - 1 }>(ph.0), vdupq_n_u32(MASK << 1));
            let pair = |o: u32| core::ptr::read_unaligned(p.add(o as usize).cast::<u32>());
            let mut v = vdupq_n_u32(pair(vgetq_lane_u32::<0>(k)));
            v = vsetq_lane_u32::<1>(pair(vgetq_lane_u32::<1>(k)), v);
            v = vsetq_lane_u32::<2>(pair(vgetq_lane_u32::<2>(k)), v);
            v = vsetq_lane_u32::<3>(pair(vgetq_lane_u32::<3>(k)), v);
            let v = vreinterpretq_s32_u32(v);
            (
                F(vcvtq_n_f32_s32::<30>(vshlq_n_s32::<16>(v))),
                F(vcvtq_n_f32_s32::<14>(vshrq_n_s32::<16>(v))),
            )
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
    fn fraction16(self) -> F {
        // SAFETY: as above; exact, as the scalar definition
        unsafe { F(vcvtq_n_f32_u32::<16>(vandq_u32(self.0, vdupq_n_u32(0xffff)))) }
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
