// SPDX-License-Identifier: AGPL-3.0-or-later
//! The controller boxes (the model's controller sets, docs/design.md 3.9): a source through a
//! curve, polarity and ratio moving a destination by steps of its raw value. The laws are the box
//! system's, measured on the reference through the sibling engines' parts (the source maps, the
//! output and its limit, the curves) and through this part type (the operator level's and the
//! cutoff's spans); a box on any other destination, from a source the host cannot send, or on a
//! curve not measured is reported by the coverage report (a curve not measured plays as linear).

use crate::params::{ctrl_set_index, Part, BOX_BASE, BOX_FIELDS, OPS};
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
/// switch for the box is on), the filter's cutoff (102) or resonance (103).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    Level,
    Cutoff,
    Resonance,
}

impl Dest {
    fn of(reference: i16) -> Option<Dest> {
        match reference {
            106 => Some(Dest::Level),
            102 => Some(Dest::Cutoff),
            103 => Some(Dest::Resonance),
            _ => None,
        }
    }

    /// A full box's steps: the level 96 and the cutoff 1024 (measured on this part type: 0.75 and
    /// 8 a ratio unit at full travel of a unipolar box), the resonance 128 (measured on the sample
    /// part's resonance, the same raw scale; not measured on this part type).
    pub fn span(self) -> i32 {
        match self {
            Dest::Level => 96,
            Dest::Cutoff => 1024,
            Dest::Resonance => 128,
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
        let g = |k: usize| s.get(BOX_BASE + BOX_FIELDS * n + k);
        let (src, dst, ratio) = (g(0), g(1), g(2));
        if dst == 0 || ratio == 0 {
            return Err(None);
        }
        let dest = Dest::of(dst).ok_or(Some("not covered (a destination without a law, or not the engine's)"))?;
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
        self.dest == Dest::Level && s.get(ctrl_set_index(op, self.n)) != 0
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
            let g = |k: usize| s.get(BOX_BASE + BOX_FIELDS * n + k);
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
}

pub fn steps(s: &Sound, src: &[f32; SOURCES]) -> Steps {
    let mut t = Steps::default();
    for b in boxes(s) {
        let k = b.steps(src.get(b.src).copied().unwrap_or(0.0));
        match b.dest {
            Dest::Cutoff => t.cutoff += k,
            Dest::Resonance => t.resonance += k,
            Dest::Level => {
                for (op, l) in t.level.iter_mut().enumerate() {
                    if b.reaches(s, op) {
                        *l += k;
                    }
                }
            }
        }
    }
    let clamp = |x: i32, d: Dest| x.clamp(-d.span(), d.span());
    t.level = t.level.map(|x| clamp(x, Dest::Level));
    t.cutoff = clamp(t.cutoff, Dest::Cutoff);
    t.resonance = clamp(t.resonance, Dest::Resonance);
    t
}

/// The most the boxes can raise each operator's level, in steps (within one span).
pub fn reach(s: &Sound) -> [i32; OPS] {
    let mut up = [0; OPS];
    for b in boxes(s) {
        for (op, u) in up.iter_mut().enumerate() {
            if b.reaches(s, op) {
                *u += b.up();
            }
        }
    }
    up.map(|u| u.min(Dest::Level.span()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
