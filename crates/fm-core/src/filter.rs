// SPDX-License-Identifier: AGPL-3.0-or-later
//! What follows a voice's operators (docs/design.md, 3.7): the part's filter with its envelope,
//! and pan. The measured laws (raw values to Hz, Q, cents and gains), a sound's prepared values
//! (`Laws`), and a lane's state and kernels (`Voices`): coefficients at control rate, ramped a
//! frame; the sections at audio rate across the lane's voices.
//!
//! Every type is built from two sections the existing profile's measurements fit with no free
//! shape parameter: a state-variable section (`lp += F bp; hp = x - lp - d bp; bp += F hp`, with
//! F = 2 sin(pi fc / rate) and damping d = 1 / Q; low-pass, band-pass, high-pass, and the notch as
//! low-pass plus high-pass) and a one-pole low-pass (`y += F (x - y)`) with the same F.

use crate::engine::STEP;
use crate::laws::{self, interpolate};
use crate::params::{Common, Filter as P, Part};
use crate::simd::{scalar, F4, W};
use crate::sound::Sound;

// ---- the laws ----

/// The filter types, in the model's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Lp24,
    Lp24A,
    Lp18,
    Lp18S,
    Lp12Hp12,
    Lp6Hp12,
    Hp24,
    Hp12,
    Bp12,
    BpWide,
    Bp6,
    Be12,
    Be6,
    DualLp,
    DualHp,
    DualBp,
    DualBe,
    Lp12Bp6,
    Thru,
}

/// The type a stored value plays: the stored values skip 9, 18 and 20 (no type of the model:
/// played as thru, and reported as not covered).
pub fn kind(raw: i32) -> Kind {
    use Kind::*;
    const STORED: [Kind; 22] = [
        Lp24, Lp24A, Lp18, Lp18S, Lp12Hp12, Lp6Hp12, Hp24, Hp12, Bp12, Thru, BpWide, Bp6, Be12, Be6, DualLp, DualHp,
        DualBp, DualBe, Thru, Lp12Bp6, Thru, Thru,
    ];
    usize::try_from(raw)
        .ok()
        .and_then(|i| STORED.get(i))
        .copied()
        .unwrap_or(Thru)
}

/// Whether a stored filter type value is one of the model's types.
pub fn is_type(raw: i16) -> bool {
    (0..=21).contains(&raw) && !matches!(raw, 9 | 18 | 20)
}

/// Straight lines through `(x, y)` (`x` ascending), held at both ends; a binary search, for
/// control rate.
fn lerp(x: &[f32], y: &[f32], v: f32) -> f32 {
    let i = x.partition_point(|&k| k <= v);
    let lo = i.checked_sub(1).and_then(|j| x.get(j).zip(y.get(j)));
    match (lo, x.get(i).zip(y.get(i))) {
        (Some((&x0, &y0)), Some((&x1, &y1))) => y0 + (y1 - y0) * (v - x0) / (x1 - x0),
        (Some((_, &y0)), None) => y0,
        (None, Some((_, &y1))) => y1,
        _ => 0.0,
    }
}

/// The cutoff, Hz, at cutoff value 0-1023 (fractions between): the existing profile's measured
/// table (every value within 10 of a measured one), straight in Hz between its measured points.
pub fn cutoff_hz(value: f32) -> f32 {
    lerp(&CUTOFF_X, &CUTOFF_HZ, value)
}

/// The highest cutoff the filter plays (F = 1 at 44.1 kHz).
pub const MAX_HZ: f32 = 7350.21;

/// The most damping (1 / Q) the filter allows at cutoff `fc` Hz: the reference raises its least Q
/// from 3.57 kHz to 1 at the top (measured).
pub fn max_damping(fc: f32) -> f32 {
    1.0 / lerp(&MIN_Q_HZ, &MIN_Q, fc).max(0.5)
}

/// Resonance 0-127 as a Q: 0.5 × 2^(r / 16) (confirmed to 0.01 at 16-127 on the high-pass).
pub fn q(resonance: i32) -> f64 {
    0.5 * laws::exp2(f64::from(resonance.clamp(0, 127)) / 16.0)
}

/// The share of a velocity sensitivity -64..+63 (both ends full scale) and the velocity it reads:
/// a negative sensitivity mirrors the velocity.
fn sensitivity(sens: i32, velocity: f64) -> (f64, f64) {
    let (s, v) = (sens.clamp(-64, 63), velocity.clamp(1.0, 127.0));
    if s >= 0 {
        (f64::from(s) / 63.0, v)
    } else {
        (f64::from(-s) / 64.0, 128.0 - v)
    }
}

/// 2^u, straight in frequency within each octave (exact at whole octaves): how the reference
/// moves the cutoff by key follow and upward velocity.
fn linear_octaves(u: f64) -> f64 {
    let t = u as i32;
    let e = if f64::from(t) > u { t.saturating_sub(1) } else { t };
    laws::exp2(f64::from(e)) * (1.0 + u - f64::from(e))
}

/// Cutoff (and high-pass) key follow -64..+63 at `key`: a frequency ratio, 3.125 % steps (+63
/// plays 200 %) about key 60, straight in frequency within each octave (measured; up to 16 cents
/// off at fractional octaves).
pub fn key_follow_ratio(sens: i32, key: i32) -> f64 {
    let s = sens.clamp(-64, 63);
    let percent = if s == 63 { 200.0 } else { f64::from(s) * 3.125 };
    linear_octaves(percent / 100.0 * f64::from(key.clamp(-128, 255) - 60) / 12.0)
}

/// Cutoff velocity sensitivity -64..+63: a frequency ratio. At +63 a measured curve in cents
/// (-9609 at velocity 1, 0 near 114, +1201 at 127); other sensitivities scale downward moves in
/// cents and upward ones in frequency ratio.
pub fn velocity_ratio(sens: i32, velocity: f64) -> f64 {
    const X: [f64; 9] = [1., 40., 80., 100., 110., 113., 116., 120., 127.];
    const C: [f64; 9] = [-9609., -6248., -2842., -1274., -375., -130., 103., 617., 1201.];
    if sens == 0 {
        return 1.0;
    }
    let (share, v) = sensitivity(sens, velocity);
    let c = interpolate(&X, &C, v);
    if c <= 0.0 {
        laws::exp2(share * c / 1200.0)
    } else {
        1.0 + share * (laws::exp2(c / 1200.0) - 1.0)
    }
}

/// Resonance velocity sensitivity -64..+63: the factor on the Q, 1 / (1 + share h(127 - v)) with h
/// measured (0.25 at 7, 1 at 31, 3 at 63), straight beyond.
pub fn resonance_velocity(sens: i32, velocity: f64) -> f64 {
    const X: [f64; 6] = [0., 7., 17., 31., 63., 126.];
    const H: [f64; 6] = [0., 0.25, 0.5625, 1.0, 3.0, 6.9375];
    if sens == 0 {
        return 1.0;
    }
    let (share, v) = sensitivity(sens, velocity);
    1.0 / (1.0 + share * interpolate(&X, &H, 127.0 - v))
}

/// Cutoff scaling at `key`: the offset (-128..+127) straight between the four break points by key,
/// held outside them; 37.5 cents a step. A frequency ratio.
pub fn scaling_ratio(key: i32, breaks: [i32; 4], offsets: [i32; 4]) -> f64 {
    let (b, o) = (breaks.map(f64::from), offsets.map(|x| f64::from(x.clamp(-128, 127))));
    let k = f64::from(key);
    let [b0, .., b3] = b;
    let [o0, .., o3] = o;
    let offset = if k <= b0 {
        o0
    } else if k >= b3 {
        o3
    } else {
        // between two break points (equal ones: the upper's offset)
        let pairs = b.iter().zip(o.iter()).zip(b.iter().zip(o.iter()).skip(1));
        pairs
            .map(|((&x0, &y0), (&x1, &y1))| (x0, y0, x1, y1))
            .find(|&(_, _, x1, _)| k <= x1)
            .map_or(o3, |(x0, y0, x1, y1)| {
                if x1 == x0 {
                    y1
                } else {
                    y0 + (y1 - y0) * (k - x0) / (x1 - x0)
                }
            })
    };
    laws::exp2(offset * 37.5 / 1200.0)
}

/// The 18 dB "s" type's resonant section: its F over the one-pole's, by cutoff value (fitted every 64
/// values from 128 to 640; 9.5-10.4 with no trend, so straight between, held at the ends).
pub fn lp18s_ratio(value: f64) -> f64 {
    const X: [f64; 9] = [128., 192., 256., 320., 384., 448., 512., 576., 640.];
    const R: [f64; 9] = [9.799, 10.225, 9.514, 10.031, 9.896, 9.646, 10.378, 9.501, 9.821];
    interpolate(&X, &R, value)
}

/// The wide band-pass: its edges above and below the cutoff, in cutoff values, at width 0-127 (3.207
/// values a step, the upper edge 31.5 values late, the lower 19.7 early; fitted at cutoff 512).
pub fn width(w: i32) -> (f64, f64) {
    let w = f64::from(w.clamp(0, 127));
    ((3.207 * w - 31.5).max(0.0), (3.207 * w - 19.7).max(0.0))
}

/// Filter gain 0-255: 0.375 dB a step below 255 (through thru too); 0 silent.
pub fn gain(g: i32) -> f64 {
    let g = g.clamp(0, 255);
    if g == 0 {
        0.0
    } else {
        laws::db_to_gain(0.375 * f64::from(g - 255))
    }
}

/// The voice LFO's filter depth 0-99: octaves at the LFO's peak (68 cents a step; measured at depth
/// 25 only).
pub fn lfo_octaves(depth: i32) -> f64 {
    f64::from(depth.clamp(0, 99)) * 68.0 / 1200.0
}

