// SPDX-License-Identifier: AGPL-3.0-or-later
//! The controller boxes (the model's controller sets, docs/design.md 3.9): a source through a
//! curve, polarity and ratio moving a destination by steps of its raw value. The laws are the box
//! system's, measured on the reference through the sibling engines' parts (the source maps, the
//! output and its limit, the curves) and through this part type (the operator level's and the
//! cutoff's spans); a box on any other destination, from a source the host cannot send, or on a
//! curve not measured is reported by the coverage report (a curve not measured plays as linear).

use crate::params::{ctrl_set_index, Common, Part, BOX_BASE, BOX_FIELDS, OPS};
use crate::sound::Sound;

pub const BOXES: usize = 32;

/// The box sources the engine takes, by the reference's numbers: 0 pitch bend, 1 mod wheel, 2
/// aftertouch, 3 and 4 the foot controllers, 6 the ribbon, 7 breath, 8-15 the assignable knobs,
/// 16 and 17 the assignable switches (5, the foot switch, is not received).
pub const SOURCES: usize = 18;

/// A controller's value 0-127 as a box sees it, 0-1024: 8 v up to 64, then 512 + (v - 64) x
/// 512 / 63 truncated (measured: within 0.1 cent through the fine tune at 17 values).
pub fn controller(v: u8) -> f32 {
    let v = i32::from(v.min(127));
    (if v <= 64 { 8 * v } else { 512 + (v - 64) * 512 / 63 }) as f32
}

/// The pitch bend (-8192..+8191) as a box sees it: its 14-bit value / 16.
pub fn bend(b: i16) -> f32 {
    f32::from(b.clamp(-8192, 8191)) / 16.0 + 512.0
}

/// The sound's own sources (ribbon, knobs, switches) into `src`: a knob as its raw value, the top
/// (1023) as 1024; the ribbon as its controller; a switch off 0, on 1024 (measured).
pub fn own(s: &Sound, src: &mut [f32; SOURCES]) {
    let g = |k: usize| s.get(k);
    if let Some(r) = src.get_mut(6) {
        *r = controller(g(Part::Ribbon as usize).clamp(0, 127) as u8);
    }
    for (k, x) in src.iter_mut().skip(8).take(8).enumerate() {
        let v = g(Part::Knob as usize + k).clamp(0, 1023);
        *x = if v >= 1023 { 1024.0 } else { f32::from(v) };
    }
    for (k, x) in src.iter_mut().skip(16).enumerate() {
        *x = if g(Part::Switch as usize + k) != 0 { 1024.0 } else { 0.0 };
    }
}

/// The measured curves: bank, type, parameters 1 and 2 (2 only for type 2), and their value
/// (0-1024) at 17 knots of the source. Bank 0, type 0, parameter 5 is linear (measured), not here.
struct Curve {
    key: (i16, i16, i16, i16),
    x: [f32; 17],
    y: [f32; 17],
}

/// The knots: the mod wheel's 17 measured values, and the analog part's.
const W17: [f32; 17] = [
    0., 64., 128., 192., 256., 320., 384., 448., 512., 577., 642., 707., 772.1, 837.1, 902.1, 967.1, 1024.,
];
const A17: [f32; 17] = [
    0., 64., 129., 193., 258., 322., 387., 451., 516., 580., 644., 709., 773., 838., 902., 966., 1023.,
];

