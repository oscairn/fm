// SPDX-License-Identifier: AGPL-3.0-or-later
//! Scalar back end: four lanes as plain arrays. The definition the SIMD back ends must match.

use super::{F4, MASK, SHIFT, U4, W};
use crate::tables::{Table, Table16};
use core::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct F(pub [f32; W]);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct U(pub [u32; W]);

#[inline(always)]
fn map(a: F, b: F, f: impl Fn(f32, f32) -> f32) -> F {
    F([
        f(a.0[0], b.0[0]),
        f(a.0[1], b.0[1]),
        f(a.0[2], b.0[2]),
        f(a.0[3], b.0[3]),
    ])
}

#[inline(always)]
fn mapu(a: U, b: U, f: impl Fn(u32, u32) -> u32) -> U {
    U([
        f(a.0[0], b.0[0]),
        f(a.0[1], b.0[1]),
        f(a.0[2], b.0[2]),
        f(a.0[3], b.0[3]),
    ])
}

impl Add for F {
    type Output = F;
    #[inline(always)]
    fn add(self, o: F) -> F {
        map(self, o, |a, b| a + b)
    }
}

impl Sub for F {
    type Output = F;
    #[inline(always)]
    fn sub(self, o: F) -> F {
        map(self, o, |a, b| a - b)
    }
}

impl Mul for F {
    type Output = F;
    #[inline(always)]
    fn mul(self, o: F) -> F {
        map(self, o, |a, b| a * b)
    }
}

impl F4 for F {
    type U = U;
    #[inline(always)]
    fn splat(x: f32) -> F {
        F([x; W])
    }
    #[inline(always)]
    fn load(a: [f32; W]) -> F {
        F(a)
    }
    #[inline(always)]
    fn store(self) -> [f32; W] {
        self.0
    }
    #[inline(always)]
    fn min(self, o: F) -> F {
        // SSE's rule, which NEON reproduces: the second operand unless the first is smaller (this
        // decides min(-0, +0) and NaN alike)
        map(self, o, |a, b| if a < b { a } else { b })
    }
    #[inline(always)]
    fn max(self, o: F) -> F {
        map(self, o, |a, b| if a > b { a } else { b })
    }
    #[inline(always)]
    fn ge(self, o: F) -> U {
        let m = |a: f32, b: f32| if a >= b { u32::MAX } else { 0 };
        U([
            m(self.0[0], o.0[0]),
            m(self.0[1], o.0[1]),
            m(self.0[2], o.0[2]),
            m(self.0[3], o.0[3]),
        ])
    }
    #[inline(always)]
    fn select(m: U, a: F, b: F) -> F {
        let pick = |m: u32, a: f32, b: f32| if m != 0 { a } else { b };
        F([
            pick(m.0[0], a.0[0], b.0[0]),
            pick(m.0[1], a.0[1], b.0[1]),
            pick(m.0[2], a.0[2], b.0[2]),
            pick(m.0[3], a.0[3], b.0[3]),
        ])
    }
    #[inline(always)]
    fn to_i32(self) -> U {
        U(self.0.map(|x| x as i32 as u32))
    }
    #[inline(always)]
    fn to_bits(self) -> U {
        U(self.0.map(f32::to_bits))
    }
    #[inline(always)]
    fn gather2(t: &Table, ph: U) -> (F, F) {
        let at = |k: u32| t.get(k as usize).copied().unwrap_or(0.0);
        let i = ph.0.map(|p| (p >> SHIFT) & MASK);
        (F(i.map(at)), F(i.map(|k| at(k + 1))))
    }
    #[inline(always)]
    fn gather2_q14(t: &Table16, ph: U) -> (F, F) {
        let at = |k: u32| f32::from(t.get(k as usize).copied().unwrap_or(0)) * (1.0 / 16384.0);
        let i = ph.0.map(|p| (p >> SHIFT) & MASK);
        (F(i.map(at)), F(i.map(|k| at(k + 1))))
    }
}

impl U4 for U {
    type F = F;
    #[inline(always)]
    fn splat(x: u32) -> U {
        U([x; W])
    }
    #[inline(always)]
    fn load(a: [u32; W]) -> U {
        U(a)
    }
    #[inline(always)]
    fn store(self) -> [u32; W] {
        self.0
    }
    #[inline(always)]
    fn add(self, o: U) -> U {
        mapu(self, o, u32::wrapping_add)
    }
    #[inline(always)]
    fn mul(self, o: U) -> U {
        mapu(self, o, u32::wrapping_mul)
    }
    #[inline(always)]
    fn and(self, o: U) -> U {
        mapu(self, o, |a, b| a & b)
    }
    #[inline(always)]
    fn shr<const N: i32>(self) -> U {
        U(self.0.map(|x| x.wrapping_shr(N as u32)))
    }
    #[inline(always)]
    fn shl<const N: i32>(self) -> U {
        U(self.0.map(|x| x.wrapping_shl(N as u32)))
    }
    #[inline(always)]
    fn small_to_f32(self) -> F {
        F(self.0.map(|x| x as f32))
    }
    #[inline(always)]
    fn i32_to_f32(self) -> F {
        F(self.0.map(|x| x as i32 as f32))
    }
    #[inline(always)]
    fn bits_to_f32(self) -> F {
        F(self.0.map(f32::from_bits))
    }
}
