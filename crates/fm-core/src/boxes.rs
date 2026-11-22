// SPDX-License-Identifier: AGPL-3.0-or-later
//! The controller boxes (the model's controller sets, docs/design.md 3.9): a source through a
//! curve, polarity and ratio moving a destination by steps of its raw value. The laws are the box
//! system's, measured on the reference through the sibling engines' parts (the source maps, the
//! output and its limit) and through this part type (the destinations' spans, the curves: the same
//! shapes as the sibling engines' measurements, within their precision); a box on any other destination, from a source the host cannot send, or on a
//! curve not measured is reported by the coverage report (a curve not measured plays as linear).

use crate::params::{ctrl_set_index, op_index, Common, Op, Part, BOX_BASE, BOX_FIELDS, OPS};
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

/// A box's curve: linear (`Some(None)`, bank 0, type 0, parameter 5: measured), a measured one (its
/// place in `crate::curves_data::CURVES`), or `None` for one not measured. Parameter 2 counts for
/// type 2 only.
fn curve(bank: i16, kind: i16, p1: i16, p2: i16) -> Option<Option<usize>> {
    if (bank, kind, p1) == (0, 0, 5) {
        return Some(None);
    }
    let p2 = if kind == 2 { p2 } else { 0 };
    if bank != 0 {
        return None;
    }
    crate::curves_data::CURVES
        .iter()
        .position(|c| (c.0, c.1, c.2) == (kind, p1, p2))
        .map(Some)
}

/// Curve `i` at `c` (0-1024): straight between its values at the wheel's 128 values (at `controller`'s
/// points).
fn curve_at(i: Option<usize>, c: f32) -> f32 {
    let Some(y) = i.and_then(|i| crate::curves_data::CURVES.get(i)).map(|c| &c.3) else {
        return c;
    };
    // the wheel value at or below c (the controller map's inverse)
    let t = if c <= 512.0 {
        c / 8.0
    } else {
        64.0 + (c - 512.0) * 63.0 / 512.0
    };
    let v = (t.max(0.0) as usize).min(126);
    let (x0, x1) = (controller(v as u8), controller((v as u8).wrapping_add(1)));
    let at = |k: usize| f32::from(y.get(k).copied().unwrap_or(0));
    at(v) + (at(v + 1) - at(v)) * ((c - x0) / (x1 - x0)).clamp(0.0, 1.0)
}