#[rustfmt::skip]
const CURVES: [Curve; 8] = [
    Curve { key: (0, 0, 0, 0), x: W17, y: [0., 754., 879., 931., 952., 973., 984., 993., 1004., 1004., 1004., 1015., 1015., 1015., 1024., 1024., 1024.] },
    Curve { key: (0, 0, 2, 0), x: W17, y: [0., 255., 432., 545., 640., 713., 765., 816., 858., 889., 910., 942., 962., 984., 993., 1015., 1024.] },
    Curve { key: (0, 0, 8, 0), x: W17, y: [0., 16., 26., 47., 68., 88., 109., 140., 172., 213., 255., 317., 390., 484., 609., 796., 1024.] },
    Curve { key: (0, 0, 6, 0), x: A17, y: [0., 33., 71., 118., 156., 204., 261., 308., 365., 431., 497., 573., 649., 734., 829., 924., 1024.] },
    Curve { key: (0, 0, 9, 0), x: A17, y: [0., 0., 14., 24., 33., 43., 52., 71., 81., 109., 137., 175., 223., 298., 412., 630., 1024.] },
    Curve { key: (0, 0, 10, 0), x: A17, y: [0., 0., 0., 0., 0., 14., 14., 14., 24., 33., 43., 52., 71., 99., 156., 298., 1024.] },
    Curve { key: (0, 2, 10, 4), x: A17, y: [0., 0., 0., 0., 0., 0., 0., 0., 33., 156., 280., 412., 526., 659., 791., 914., 1024.] },
    Curve { key: (0, 2, 0, 4), x: A17, y: [0., 128., 251., 374., 507., 621., 753., 877., 1009., 1024., 1024., 1024., 1024., 1024., 1024., 1024., 1024.] },
];

/// A box's curve: linear (`Some(None)`), a measured one, or `None` for one not measured.
fn curve(bank: i16, kind: i16, p1: i16, p2: i16) -> Option<Option<usize>> {
    if (bank, kind, p1) == (0, 0, 5) {
        return Some(None);
    }
    let p2 = if kind == 2 { p2 } else { 0 };
    CURVES.iter().position(|c| c.key == (bank, kind, p1, p2)).map(Some)
}

/// Curve `i` at `c` (0-1024), linear between its knots.
fn curve_at(i: Option<usize>, c: f32) -> f32 {
    let Some(t) = i.and_then(|i| CURVES.get(i)) else {
        return c;
    };
    let k = t.x.iter().skip(1).position(|&x| c <= x).unwrap_or(15);
    let (x0, x1, y0, y1) = (at(&t.x, k), at(&t.x, k + 1), at(&t.y, k), at(&t.y, k + 1));
    y0 + (y1 - y0) * ((c - x0) / (x1 - x0)).clamp(0.0, 1.0)
}

fn at(a: &[f32; 17], i: usize) -> f32 {
    a.get(i).copied().unwrap_or(0.0)
}

/// What a box moves: an operator's level (the reference's destination 106, the operators whose
/// switch for the box is on), the filter's cutoff (102) or resonance (103), the voice LFO's speed
/// (89, its normal range), pitch depth (90) or amplitude depth (91), the sound's pitch (54), an
/// operator's frequency (105, the operators whose switch for the box is on, by their pitch
/// controller sensitivity).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    Level,
    Cutoff,
    Resonance,
    LfoSpeed,
    LfoPitch,
    LfoAmp,
    Pitch,
    OpPitch,
}

impl Dest {
    fn of(reference: i16) -> Option<Dest> {
        match reference {
            106 => Some(Dest::Level),
            105 => Some(Dest::OpPitch),
            102 => Some(Dest::Cutoff),
            103 => Some(Dest::Resonance),
            89 => Some(Dest::LfoSpeed),
            90 => Some(Dest::LfoPitch),
            91 => Some(Dest::LfoAmp),
            54 => Some(Dest::Pitch),
            _ => None,
        }
    }

    /// A full box's steps: the level 96 and the cutoff 1024 (measured on this part type: 0.75 and
    /// 8 a ratio unit at full travel of a unipolar box), the resonance 128 (measured on the sample
    /// part's resonance, the same raw scale; not measured on this part type); the voice LFO's
    /// speed and depths 100 of their 0-99 values, the pitch 8192 cents, an operator's frequency
    /// 3600 cents at sensitivity +7 (measured on this part type; the pitches up to a quarter of
    /// a full box).
    pub fn span(self) -> i32 {
        match self {
            Dest::Level => 96,
            Dest::Cutoff => 1024,
            Dest::Resonance => 128,
            Dest::LfoSpeed | Dest::LfoPitch | Dest::LfoAmp => 100,
            Dest::Pitch => 8192,
            Dest::OpPitch => 3600,
        }
    }
}