/// The sound LFO's cutoff destination: cutoff values a depth step at the LFO's top (measured at
/// depths 2-32 with a square wave, 7.93-8.13).
pub const SLFO_CUTOFF_PER_DEPTH: f64 = 8.0;

/// The sound LFO's pan destination: pan positions at the LFO's top, 64 (1 - e^(-depth / 20))
/// (fitted to a square wave at depths 8, 32, 64).
pub fn slfo_pan_positions(depth: i32) -> f64 {
    64.0 * (1.0 - laws::exp2(-f64::from(depth.clamp(0, 127)) / 20.0 * core::f64::consts::LOG2_E))
}

/// The filter envelope's level -128..+127: cents (75 a step).
pub fn feg_cents(level: i32) -> f64 {
    75.0 * f64::from(level.clamp(-128, 127))
}

/// The filter envelope's depth -64..+63: the share of its level that moves the cutoff (measured at
/// seven depths, straight between).
pub fn feg_depth(depth: i32) -> f64 {
    const X: [f64; 7] = [-64., -32., 0., 16., 32., 48., 63.];
    const Y: [f64; 7] = [-1.0, -0.497, 0.0, 0.274, 0.505, 0.704, 1.004];
    interpolate(&X, &Y, f64::from(depth.clamp(-64, 63)))
}

/// A filter envelope segment's duration, ms, at time value 0-127 (fractions between), whatever its
/// distance: measured every second value 20-92 and at 100 and 108 (0-20 instant; from 96 the same).
pub fn feg_segment_ms(time: f64) -> f64 {
    interpolate(&FEG_X, &FEG_MS, time.clamp(0.0, 127.0))
}

/// The filter envelope's hold, ms, at time value 0-127 (measured every fourth value to 96).
pub fn feg_hold_ms(time: i32) -> f64 {
    const X: [f64; 25] = [
        0., 4., 8., 12., 16., 20., 24., 28., 32., 36., 40., 44., 48., 52., 56., 60., 64., 68., 72., 76., 80., 84., 88.,
        92., 96.,
    ];
    const MS: [f64; 25] = [
        0.0, 22.9025, 15.6463, 30.1587, 24.3537, 40.3175, 44.6712, 67.8912, 82.4036, 98.3673, 118.685, 147.71, 183.991,
        230.431, 273.968, 346.531, 432.154, 523.583, 642.585, 796.417, 989.433, 1220.18, 1493.02, 1834.06, 2266.53,
    ];
    interpolate(&X, &MS, f64::from(time.clamp(0, 127)))
}

/// The filter envelope's depth velocity sensitivity -64..+63 with curve 0-4: the factor on its
/// depth (at +63 per curve, measured: curve 2 at five velocities, the others at 32 and 96 through
/// curve 2's ends; other sensitivities in proportion).
pub fn feg_depth_velocity(sens: i32, curve: i32, velocity: f64) -> f64 {
    const X: [f64; 5] = [1., 32., 64., 96., 127.];
    const G: [[f64; 5]; 5] = [
        [0.0, 0.427, 0.543, 0.659, 1.142],
        [0.0, 0.074, 0.338, 0.602, 1.142],
        [0.0, 0.296, 0.548, 0.783, 1.142],
        [0.0, 0.485, 0.772, 1.059, 1.142],
        [0.0, 0.149, 0.559, 0.968, 1.142],
    ];
    if sens == 0 {
        return 1.0;
    }
    let (share, v) = sensitivity(sens, velocity);
    let g = G.get(curve.clamp(0, 4) as usize).copied().unwrap_or(G[2]);
    1.0 + share * (interpolate(&X, &g, v) - 1.0)
}

/// The filter envelope's time velocity sensitivity -64..+63: added to the time values of the
/// segments it reaches; positive: sens (v - 1) / 127 shorter (measured within 7 % of the duration);
/// negative: instant above velocity 1 (measured from velocity 16).
pub fn feg_time_velocity(sens: i32, velocity: f64) -> f64 {
    let (s, v) = (sens.clamp(-64, 63), velocity.clamp(1.0, 127.0));
    if v <= 1.0 || s == 0 {
        0.0
    } else if s < 0 {
        -1000.0
    } else {
        -f64::from(s) * (v - 1.0) / 127.0
    }
}

/// The segments (bit i: time i of hold, attack, decay 1, decay 2, release) each time velocity
/// segment value reaches: attack, attack and decay, decay, attack and release, all (decay 2 goes
/// with decay 1, the release by the names: unmeasured).
const TIME_VELOCITY_SEGMENTS: [u8; 5] = [0x2, 0xe, 0xc, 0x12, 0x1e];

/// The filter envelope's time key follow -64..+63 about the centre key: the factor on its times
/// (+63 two octaves up 12 times shorter, measured; straight in octaves; negative values mirror).
pub fn feg_time_key_scale(sens: i32, key: i32, centre: i32) -> f64 {
    laws::exp2(-1.79 * f64::from(sens.clamp(-64, 63)) / 63.0 * f64::from(key.clamp(-128, 255) - centre) / 12.0)
}

/// The pan law at position -64 (left) to +63 (right): the gains (left, right). The near side keeps
/// full level (no dip at the centre); the far side falls by the measured table every 4 positions
/// (straight between), to nothing from 63.
pub fn pan_gains(position: i32) -> (f32, f32) {
    const FAR: [f32; 17] = [
        1.0, 0.99916, 0.95598, 0.91621, 0.83947, 0.7698, 0.67598, 0.56891, 0.47799, 0.3689, 0.28422, 0.20131, 0.13042,
        0.07105, 0.02864, 0.00528, 0.0,
    ];
    let p = position.clamp(-64, 63);
    let d = p.unsigned_abs() as usize;
    let at = |i: usize| FAR.get(i).copied().unwrap_or(0.0);
    let far = if d >= 63 {
        0.0
    } else if d >= 60 {
        at(15) * (63 - d) as f32 / 3.0
    } else {
        at(d / 4) + (at(d / 4 + 1) - at(d / 4)) * (d % 4) as f32 / 4.0
    };
    (if p > 0 { far } else { 1.0 }, if p < 0 { far } else { 1.0 })
}

/// Scaling pan -64..+63 at `key`: positions, depth / 32 a key below key 60 and above key 64, none
/// between; +63 plays as 64.
pub fn scaling_pan(depth: i32, key: i32) -> i32 {
    let k = key.clamp(-128, 255);
    let n = if k < 60 {
        k - 60
    } else if k > 64 {
        k - 64
    } else {
        0
    };
    let d = depth.clamp(-64, 63);
    laws::round(f64::from(if d == 63 { 64 } else { d }) * f64::from(n) / 32.0)
}

/// Random pan 0-127 from a random draw: uniform within depth / 2 positions either way (the
/// reference's narrower spread at 127 and its left lean are not modelled).
pub fn random_pan(depth: i32, r: u32) -> i32 {
    let u = f64::from(r >> 8) * (2.0 / 16_777_216.0) - 1.0;
    laws::round(u * f64::from(depth.clamp(0, 127)) / 2.0)
}

// ---- a sound's prepared values ----

/// A sound's filter and pan, the laws applied once (docs/design.md, 3.7).
#[derive(Clone, Copy, PartialEq)]
pub struct Laws {
    pub kind: Kind,
    /// Whether the filter runs (a type, or a gain other than 1), and whether pan does.
    pub on: bool,
    pub pan_on: bool,
    /// The cutoff value (offset applied), its resonance and Q; the cutoff (Hz) of the section, or
    /// of the wide band-pass's upper and lower edges; the edges in cutoff values; the dual types'
    /// second section's F ratio (distance), or the 18 dB "s" type's.
    value: f32,
    resonance: i32,
    q: f32,
    hz: [f32; 2],
    width: [f32; 2],
    ratio2: f32,
    /// The high-pass of the low-pass and high-pass types (Hz) and its key follow; the gain.
    hpf_hz: f32,
    hpf_key: i8,
    gain: f32,
    /// The voice LFO's octaves at its peak; the sound LFO's cutoff values and resonance steps at
    /// its top.
    lfo_oct: f32,
    slfo_cut: f32,
    slfo_res: f32,
    /// The envelope: levels (cents) and time values (hold, attack, decay 1, decay 2, release),
    /// depth share, depth velocity (sensitivity, curve), time velocity and the segments it
    /// reaches, time key follow (sensitivity, centre).
    levels: [f32; 5],
    times: [u8; 5],
    depth: f32,
    depth_vel: (i8, u8),
    time_vel: i8,
    time_mask: u8,
    time_key: (i8, u8),
    /// Cutoff key follow, cutoff and resonance velocity sensitivity, scaling.
    key_follow: i8,
    cut_vel: i8,
    res_vel: i8,
    breaks: [u8; 4],
    offsets: [i16; 4],
    /// Pan: alternate, scaling and random depths, and the sound LFO's positions at its top.
    alternate: i8,
    scaling: i8,
    random: u8,
    slfo_pan: f32,
}

/// What a note keeps of the filter: the cutoff's ratio (key follow, velocity, scaling), the
/// damping (1 / Q, with the resonance velocity), the envelope's octaves a cent (depth and its
/// velocity), the high-pass (Hz), the envelope times' factor (key follow) and the time velocity's
/// offset.
struct Note {
    ratio: f32,
    damp: f32,
    depth: f32,
    hpf: f32,
    kscale: f32,
    tv: f32,
}

