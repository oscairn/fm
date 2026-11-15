// SPDX-License-Identifier: AGPL-3.0-or-later
//! The operator forms (docs/design.md, "Tables"): single-cycle tables of 1024 points and a guard
//! point, computed at build time from this project's measurements (`forms_data`), stored as Q14
//! (`i16`, value / 16384: the forms peak near 1.01). Not band-limited, as the reference's forms
//! are not (their spectrum is the same at every note).
//!
//! - Forms 1 and 2 (all harmonics): one table each spread, read at the phase plus the modulation.
//! - Forms 3 and 4 (odd harmonics): the form is E(2t) sin(t); a table of E each spread, played as
//!   E(2t + m) sin(t + m) with the modulation m (the measured law of a modulated odd form).
//! - Forms 5 and 6 (resonant): a window table each spread times a sine at the centre harmonic
//!   (formant + 1), played as window(t + m) sin(centre t + m): the centre's sine takes the
//!   modulation once (measured). No table per formant value.

use crate::tables::Table16;

// `FORMS_DATA: [Table16; 48]`, written by the build script (`forms_build`): forms 1-2, then the E
// of forms 3-4, then the windows of forms 5-6, eight spreads each, so form f at spread s is table
// (f - 1) * 8 + s; 48 × 1025 × 2 = 98,400 bytes of read-only data, kept in `tables::BLOCK`.
include!(concat!(env!("OUT_DIR"), "/forms.rs"));

pub static FORMS: &[Table16; 48] = &crate::tables::BLOCK.forms;