/// A box the engine plays.
#[derive(Clone, Copy, Debug)]
pub struct Box {
    /// Its number (0-31): the operators' switch for it.
    pub n: usize,
    pub src: usize,
    pub dest: Dest,
    bipolar: bool,
    ratio: f32,
    curve: Option<usize>,
}

impl Box {
    /// Box `n` of `s`, if the engine plays it (a destination it has, a source it receives, a
    /// ratio), and why not otherwise (`None`: switched off or no ratio).
    fn read(s: &Sound, n: usize) -> Result<Box, Option<&'static str>> {
        let g = |k: usize| s.get(BOX_BASE.wrapping_add(BOX_FIELDS.wrapping_mul(n)).wrapping_add(k));
        let (src, dst, ratio) = (g(0), g(1), g(2));
        if dst == 0 || ratio == 0 {
            return Err(None);
        }
        let dest = Dest::of(dst).ok_or(Some("not covered (a destination without a law, or not the engine's)"))?;
        if dest == Dest::LfoSpeed && s.get(Common::LfoSpeedRange as usize) != 0 {
            return Err(Some("not covered (the voice LFO's extended speed range: not measured)"));
        }
        if !(0..SOURCES as i16).contains(&src) || src == 5 {
            return Err(Some("not covered (a source the host does not send)"));
        }
        Ok(Box {
            n,
            src: src as usize,
            dest,
            bipolar: g(3) != 0,
            // +127 plays as +128 (measured)
            ratio: f32::from(if ratio == 127 { 128 } else { ratio }),
            curve: curve(g(4), g(5), g(6), g(7)).flatten(),
        })
    }

    /// Its steps with its source at `c` (0-1024): the output (curve(c) + o - 1024) x ratio / 8
    /// within +-8192 (o 1024 unipolar, 512 bipolar), times span / 8192, toward zero (measured).
    pub fn steps(&self, c: f32) -> i32 {
        let o = if self.bipolar { 512.0 } else { 1024.0 };
        let out = ((curve_at(self.curve, c) + o - 1024.0) * self.ratio / 8.0).clamp(-8192.0, 8192.0);
        (out * self.dest.span() as f32 / 8192.0) as i32
    }

    /// The most it raises its destination (0 if it only lowers it): at a curve's ends.
    fn up(&self) -> i32 {
        let (lo, hi) = (
            Box { curve: None, ..*self }.steps(0.0),
            Box { curve: None, ..*self }.steps(1024.0),
        );
        lo.max(hi).max(0)
    }

    /// Whether it reaches operator `op` (that operator's switch for it).
    pub fn reaches(&self, s: &Sound, op: usize) -> bool {
        matches!(self.dest, Dest::Level | Dest::OpPitch) && s.get(ctrl_set_index(op, self.n)) != 0
    }
}

/// The boxes of `s` the engine plays.
pub fn boxes(s: &Sound) -> impl Iterator<Item = Box> + '_ {
    (0..BOXES).filter_map(|n| Box::read(s, n).ok())
}

/// Why box `n` of `s` is not played, or played only in part (a curve not measured, as linear).
pub fn not_played(s: &Sound, n: usize) -> Option<&'static str> {
    match Box::read(s, n) {
        Err(why) => why,
        Ok(_) => {
            let g = |k: usize| s.get(BOX_BASE.wrapping_add(BOX_FIELDS.wrapping_mul(n)).wrapping_add(k));
            curve(g(4), g(5), g(6), g(7))
                .is_none()
                .then_some("not covered (a curve not measured: played as linear)")
        }
    }
}

/// The boxes' steps on each destination with the sources at `src`: each box's steps summed, the
/// sum within +-span (measured).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Steps {
    pub level: [i32; OPS],
    pub cutoff: i32,
    pub resonance: i32,
    pub lfo_speed: i32,
    pub lfo_pitch: i32,
    pub lfo_amp: i32,
    /// Cents: the sound's, and each operator's at sensitivity +7.
    pub pitch: i32,
    pub op_pitch: [i32; OPS],
}