impl Laws {
    /// From the sound `s` and its sound LFO destinations (number, depth).
    pub fn new(s: &Sound, dests: &[(i32, i32); 3]) -> Laws {
        let g = |i: usize| i32::from(s.get(i));
        let f = |k: P| g(k as usize);
        let fi = |k: P, i: usize| g((k as usize).wrapping_add(i));
        let value = (f(P::Cutoff) + 8 * g(Part::CutoffOffset as usize).clamp(-64, 63)).clamp(0, 1023);
        let resonance = (f(P::Resonance) + 2 * g(Part::ResonanceOffset as usize).clamp(-64, 63)).clamp(0, 127);
        let depth = (f(P::EgDepth) + 2 * g(Part::FegDepthOffset as usize).clamp(-64, 63)).clamp(-64, 63);
        let kind = kind(f(P::Type));
        let (hi, lo) = width(resonance);
        let sum = |n: i32| {
            dests
                .iter()
                .filter(|d| d.0 == n)
                .map(|d| d.1.clamp(0, 127))
                .sum::<i32>()
        };
        let pan_d = dests
            .iter()
            .filter(|d| d.0 == 70)
            .map(|d| slfo_pan_positions(d.1))
            .sum::<f64>();
        let gain = gain(f(P::Gain)) as f32;
        let mut l = Laws {
            kind,
            on: kind != Kind::Thru || gain != 1.0,
            pan_on: false,
            value: value as f32,
            resonance,
            q: q(resonance) as f32,
            hz: [0.0; 2],
            width: [hi as f32, lo as f32],
            ratio2: if kind == Kind::Lp18S {
                lp18s_ratio(f64::from(value))
            } else {
                laws::exp2(f64::from(f(P::Distance).clamp(-128, 127)) / 16.0)
            } as f32,
            hpf_hz: cutoff_hz(f(P::HpfCutoff) as f32),
            hpf_key: f(P::HpfKeyFollow).clamp(-64, 63) as i8,
            gain,
            lfo_oct: lfo_octaves(g(Common::LfoFilterDepth as usize)) as f32,
            slfo_cut: (f64::from(sum(72)) * SLFO_CUTOFF_PER_DEPTH) as f32,
            slfo_res: sum(73) as f32,
            levels: core::array::from_fn(|i| feg_cents(fi(P::EgLevel, i)) as f32),
            times: core::array::from_fn(|i| fi(P::EgTime, i).clamp(0, 127) as u8),
            depth: feg_depth(depth) as f32,
            depth_vel: (
                f(P::EgDepthVelocity).clamp(-64, 63) as i8,
                f(P::EgDepthVelocityCurve).clamp(0, 4) as u8,
            ),
            time_vel: f(P::EgTimeVelocity).clamp(-64, 63) as i8,
            time_mask: TIME_VELOCITY_SEGMENTS
                .get(f(P::EgTimeVelocitySegment).clamp(0, 4) as usize)
                .copied()
                .unwrap_or(0),
            time_key: (
                f(P::EgTimeKeyFollow).clamp(-64, 63) as i8,
                f(P::EgTimeKeyCentre).clamp(0, 127) as u8,
            ),
            key_follow: f(P::KeyFollow).clamp(-64, 63) as i8,
            cut_vel: f(P::CutoffVelocity).clamp(-64, 63) as i8,
            res_vel: f(P::ResonanceVelocity).clamp(-64, 63) as i8,
            breaks: core::array::from_fn(|i| fi(P::Break, i).clamp(0, 127) as u8),
            offsets: core::array::from_fn(|i| fi(P::Offset, i).clamp(-128, 127) as i16),
            alternate: g(Common::AlternatePan as usize).clamp(-64, 63) as i8,
            scaling: g(Common::ScalingPan as usize).clamp(-64, 63) as i8,
            random: g(Common::RandomPan as usize).clamp(0, 127) as u8,
            slfo_pan: pan_d as f32,
        };
        l.hz = l.cutoffs(0.0);
        l.pan_on = l.alternate != 0 || l.scaling != 0 || l.random != 0 || l.slfo_pan != 0.0;
        l
    }

    /// The section's cutoff (Hz), or the wide band-pass's upper and lower edges, with the cutoff
    /// value moved by `values`.
    fn cutoffs(&self, values: f32) -> [f32; 2] {
        let v = self.value + values;
        match self.kind {
            Kind::BpWide => [cutoff_hz(v + self.width[0]), cutoff_hz(v - self.width[1])],
            _ => [cutoff_hz(v), 0.0],
        }
    }

    /// Whether the envelope or the voice LFO moves the cutoff.
    pub fn uses_lfo(&self) -> bool {
        self.lfo_route() && self.lfo_oct != 0.0
    }

    /// Whether the voice LFO could move the cutoff (at a depth above 0).
    pub fn lfo_route(&self) -> bool {
        self.on && self.kind != Kind::Thru
    }

    /// The voice LFO's filter depth 0-99 (a controller box moves it).
    pub fn set_lfo_depth(&mut self, depth: i32) {
        self.lfo_oct = lfo_octaves(depth) as f32;
    }

    /// Whether the sound LFO moves the cutoff, the resonance or pan.
    pub fn uses_slfo(&self) -> bool {
        (self.on && self.kind != Kind::Thru && (self.slfo_cut != 0.0 || self.slfo_res != 0.0)) || self.slfo_pan != 0.0
    }

    fn note(&self, key: i32, v: f64) -> Note {
        let b = self.breaks.map(i32::from);
        let ratio = scaling_ratio(key, b, self.offsets.map(i32::from))
            * key_follow_ratio(i32::from(self.key_follow), key)
            * velocity_ratio(i32::from(self.cut_vel), v);
        let depth = f64::from(self.depth)
            * feg_depth_velocity(i32::from(self.depth_vel.0), i32::from(self.depth_vel.1), v)
            / 1200.0;
        Note {
            ratio: ratio as f32,
            damp: (1.0 / (f64::from(self.q) * resonance_velocity(i32::from(self.res_vel), v))) as f32,
            depth: depth as f32,
            hpf: (f64::from(self.hpf_hz) * key_follow_ratio(i32::from(self.hpf_key), key)) as f32,
            kscale: feg_time_key_scale(i32::from(self.time_key.0), key, i32::from(self.time_key.1)) as f32,
            tv: feg_time_velocity(i32::from(self.time_vel), v) as f32,
        }
    }

    /// The frames envelope segment `i` (0 the hold) takes for a note with time factor `kscale` and
    /// time velocity offset `tv`.
    fn frames(&self, i: usize, kscale: f32, tv: f32, rate: f64) -> f32 {
        let t = self.times.get(i).copied().map_or(0, i32::from);
        let ms = if i == 0 {
            feg_hold_ms(t)
        } else {
            let reach = self.time_mask & (1u8 << (i & 7)) != 0;
            feg_segment_ms((f64::from(t) + if reach { f64::from(tv) } else { 0.0 }).max(0.0))
        };
        (ms * f64::from(kscale) * rate / 1000.0) as f32
    }

    /// A note's pan position: alternate pan's side (`other`: the other side), scaling and random
    /// positions, added and held within -64..+63.
    pub fn position(&self, key: i32, other: bool, r: Option<u32>) -> f32 {
        let alt = i32::from(self.alternate);
        let p = if other { -alt } else { alt }
            + scaling_pan(i32::from(self.scaling), key)
            + r.map_or(0, |r| random_pan(i32::from(self.random), r));
        p.clamp(-64, 63) as f32
    }

    /// Whether a note draws a random pan.
    pub fn random_pan(&self) -> bool {
        self.random != 0
    }
}

// ---- a lane's state and kernels ----

/// States are held within this (18 dB above full scale): the reference overloads at high
/// resonance, which is not modelled; this only keeps every output bounded.
const LIMIT: f32 = 8.0;
/// A state below this at a step's end is set to 0, so a decaying filter never reaches subnormals.
const TINY: f32 = 1e-15;
/// The filter envelope's segments: hold, attack, decay 1, decay 2 (held while the key is down),
/// release, done.
const DONE: u8 = 5;

/// A lane's filter and pan state, one voice a slot, read only when its sound uses them.
#[derive(Clone, Copy)]
pub struct Voices {
    /// The sections' states: low-pass and band-pass of the first and the second, the one-pole's.
    st: [[f32; W]; 5],
    /// Their coefficients at the last step's end: F and damping of each section, the one-pole's.
    c: [[f32; W]; 5],
    /// The gain at the last step's end (the sound's).
    g: f32,
    /// The envelope: level (cents), frames left of its segment, its move a frame, the segment.
    lvl: [f32; W],
    left: [f32; W],
    step: [f32; W],
    seg: [u8; W],
    /// What the note keeps (`Note`).
    ratio: [f32; W],
    damp: [f32; W],
    depth: [f32; W],
    hpf: [f32; W],
    kscale: [f32; W],
    tv: [f32; W],
    /// Pan: the note's position, the gains at the last step's end.
    pos: [f32; W],
    gl: [f32; W],
    gr: [f32; W],
    /// Voices whose coefficients, and whose pan gains, start at their first step's values.
    fresh: u8,
    fresh_pan: u8,
}

pub const VOICES0: Voices = Voices {
    st: [[0.0; W]; 5],
    c: [[0.0; W]; 5],
    g: 1.0,
    lvl: [0.0; W],
    left: [0.0; W],
    step: [0.0; W],
    seg: [DONE; W],
    ratio: [1.0; W],
    damp: [2.0; W],
    depth: [0.0; W],
    hpf: [0.0; W],
    kscale: [1.0; W],
    tv: [0.0; W],
    pos: [0.0; W],
    gl: [1.0; W],
    gr: [1.0; W],
    fresh: 0,
    fresh_pan: 0,
};

fn put(a: &mut [f32; W], s: usize, x: f32) {
    if let Some(e) = a.get_mut(s) {
        *e = x;
    }
}