/// How an operator's form is played (a plan's choice, per operator).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    Sine,
    /// One table read (forms 1 and 2).
    Table(&'static Table16),
    /// E(2t + m) sin(t + m) (forms 3 and 4).
    Odd(&'static Table16),
    /// window(t + m) sin(centre t + m) (forms 5 and 6), silent while the centre is at or above
    /// Nyquist.
    Resonant(&'static Table16, u32),
}

impl Wave {
    /// Form 0-6 (sine, all harmonics 1 and 2, odd harmonics 1 and 2, resonant 1 and 2), spread 0-7,
    /// formant 0-99 (resonant forms only); out of range: a sine.
    pub fn new(form: i32, spread: i32, formant: i32) -> Wave {
        // form f's table at spread s is FORMS[(f - 1) * 8 + s]
        let t = FORMS.get((form.clamp(1, 6) as usize).saturating_sub(1) * 8 + spread.clamp(0, 7) as usize);
        match (form, t) {
            (1 | 2, Some(t)) => Wave::Table(t),
            (3 | 4, Some(t)) => Wave::Odd(t),
            (5 | 6, Some(t)) => Wave::Resonant(t, formant.clamp(0, 99) as u32 + 1),
            _ => Wave::Sine,
        }
    }

    /// Two table reads a frame (the costlier kernels).
    pub fn two_reads(self) -> bool {
        matches!(self, Wave::Odd(_) | Wave::Resonant(..))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms_data::{DC, HARMONIC, WINDOW};
    use crate::tables::{N, SINE};
    use std::f64::consts::{FRAC_PI_2, PI};

    /// Harmonic k of one cycle of `x` (N points): amplitude and phase of a cos(k x + phase).
    fn dft(x: &[f64], k: usize) -> (f64, f64) {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let a = 2.0 * PI * (k * i % N) as f64 / N as f64;
            re += v * a.cos();
            im -= v * a.sin();
        }
        let s = if k == 0 { 1.0 } else { 2.0 } / N as f64;
        ((re * s).hypot(im * s), im.atan2(re))
    }

    fn value(t: &Table16, i: usize) -> f64 {
        f64::from(t[i % N]) / 16384.0
    }

    /// One cycle of form 1-4 at a spread as played unmodulated (forms 3-4: E(2t) sin(t)).
    fn cycle(form: usize, spread: usize) -> Vec<f64> {
        let t = &FORMS[(form - 1) * 8 + spread];
        (0..N)
            .map(|i| {
                if form <= 2 {
                    value(t, i)
                } else {
                    value(t, 2 * i) * f64::from(SINE[i])
                }
            })
            .collect()
    }

    /// The level and phase errors of components against their measured (component, amplitude,
    /// phase), the phase law's offset per component added: the worst level error (dB) of those
    /// within 40 dB of the strongest, and 60 dB; the worst phase error (radians) within 40 dB.
    fn errors(x: &[f64], want: &[(u16, f32, f32)], step: f64, first: usize) -> (f64, f64, f64) {
        let top = want.iter().fold(0.0f32, |m, w| m.max(w.1.abs()));
        let (mut l40, mut l60, mut ph) = (0.0f64, 0.0f64, 0.0f64);
        for &(k, a, p) in want.iter().filter(|w| usize::from(w.0) < first + 16) {
            let rel = 20.0 * f64::from(a.abs() / top).log10();
            if rel < -60.0 {
                continue;
            }
            let (got, gp) = dft(x, usize::from(k));
            let db = (20.0 * (got / f64::from(a.abs())).log10()).abs();
            // a negative amplitude is the component half a cycle on
            let want_p = f64::from(p) + step * f64::from(k) + if a < 0.0 { PI } else { 0.0 };
            let dp = ((gp - want_p) / (2.0 * PI))
                .round()
                .mul_add(-2.0 * PI, gp - want_p)
                .abs();
            l60 = l60.max(db);
            if rel >= -40.0 {
                l40 = l40.max(db);
                ph = ph.max(dp);
            }
        }
        (l40, l60, ph)
    }

    /// Every form and spread against its measured amplitudes and phases (the first 16 components):
    /// what Q14 storage and the E construction cost.
    #[test]
    fn tables_against_the_measured_data() {
        let (mut w40, mut w60, mut wph) = (0.0f64, 0.0f64, 0.0f64);
        for form in 1..=4 {
            for spread in 0..8 {
                let set = (form - 1) * 8 + spread;
                let x = cycle(form, spread);
                let step = if form <= 2 { FRAC_PI_2 } else { -FRAC_PI_2 };
                let (l40, l60, ph) = errors(&x, HARMONIC[set], step, 1);
                if form <= 2 {
                    assert!(
                        (dft(&x, 0).0 - f64::from(DC[set])).abs() < 1e-4,
                        "form {form} spread {spread} constant"
                    );
                }
                (w40, w60, wph) = (w40.max(l40), w60.max(l60), wph.max(ph));
            }
        }
        for set in 0..16 {
            let x: Vec<f64> = (0..N).map(|i| value(&FORMS[32 + set], i)).collect();
            let (l40, l60, ph) = errors(&x, WINDOW[set], 0.0, 0);
            (w40, w60, wph) = (w40.max(l40), w60.max(l60), wph.max(ph));
        }
        println!("forms against the data: {w40:.4} dB within 40 dB, {w60:.4} dB within 60 dB, {wph:.4} rad");
        assert!(w40 < 0.01 && w60 < 0.1 && wph < 0.002, "{w40} {w60} {wph}");
    }

    /// Where each form sits in its cycle (it shows when it modulates): forms 1 and 2 run from 0 and
    /// peak half a cycle in, forms 3 and 4 peak a quarter cycle in, resonant 2's window peaks half a
    /// cycle in.
    #[test]
    fn where_the_forms_sit() {
        let peak = |x: &[f64]| (0..N).fold(0, |m, i| if x[i] > x[m] { i } else { m });
        for form in 1..=4 {
            for spread in 0..8 {
                let x = cycle(form, spread);
                let want = if form <= 2 { N / 2 } else { N / 4 };
                let off = peak(&x).abs_diff(want);
                let lo = x.iter().fold(1.0f64, |m, &v| m.min(v));
                assert!(off <= N / 256, "form {form} spread {spread}: peak at {}", peak(&x));
                assert!(form > 2 || lo.abs() < 0.01, "form {form} spread {spread}: minimum {lo}");
            }
        }
        for spread in 1..8 {
            let x: Vec<f64> = (0..N).map(|i| value(&FORMS[40 + spread], i)).collect();
            assert!(peak(&x).abs_diff(N / 2) <= N / 256, "resonant 2 spread {spread}");
        }
    }

    #[test]
    fn choosing_a_form() {
        assert_eq!(Wave::new(0, 3, 9), Wave::Sine);
        assert_eq!(Wave::new(2, 3, 9), Wave::Table(&FORMS[11]));
        assert_eq!(Wave::new(4, 9, 9), Wave::Odd(&FORMS[31]));
        assert_eq!(Wave::new(5, 0, 9), Wave::Resonant(&FORMS[32], 10));
        assert_eq!(Wave::new(7, 0, 0), Wave::Sine);
        assert!(FORMS.iter().all(|t| t[N] == t[0]));
        // the build script's dimensions are the tables'
        assert_eq!(
            (crate::forms_build::N, crate::forms_build::LEN),
            (N, crate::tables::LEN)
        );
        assert!(crate::forms_build::build() == *FORMS);
        // (the device's and the cost gate's targets; macOS x86_64's linker places it at 4 KB)
        #[cfg(target_arch = "aarch64")]
        assert_eq!(core::ptr::addr_of!(crate::tables::BLOCK) as usize % 8192, 0);
    }
}