pub fn steps(s: &Sound, src: &[f32; SOURCES]) -> Steps {
    let mut t = Steps::default();
    for b in boxes(s) {
        let k = b.steps(src.get(b.src).copied().unwrap_or(0.0));
        let add = |x: &mut i32| *x = x.saturating_add(k);
        match b.dest {
            Dest::Cutoff => add(&mut t.cutoff),
            Dest::Resonance => add(&mut t.resonance),
            Dest::LfoSpeed => add(&mut t.lfo_speed),
            Dest::LfoPitch => add(&mut t.lfo_pitch),
            Dest::LfoAmp => add(&mut t.lfo_amp),
            Dest::Pitch => add(&mut t.pitch),
            Dest::Level | Dest::OpPitch => {
                let ops = if b.dest == Dest::Level {
                    &mut t.level
                } else {
                    &mut t.op_pitch
                };
                for (op, l) in ops.iter_mut().enumerate() {
                    if b.reaches(s, op) {
                        add(l);
                    }
                }
            }
        }
    }
    let clamp = |x: i32, d: Dest| x.clamp(-d.span(), d.span());
    t.level = t.level.map(|x| clamp(x, Dest::Level));
    t.op_pitch = t.op_pitch.map(|x| clamp(x, Dest::OpPitch));
    t.cutoff = clamp(t.cutoff, Dest::Cutoff);
    t.resonance = clamp(t.resonance, Dest::Resonance);
    t.lfo_speed = clamp(t.lfo_speed, Dest::LfoSpeed);
    t.lfo_pitch = clamp(t.lfo_pitch, Dest::LfoPitch);
    t.lfo_amp = clamp(t.lfo_amp, Dest::LfoAmp);
    t.pitch = clamp(t.pitch, Dest::Pitch);
    t
}

/// A voice LFO value 0-99 (speed, pitch or amplitude depth) moved by its boxes' steps: within
/// 0-99 (measured).
pub fn lfo_value(stored: i16, steps: i32) -> i32 {
    i32::from(stored).saturating_add(steps).clamp(0, 99)
}

/// Whether `s` has a box the engine plays on `d`.
pub fn moves(s: &Sound, d: Dest) -> bool {
    boxes(s).any(|b| b.dest == d)
}