fn at(a: &[f32; W], s: usize) -> f32 {
    a.get(s).copied().unwrap_or(0.0)
}

fn bit(s: usize) -> u8 {
    1u8.checked_shl(s as u32).unwrap_or(0)
}

/// 2 sin(pi x) for x in [0, 1/6] (the cutoff over the rate): a degree-7 polynomial, 1e-8.
#[inline(always)]
fn two_sin_pi<const L: usize, V: F4<L>>(x: V) -> V {
    let t = x * V::splat(core::f32::consts::PI);
    let t2 = t * t;
    let p = t2.mul_add(V::splat(-1.0 / 5040.0), V::splat(1.0 / 120.0));
    let p = t2.mul_add(p, V::splat(-1.0 / 6.0));
    let p = t2.mul_add(p, V::splat(1.0));
    t * V::splat(2.0) * p
}

/// `f` lane by lane of `a` and `b` (the rare scalar parts of the coefficients).
#[inline(always)]
fn lanes<const L: usize, V: F4<L>>(a: V, b: V, f: impl Fn(f32, f32) -> f32) -> V {
    let (mut a, b) = (a.store(), b.store());
    a.iter_mut().zip(b).for_each(|(a, b)| *a = f(*a, b));
    V::load(a)
}

/// The rate's part in the coefficients: 1 / rate, the top cutoff (7350 Hz, or a sixth of the rate
/// below 44.1 kHz), and whether the rate is below 44.1 kHz.
struct Rate {
    inv: f32,
    top: f32,
    slow: bool,
}

impl Rate {
    fn new(rate: f64) -> Rate {
        let r = rate as f32;
        Rate {
            inv: 1.0 / r,
            top: MAX_HZ.min(r / 6.0),
            slow: rate < 44_100.0,
        }
    }

    /// Sections at cutoffs `fc` Hz with `damping`, held at the top and to the most damping there,
    /// and never where they turn unstable (which at 44.1 kHz and above the least Q already keeps
    /// them from).
    #[inline(always)]
    fn section<const L: usize, V: F4<L>>(&self, fc: V, damping: V) -> (V, V) {
        let fc = fc.max(V::splat(1.0)).min(V::splat(self.top));
        let f = two_sin_pi(fc * V::splat(self.inv));
        let mut d = damping.min(V::splat(2.0));
        // the least Q rises from 3.06 kHz
        if fc.store().iter().any(|&x| x > MIN_Q_HZ[0]) {
            d = lanes(d, fc, |d, fc| d.min(max_damping(fc)));
        }
        if self.slow {
            d = lanes(d, f, |d, f| d.min(0.95 * (2.0 / f - f / 2.0)));
        }
        (f, d)
    }
}

/// Second sections at `ratio` times the first's F (the dual types, the 18 dB "s" type): beyond the
/// first's limit up to F = 1.75, their damping limited only where they would turn unstable
/// (shortcut: the reference's limit above F = 1 is unmeasured).
#[inline(always)]
fn second<const L: usize, V: F4<L>>(f: V, d: V, damping: V, ratio: f32) -> (V, V) {
    let f2 = (f * V::splat(ratio)).min(V::splat(1.75));
    let (fs, mut ds, dps) = (f2.store(), d.store(), damping.store());
    for ((d, f2), damping) in ds.iter_mut().zip(fs).zip(dps) {
        let edge = 2.0 / f2 - f2 / 2.0;
        *d = if f2 > 1.0 {
            damping.min(0.9 * edge)
        } else {
            d.min(0.95 * edge)
        };
    }
    (f2, V::load(ds))
}

impl Laws {
    /// The coefficient targets of a lane's voices (F and damping of each section, the one-pole's
    /// F) for the cutoffs `hz`, at the cutoff ratios `ratio` and `oct` octaves of the envelope and
    /// the LFOs, with `damping` (1 / Q) and the high-passes `hpf` (Hz).
    #[inline(always)]
    fn targets<const L: usize, V: F4<L>>(
        &self,
        rate: &Rate,
        hz: [f32; 2],
        ratio: V,
        oct: V,
        damping: V,
        hpf: V,
    ) -> [V; 5] {
        let (zero, one) = (V::splat(0.0), V::splat(1.0));
        let oct = oct.max(V::splat(-15.0)).min(V::splat(15.0)) - V::splat(15.0);
        let mv = oct.exp2() * V::splat(32768.0) * ratio;
        let at = |hz: f32| mv * V::splat(hz);
        if self.kind == Kind::BpWide {
            let ((f0, d0), (f1, d1)) = (rate.section(at(hz[1]), one), rate.section(at(hz[0]), one));
            return [f0, d0, f1, d1, zero];
        }
        let (f, d) = rate.section(at(hz[0]), damping);
        match self.kind {
            Kind::Lp24A => [f, rate.section(at(hz[0]), one).1, f, d, zero],
            Kind::Lp18S => {
                let (f2, d2) = second(f, d, damping, self.ratio2);
                [f2, d2, zero, zero, f]
            }
            Kind::Lp12Hp12 | Kind::Lp6Hp12 => {
                let (fh, dh) = rate.section(hpf, one);
                [f, d, fh, dh, f]
            }
            Kind::DualLp | Kind::DualHp | Kind::DualBp | Kind::DualBe => {
                let (f2, d2) = second(f, d, damping, self.ratio2);
                [f, d, f2, d2, zero]
            }
            // the band-pass section's Q is 1.13 times the low-pass's (fitted)
            Kind::Lp12Bp6 => [f, d, f, d * V::splat(1.0 / 1.13), zero],
            _ => [f, d, f, d, f],
        }
    }
}

/// A state-variable section across a lane, its coefficients ramped a frame.
struct Sec<V, const L: usize> {
    lp: V,
    bp: V,
    f: V,
    d: V,
    df: V,
    dd: V,
}

impl<const L: usize, V: F4<L>> Sec<V, L> {
    /// A frame: low-pass, band-pass and high-pass.
    #[inline(always)]
    fn tick(&mut self, x: V) -> [V; 3] {
        let lim = V::splat(LIMIT);
        let neg = V::splat(-LIMIT);
        self.f = self.f + self.df;
        self.d = self.d + self.dd;
        self.lp = self.f.mul_add(self.bp, self.lp).max(neg).min(lim);
        let hp = x - self.lp - self.d * self.bp;
        self.bp = self.f.mul_add(hp, self.bp).max(neg).min(lim);
        [self.lp, self.bp, hp]
    }

    #[inline(always)]
    fn notch(&mut self, x: V) -> V {
        let [lp, _, hp] = self.tick(x);
        lp + hp
    }
}

/// The one-pole low-pass across a lane, its coefficient ramped a frame.
struct Pole<V, const L: usize> {
    y: V,
    a: V,
    da: V,
}

impl<const L: usize, V: F4<L>> Pole<V, L> {
    #[inline(always)]
    fn tick(&mut self, x: V) -> V {
        self.a = self.a + self.da;
        self.y = self.a.mul_add(x - self.y, self.y);
        self.y
    }
}

/// A step's run of one type: the sections, the gain and its move a frame, the frames.
struct Kern<'a, V, const L: usize> {
    a: &'a mut Sec<V, L>,
    b: &'a mut Sec<V, L>,
    o: &'a mut Pole<V, L>,
    g: f32,
    dg: f32,
    n: usize,
}

impl<const L: usize, V: F4<L>> Kern<'_, V, L> {
    /// The type's frame `tick` over the step's frames of `y`, in place, times the ramped gain.
    #[inline(always)]
    fn run(
        &mut self,
        y: &mut [[f32; L]; STEP],
        tick: impl Fn(V, &mut Sec<V, L>, &mut Sec<V, L>, &mut Pole<V, L>) -> V,
    ) {
        // (the gain at 1, the default, costs nothing)
        if self.g == 1.0 && self.dg == 0.0 {
            for x in y.iter_mut().take(self.n) {
                *x = tick(V::load(*x), self.a, self.b, self.o).store();
            }
            return;
        }
        for x in y.iter_mut().take(self.n) {
            self.g += self.dg;
            *x = (tick(V::load(*x), self.a, self.b, self.o) * V::splat(self.g)).store();
        }
    }
}

/// A lane's voices summed in pairs, as four-wide (so a missing voice's zero changes no bits).
#[inline(always)]
pub(crate) fn sum<const L: usize>(v: &[f32; L]) -> f32 {
    match v.as_slice() {
        [a] => *a,
        [a, b] => *a + *b,
        [a, b, c, d] => (*a + *b) + (*c + *d),
        _ => 0.0,
    }
}

/// `x` with every lane below `TINY` in magnitude set to 0.
#[inline(always)]
fn flush<const L: usize, V: F4<L>>(x: V) -> V {
    let zero = V::splat(0.0);
    V::select(x.max(zero - x).ge(V::splat(TINY)), x, zero)
}

impl Voices {
    /// Starts a note in slot `s` at `key` (note shift applied), part velocity `v`, pan position
    /// `pos`: its envelope from its hold level, its sections empty, coefficients and pan gains taken
    /// at its first step; `first`: the lane's first voice (the gain starts at the sound's).
    #[allow(clippy::too_many_arguments)]
    pub fn start(&mut self, f: &Laws, s: usize, key: i32, v: f64, pos: f32, first: bool, rate: f64) {
        self.clear(s);
        self.retune(f, s, key, v);
        put(&mut self.lvl, s, f.levels[0]);
        self.enter(f, s, 0, rate);
        put(&mut self.pos, s, pos);
        (self.fresh, self.fresh_pan) = (self.fresh | bit(s), self.fresh_pan | bit(s));
        if first {
            self.g = f.gain;
        }
    }

