// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sine the tables are computed with, shared with the build script (exact IEEE arithmetic, so
//! the same on every host).

/// sin(2 pi t) for t >= 0, in f64 (compile time and the build script only).
pub const fn sin_turns(t: f64) -> f64 {
    let mut r = t - (t as u64) as f64;
    let mut sign = 1.0;
    if r >= 0.5 {
        r -= 0.5;
        sign = -1.0;
    }
    if r > 0.25 {
        r = 0.5 - r;
    }
    let x = 2.0 * core::f64::consts::PI * r;
    let x2 = x * x;
    // Taylor series to x^19 on [0, pi/2]: error below 1e-15
    let mut term = x;
    let mut sum = x;
    let mut k = 1;
    while k < 10 {
        term = -term * x2 / ((2 * k) as f64 * (2 * k + 1) as f64);
        sum += term;
        k += 1;
    }
    sign * sum
}