/// The most the boxes can raise each operator's level, in steps (within one span).
pub fn reach(s: &Sound) -> [i32; OPS] {
    let mut up = [0i32; OPS];
    for b in boxes(s).filter(|b| b.dest == Dest::Level) {
        for (op, u) in up.iter_mut().enumerate() {
            if b.reaches(s, op) {
                *u = (*u).saturating_add(b.up());
            }
        }
    }
    up.map(|u| u.min(Dest::Level.span()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laws;

    fn sound(t: &str) -> Sound {
        Sound::from_text(&format!("fm-sound/1\n{t}\n")).unwrap()
    }

    #[test]
    fn source_maps() {
        assert_eq!(
            [0, 1, 64, 65, 126, 127].map(controller),
            [0.0, 8.0, 512.0, 520.0, 1015.0, 1024.0]
        );
        assert_eq!([-8192, 0, 8191].map(bend), [0.0, 512.0, 1023.9375]);
        let mut src = [0.0; SOURCES];
        own(&sound("knob1=1023 knob2=300 ribbon=64 switch2=1"), &mut src);
        assert_eq!(
            (src[8], src[9], src[10], src[6], src[16], src[17]),
            (1024.0, 300.0, 512.0, 512.0, 0.0, 1024.0)
        );
    }

    /// A unipolar box adds ratio x span / 64 at full travel (the measured 0.75 level and 8
    /// cutoff steps a ratio unit), up to the span (an output of 8192); a bipolar one swings half
    /// that either way about its centre; each box is truncated toward zero, the sum held within
    /// the span.
    #[test]
    fn box_output_law() {
        let s = sound("box1_dst=106 box1_ratio=16 box2_dst=102 box2_ratio=-20 box2_pol=1 box3_dst=103 box3_ratio=127");
        let b: Vec<Box> = boxes(&s).collect();
        assert_eq!(b.len(), 3);
        assert_eq!([0.0, 512.0, 1024.0].map(|c| b[0].steps(c)), [0, 12, 24]);
        assert_eq!([0.0, 512.0, 1024.0].map(|c| b[1].steps(c)), [160, 0, -160]);
        assert_eq!(b[2].steps(1024.0), 128);
        assert_eq!(b[2].steps(64.0), 128 * 64 * 128 / 8 / 8192);
        // five boxes of +16 on the level: 5 x 24 steps held at the span; operator 3 with two of
        // its switches off takes three
        let many = (1..=5)
            .map(|n| format!("box{n}_dst=106 box{n}_ratio=16 "))
            .collect::<String>();
        let s = sound(&format!("{many}op3.ctrl_set2=0 op3.ctrl_set3=0"));
        let t = steps(&s, &[1024.0; SOURCES]);
        assert_eq!((t.level[0], t.level[2]), (96, 72));
        assert_eq!(reach(&s), [96, 96, 72, 96, 96, 96, 96, 96]);
    }

    /// One unipolar box from the wheel at `v` on destination `dst`, with `extra`: its steps.
    fn one(dst: i16, ratio: i16, v: u8, extra: &str) -> Steps {
        let s = sound(&format!("box1_dst={dst} box1_ratio={ratio} {extra}"));
        let mut src = [0.0; SOURCES];
        src[1] = controller(v);
        steps(&s, &src)
    }

    /// The voice LFO's pitch depth (destination 90), measured on the reference: a lone carrier at
    /// pitch depth offset 5, its swing peak to peak in cents by (ratio, wheel, stored depth); the
    /// box moves the depth 100 steps at full output, toward zero, within 0-99, and the depth plays
    /// by its law (within 0.1 cent at every point; a second LFO speed alike).
    #[test]
    fn lfo_pitch_depth_box_measured() {
        #[rustfmt::skip]
        let points: [(i16, u8, i16, f64); 30] = [
            (16, 32, 0, 105.41), (16, 64, 0, 210.94), (16, 96, 0, 323.42), (16, 127, 0, 449.98),
            (64, 32, 0, 450.00), (64, 64, 0, 900.00), (64, 96, 0, 1357.05), (64, 127, 0, 1800.02),
            (8, 32, 0, 49.14), (8, 64, 0, 105.41), (8, 96, 0, 161.67), (8, 127, 0, 210.95),
            (127, 32, 0, 900.00), (127, 64, 0, 1800.03),
            (32, 96, 0, 667.92), (48, 32, 0, 323.44), (48, 96, 0, 1012.51), (48, 127, 0, 1357.04),
            (16, 0, 40, 724.18), (16, 32, 40, 829.66), (16, 64, 40, 942.13), (16, 96, 40, 1047.60),
            (127, 32, 40, 1631.25), (-16, 32, 40, 611.66), (-16, 96, 40, 393.73), (-16, 127, 40, 267.17),
            (-64, 64, 40, 0.0), (32, 96, 20, 1026.60), (32, 127, 20, 1265.59), (-32, 32, 20, 140.61),
        ];
        for (ratio, v, depth, cents) in points {
            let st = one(90, ratio, v, &format!("lfo_pitch_depth={depth}")).lfo_pitch;
            let d = laws::lfo_pitch_d(lfo_value(depth, st));
            let pp = 2.0 * d * laws::lfo_pitch_offset(5) * 1200.0 / 32768.0;
            assert!((pp - cents).abs() < 0.1, "{ratio} {v} {depth}: {pp} against {cents}");
        }
    }

    /// The voice LFO's amplitude depth (91), speed (89, the normal range) and the sound's pitch
    /// (54), measured on the reference: the amplitude swing peak to peak at offset 7 (dB, within
    /// 0.2: depth 37 falls between the depth law's measured points), the rate (within 0.5 %) and
    /// the pitch (cents, within 0.5), the first two moved 100 steps a full box like the depth.
    #[test]
    fn lfo_amp_speed_and_pitch_boxes_measured() {
        for (v, db) in [(32, 1.881), (64, 4.893), (96, 7.905), (127, 12.047)] {
            let got = laws::lfo_amp_db(lfo_value(0, one(91, 32, v, "").lfo_amp));
            assert!((got - db).abs() < 0.2, "{v}: {got} against {db}");
        }
        for (ratio, v, hz) in [
            (32, 32, 5.299),
            (32, 64, 7.486),
            (32, 96, 9.421),
            (32, 127, 16.823),
            (127, 64, 49.796),
        ] {
            let st = one(89, ratio, v, "lfo_speed_range=0 lfo_speed=20").lfo_speed;
            let got = laws::lfo_hz(false, lfo_value(20, st));
            assert!((got / hz - 1.0).abs() < 0.005, "{ratio} {v}: {got} against {hz}");
        }
        for (v, cents) in [(32, 511.56), (64, 1023.58), (96, 1544.19), (127, 2047.92)] {
            let got = f64::from(one(54, 16, v, "").pitch);
            assert!((got - cents).abs() < 0.5, "{v}: {got} against {cents}");
        }
        // the extended speed range is not measured: reported
        let s = sound("box1_dst=89 box1_ratio=8 lfo_speed_range=1");
        assert!(not_played(&s, 0).unwrap().contains("extended"));
    }

    /// An operator's frequency (105) by its pitch controller sensitivity, measured on the
    /// reference: a lone carrier's pitch (cents) by (sensitivity, wheel) with ratio 16, 3600 cents
    /// a full box at +7, sensitivity / 7 of that (within 1.5 cents; none at 0). The level
    /// controller sensitivity left the level boxes as they are at -7, 0, +3 and +7: ratio -32 at
    /// the wheel's 64 and top, 18.07 and 37.00 dB down from level 99 at each.
    #[test]
    fn operator_pitch_box_and_the_sensitivities_measured() {
        #[rustfmt::skip]
        let points = [
            (7, 64, 449.66), (7, 127, 899.45), (3, 64, 191.78), (3, 127, 384.40),
            (-7, 64, -449.22), (-7, 127, -899.20), (0, 64, 0.0), (0, 127, 0.0),
        ];
        for (sens, v, cents) in points {
            let t = one(105, 16, v, &format!("op1.pitch_ctrl_sens={sens} op2.ctrl_set1=0"));
            let got = f64::from(t.op_pitch[0]) * f64::from(sens) / 7.0;
            assert!((got - cents).abs() < 1.5, "{sens} {v}: {got} against {cents}");
            assert_eq!(t.op_pitch[1], 0);
        }
        // (level 51 falls between the level law's measured points: 0.62 dB off there)
        for (v, db, within) in [(64, -18.07, 0.2), (127, -37.00, 0.65)] {
            let t = one(106, -32, v, "");
            let got = laws::carrier_level_db(99 + t.level[0]) - laws::carrier_level_db(99);
            assert!((got - db).abs() < within, "{v}: {got} against {db}");
        }
    }

    #[test]
    fn curves_and_what_is_not_played() {
        // the measured curve (0, 0, 8): 172 at the wheel's 64
        let s = sound("box1_dst=102 box1_ratio=8 box1_p1=8 box2_dst=55 box2_ratio=8 box3_dst=102 box3_ratio=8 box3_src=20 box4_dst=102 box4_ratio=8 box4_p1=1");
        let b = boxes(&s).next().unwrap();
        assert_eq!(b.steps(512.0), (172.0f32 * 8.0 / 8.0 * 1024.0 / 8192.0) as i32);
        assert_eq!(curve_at(b.curve, 1024.0), 1024.0);
        assert_eq!(not_played(&s, 0), None);
        assert!(not_played(&s, 1).unwrap().contains("destination"));
        assert!(not_played(&s, 2).unwrap().contains("source"));
        assert!(not_played(&s, 3).unwrap().contains("curve"));
        assert_eq!(not_played(&s, 4), None);
    }
}