    /// Slot `s` goes on with a note at `key`, velocity `v` (a mono legato note): what the note
    /// keeps, the envelope going on.
    pub fn retune(&mut self, f: &Laws, s: usize, key: i32, v: f64) {
        let n = f.note(key, v);
        put(&mut self.ratio, s, n.ratio);
        put(&mut self.damp, s, n.damp);
        put(&mut self.depth, s, n.depth);
        put(&mut self.hpf, s, n.hpf);
        put(&mut self.kscale, s, n.kscale);
        put(&mut self.tv, s, n.tv);
    }

    /// The key of slot `s` is released: its envelope's release from where it is.
    pub fn release(&mut self, f: &Laws, s: usize, rate: f64) {
        if self.seg.get(s).is_some_and(|&g| g < 4) {
            self.enter(f, s, 4, rate);
        }
    }

    /// Enters envelope segment `i` of slot `s`: its duration, and a move a frame that reaches its
    /// level then (the hold stands still).
    fn enter(&mut self, f: &Laws, s: usize, i: u8, rate: f64) {
        let left = f.frames(usize::from(i), at(&self.kscale, s), at(&self.tv, s), rate);
        let target = f.levels.get(usize::from(i)).copied().unwrap_or(0.0);
        let step = if i == 0 {
            0.0
        } else {
            (target - at(&self.lvl, s)) / left.max(1.0)
        };
        put(&mut self.left, s, left);
        put(&mut self.step, s, step);
        if let Some(g) = self.seg.get_mut(s) {
            *g = i;
        }
    }

    /// Slot `s`'s envelope `n` frames on: its level (cents).
    fn advance(&mut self, f: &Laws, s: usize, n: f32, rate: f64) -> f32 {
        // most steps: standing still, or inside a segment
        let (seg, left, lvl) = (
            self.seg.get(s).copied().unwrap_or(DONE),
            at(&self.left, s),
            at(&self.lvl, s),
        );
        if seg >= DONE || (seg == 3 && left <= 0.0) {
            return lvl;
        }
        if left > n {
            let lvl = lvl + at(&self.step, s) * n;
            put(&mut self.lvl, s, lvl);
            put(&mut self.left, s, left - n);
            return lvl;
        }
        let mut todo = n;
        // a step crosses at most every segment once
        for _ in 0..6 {
            let seg = self.seg.get(s).copied().unwrap_or(DONE);
            let target = f.levels.get(usize::from(seg)).copied().unwrap_or(0.0);
            let left = at(&self.left, s);
            if seg >= DONE || todo <= 0.0 {
                break;
            }
            if seg == 3 && left <= 0.0 {
                put(&mut self.lvl, s, target); // decay 2's level holds while the key is down
                break;
            }
            let k = todo.min(left);
            let lvl = at(&self.lvl, s) + at(&self.step, s) * k;
            put(&mut self.lvl, s, lvl);
            put(&mut self.left, s, left - k);
            todo -= k;
            if left - k <= 0.0 {
                if seg > 0 {
                    put(&mut self.lvl, s, target);
                }
                match seg {
                    0..=2 => self.enter(f, s, seg + 1, rate),
                    4 => {
                        if let Some(g) = self.seg.get_mut(s) {
                            *g = DONE;
                        }
                    }
                    _ => {}
                }
            }
        }
        at(&self.lvl, s)
    }

    /// The filter over a step of `n` frames of the lane's first `L` voices' outputs `y`, in place:
    /// each voice's envelope and coefficients at the step's end (`sv` the sound LFO there, `lfo` each
    /// voice's LFO share), ramped from the last step's across its frames; `bx` the controller boxes'
    /// cutoff values and resonance steps.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn filter<const L: usize, V: F4<L>>(
        &mut self,
        f: &Laws,
        rate: f64,
        sv: f32,
        lfo: &[f32; W],
        n: usize,
        y: &mut [[f32; L]; STEP],
        bx: (f32, f32),
    ) {
        let n = n.clamp(1, STEP);
        let inv = 1.0 / n as f32;
        let dg = (f.gain - self.g) * inv;
        if f.kind == Kind::Thru {
            // a gain only
            for x in y.iter_mut().take(n) {
                self.g += dg;
                *x = (V::load(*x) * V::splat(self.g)).store();
            }
            self.g = f.gain;
            return;
        }
        let rt = Rate::new(rate);
        // the boxes' cutoff within the cutoff's range
        let hz = if f.slfo_cut != 0.0 || bx.0 != 0.0 {
            f.cutoffs(sv * f.slfo_cut + bx.0.max(-f.value).min(1023.0 - f.value))
        } else {
            f.hz
        };
        // the sound LFO's and the boxes' resonance steps: the damping times 2^(-steps / 16)
        let dscale = if f.slfo_res != 0.0 || bx.1 != 0.0 {
            let res = f
                .resonance
                .saturating_add((sv * f.slfo_res + bx.1) as i32)
                .clamp(0, 127);
            scalar::F::<1>::splat((f.resonance as f32 - res as f32) / 16.0 - 8.0)
                .exp2()
                .0[0]
                * 256.0
        } else {
            1.0
        };
        // the envelopes, then the coefficients of every voice at once
        let mut lvl = [0.0f32; W];
        for s in 0..L {
            put(&mut lvl, s, self.advance(f, s, n as f32, rate));
        }
        let ld = |a: &[f32; W]| V::load4(a);
        let oct = ld(&lvl) * ld(&self.depth) + ld(lfo) * V::splat(f.lfo_oct);
        let damping = ld(&self.damp) * V::splat(dscale);
        let targets = f.targets(&rt, hz, ld(&self.ratio), oct, damping, ld(&self.hpf));
        let mut t = [[0.0f32; W]; 5];
        for ((t, x), c) in t.iter_mut().zip(targets).zip(self.c.iter_mut()) {
            x.store4(t);
            for s in (0..L).filter(|&s| self.fresh & bit(s) != 0) {
                put(c, s, at(t, s));
            }
        }
        self.fresh = 0;
        let iv = V::splat(inv);
        let [l0, b0, l1, b1, p] = &mut self.st;
        let [f0, d0, f1, d1, a] = &self.c;
        let [tf0, td0, tf1, td1, ta] = &t;
        let sec = |lp: &[f32; W], bp: &[f32; W], f: &[f32; W], d: &[f32; W], tf: &[f32; W], td: &[f32; W]| {
            let (f, d) = (V::load4(f), V::load4(d));
            Sec {
                lp: V::load4(lp),
                bp: V::load4(bp),
                f,
                d,
                df: (V::load4(tf) - f) * iv,
                dd: (V::load4(td) - d) * iv,
            }
        };
        let mut sa = sec(l0, b0, f0, d0, tf0, td0);
        let mut sb = sec(l1, b1, f1, d1, tf1, td1);
        let a0 = V::load4(a);
        let mut o = Pole {
            y: V::load4(p),
            a: a0,
            da: (V::load4(ta) - a0) * iv,
        };
        let mut k = Kern {
            a: &mut sa,
            b: &mut sb,
            o: &mut o,
            g: self.g,
            dg,
            n,
        };
        use Kind::*;
        match f.kind {
            Lp24 | Lp24A => k.run(y, |x, a, b, _| b.tick(a.tick(x)[0])[0]),
            Lp18 => k.run(y, |x, a, _, o| o.tick(a.tick(x)[0])),
            Lp18S => k.run(y, |x, a, _, o| a.tick(o.tick(x))[0]),
            Lp12Hp12 => k.run(y, |x, a, b, _| b.tick(a.tick(x)[0])[2]),
            Lp6Hp12 => k.run(y, |x, _, b, o| b.tick(o.tick(x))[2]),
            Hp24 => k.run(y, |x, a, b, _| b.tick(a.tick(x)[2])[2]),
            Hp12 => k.run(y, |x, a, _, _| a.tick(x)[2]),
            Bp12 => k.run(y, |x, a, b, _| b.tick(a.tick(x)[1])[1]),
            BpWide => k.run(y, |x, a, b, _| b.tick(a.tick(x)[2])[0]),
            Bp6 => k.run(y, |x, a, _, _| a.tick(x)[1]),
            Be12 | DualBe => k.run(y, |x, a, b, _| b.notch(a.notch(x))),
            Be6 => k.run(y, |x, a, _, _| a.notch(x)),
            DualLp => k.run(y, |x, a, b, _| a.tick(x)[0] + b.tick(x)[0]),
            DualHp => k.run(y, |x, a, b, _| a.tick(x)[2] + b.tick(x)[2]),
            DualBp => k.run(y, |x, a, b, _| a.tick(x)[1] + b.tick(x)[1]),
            Lp12Bp6 => k.run(y, |x, a, b, _| a.tick(x)[0] + b.tick(x)[1]),
            Thru => {}
        }
        flush(sa.lp).store4(l0);
        flush(sa.bp).store4(b0);
        flush(sb.lp).store4(l1);
        flush(sb.bp).store4(b1);
        flush(o.y).store4(p);
        // the step ends exactly on its targets
        for (c, t) in self.c.iter_mut().zip(t.iter()) {
            c.iter_mut().zip(t).take(L).for_each(|(c, t)| *c = *t);
        }
        self.g = f.gain;
    }

    /// Adds a step of `n` frames of the lane's first `L` voices' outputs `y` to `left` and `right`
    /// through each voice's pan (its position, moved by the sound LFO's `sv`), the gains ramped
    /// across the step from the last step's.
    pub(crate) fn mix<const L: usize, V: F4<L>>(
        &mut self,
        f: &Laws,
        sv: f32,
        y: &[[f32; L]; STEP],
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let n = left.len().min(right.len()).clamp(1, STEP);
        let (mut tl, mut tr) = (self.gl, self.gr);
        for s in 0..L {
            let (l, r) = pan_gains(laws::round(f64::from(at(&self.pos, s) + sv * f.slfo_pan)));
            put(&mut tl, s, l);
            put(&mut tr, s, r);
        }
        let fresh = self.fresh_pan;
        for s in (0..L).filter(|&s| fresh & bit(s) != 0) {
            put(&mut self.gl, s, at(&tl, s));
            put(&mut self.gr, s, at(&tr, s));
        }
        self.fresh_pan = 0;
        let (mut gl, mut gr) = (V::load4(&self.gl), V::load4(&self.gr));
        let frames = y.iter().zip(left.iter_mut()).zip(right.iter_mut()).take(n);
        if tl == self.gl && tr == self.gr {
            for ((v, l), r) in frames {
                let v = V::load(*v);
                *l += sum(&(v * gl).store());
                *r += sum(&(v * gr).store());
            }
            return;
        }
        let k = V::splat(1.0 / n as f32);
        let (dl, dr) = ((V::load4(&tl) - gl) * k, (V::load4(&tr) - gr) * k);
        for ((v, l), r) in frames {
            let v = V::load(*v);
            (gl, gr) = (gl + dl, gr + dr);
            *l += sum(&(v * gl).store());
            *r += sum(&(v * gr).store());
        }
        (self.gl, self.gr) = (tl, tr);
    }

    /// Empties slot `s`'s sections (a free slot in a four-wide lane is filtered too: it stays
    /// silent).
    pub fn clear(&mut self, s: usize) {
        for st in self.st.iter_mut() {
            put(st, s, 0.0);
        }
    }

    /// Moves the voice in slot `from` to slot `to`.
    pub fn mv(&mut self, from: usize, to: usize) {
        fn one<T: Copy>(a: &mut [T; W], from: usize, to: usize) {
            if let (Some(x), Some(y)) = (a.get(from).copied(), a.get_mut(to)) {
                *y = x;
            }
        }
        for a in self.st.iter_mut().chain(self.c.iter_mut()) {
            one(a, from, to);
        }
        for a in [
            &mut self.lvl,
            &mut self.left,
            &mut self.step,
            &mut self.ratio,
            &mut self.damp,
            &mut self.depth,
            &mut self.hpf,
            &mut self.kscale,
            &mut self.tv,
            &mut self.pos,
            &mut self.gl,
            &mut self.gr,
        ] {
            one(a, from, to);
        }
        one(&mut self.seg, from, to);
        let (fb, tb) = (bit(from), bit(to));
        for m in [&mut self.fresh, &mut self.fresh_pan] {
            *m = if *m & fb != 0 { *m | tb } else { *m & !tb };
        }
    }
}