/// What a box moves: an operator's level (the reference's destination 106, the operators whose
/// switch for the box is on), the filter's cutoff (102) or resonance (103), the voice LFO's speed
/// (89; in the extended range `LfoExtSpeed`), pitch depth (90), amplitude depth (91) or filter depth (92), the sound's
/// pitch (54), an operator's frequency (105, the operators whose switch for the box is on, by their
/// pitch controller sensitivity), the part's amplitude envelope attack (95), decay 1 and 2 (96)
/// and release (97) offsets, the feedback (104); for the operators whose switch for the box is on:
/// the form (107), skirt (108) and resonance (109), the level by the level controller sensitivity
/// (111), the attack, decay 1, decay 2 and release times (112-115).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    Level,
    Cutoff,
    Resonance,
    LfoSpeed,
    LfoExtSpeed,
    LfoPitch,
    LfoAmp,
    LfoFilter,
    Pitch,
    OpPitch,
    AmpAttack,
    AmpDecay,
    AmpRelease,
    Feedback,
    OpForm,
    OpSpread,
    OpFormant,
    OpLevelSens,
    OpAttack,
    OpDecay1,
    OpDecay2,
    OpRelease,
    OpDetune,
    OpPegDecay,
    MonoPoly,
    Pan,
    FegDepth,
    PortaTime,
    SlfoSpeed,
    SlfoDepth1,
    SlfoDepth2,
    SlfoDepth3,
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
            92 => Some(Dest::LfoFilter),
            95 => Some(Dest::AmpAttack),
            96 => Some(Dest::AmpDecay),
            97 => Some(Dest::AmpRelease),
            104 => Some(Dest::Feedback),
            107 => Some(Dest::OpForm),
            108 => Some(Dest::OpSpread),
            109 => Some(Dest::OpFormant),
            111 => Some(Dest::OpLevelSens),
            112 => Some(Dest::OpAttack),
            113 => Some(Dest::OpDecay1),
            114 => Some(Dest::OpDecay2),
            115 => Some(Dest::OpRelease),
            110 => Some(Dest::OpDetune),
            117 => Some(Dest::OpPegDecay),
            53 => Some(Dest::MonoPoly),
            88 => Some(Dest::Pan),
            98 => Some(Dest::FegDepth),
            52 => Some(Dest::PortaTime),
            56 => Some(Dest::SlfoSpeed),
            57 => Some(Dest::SlfoDepth1),
            58 => Some(Dest::SlfoDepth2),
            59 => Some(Dest::SlfoDepth3),
            54 => Some(Dest::Pitch),
            _ => None,
        }
    }

    /// A full box's steps: the level 96 and the cutoff 1024 (measured on this part type: 0.75 and
    /// 8 a ratio unit at full travel of a unipolar box), the resonance 128 (measured on the sample
    /// part's resonance, the same raw scale; not measured on this part type); the voice LFO's
    /// speed and depths 100 of their 0-99 values (the extended speed 415 of its 0-415: measured at
    /// 25, 51 and 103 steps, all the time, nothing below 0), the pitch 8192 cents, an operator's frequency
    /// 3600 cents at sensitivity +7 (measured on this part type; the pitches up to a quarter of
    /// a full box); the voice LFO's filter depth 100 (measured on this part type: 12, 25, 31 and 37
    /// steps); the part's envelope offsets 64 (measured: 8, 16, 32 and 63 steps); the feedback 7
    /// (measured: every step from 0 and from 7); an operator's form 6 and skirt 7 (measured: 1, 2, 3, 4
    /// and the ends), resonance 96 (12 and 24), envelope times 96 (12 and 24 on the attack and both
    /// decays, 12 on the release), level 96 times its level controller sensitivity / 7 (20 and 48
    /// steps at +3 and +7, none at 0).
    pub fn span(self) -> i32 {
        match self {
            Dest::Level => 96,
            Dest::Cutoff => 1024,
            Dest::Resonance => 128,
            Dest::LfoSpeed | Dest::LfoPitch | Dest::LfoAmp | Dest::LfoFilter => 100,
            Dest::LfoExtSpeed => 415,
            Dest::Pitch => 8192,
            Dest::OpPitch => 3600,
            Dest::AmpAttack | Dest::AmpDecay | Dest::AmpRelease => 64,
            Dest::Feedback | Dest::OpSpread => 7,
            Dest::OpForm => 6,
            Dest::OpLevelSens => 96,
            Dest::OpFormant => 99,
            Dest::OpAttack | Dest::OpDecay1 | Dest::OpDecay2 | Dest::OpRelease | Dest::OpPegDecay => 96,
            Dest::OpDetune => 15,
            Dest::MonoPoly => 1,
            Dest::Pan | Dest::PortaTime | Dest::SlfoDepth1 | Dest::SlfoDepth2 | Dest::SlfoDepth3 => 128,
            Dest::FegDepth | Dest::SlfoSpeed => 64,
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
    #[inline(always)] // (read for every box at each controller event: kept inlined as before 0.6.3)
    fn read(s: &Sound, n: usize) -> Result<Box, Option<&'static str>> {
        let g = |k: usize| s.get(BOX_BASE.wrapping_add(BOX_FIELDS.wrapping_mul(n)).wrapping_add(k));
        let (src, dst, ratio) = (g(0), g(1), g(2));
        if dst == 0 || ratio == 0 {
            return Err(None);
        }
        let mut dest = Dest::of(dst).ok_or(Some("not covered (a destination without a law, or not the engine's)"))?;
        if dest == Dest::LfoSpeed && s.get(Common::LfoSpeedRange as usize) != 0 {
            dest = Dest::LfoExtSpeed;
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
    #[inline(always)] // (as `read`)
    pub fn steps(&self, c: f32) -> i32 {
        let o = if self.bipolar { 512.0 } else { 1024.0 };
        let out = ((curve_at(self.curve, c) + o - 1024.0) * self.ratio / 8.0).clamp(-8192.0, 8192.0);
        if self.dest == Dest::MonoPoly {
            // a switch: it turns past half an output either way (measured: 4096 does not, 4160 does)
            return i32::from(out > 4096.0).wrapping_sub(i32::from(out < -4096.0));
        }
        (out * self.dest.span() as f32 / 8192.0) as i32
    }

    /// Operator `op`'s share of its `k` steps: all of them, but on the level by the level controller
    /// sensitivity (111): a lowering k x sens / 7 toward zero (measured: at +3 and +7, none at 0),
    /// a raising none (measured: +24 steps at +7 and +3 left the level as it was).
    #[inline(always)]
    fn share(&self, s: &Sound, op: usize, k: i32) -> i32 {
        if self.dest != Dest::OpLevelSens {
            return k;
        }
        // below 0 nothing (measured: a lowering box at -7 and -3 left the level as it was)
        let sens = i32::from(s.get(op_index(op, Op::LevelCtrlSens, 0))).clamp(0, 7);
        k.min(0).saturating_mul(sens) / 7
    }

    /// Whether it reaches operator `op` (that operator's switch for it).
    pub fn reaches(&self, s: &Sound, op: usize) -> bool {
        self.per_op() && s.get(ctrl_set_index(op, self.n)) != 0
    }

    /// Whether it moves operators (those whose switch for it is on).
    fn per_op(&self) -> bool {
        matches!(
            self.dest,
            Dest::Level
                | Dest::OpPitch
                | Dest::OpForm
                | Dest::OpSpread
                | Dest::OpFormant
                | Dest::OpLevelSens
                | Dest::OpAttack
                | Dest::OpDecay1
                | Dest::OpDecay2
                | Dest::OpRelease
                | Dest::OpDetune
                | Dest::OpPegDecay
        )
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
    pub lfo_filter: i32,
    /// Cents: the sound's, and each operator's at sensitivity +7.
    pub pitch: i32,
    pub op_pitch: [i32; OPS],
    /// The part's amplitude envelope attack, decay and release offsets; the feedback; each operator's
    /// form, skirt and resonance, and its attack, decay 1, decay 2 and release times.
    pub amp: [i32; 3],
    pub feedback: i32,
    pub op_form: [[i32; 3]; OPS],
    pub op_time: [[i32; 4]; OPS],
    /// Each operator's detune and pitch envelope decay time; mono/poly (a switch: -1, 0, +1), pan
    /// positions, the filter envelope's depth, the portamento time, the sound LFO's speed and its
    /// three depths.
    pub op_detune: [i32; OPS],
    pub op_peg_decay: [i32; OPS],
    pub mono_poly: i32,
    pub pan: i32,
    pub feg_depth: i32,
    pub porta_time: i32,
    pub slfo_speed: i32,
    pub slfo_depth: [i32; 3],
}

pub fn steps(s: &Sound, src: &[f32; SOURCES]) -> Steps {
    let mut t = Steps::default();
    // (a loop, not `boxes`, so that each box's read stays inlined here: this runs at every event)
    for b in (0..BOXES).filter_map(|n| Box::read(s, n).ok()) {
        let k = b.steps(src.get(b.src).copied().unwrap_or(0.0));
        let add = |x: &mut i32| *x = x.saturating_add(k);
        match b.dest {
            Dest::Cutoff => add(&mut t.cutoff),
            Dest::Resonance => add(&mut t.resonance),
            Dest::LfoSpeed | Dest::LfoExtSpeed => add(&mut t.lfo_speed),
            Dest::LfoPitch => add(&mut t.lfo_pitch),
            Dest::LfoAmp => add(&mut t.lfo_amp),
            Dest::LfoFilter => add(&mut t.lfo_filter),
            Dest::Pitch => add(&mut t.pitch),
            Dest::AmpAttack => add(&mut t.amp[0]),
            Dest::AmpDecay => add(&mut t.amp[1]),
            Dest::AmpRelease => add(&mut t.amp[2]),
            Dest::Feedback => add(&mut t.feedback),
            Dest::MonoPoly => add(&mut t.mono_poly),
            Dest::Pan => add(&mut t.pan),
            Dest::FegDepth => add(&mut t.feg_depth),
            Dest::PortaTime => add(&mut t.porta_time),
            Dest::SlfoSpeed => add(&mut t.slfo_speed),
            Dest::SlfoDepth1 => add(&mut t.slfo_depth[0]),
            Dest::SlfoDepth2 => add(&mut t.slfo_depth[1]),
            Dest::SlfoDepth3 => add(&mut t.slfo_depth[2]),
            Dest::OpDetune | Dest::OpPegDecay => {
                let ops = if b.dest == Dest::OpDetune {
                    &mut t.op_detune
                } else {
                    &mut t.op_peg_decay
                };
                for (op, x) in ops.iter_mut().enumerate() {
                    if b.reaches(s, op) {
                        add(x);
                    }
                }
            }
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
            Dest::OpLevelSens => {
                for (op, l) in t.level.iter_mut().enumerate() {
                    if b.reaches(s, op) {
                        *l = l.saturating_add(b.share(s, op, k));
                    }
                }
            }
            _ => {
                let (i, j) = match b.dest {
                    Dest::OpForm => (0, 0),
                    Dest::OpSpread => (0, 1),
                    Dest::OpFormant => (0, 2),
                    Dest::OpAttack => (1, 0),
                    Dest::OpDecay1 => (1, 1),
                    Dest::OpDecay2 => (1, 2),
                    _ => (1, 3),
                };
                for op in (0..OPS).filter(|&op| b.reaches(s, op)) {
                    let x = if i == 0 {
                        t.op_form.get_mut(op).and_then(|f| f.get_mut(j))
                    } else {
                        t.op_time.get_mut(op).and_then(|f| f.get_mut(j))
                    };
                    if let Some(x) = x {
                        add(x);
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
    t.lfo_speed = clamp(
        t.lfo_speed,
        if s.get(Common::LfoSpeedRange as usize) != 0 {
            Dest::LfoExtSpeed
        } else {
            Dest::LfoSpeed
        },
    );
    t.lfo_pitch = clamp(t.lfo_pitch, Dest::LfoPitch);
    t.lfo_amp = clamp(t.lfo_amp, Dest::LfoAmp);
    t.lfo_filter = clamp(t.lfo_filter, Dest::LfoFilter);
    t.amp = t.amp.map(|x| clamp(x, Dest::AmpAttack));
    t.feedback = clamp(t.feedback, Dest::Feedback);
    t.op_detune = t.op_detune.map(|x| clamp(x, Dest::OpDetune));
    t.op_peg_decay = t.op_peg_decay.map(|x| clamp(x, Dest::OpPegDecay));
    t.mono_poly = clamp(t.mono_poly, Dest::MonoPoly);
    t.pan = clamp(t.pan, Dest::Pan);
    t.feg_depth = clamp(t.feg_depth, Dest::FegDepth);
    t.porta_time = clamp(t.porta_time, Dest::PortaTime);
    t.slfo_speed = clamp(t.slfo_speed, Dest::SlfoSpeed);
    t.slfo_depth = t.slfo_depth.map(|x| clamp(x, Dest::SlfoDepth1));
    t.op_form = t.op_form.map(|[f, s, n]| {
        [
            clamp(f, Dest::OpForm),
            clamp(s, Dest::OpSpread),
            clamp(n, Dest::OpFormant),
        ]
    });
    t.op_time = t.op_time.map(|x| x.map(|x| clamp(x, Dest::OpAttack)));
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
    for b in boxes(s).filter(|b| matches!(b.dest, Dest::Level | Dest::OpLevelSens)) {
        let ends = [0.0, 1024.0].map(|c| Box { curve: None, ..b }.steps(c));
        for (op, u) in up.iter_mut().enumerate() {
            if b.reaches(s, op) {
                let most = ends.iter().map(|&k| b.share(s, op, k)).max().unwrap_or(0).max(0);
                *u = (*u).saturating_add(most);
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
        // the extended range (measured on this part type from speed 40 and 100: the rate in Hz,
        // read back through the extended table): 415 steps a full box, within 0-415
        for (base, ratio, v, hz) in [
            (40, 4, 127, 5.4686),
            (40, 16, 64, 7.6546),
            (40, 16, 127, 13.2904),
            (100, -8, 64, 6.3100),
            (100, 16, 64, 14.6356),
        ] {
            let st = one(89, ratio, v, &format!("lfo_speed_range=1 lfo_extended_speed={base}")).lfo_speed;
            let got = laws::lfo_hz(true, (base + st).clamp(0, 415));
            assert!((got / hz - 1.0).abs() < 0.01, "{base} {ratio} {v}: {got} against {hz}");
        }
        let s = sound("box1_dst=89 box1_ratio=8 lfo_speed_range=1");
        assert_eq!(not_played(&s, 0), None);
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

    /// The envelope boxes and the voice LFO's filter depth, measured on the reference: the part's
    /// attack (95), decay (96) and release (97) offsets 64 steps a full box (8, 16, 32 and 63 read
    /// from the wheel at 64 and 127 with ratios 16 and 64); an operator's attack time (112) 96 (12
    /// and 24, the operators whose switch is on); the filter depth (92) 100 (12 and 25).
    #[test]
    fn envelope_and_filter_depth_boxes_measured() {
        assert_eq!(
            [one(95, 16, 64, "").amp, one(95, 16, 127, "").amp],
            [[8, 0, 0], [16, 0, 0]]
        );
        assert_eq!(
            [one(96, 64, 64, "").amp, one(97, 64, 127, "").amp],
            [[0, 32, 0], [0, 0, 64]]
        );
        assert_eq!(one(97, -64, 127, "").amp, [0, 0, -64]);
        let t = one(112, 16, 127, "op2.ctrl_set1=0");
        assert_eq!(
            (t.op_time[0][0], t.op_time[1][0], one(112, 16, 64, "").op_time[0][0]),
            (24, 0, 12)
        );
        assert_eq!([16, 20, 24].map(|r| one(92, r, 127, "").lfo_filter), [25, 31, 37]);
        assert_eq!(one(92, 16, 64, "").lfo_filter, 12);
    }

    /// The kernels' boxes, the operators' other envelope times and the level by the level controller
    /// sensitivity, measured on the reference: the feedback (104) 7 steps a full box (every reading
    /// from feedback 0 and 7 matched one stored value's spectrum exactly); the form (107) 6 (ratio 48
    /// at the wheel's top 4, not 5), the skirt (108) 7, the resonance (109) 96 (12 and 24); the decay
    /// 1, decay 2 (113, 114) and release (115) times 96; 111 lowers the level by 96 steps a full box
    /// at sensitivity +7, sens / 7 of that toward zero (20 at +3), nothing at 0 or below; it never
    /// raises it (a +16 box at the wheel's top and at 64, at +7 and +3: the level stayed at 40).
    #[test]
    fn kernel_time_and_level_sensitivity_boxes_measured() {
        let fb = |r: i16, v: u8| one(104, r, v, "").feedback;
        assert_eq!(
            [fb(8, 127), fb(16, 127), fb(32, 127), fb(64, 127), fb(127, 64)],
            [0, 1, 3, 7, 7]
        );
        assert_eq!(
            [fb(-32, 64), fb(-32, 127), fb(-127, 64), fb(-127, 127)],
            [-1, -3, -6, -7]
        );
        let f = |d: i16, r: i16, v: u8, i: usize| one(d, r, v, "").op_form[0][i];
        assert_eq!(
            [
                f(107, 48, 127, 0),
                f(107, 48, 64, 0),
                f(107, 16, 127, 0),
                f(107, 64, 127, 0)
            ],
            [4, 2, 1, 6]
        );
        assert_eq!([f(108, 16, 127, 1), f(108, 32, 127, 1), f(108, 64, 127, 1)], [1, 3, 7]);
        assert_eq!([f(109, 16, 64, 2), f(109, 16, 127, 2)], [12, 24]);
        for (d, i) in [(113, 1), (114, 2), (115, 3)] {
            assert_eq!(
                [one(d, 16, 64, "").op_time[0][i], one(d, 16, 127, "").op_time[0][i]],
                [12, 24]
            );
        }
        let l = |sens: i16| one(111, -32, 127, &format!("op1.level_ctrl_sens={sens}")).level[0];
        assert_eq!([l(-7), l(-3), l(0), l(3), l(7)], [0, 0, 0, -20, -48]);
        let up = |sens: i16, v: u8| one(111, 16, v, &format!("op1.level_ctrl_sens={sens}")).level[0];
        assert_eq!([up(7, 127), up(3, 127), up(7, 64)], [0, 0, 0]);
        let s = sound("box1_dst=111 box1_ratio=32 op1.level_ctrl_sens=7 op2.level_ctrl_sens=-7");
        assert_eq!(reach(&s)[..2], [0, 0], "no raise");
    }

    /// Round three's destinations, measured on the reference: detune (110) 15 steps a full box
    /// (every reading the frequency of one stored value, exactly); mono/poly (53) turning past half
    /// an output either way (the wheel's 64 does not at ratio -64, 65 does; -16 at the top does
    /// not); pan (88) 128 positions, the filter envelope's depth (98) 64, the portamento time (52)
    /// 128, the sound LFO's speed (56) 64 and depths (57-59) 128, an operator's pitch envelope decay
    /// time (117) 96; the resonance 99 (49 at half a box); the release time 48 at half a box.
    #[test]
    fn round_three_boxes_measured() {
        let t = |d: i16, r: i16, v: u8| one(d, r, v, "");
        let det = [
            (16, 127),
            (32, 127),
            (48, 127),
            (16, 64),
            (32, 64),
            (64, 64),
            (-16, 127),
            (-32, 127),
        ];
        assert_eq!(det.map(|(r, v)| t(110, r, v).op_detune[0]), [3, 7, 11, 1, 3, 7, -3, -7]);
        let mono = [(-64, 64), (-64, 65), (-16, 127), (64, 127), (64, 64)];
        assert_eq!(mono.map(|(r, v)| t(53, r, v).mono_poly), [0, -1, 0, 1, 0]);
        let pan = [(16, 64), (16, 127), (32, 127), (-32, 127)];
        assert_eq!(pan.map(|(r, v)| t(88, r, v).pan), [16, 32, 64, -64]);
        assert_eq!(pan.map(|(r, v)| t(98, r, v).feg_depth), [8, 16, 32, -32]);
        assert_eq!(
            [t(52, 16, 64), t(52, 16, 127), t(52, 64, 64)].map(|x| x.porta_time),
            [16, 32, 64]
        );
        assert_eq!(
            [t(56, 16, 64), t(56, 16, 127), t(56, 24, 127)].map(|x| x.slfo_speed),
            [8, 16, 24]
        );
        assert_eq!(
            [t(57, 16, 64), t(57, 16, 127), t(57, 64, 64)].map(|x| x.slfo_depth[0]),
            [16, 32, 64]
        );
        assert_eq!([t(58, 16, 127).slfo_depth[1], t(59, 16, 127).slfo_depth[2]], [32, 32]);
        assert_eq!([t(117, 16, 64), t(117, 16, 127)].map(|x| x.op_peg_decay[0]), [12, 24]);
        assert_eq!(
            [t(109, 16, 64), t(109, 16, 127), t(109, 64, 64)].map(|x| x.op_form[0][2]),
            [12, 24, 49]
        );
        assert_eq!(t(115, 32, 127).op_time[0][3], 48);
    }

    #[test]
    fn curves_and_what_is_not_played() {
        // the measured curve (0, 0, 8): 171 at the wheel's 64
        let s = sound("box1_dst=102 box1_ratio=8 box1_p1=8 box2_dst=55 box2_ratio=8 box3_dst=102 box3_ratio=8 box3_src=20 box4_dst=102 box4_ratio=8 box4_curve=2 box4_p1=1 box4_p2=1");
        let b = boxes(&s).next().unwrap();
        assert_eq!(b.steps(512.0), (171.0f32 * 8.0 / 8.0 * 1024.0 / 8192.0) as i32);
        assert_eq!(curve_at(b.curve, 1024.0), 1024.0);
        assert_eq!(not_played(&s, 0), None);
        assert!(not_played(&s, 1).unwrap().contains("destination"));
        assert!(not_played(&s, 2).unwrap().contains("source"));
        assert!(
            not_played(&s, 3).unwrap().contains("curve"),
            "type 2 (1, 1) is not measured"
        );
        assert_eq!(not_played(&s, 4), None);
    }

    /// The curves measured on this part type (a box on the sound's pitch at ratio +8, so the cents
    /// read the curve's value), at some of their 128 points: the standard family's members, a
    /// two-point curve, a falling one, a peak, a staircase; between the wheel's values straight;
    /// the knobs' values (0-1024) read the same table. The curves the sibling engines measured agree
    /// with theirs within their stated 5 of 1024.
    #[test]
    fn curves_measured() {
        let at = |kind: i16, p1: i16, p2: i16, v: u8| curve_at(curve(0, kind, p1, p2).flatten(), controller(v));
        assert_eq!([0, 32, 64, 96].map(|v| at(0, 8, 0, v)), [0.0, 64.0, 171.0, 390.0]);
        assert_eq!([8, 64, 120].map(|v| at(0, 1, 0, v)), [433.0, 939.0, 1019.0]);
        assert_eq!([56, 64, 72, 127].map(|v| at(2, 6, 1, v)), [0.0, 128.0, 384.0, 1024.0]);
        assert_eq!([8, 32, 127].map(|v| at(3, 1, 0, v)), [-367.0, -1024.0, -1024.0]);
        assert_eq!([0, 64, 120].map(|v| at(10, 0, 0, v)), [0.0, 1024.0, 103.0]);
        assert_eq!([16, 31, 32].map(|v| at(18, 4, 0, v)), [341.0, 341.0, 541.0]);
        assert_eq!(at(17, 0, 0, 100), 0.0);
        // measured later: a jagged one (repeating exactly), a window, a triangle, a falling one
        // whose top is 0 (the wheel held there), one flattening at about half, a two-point ramp
        assert_eq!(
            [8, 16, 24, 64].map(|v| at(16, 9, 0, v)),
            [-244.0, -920.0, -220.0, -318.0]
        );
        assert_eq!([10, 11, 21, 22].map(|v| at(11, 1, 0, v)), [0.0, 1024.0, 1024.0, 0.0]);
        assert_eq!(
            [32, 63, 64, 96].map(|v| at(14, 0, 0, v)),
            [-261.0, -1024.0, -1024.0, -261.0]
        );
        assert_eq!([64, 126, 127].map(|v| at(4, 0, 0, v)), [-424.0, -1017.0, 0.0]);
        assert_eq!([1, 64, 127].map(|v| at(15, 0, 0, v)), [242.0, 548.0, 552.0]);
        assert_eq!([0, 32, 48, 64].map(|v| at(2, 3, 1, v)), [0.0, 304.0, 812.0, 1024.0]);
        // halfway between the wheel's 64 and 65 (c 512 and 520)
        let i = curve(0, 0, 8, 0).flatten();
        assert_eq!(curve_at(i, 516.0), (curve_at(i, 512.0) + curve_at(i, 520.0)) / 2.0);
        // the sibling engines' tables, at the wheel's 0, 8, ..., 120 (their 127 is 1024 too)
        let theirs: [(i16, i16, i16, [f32; 16]); 3] = [
            (
                0,
                2,
                0,
                [
                    0., 255., 432., 545., 640., 713., 765., 816., 858., 889., 910., 942., 962., 984., 993., 1015.,
                ],
            ),
            (
                0,
                9,
                0,
                [
                    0., 0., 14., 24., 33., 43., 52., 71., 81., 109., 137., 175., 223., 298., 412., 630.,
                ],
            ),
            (
                2,
                10,
                4,
                [
                    0., 0., 0., 0., 0., 0., 0., 0., 33., 156., 280., 412., 526., 659., 791., 914.,
                ],
            ),
        ];
        for (kind, p1, p2, y) in theirs {
            for (k, &want) in y.iter().enumerate() {
                let got = at(kind, p1, p2, 8 * k as u8);
                assert!(
                    (got - want).abs() <= 7.0,
                    "({kind}, {p1}, {p2}) at {}: {got} against {want}",
                    8 * k
                );
            }
        }
        // not measured: type 5 (its held and swept readings disagree), any member of type 2 not
        // probed, other banks
        assert_eq!(curve(0, 5, 0, 0), None);
        assert_eq!(curve(0, 2, 1, 1), None);
        assert_eq!(curve(1, 0, 8, 0), None);
    }
}
