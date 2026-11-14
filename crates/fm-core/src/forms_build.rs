// SPDX-License-Identifier: AGPL-3.0-or-later
//! The operator form tables computed from the measured data (`forms_data`): run by the build
//! script, which writes them as read-only data for `forms` (evaluating them as constants took 30 s
//! a build). Plain IEEE arithmetic, so the tables are the same whichever host builds them.
#![allow(clippy::indexing_slicing)] // build time only: an out-of-range index fails the build

use crate::forms_data::{DC, HARMONIC, WINDOW};
use crate::turns::sin_turns;

/// Points a cycle and stored points (the guard point repeats the first), as `tables::N`, `LEN`.
pub const N: usize = 1024;
pub const LEN: usize = N + 1;
/// Tables: forms 1-2, then the E of forms 3-4, then the windows of forms 5-6; eight spreads each,
/// so form f at spread s is table (f - 1) * 8 + s.
pub const COUNT: usize = 48;
type Table16 = [i16; LEN];

/// cos(2 pi i / N).
const COS: [f64; N] = {
    let mut c = [0.0; N];
    let mut i = 0;
    while i < N {
        c[i] = sin_turns(i as f64 / N as f64 + 0.25);
        i += 1;
    }
    c
};

/// cos(x) and sin(x) for |x| < 1000 radians (compile time).
const fn cos_sin(x: f64) -> (f64, f64) {
    let t = x / (2.0 * core::f64::consts::PI) + 1000.0;
    (sin_turns(t + 0.25), sin_turns(t))
}

/// Adds a cos(k x) + b sin(k x), x = 2 pi i / N, to every point.
const fn add(acc: &mut [f64; N], k: usize, a: f64, b: f64) {
    let mask = N - 1;
    let (mut i, mut j) = (0, 0);
    while i < N {
        acc[i] += a * COS[j] + b * COS[(j + 3 * N / 4) & mask];
        j = (j + k) & mask;
        i += 1;
    }
}

/// a cos(x + phase) as (cos, sin) coefficients.
const fn coefficients(a: f64, phase: f64) -> (f64, f64) {
    let (c, s) = cos_sin(phase);
    (a * c, -a * s)
}

const fn quantise(acc: &[f64; N]) -> Table16 {
    let mut t = [0i16; LEN];
    let mut i = 0;
    while i < LEN {
        let x = acc[i % N] * 16384.0;
        // rounded to nearest; every form is within +-2
        t[i] = if x >= 0.0 {
            (x + 0.5) as i16
        } else {
            -((0.5 - x) as i16)
        };
        i += 1;
    }
    t
}

/// Form 1 or 2 at a spread (`set` 0-15): the constant plus each harmonic h as
/// cos(h x + phase + h pi / 2), the measured phases placing the form in its cycle as the reference
/// does (it starts at its minimum, 0, and peaks half a cycle in).
const fn harmonic(set: usize) -> Table16 {
    let mut acc = [DC[set] as f64; N];
    let h = HARMONIC[set];
    let mut k = 0;
    while k < h.len() {
        let (n, a, p) = h[k];
        let (c, s) = coefficients(a as f64, p as f64 + n as f64 * core::f64::consts::FRAC_PI_2);
        add(&mut acc, n as usize, c, s);
        k += 1;
    }
    quantise(&acc)
}

/// E of form 3 or 4 at a spread (`set` 16-31), the form being E(2t) sin(t), t in radians, with
/// each odd harmonic h = 2K + 1 as cos(h t + phase - h pi / 2) = C cos(h t) + S sin(h t):
/// sin(h t) = sin(t) (1 + 2 sum cos(2k t)) and cos(h t) = cos(t) - 2 sin(t) sum sin(2k t), k = 1..K,
/// so E gains S (1 + 2 sum cos(k x)) - 2 C sum sin(k x). The cos(t) terms cannot be written so:
/// they are left out (their sum is at most 6e-4 of a sine here).
const fn odd_envelope(set: usize) -> Table16 {
    const K: usize = 128;
    let (mut ea, mut eb) = ([0.0f64; K], [0.0f64; K]);
    let h = HARMONIC[set];
    let mut top = 0;
    let mut k = 0;
    while k < h.len() {
        let (n, a, p) = h[k];
        k += 1;
        if n % 2 == 0 {
            continue; // (an odd form has none)
        }
        let (c, s) = coefficients(a as f64, p as f64 - n as f64 * core::f64::consts::FRAC_PI_2);
        let kk = (n as usize - 1) / 2;
        ea[0] += s;
        let mut j = 1;
        while j <= kk {
            ea[j] += 2.0 * s;
            eb[j] -= 2.0 * c;
            j += 1;
        }
        if kk > top {
            top = kk;
        }
    }
    let mut acc = [0.0f64; N];
    let mut j = 0;
    while j <= top {
        add(&mut acc, j, ea[j], eb[j]);
        j += 1;
    }
    quantise(&acc)
}

/// The window of form 5 or 6 at a spread (`set` 0-15): each component d as cos(d x + phase).
const fn window(set: usize) -> Table16 {
    let mut acc = [0.0f64; N];
    let w = WINDOW[set];
    let mut k = 0;
    while k < w.len() {
        let (d, a, p) = w[k];
        let (c, s) = coefficients(a as f64, p as f64);
        add(&mut acc, d as usize, c, s);
        k += 1;
    }
    quantise(&acc)
}

pub fn build() -> [Table16; COUNT] {
    let mut t = [[0i16; LEN]; COUNT];
    let mut s = 0;
    while s < 16 {
        t[s] = harmonic(s);
        t[16 + s] = odd_envelope(16 + s);
        t[32 + s] = window(s);
        s += 1;
    }
    t
}