// the cutoff's measured points (value, Hz)
#[rustfmt::skip]
const CUTOFF_X: [f32; 193] = [
    0., 8., 16., 24., 32., 40., 48., 56., 64., 72., 84., 88., 96., 104., 112., 120., 128., 136.,
    144., 152., 160., 168., 176., 185., 194., 195., 197., 200., 201., 202., 203., 204., 205., 210.,
    216., 224., 232., 240., 248., 256., 264., 272., 280., 288., 299., 300., 301., 302., 303., 304.,
    305., 306., 307., 308., 312., 320., 328., 336., 344., 352., 360., 368., 376., 384., 393., 400.,
    401., 403., 404., 406., 407., 408., 409., 411., 412., 413., 415., 419., 424., 432., 440., 448.,
    456., 464., 472., 480., 488., 496., 502., 503., 505., 506., 508., 510., 511., 512., 513., 514.,
    515., 516., 517., 520., 528., 536., 544., 552., 560., 568., 576., 584., 593., 600., 603., 604.,
    607., 608., 610., 611., 612., 613., 614., 615., 616., 617., 624., 632., 640., 648., 656., 664.,
    672., 680., 688., 696., 707., 709., 710., 712., 713., 714., 715., 716., 717., 718., 719., 720.,
    728., 736., 744., 752., 760., 768., 776., 784., 792., 800., 810., 811., 814., 815., 816., 817.,
    818., 819., 820., 821., 822., 826., 828., 832., 834., 836., 839., 842., 845., 846., 852., 855.,
    858., 862., 866., 870., 876., 884., 886., 892., 904., 911., 912., 916., 920., 924., 1023.,
];
#[rustfmt::skip]
const CUTOFF_HZ: [f32; 193] = [
    14.328, 15.7428, 17.0835, 18.3441, 19.5361, 20.6628, 21.7248, 22.7392, 23.6949, 24.5854,
    25.8529, 26.2711, 27.0593, 29.1189, 31.9587, 34.6188, 37.1043, 39.4519, 41.6439, 43.7431,
    45.7397, 47.6313, 49.4371, 51.3607, 53.2351, 53.4137, 53.8226, 54.3962, 54.5998, 54.7878,
    55.4633, 56.1994, 56.9746, 60.6295, 64.7928, 70.0417, 74.9819, 79.6416, 84.0324, 88.1411,
    92.1176, 95.8671, 99.4273, 102.855, 107.38, 107.741, 108.156, 108.518, 108.933, 109.295,
    109.829, 111.383, 112.855, 114.408, 120.246, 131.411, 141.746, 151.52, 160.757, 169.433,
    177.653, 185.419, 192.916, 200.039, 207.636, 213.401, 214.124, 215.757, 216.507, 218.14,
    218.863, 220.577, 223.683, 229.574, 232.681, 235.573, 241.464, 252.49, 266.315, 286.774,
    306.163, 324.267, 341.621, 357.905, 373.44, 388.225, 402.314, 415.867, 425.724, 427.171,
    430.492, 431.939, 435.26, 438.153, 443.029, 448.922, 455.137, 461.03, 467.245, 473.032,
    478.604, 495.75, 539.261, 580.204, 618.259, 654.494, 688.59, 721.188, 751.86, 781.57, 812.745,
    836.495, 846.365, 849.262, 859.133, 862.137, 868.682, 871.58, 874.906, 877.911, 890.359,
    901.948, 914.398, 926.203, 1006.72, 1093.28, 1173.86, 1250.18, 1321.59, 1389.8, 1454.39,
    1515.55, 1574.38, 1630.21, 1703.96, 1717.12, 1722.95, 1736.33, 1742.15, 1748.85, 1754.67,
    1766.54, 1791.58, 1814.9, 1839.96, 1863.72, 2046.35, 2217.1, 2378.33, 2530.53, 2674.31,
    2810.96, 2940.45, 3064.09, 3182.28, 3296.35, 3431.34, 3443.24, 3483.78, 3496.15, 3509.37,
    3521.28, 3534.52, 3569.42, 3620.69, 3668.48, 3719.86, 3914.38, 4008.82, 4191.18, 4283.53,
    4372.5, 4501.38, 4630.62, 4756.55, 4798.15, 5040.88, 5158.3, 5276.07, 5427.54, 5573.22,
    5728.62, 5939.32, 6210.2, 6278.54, 6473.06, 6848.73, 7057.42, 7087.45, 7201.27, 7319.56,
    7350.21, 7350.21,
];
// the least Q by cutoff (Hz)
#[rustfmt::skip]
const MIN_Q_HZ: [f32; 65] = [
    3064.09, 3182.28, 3296.35, 3377.21, 3390.84, 3404.05, 3417.71, 3431.34, 3443.24, 3456.89,
    3470.11, 3483.78, 3496.15, 3509.37, 3521.28, 3534.52, 3569.42, 3620.69, 3668.48, 3719.86,
    3817.46, 3914.38, 4008.82, 4099.91, 4191.18, 4283.53, 4372.5, 4458.06, 4544.7, 4630.62,
    4714.95, 4798.15, 4879.19, 4959.49, 5040.88, 5118.81, 5197.78, 5276.07, 5351.76, 5427.54,
    5499.83, 5573.22, 5728.62, 5798.7, 5868.93, 5939.32, 6007.06, 6074.95, 6142.03, 6210.2,
    6278.54, 6343.24, 6408.08, 6473.06, 6535.31, 6597.71, 6660.24, 6722.91, 6785.75, 6848.73,
    6967.33, 7087.45, 7201.27, 7319.56, 7350.21,
];
#[rustfmt::skip]
const MIN_Q: [f32; 65] = [
    0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5021,
    0.5054, 0.509, 0.5161, 0.5233, 0.5305, 0.5376, 0.545, 0.5526, 0.5601, 0.5675, 0.5752, 0.583,
    0.5909, 0.5988, 0.6068, 0.6149, 0.6232, 0.6315, 0.64, 0.6487, 0.6573, 0.6661, 0.6748, 0.6837,
    0.708, 0.7173, 0.7268, 0.7366, 0.7463, 0.7562, 0.7662, 0.7767, 0.7874, 0.7979, 0.8086, 0.8196,
    0.8304, 0.8415, 0.8529, 0.8645, 0.8766, 0.8889, 0.9131, 0.9389, 0.9645, 0.9925, 1.0,
];
// the filter envelope's segment durations (time value, ms)
#[rustfmt::skip]
const FEG_X: [f64; 50] = [
    0., 2., 4., 6., 8., 10., 12., 14., 16., 18., 20., 22., 24., 26., 28., 30., 32., 34., 36., 38.,
    40., 42., 44., 46., 48., 50., 52., 54., 56., 58., 60., 62., 64., 66., 68., 70., 72., 74., 76.,
    78., 80., 82., 84., 86., 88., 90., 92., 96., 100., 108.,
];
#[rustfmt::skip]
const FEG_MS: [f64; 50] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 4.07596, 32.7041, 25.7823, 23.9002,
    35.8787, 39.9546, 31.9104, 38.1803, 50.4535, 64.4274, 61.0658, 89.2574, 108.214, 137.67,
    153.265, 204.836, 281.661, 384.751, 447.63, 523.747, 645.731, 776.344, 898.832, 1063.89,
    1314.22, 1570.96, 1815.05, 2139.76, 2631.72, 3150.35, 3641.58, 4300.5, 5262.17, 6311.62,
    7285.46, 8613.87, 11846.6, 11846.6, 11845.0,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::PARAMS;
    use crate::simd::Native;

    const RATE: f64 = 48000.0;

    fn laws(text: &str) -> Laws {
        let s = Sound::from_text(&format!("fm-sound/1\n{text}\n")).unwrap();
        let dests = [0, 2, 4].map(|i| {
            let d = Part::SlfoDest as usize + i;
            (i32::from(s.get(d)), i32::from(s.get(d + 1)))
        });
        Laws::new(&s, &dests)
    }

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    /// The parameter indices the laws read.
    #[test]
    fn indices() {
        let name = |i: usize| PARAMS[i].name;
        assert_eq!(name(P::Type as usize), "filter_type");
        assert_eq!(name(P::EgTime as usize), "filter_eg_time_hold");
        assert_eq!(name(P::EgLevel as usize + 4), "filter_eg_level_release");
        assert_eq!(name(P::EgTimeKeyCentre as usize), "filter_eg_time_key_centre");
        assert_eq!(name(P::Break as usize + 3), "cutoff_break4");
        assert_eq!(name(P::Offset as usize), "cutoff_offset1");
        assert_eq!(name(P::HpfKeyFollow as usize), "hpf_key_follow");
        assert_eq!(name(Part::FegDepthOffset as usize), "filter_eg_depth_offset");
        assert_eq!(name(Part::ResonanceOffset as usize), "resonance_offset");
        assert_eq!(name(Common::LfoFilterDepth as usize), "lfo_filter_depth");
        assert_eq!(name(Common::ScalingPan as usize), "scaling_pan");
    }

    /// Each law against the existing profile's measured numbers.
    #[test]
    fn filter_laws() {
        // the cutoff table at measured points, straight between, held at the top
        assert!(near(f64::from(cutoff_hz(0.0)), 14.328, 1e-3));
        assert!(near(f64::from(cutoff_hz(819.0)), 3569.42, 1e-2));
        assert!(near(f64::from(cutoff_hz(1023.0)), 7350.21, 1e-2));
        assert!(near(f64::from(cutoff_hz(4.0)), (14.328 + 15.7428) / 2.0, 1e-3));
        // the least Q from 3.57 kHz, 1 at the top; Q 0.5 x 2^(r / 16)
        assert_eq!(max_damping(1000.0), 2.0);
        assert!(near(f64::from(max_damping(7350.21)), 1.0, 1e-6));
        assert!(near(q(16), 1.0, 1e-12) && near(q(0), 0.5, 1e-12));
        // key follow: +63 is 200 %, straight in frequency within an octave
        assert!(near(key_follow_ratio(63, 72), 4.0, 1e-12));
        assert!(near(key_follow_ratio(10, 72), 1.3125, 1e-12));
        assert!(near(key_follow_ratio(32, 48), 0.5, 1e-12));
        assert!(near(key_follow_ratio(10, 54), 0.5 * (1.0 + 0.84375), 1e-12));
        // velocity: the +63 curve, a mirror below 0, upward moves in ratio
        assert!(near(velocity_ratio(63, 1.0), laws::exp2(-9609.0 / 1200.0), 1e-12));
        assert!(near(velocity_ratio(-64, 127.0), laws::exp2(-9609.0 / 1200.0), 1e-12));
        assert!(near(
            velocity_ratio(32, 127.0),
            1.0 + 32.0 / 63.0 * (laws::exp2(1201.0 / 1200.0) - 1.0),
            1e-12
        ));
        assert!(near(resonance_velocity(63, 64.0), 0.25, 1e-12));
        assert!(near(resonance_velocity(0, 1.0), 1.0, 0.0));
        // scaling: 37.5 cents a step between break points, held outside
        let (b, o) = ([36, 48, 60, 72], [32, 0, 0, -32]);
        assert!(near(scaling_ratio(30, b, o), laws::exp2(1.0), 1e-12));
        assert!(near(scaling_ratio(42, b, o), laws::exp2(0.5), 1e-12));
        assert!(near(scaling_ratio(90, b, o), 0.5, 1e-12));
        assert!(near(lp18s_ratio(160.0), (9.799 + 10.225) / 2.0, 1e-12));
        assert_eq!(width(0), (0.0, 0.0));
        assert!(near(width(127).0, 3.207 * 127.0 - 31.5, 1e-9));
        assert_eq!((gain(0), gain(255)), (0.0, 1.0));
        assert!(near(20.0 * gain(191).log10(), -24.0, 1e-9));
        assert!(near(lfo_octaves(25), 25.0 * 68.0 / 1200.0, 1e-12));
        assert!(near(slfo_pan_positions(127), 64.0 * (1.0 - (-6.35f64).exp()), 1e-9));
        // the envelope
        assert_eq!(feg_cents(16), 1200.0);
        assert!(near(feg_depth(63), 1.004, 1e-12) && near(feg_depth(-32), -0.497, 1e-12));
        assert!(near(feg_segment_ms(64.0), 776.344, 1e-9) && feg_segment_ms(20.0) == 0.0);
        assert!(near(feg_segment_ms(127.0), 11845.0, 1e-9));
        assert!(near(feg_hold_ms(48), 183.991, 1e-9));
        assert!(near(feg_depth_velocity(63, 2, 64.0), 0.548, 1e-12));
        assert!(near(feg_depth_velocity(-64, 0, 96.0), 0.427, 1e-12));
        assert_eq!(feg_time_velocity(-8, 64.0), -1000.0);
        assert!(near(feg_time_velocity(63, 127.0), -63.0 * 126.0 / 127.0, 1e-12));
        assert!(near(feg_time_key_scale(63, 48, 24), laws::exp2(-1.79 * 2.0), 1e-12));
        // the type numbers: 9, 18 and 20 are none
        assert_eq!(
            (kind(0), kind(10), kind(19), kind(21)),
            (Kind::Lp24, Kind::BpWide, Kind::Lp12Bp6, Kind::Thru)
        );
        assert_eq!(
            (kind(9), kind(18), kind(20), kind(-1)),
            (Kind::Thru, Kind::Thru, Kind::Thru, Kind::Thru)
        );
        // the part's offsets: 8 cutoff values, 2 resonance and 2 envelope depth values a step
        let l = laws(
            "cutoff=500 resonance=40 filter_eg_depth=10 cutoff_offset=-4 resonance_offset=3 filter_eg_depth_offset=1",
        );
        assert_eq!((l.value, l.resonance), (468.0, 46));
        assert!(near(f64::from(l.depth), feg_depth(12), 1e-7));
    }

    #[test]
    fn pan_laws() {
        assert_eq!(pan_gains(0), (1.0, 1.0));
        assert_eq!(pan_gains(32), (0.47799, 1.0));
        assert_eq!(pan_gains(-32), (1.0, 0.47799));
        assert_eq!(pan_gains(63), (0.0, 1.0));
        assert_eq!(pan_gains(-64), (1.0, 0.0));
        assert!(near(f64::from(pan_gains(61).0), 0.00528 * 2.0 / 3.0, 1e-7));
        assert!(near(f64::from(pan_gains(2).0), (1.0 + 0.99916) / 2.0, 1e-6));
        // scaling pan: depth / 32 a key outside 60-64, +63 as 64
        assert_eq!(
            (scaling_pan(63, 30), scaling_pan(32, 72), scaling_pan(-64, 62)),
            (-60, 8, 0)
        );
        // random pan within depth / 2
        assert_eq!((random_pan(127, 0), random_pan(127, u32::MAX)), (-64, 63));
        assert_eq!(random_pan(0, 12345), 0);
        // positions add and are held
        let l = laws("alternate_pan=40 scaling_pan=63");
        assert_eq!(
            (l.position(30, false, None), l.position(30, true, None)),
            (-20.0, -64.0)
        );
        assert!(l.pan_on && !laws("").pan_on);
    }

    // ---- the kernels against the sections' transfer functions ----

    type C = (f64, f64);
    fn mul(a: C, b: C) -> C {
        (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
    }
    fn div(a: C, b: C) -> C {
        let d = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
    }
    fn add(a: C, b: C) -> C {
        (a.0 + b.0, a.1 + b.1)
    }

    /// A section's low-pass, band-pass and high-pass at `w` radians a frame.
    fn sv(f: f64, d: f64, w: f64) -> [C; 3] {
        let z = (w.cos(), -w.sin()); // z^-1
        let a = (1.0 - z.0, -z.1);
        let hp = div(
            mul(a, a),
            add(add(mul(a, a), mul((f * f, 0.0), z)), mul(mul((d * f, 0.0), z), a)),
        );
        let bp = div(mul((f, 0.0), hp), a);
        let lp = div(mul(mul((f * f, 0.0), z), hp), mul(a, a));
        [lp, bp, hp]
    }

    fn pole(a: f64, w: f64) -> C {
        let z = (w.cos(), -w.sin());
        div((a, 0.0), (1.0 - (1.0 - a) * z.0, -(1.0 - a) * z.1))
    }

    /// The type's response from its coefficients `c` (as `targets` gives them).
    fn response(k: Kind, c: [f32; 5], w: f64) -> C {
        let [f0, d0, f1, d1, a] = c.map(f64::from);
        let (s0, s1, o) = (sv(f0, d0, w), sv(f1, d1, w), pole(a, w));
        let notch = |s: [C; 3]| add(s[0], s[2]);
        use Kind::*;
        match k {
            Lp24 | Lp24A => mul(s0[0], s1[0]),
            Lp18 => mul(s0[0], o),
            Lp18S => mul(o, s0[0]),
            Lp12Hp12 => mul(s0[0], s1[2]),
            Lp6Hp12 => mul(o, s1[2]),
            Hp24 => mul(s0[2], s1[2]),
            Hp12 => s0[2],
            Bp12 => mul(s0[1], s1[1]),
            BpWide => mul(s0[2], s1[0]),
            Bp6 => s0[1],
            Be12 | DualBe => mul(notch(s0), notch(s1)),
            Be6 => notch(s0),
            DualLp => add(s0[0], s1[0]),
            DualHp => add(s0[2], s1[2]),
            DualBp => add(s0[1], s1[1]),
            Lp12Bp6 => add(s0[0], s1[1]),
            Thru => (1.0, 0.0),
        }
    }

    /// One voice's filter, held still, on a sine at `hz` (at 0.1, so no state reaches the limit):
    /// its gain (the steady state's amplitude).
    fn measured(l: &Laws, hz: f64) -> (f64, [f32; 5]) {
        let mut v = VOICES0;
        v.start(l, 0, 60, 100.0, 0.0, true, RATE);
        let (mut re, mut im) = (0.0, 0.0);
        let n = 48000 / 10; // whole cycles of every test frequency (multiples of 10 Hz)
        for b in 0..(3 * n / STEP) {
            let mut y = [[0.0f32; 1]; STEP];
            for (i, y) in y.iter_mut().enumerate() {
                y[0] = 0.1 * (2.0 * core::f64::consts::PI * hz * (b * STEP + i) as f64 / RATE).sin() as f32;
            }
            v.filter::<1, scalar::F<1>>(l, RATE, 0.0, &[0.0; W], STEP, &mut y, (0.0, 0.0));
            if b >= 2 * n / STEP {
                for (i, y) in y.iter().enumerate() {
                    let t = 2.0 * core::f64::consts::PI * hz * (b * STEP + i) as f64 / RATE;
                    (re, im) = (re + f64::from(y[0]) * t.sin(), im + f64::from(y[0]) * t.cos());
                }
            }
        }
        (20.0 * re.hypot(im) / n as f64, v.c.map(|c| c[0]))
    }

    /// Every type's kernel plays its sections' structure: its steady-state gain within 0.01 dB of
    /// the transfer function's at its coefficients, at frequencies around the cutoff.
    #[test]
    fn every_type_plays_its_structure() {
        for raw in (0..=21).filter(|&r| is_type(r)) {
            let l = laws(&format!(
                "filter_type={raw} cutoff=600 resonance=50 hpf_cutoff=300 distance=20 cutoff_key_follow=0 filter_eg_depth=0"
            ));
            for hz in [100.0, 400.0, 900.0, 1500.0, 3000.0, 8000.0] {
                let (got, c) = measured(&l, hz);
                let w = 2.0 * core::f64::consts::PI * hz / RATE;
                let (re, im) = response(l.kind, c, w);
                let want = re.hypot(im);
                let db = 20.0 * (got / want).log10();
                assert!(
                    db.abs() < 0.01 || (got < 1e-5 && want < 1e-5),
                    "type {raw} at {hz} Hz: {db:.4} dB ({got} against {want})"
                );
            }
        }
    }

    /// High-pass 12 at resonance 0 is the measured section: Q 0.5, its cutoff the table's.
    #[test]
    fn high_pass_at_its_cutoff() {
        let l = laws("filter_type=7 cutoff=512 resonance=0 cutoff_key_follow=0 filter_eg_depth=0");
        let (_, c) = measured(&l, 100.0);
        let fc = f64::from(cutoff_hz(512.0));
        assert!(near(
            f64::from(c[0]),
            2.0 * (core::f64::consts::PI * fc / RATE).sin(),
            1e-6
        ));
        assert!(near(f64::from(c[1]), 2.0, 1e-6));
    }

    /// The envelope: from the hold level, each segment straight in cents taking its time value's
    /// duration whatever its distance, decay 2 held while the key is down, then the release.
    #[test]
    fn envelope_segments() {
        let l = laws(
            "filter_type=7 filter_eg_level_hold=-16 filter_eg_level_attack=32 filter_eg_level_decay1=8 \
             filter_eg_level_decay2=4 filter_eg_level_release=-8 filter_eg_time_hold=48 filter_eg_time_attack=40 \
             filter_eg_time_decay1=64 filter_eg_time_decay2=52 filter_eg_time_release=60 filter_eg_depth=63",
        );
        let mut v = VOICES0;
        v.start(&l, 0, 60, 100.0, 0.0, true, RATE);
        let frames = |ms: f64| ms * RATE / 1000.0;
        let (hold, attack, d1, d2) = (frames(183.991), frames(64.4274), frames(776.344), frames(204.836));
        // the level after `frames` more frames
        let on = |v: &mut Voices, frames: f64| f64::from(v.advance(&l, 0, frames as f32, RATE));
        assert_eq!(on(&mut v, hold - 1.0), -1200.0);
        assert!(near(on(&mut v, 1.0 + attack / 2.0), (-1200.0 + 2400.0) / 2.0, 1.0));
        assert!(near(
            on(&mut v, attack / 2.0 + d1 / 4.0),
            2400.0 - (2400.0 - 600.0) / 4.0,
            1.0
        ));
        assert!(near(on(&mut v, d1 * 0.75 + d2 + 48000.0), 300.0, 1e-3));
        v.release(&l, 0, RATE);
        let r = frames(523.747);
        assert!(near(on(&mut v, r / 2.0), (300.0 - 600.0) / 2.0, 1.0));
        assert_eq!(on(&mut v, r / 2.0), -600.0);
        assert_eq!(on(&mut v, 48000.0), -600.0);
        // time key follow +63 two octaves above the centre: 2^3.58 (12) times shorter
        let k = laws("filter_eg_time_attack=64 filter_eg_level_attack=16 filter_eg_time_key_follow=63 filter_eg_time_key_centre=36");
        let mut v = VOICES0;
        v.start(&k, 0, 60, 100.0, 0.0, true, RATE);
        let half = frames(776.344) / laws::exp2(3.58) / 2.0;
        assert!(near(f64::from(v.advance(&k, 0, half as f32, RATE)), 600.0, 1.0));
    }

    /// No parameters, input or LFO values make a non-finite or unbounded output: random types,
    /// cutoffs (to the top), resonances (to 127), distances, envelopes and LFOs on a loud noise.
    #[test]
    fn stable_for_any_parameters() {
        let mut seed = 0x9e37_79b9u32;
        let mut r = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for case in 0..400 {
            let mut s = Sound::default();
            for p in PARAMS
                .iter()
                .filter(|p| (100..=134).contains(&p.id) || [29, 253, 254, 255].contains(&p.id))
            {
                let span = (i32::from(p.max) - i32::from(p.min) + 1) as u32;
                s.set(p.id, (i32::from(p.min) + (r() % span) as i32) as i16).unwrap();
            }
            // the worst cases often: the top cutoff, full resonance, the widest distance
            if case % 2 == 0 {
                s.set(101, 1023).unwrap();
                s.set(103, 127).unwrap();
                s.set(106, 127).unwrap();
            }
            let l = Laws::new(&s, &[(72, (r() % 128) as i32), (73, (r() % 128) as i32), (70, 64)]);
            let mut v = VOICES0;
            for slot in 0..W {
                v.start(
                    &l,
                    slot,
                    (r() % 128) as i32,
                    f64::from(1 + r() % 127),
                    0.0,
                    slot == 0,
                    RATE,
                );
            }
            for b in 0..300 {
                if b == 150 {
                    (0..W).for_each(|s| v.release(&l, s, RATE));
                }
                let mut y = [[0.0f32; W]; STEP];
                for y in y.iter_mut().flatten() {
                    *y = (r() as f32 / u32::MAX as f32 - 0.5) * 8.0;
                }
                let sv = r() as f32 / u32::MAX as f32 * 2.0 - 1.0;
                let lfo = [sv, -sv, 1.0, -1.0];
                // the controller boxes' cutoff and resonance at any value within their spans
                let bx = ((r() % 2049) as f32 - 1024.0, (r() % 257) as f32 - 128.0);
                v.filter::<W, Native>(&l, RATE, sv, &lfo, 1 + (r() as usize % STEP), &mut y, bx);
                assert!(
                    y.iter().flatten().all(|x| x.is_finite() && x.abs() <= 64.0),
                    "case {case} block {b}"
                );
            }
        }
    }

    /// At rest the states fall to exactly 0 (never subnormal): an impulse at high resonance, then
    /// silence.
    #[test]
    fn states_flush_at_rest() {
        let l = laws("filter_type=0 cutoff=300 resonance=100");
        let mut v = VOICES0;
        v.start(&l, 0, 60, 100.0, 0.0, true, RATE);
        for b in 0..20_000 {
            let mut y = [[if b == 0 { 1.0 } else { 0.0 }; 1]; STEP];
            v.filter::<1, scalar::F<1>>(&l, RATE, 0.0, &[0.0; W], STEP, &mut y, (0.0, 0.0));
            assert!(
                y.iter().all(|x| x[0] == 0.0 || x[0].abs() >= f32::MIN_POSITIVE),
                "block {b}"
            );
        }
        assert!(v.st.iter().all(|s| s[0] == 0.0), "{:?}", v.st);
    }
}
