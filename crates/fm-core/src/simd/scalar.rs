// SPDX-License-Identifier: AGPL-3.0-or-later
//! Scalar back end: `L` lanes (four unless narrower) as plain arrays. The definition the SIMD back
//! ends must match, and the narrow vectors of lanes holding one or two voices.

use super::{F4, MASK, SHIFT, U4, W};
use crate::tables::{Table, Table16};
use core::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct F<const L: usize = W>(pub [f32; L]);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct U<const L: usize = W>(pub [u32; L]);

#[inline(always)]
fn map<T: Copy, const L: usize>(mut a: [T; L], b: [T; L], f: impl Fn(T, T) -> T) -> [T; L] {
    a.iter_mut().zip(b).for_each(|(a, b)| *a = f(*a, b));
    a
}

impl<const L: usize> Add for F<L> {
    type Output = Self;
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        F(map(self.0, o.0, |a, b| a + b))
    }
}

impl<const L: usize> Sub for F<L> {
    type Output = Self;
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        F(map(self.0, o.0, |a, b| a - b))
    }
}

impl<const L: usize> Mul for F<L> {
    type Output = Self;
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        F(map(self.0, o.0, |a, b| a * b))
    }
}

impl<const L: usize> F4<L> for F<L> {
    type U = U<L>;
    #[inline(always)]
    fn splat(x: f32) -> Self {
        F([x; L])
    }
    #[inline(always)]
    fn load(a: [f32; L]) -> Self {
        F(a)
    }
    #[inline(always)]
    fn store(self) -> [f32; L] {
        self.0
    }
    #[inline(always)]
    fn min(self, o: Self) -> Self {
        // SSE's rule, which NEON reproduces: the second operand unless the first is smaller (this
        // decides min(-0, +0) and NaN alike)
        F(map(self.0, o.0, |a, b| if a < b { a } else { b }))
    }
    #[inline(always)]
    fn max(self, o: Self) -> Self {
        F(map(self.0, o.0, |a, b| if a > b { a } else { b }))
    }
    #[inline(always)]
    fn ge(self, o: Self) -> U<L> {
        U(map(self.to_bits().0, o.to_bits().0, |a, b| {
            if f32::from_bits(a) >= f32::from_bits(b) {
                u32::MAX
            } else {
                0
            }
        }))
    }
    #[inline(always)]
    fn select(m: U<L>, a: Self, b: Self) -> Self {
        let mut r = b.0;
        r.iter_mut().zip(m.0.into_iter().zip(a.0)).for_each(|(r, (m, a))| {
            if m != 0 {
                *r = a
            }
        });
        F(r)
    }
    #[inline(always)]
    fn to_i32(self) -> U<L> {
        U(self.0.map(|x| x as i32 as u32))
    }
    #[inline(always)]
    fn to_bits(self) -> U<L> {
        U(self.0.map(f32::to_bits))
    }
    #[inline(always)]
    fn gather2(t: &Table, ph: U<L>) -> (Self, Self) {
        let at = |k: u32| t.get(k as usize).copied().unwrap_or(0.0);
        let i = ph.0.map(|p| (p >> SHIFT) & MASK);
        (F(i.map(at)), F(i.map(|k| at(k + 1))))
    }
    #[inline(always)]
    fn gather2_q14(t: &Table16, ph: U<L>) -> (Self, Self) {
        let at = |k: u32| f32::from(t.get(k as usize).copied().unwrap_or(0)) * (1.0 / 16384.0);
        let i = ph.0.map(|p| (p >> SHIFT) & MASK);
        (F(i.map(at)), F(i.map(|k| at(k + 1))))
    }
}

impl<const L: usize> U4<L> for U<L> {
    type F = F<L>;
    #[inline(always)]
    fn splat(x: u32) -> Self {
        U([x; L])
    }
    #[inline(always)]
    fn load(a: [u32; L]) -> Self {
        U(a)
    }
    #[inline(always)]
    fn store(self) -> [u32; L] {
        self.0
    }
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        U(map(self.0, o.0, u32::wrapping_add))
    }
    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        U(map(self.0, o.0, u32::wrapping_mul))
    }
    #[inline(always)]
    fn and(self, o: Self) -> Self {
        U(map(self.0, o.0, |a, b| a & b))
    }
    #[inline(always)]
    fn shr<const N: i32>(self) -> Self {
        U(self.0.map(|x| x.wrapping_shr(N as u32)))
    }
    #[inline(always)]
    fn shl<const N: i32>(self) -> Self {
        U(self.0.map(|x| x.wrapping_shl(N as u32)))
    }
    #[inline(always)]
    fn small_to_f32(self) -> F<L> {
        F(self.0.map(|x| x as f32))
    }
    #[inline(always)]
    fn i32_to_f32(self) -> F<L> {
        F(self.0.map(|x| x as i32 as f32))
    }
    #[inline(always)]
    fn bits_to_f32(self) -> F<L> {
        F(self.0.map(f32::from_bits))
    }
}
