// SPDX-License-Identifier: AGPL-3.0-or-later
//! The voice engine (docs/design.md, section 2): up to eight prepared sounds, a pool of lanes of
//! four voices each, a lane holding one sound's voices only and rendered four-wide, or one-wide
//! while it holds one voice; notes on and off, the operator
//! envelopes at control rate, sine operators on the routings, the end rule and whole-note shedding.
//! Fixed size; nothing here allocates, and the block path (`note_on`, `note_off`, `pedal`,
//! `render`) has no panic path (tools/check-no-panic.sh).

use crate::forms::Wave;
use crate::laws::{self, DOUBLINGS_PER_DB as DPD};
use crate::params::{op_index, Common, Op, Part, OPS};
use crate::plan::{Feedback, Plan};
use crate::routing::routing;
use crate::simd::{first, scalar, F4, U4, W};
use crate::sound::Sound;
use crate::tables::{offset, read, read16, Table16, SINE};

/// Frames a control step.
pub const STEP: usize = 16;
/// Frames a block.
pub const BLOCK: usize = 128;
/// Sounds an instance plays.
pub const SOUNDS: usize = 8;
/// Voices an instance keeps sounding at most (the ceiling of all its sounds together).
pub const VOICES: usize = 128;
/// Lanes: the ceiling's, and a partly filled lane per sound, so a note under the ceiling always
/// finds a slot.
pub const LANES: usize = VOICES / W + SOUNDS;

/// Levels are log2 of gain less `HEAD` (so that a modulator's deviation, up to 3.2 cycles, stays
/// within the vector `exp2`'s range of [-30, 0]).
const HEAD: f32 = 4.0;
/// The lowest level: silence (about -180 dB below full scale).
const FLOOR: f32 = -30.0;
/// The end rule: a released voice ends once its carriers' gain bound is below -96 dBFS.
const END_GAIN: f32 = 1.584_893e-5;
/// A shed note fades out over this long.
const SHED_MS: f64 = 3.0;
/// The smallest rise a rising segment makes in a control step (log2), so it reaches its target.
const MIN_RISE: f32 = 4e-6;
/// Where each sound's random source starts.
const SEED: u32 = 0x2545_f491;

// envelope segments
const HOLD: u8 = 0;
const ATTACK: u8 = 1;
const DECAY1: u8 = 2;
const DECAY2: u8 = 3;
const RELEASE: u8 = 4;
const DONE: u8 = 5;

/// A note's end, reported exactly once for every note-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteEnd {
    pub sound: u8,
    pub key: u8,
    pub id: u32,
}

/// A note, pedal or bend event at a frame of a block (see [`Engine::render_events`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    /// The frame it takes effect at (0 to `BLOCK` - 1).
    pub frame: usize,
    pub sound: u8,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// [`Engine::note_on`]
    On { id: u32, key: u8, velocity: f32 },
    /// [`Engine::note_off`]
    Off { id: u32 },
    /// [`Engine::pedal`]
    Pedal { down: bool },
    /// [`Engine::bend`]
    Bend { value: i16 },
}

/// A sound's place in the voice budget's order (oscairn.voice-budget): voices it keeps from other
/// sounds' new notes, its priority (lower gives way first) and its own limit (0: none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub reserve: u8,
    pub priority: u8,
    pub limit: u8,
}

/// A sound as the engine plays it: the laws applied once, off the audio thread.
#[derive(Clone, Copy)]
pub struct Prepared {
    pub plan: Plan,
    /// Feedback: the phase offset in cycles per unit of the source's last two outputs (as stored).
    fbk: f32,
    /// Per operator, log2 of its gain at envelope level 99 and velocity sensitivity 0, less HEAD.
    full: [f32; OPS],
    level_velocity: [i8; OPS],
    /// The four envelope levels (attack, decay 1, decay 2, release and hold), log2 offsets.
    eg: [[f32; 4]; OPS],
    /// The four segments' ms per dB, and their places on the rate scale (for time key follow).
    ms: [[f32; 4]; OPS],
    rate_step: [[u8; 4]; OPS],
    time_key_follow: [u8; OPS],
    hold_ms: [f32; OPS],
    fixed: u8,
    reset: u8,
    /// Ratio-mode operators: the ratio's steps; fixed-mode: the frequency's.
    steps: [i32; OPS],
    detune: [i8; OPS],
    note_shift: i8,
    velocity: (u8, u8),
    /// Pitch a note keeps: key follow and its centre, random pitch depth, the part's pitch
    /// velocity and detune (cents), each fixed-mode operator's key follow and velocity.
    key_follow: (u8, u8),
    random_pitch: u8,
    pitch_velocity: i8,
    detune_cents: f32,
    fixed_key_follow: [u8; OPS],
    fixed_velocity: [i8; OPS],
    /// The common pitch envelope (ratio-mode operators): its five levels (octaves, at its depth),
    /// its four rates (octaves a millisecond; infinite when instant), its velocity sensitivity and
    /// time key follow, and whether any level is away from 0.
    peg: [f32; 5],
    peg_rate: [f32; 4],
    peg_velocity: u8,
    peg_key: u8,
    peg_on: bool,
    /// Each operator's pitch envelope: initial and attack levels (octaves), attack and decay rates
    /// (octaves a millisecond); the operators whose levels are away from 0.
    opeg: [[f32; 4]; OPS],
    opeg_on: u8,
    /// The pitch at full bend up and at full bend down, semitones.
    bend: (i8, i8),
    /// Portamento: on, its time, full-time (else fingered), its time mode.
    porta: bool,
    porta_time: u8,
    porta_full: bool,
    porta_mode: u8,
}

impl Prepared {
    pub fn new(s: &Sound) -> Prepared {
        let r = routing(s.get(Common::Algorithm as usize));
        let feedback = i32::from(s.get(Common::Feedback as usize));
        let o = |op: usize, k: Op, i: usize| i32::from(s.get(op_index(op, k, i)));
        let mut p = Prepared {
            plan: Plan::new(&r, 0, false, [Wave::Sine; OPS]),
            fbk: 0.0,
            full: [0.0; OPS],
            level_velocity: [0; OPS],
            eg: [[0.0; 4]; OPS],
            ms: [[0.0; 4]; OPS],
            rate_step: [[0; 4]; OPS],
            time_key_follow: [0; OPS],
            hold_ms: [0.0; OPS],
            fixed: 0,
            reset: 0,
            steps: [0; OPS],
            detune: [0; OPS],
            note_shift: s.get(Part::NoteShift as usize) as i8,
            velocity: (
                s.get(Part::VelocityDepth as usize) as u8,
                s.get(Part::VelocityOffset as usize) as u8,
            ),
            key_follow: (
                s.get(Common::PitchKeyFollow as usize) as u8,
                s.get(Common::PitchKeyCentre as usize) as u8,
            ),
            random_pitch: s.get(Common::RandomPitch as usize) as u8,
            pitch_velocity: s.get(Common::PitchVelocity as usize) as i8,
            detune_cents: laws::part_detune_cents(i32::from(s.get(Part::Detune as usize))) as f32,
            fixed_key_follow: core::array::from_fn(|op| o(op, Op::FixedKeyFollow, 0) as u8),
            fixed_velocity: core::array::from_fn(|op| o(op, Op::PitchVelocity, 0) as i8),
            peg: core::array::from_fn(|i| {
                let c = |k: Common, i| i32::from(s.get(k as usize + i));
                (laws::part_peg_cents(c(Common::PegLevel, i), c(Common::PegDepth, 0)) / 1200.0) as f32
            }),
            peg_rate: core::array::from_fn(|i| {
                let r = laws::part_peg_cents_per_ms(i32::from(s.get(Common::PegTime as usize + i))) / 1200.0;
                if r > 0.0 {
                    r as f32
                } else {
                    f32::INFINITY
                }
            }),
            peg_velocity: s.get(Common::PegVelocity as usize) as u8,
            peg_key: s.get(Common::PegTimeKeyFollow as usize) as u8,
            peg_on: (0..5).any(|i| s.get(Common::PegLevel as usize + i) != 0),
            opeg: core::array::from_fn(|op| {
                let c = |i| (laws::peg_cents(o(op, Op::PegLevel, i)) / 1200.0) as f32;
                let r = |i| (laws::peg_cents_per_ms(o(op, Op::PegTime, i)) / 1200.0) as f32;
                [c(0), c(1), r(0), r(1)]
            }),
            opeg_on: (0..OPS)
                .filter(|&op| o(op, Op::PegLevel, 0) != 0 || o(op, Op::PegLevel, 1) != 0)
                .fold(0, |m, op| m | bit(op)),
            bend: (s.get(Part::BendUp as usize) as i8, s.get(Part::BendDown as usize) as i8),
            porta: s.get(Part::Portamento as usize) != 0,
            porta_time: s.get(Part::PortamentoTime as usize) as u8,
            porta_full: s.get(Part::PortamentoMode as usize) != 0,
            porta_mode: s.get(Part::PortamentoTimeMode as usize) as u8,
        };
        let carrier_gain_db =
            laws::CARRIER_FULL_DBFS + laws::routing_gain_db(i32::from(s.get(Common::Algorithm as usize)));
        let mut audible = 0u8;
        for op in 0..OPS {
            let bit = 1u8 << op;
            let carrier = r.carriers & bit != 0;
            let level = o(op, Op::Level, 0);
            // a modulator's full deviation is 2 cycles (4 pi radians) at 0 dB
            let (db, base) = if carrier {
                (laws::carrier_level_db(level), carrier_gain_db * DPD)
            } else {
                (laws::modulator_level_db(level), 1.0)
            };
            let eg = [0, 1, 2, 3].map(|i| {
                let l = o(op, Op::EgLevel, i);
                (if carrier {
                    laws::carrier_eg_db(l)
                } else {
                    laws::modulator_eg_db(l)
                }) * DPD
            });
            if db > laws::SILENT_DB && eg.iter().any(|&e| e > laws::SILENT_DB * DPD) {
                audible |= bit;
            }
            let times = [0, 1, 2, 3].map(|i| o(op, Op::EgTime, i));
            if let (Some(f), Some(e), Some(ms), Some(rs), Some(v)) = (
                p.full.get_mut(op),
                p.eg.get_mut(op),
                p.ms.get_mut(op),
                p.rate_step.get_mut(op),
                p.level_velocity.get_mut(op),
            ) {
                *f = (base + db * DPD) as f32 - HEAD;
                *e = eg.map(|x| x as f32);
                *ms = [
                    laws::attack_ms_per_db(times[0]),
                    laws::eg_ms_per_db(times[1]),
                    laws::eg_ms_per_db(times[2]),
                    laws::eg_ms_per_db(times[3]),
                ]
                .map(|x| x as f32);
                *rs = times.map(|t| laws::rate_step(t) as u8);
                *v = o(op, Op::LevelVelocity, 0) as i8;
            }
            let fixed = o(op, Op::Mode, 0) != 0;
            let (coarse, fine) = (o(op, Op::Coarse, 0), o(op, Op::Fine, 0));
            if let (Some(t), Some(h), Some(st), Some(d)) = (
                p.time_key_follow.get_mut(op),
                p.hold_ms.get_mut(op),
                p.steps.get_mut(op),
                p.detune.get_mut(op),
            ) {
                *t = o(op, Op::EgTimeKeyFollow, 0) as u8;
                *h = laws::hold_ms(o(op, Op::EgHold, 0)) as f32;
                *st = if fixed {
                    laws::fixed_steps(coarse, fine)
                } else {
                    laws::ratio_steps(laws::ratio(coarse, fine))
                };
                *d = o(op, Op::Detune, 0) as i8;
            }
            p.fixed |= if fixed { bit } else { 0 };
            p.reset |= if o(op, Op::KeyOnReset, 0) != 0 { bit } else { 0 };
        }
        let waves =
            core::array::from_fn(|op| Wave::new(o(op, Op::Form, 0), o(op, Op::Spread, 0), o(op, Op::Formant, 0)));
        p.plan = Plan::new(&r, audible, feedback > 0, waves);
        // a carrier feeds back its measured share of a modulator's amount, of its signal before the
        // routing's output level
        let fb_carrier = has(u32::from(r.carriers), usize::from(r.fb_from));
        let fbk = laws::feedback_amount(feedback)
            * if fb_carrier {
                laws::carrier_feedback_scale(feedback) * 2.0 / laws::db_to_gain(carrier_gain_db)
            } else {
                1.0
            };
        p.fbk = fbk as f32;
        p
    }
}

/// Four voices of one sound, plain arrays (structure of arrays), the same on every back end.
#[derive(Clone, Copy)]
struct Lane {
    ph: [[u32; W]; OPS],
    inc: [[u32; W]; OPS],
    /// Each operator's gain at the end of the last control step.
    gain: [[f32; W]; OPS],
    /// The feedback source's last two outputs.
    fb: [[f32; W]; 2],
    // the envelopes, log2 less HEAD: level, the segment's target, its fall a step, its rise's
    // share a step, and the point a rise approaches
    lvl: [[f32; W]; OPS],
    tgt: [[f32; W]; OPS],
    fall: [[f32; W]; OPS],
    rise: [[f32; W]; OPS],
    top: [[f32; W]; OPS],
    /// Each voice's gain on everything: 1, falling to 0 while it is shed; 0 in a free slot.
    fade: [f32; W],
    /// Read only when a segment changes: the segment, frames left of the hold.
    seg: [[u8; W]; OPS],
    count: [[u32; W]; OPS],
    /// Envelopes that can change segment, bit 4 op + slot.
    watch: u32,
    sound: u8,
    /// Slots in use; keys down; released keys the pedal holds; voices being shed.
    used: u8,
    held: u8,
    sustained: u8,
    ending: u8,
    key: [u8; W],
    id: [u32; W],
    age: [u32; W],
    /// The carriers' gain bound at the last step: the end rule's measure, and loudness for shedding.
    bound: [f32; W],
    pt: Pitch,
}

/// What a lane's increments are made of while a voice's pitch moves (docs/design.md, 2.3), read
/// only when it does: `inc` is `base` times 2 to the octaves that move it.
#[derive(Clone, Copy)]
struct Pitch {
    /// Each operator's increment from the pitch the note keeps.
    base: [[u32; W]; OPS],
    /// The operators' pitch envelopes: level (octaves); those in their attack and in their decay,
    /// bit 4 op + slot.
    op: [[f32; W]; OPS],
    op_attack: u32,
    op_decay: u32,
    /// The common pitch envelope: level (octaves), segment (`ATTACK` to `RELEASE`, `HOLD` once at
    /// decay 2's level, `DONE`), the note's factors on its levels and on its rates.
    c: [f32; W],
    c_seg: [u8; W],
    c_scale: [f32; W],
    c_rate: [f32; W],
    /// Portamento: octaves still to glide (from where the glide started, less the note's own
    /// pitch), and the step a tick of the glide clock moves it.
    glide: [f32; W],
    glide_step: [f32; W],
    /// Voices whose pitch moves; the bend (octaves) the increments hold; whether the increments
    /// are to be made again at the next step's start (something jumped).
    moving: u8,
    bend: f32,
    jump: bool,
}

const LANE0: Lane = Lane {
    ph: [[0; W]; OPS],
    inc: [[0; W]; OPS],
    gain: [[0.0; W]; OPS],
    fb: [[0.0; W]; 2],
    lvl: [[FLOOR; W]; OPS],
    tgt: [[FLOOR; W]; OPS],
    fall: [[0.0; W]; OPS],
    rise: [[0.0; W]; OPS],
    top: [[0.0; W]; OPS],
    fade: [0.0; W],
    seg: [[DONE; W]; OPS],
    count: [[0; W]; OPS],
    watch: 0,
    sound: 0,
    used: 0,
    held: 0,
    sustained: 0,
    ending: 0,
    key: [0; W],
    id: [0; W],
    age: [0; W],
    bound: [0.0; W],
    pt: Pitch {
        base: [[0; W]; OPS],
        op: [[0.0; W]; OPS],
        op_attack: 0,
        op_decay: 0,
        c: [0.0; W],
        c_seg: [DONE; W],
        c_scale: [0.0; W],
        c_rate: [0.0; W],
        glide: [0.0; W],
        glide_step: [0.0; W],
        moving: 0,
        bend: 0.0,
        jump: false,
    },
};

/// Bit `i` of `m` (false past the end; no shift can overflow).
#[inline(always)]
fn has(m: u32, i: usize) -> bool {
    u32::try_from(i)
        .ok()
        .and_then(|i| m.checked_shr(i))
        .is_some_and(|m| m & 1 != 0)
}

#[inline(always)]
fn bit(i: usize) -> u8 {
    u32::try_from(i).ok().and_then(|i| 1u8.checked_shl(i)).unwrap_or(0)
}

#[inline(always)]
fn at<const N: usize>(a: &[f32; N], i: usize) -> f32 {
    a.get(i).copied().unwrap_or(0.0)
}

/// The next draw of a random source (xorshift32).
fn next_random(r: Option<&mut u32>) -> u32 {
    r.map_or(0, |r| {
        *r ^= *r << 13;
        *r ^= *r >> 17;
        *r ^= *r << 5;
        *r
    })
}

/// Slot `s`'s bits in a mask of bit 4 op + slot.
#[inline(always)]
fn slot_ops(s: usize) -> u32 {
    0x1111_1111u32.checked_shl(s as u32).unwrap_or(0)
}

/// `a[op][s] = x` when both are in range.
#[inline(always)]
fn put<T: Copy>(a: &mut [[T; W]; OPS], op: usize, s: usize, x: T) {
    if let Some(e) = a.get_mut(op).and_then(|r| r.get_mut(s)) {
        *e = x;
    }
}

#[inline(always)]
fn get<T: Copy + Default>(a: &[[T; W]; OPS], op: usize, s: usize) -> T {
    a.get(op).and_then(|r| r.get(s)).copied().unwrap_or_default()
}

impl Lane {
    /// Enters segment `seg` of operator `op`'s envelope in slot `s` (at `rate` Hz).
    fn enter(&mut self, p: &Prepared, op: usize, s: usize, seg: u8, rate: f64) {
        let top = get(&self.top, op, s);
        let full = top - (laws::ATTACK_OVER_DB * DPD) as f32;
        let k = usize::from(seg.saturating_sub(1)).min(3);
        let (mut tgt, mut fall) = (get(&self.lvl, op, s), 0.0);
        if (ATTACK..=RELEASE).contains(&seg) {
            tgt = (full + p.eg.get(op).and_then(|e| e.get(k)).copied().unwrap_or(FLOOR)).max(FLOOR);
            // the segment's fall a step, its time moved by time key follow at the note's key
            let key = i32::from(self.key.get(s).copied().unwrap_or(0)) + i32::from(p.note_shift);
            let ks = laws::time_key_follow_steps(i32::from(p.time_key_follow.get(op).copied().unwrap_or(0)), key);
            let ms = f64::from(p.ms.get(op).and_then(|m| m.get(k)).copied().unwrap_or(1.0));
            let from = i32::from(p.rate_step.get(op).and_then(|r| r.get(k)).copied().unwrap_or(0));
            let scale = laws::rate(from) / laws::rate(from.saturating_add(ks).clamp(0, 127));
            fall = (STEP as f64 * 1000.0 / (ms * scale * rate) * DPD) as f32;
        }
        if seg == ATTACK {
            let start = get(&self.lvl, op, s).max(full - (laws::ATTACK_START_DB * DPD) as f32);
            put(&mut self.lvl, op, s, start);
        }
        // a rise approaches `top` exponentially: the deficit shrinks by e^-rate a step
        let rise = 1.0 - laws::exp2(-f64::from(fall) * core::f64::consts::LOG2_E) as f32;
        put(&mut self.tgt, op, s, tgt);
        put(&mut self.fall, op, s, fall);
        put(&mut self.rise, op, s, rise);
        put(&mut self.seg, op, s, seg);
        let w = 1u32
            .checked_shl((op.wrapping_mul(4).wrapping_add(s)) as u32)
            .unwrap_or(0);
        // decay 2 holds while the key is down; nothing follows the end
        if matches!(seg, DECAY2 | DONE) {
            self.watch &= !w;
        } else {
            self.watch |= w;
        }
    }

    /// Frees slot `s`: silent from the next step.
    fn free(&mut self, s: usize) {
        let b = bit(s);
        (self.used, self.held, self.sustained, self.ending) =
            (self.used & !b, self.held & !b, self.sustained & !b, self.ending & !b);
        if let Some(f) = self.fade.get_mut(s) {
            *f = 0.0;
        }
        for op in 0..OPS {
            put(&mut self.gain, op, s, 0.0);
        }
        let ops = !slot_ops(s);
        (self.watch, self.pt.op_attack, self.pt.op_decay) =
            (self.watch & ops, self.pt.op_attack & ops, self.pt.op_decay & ops);
        self.pt.moving &= !b;
        if let (Some(c), Some(g)) = (self.pt.c_seg.get_mut(s), self.pt.glide.get_mut(s)) {
            (*c, *g) = (DONE, 0.0);
        }
    }

    /// Moves the voices in the highest slots into free lower ones, so a lane's voices fill its
    /// first slots and it renders at the width of its voice count. A voice's state moves whole, so
    /// its output does not change.
    #[inline(always)]
    fn compact(&mut self) {
        while self.used & self.used.wrapping_add(1) != 0 {
            // the lowest free slot is below the highest used one
            let (to, from) = (
                (!self.used).trailing_zeros(),
                7u32.wrapping_sub(self.used.leading_zeros()),
            );
            self.mv(from as usize, to as usize);
        }
    }

    /// Moves the voice in slot `from` to the free slot `to`, below it.
    #[inline(never)]
    fn mv(&mut self, from: usize, to: usize) {
        fn col<T: Copy, const K: usize>(a: &mut [[T; W]; K], from: usize, to: usize) {
            a.iter_mut().for_each(|r| one(r, from, to));
        }
        fn one<T: Copy>(a: &mut [T; W], from: usize, to: usize) {
            if let (Some(x), Some(y)) = (a.get(from).copied(), a.get_mut(to)) {
                *y = x;
            }
        }
        col(&mut self.ph, from, to);
        col(&mut self.inc, from, to);
        col(&mut self.gain, from, to);
        col(&mut self.fb, from, to);
        col(&mut self.lvl, from, to);
        col(&mut self.tgt, from, to);
        col(&mut self.fall, from, to);
        col(&mut self.rise, from, to);
        col(&mut self.top, from, to);
        col(&mut self.seg, from, to);
        col(&mut self.count, from, to);
        one(&mut self.fade, from, to);
        one(&mut self.key, from, to);
        one(&mut self.id, from, to);
        one(&mut self.age, from, to);
        one(&mut self.bound, from, to);
        let pt = &mut self.pt;
        col(&mut pt.base, from, to);
        col(&mut pt.op, from, to);
        for x in [
            &mut pt.c,
            &mut pt.c_scale,
            &mut pt.c_rate,
            &mut pt.glide,
            &mut pt.glide_step,
        ] {
            one(x, from, to);
        }
        one(&mut pt.c_seg, from, to);
        let (f, t) = (bit(from), bit(to));
        for m in [
            &mut self.used,
            &mut self.held,
            &mut self.sustained,
            &mut self.ending,
            &mut pt.moving,
        ] {
            *m = if *m & f != 0 { *m | t } else { *m & !t };
        }
        for m in [&mut self.watch, &mut pt.op_attack, &mut pt.op_decay] {
            let w = (*m & slot_ops(from)).checked_shr(from.wrapping_sub(to) as u32);
            *m = (*m & !slot_ops(to)) | w.unwrap_or(0);
        }
        self.free(from);
    }

    fn release(&mut self, p: &Prepared, s: usize, rate: f64) {
        self.held &= !bit(s);
        self.sustained &= !bit(s);
        for op in (0..OPS).filter(|&op| has(u32::from(p.plan.live), op)) {
            self.enter(p, op, s, RELEASE, rate);
        }
        if p.peg_on {
            if let Some(c) = self.pt.c_seg.get_mut(s) {
                *c = RELEASE;
            }
            self.pt.moving |= bit(s);
        }
    }

    /// Slot `s`'s pitch for `key` (note shift applied) and velocity `v`, `r` its random draw:
    /// each live operator's increment from the pitch the note keeps (key follow, velocity, random
    /// pitch and detune, in cents, on the grid), its pitch envelope from the start, with `common`
    /// the common pitch envelope too, and `glide` (octaves, the step a tick) to glide from.
    #[allow(clippy::too_many_arguments)]
    fn tune(&mut self, p: &Prepared, s: usize, key: i32, v: f64, r: u32, rate: f64, common: bool, glide: (f32, f32)) {
        // cents: every operator's, then ratio-mode ones'
        let all = laws::pitch_velocity_cents(i32::from(p.pitch_velocity), v);
        let ratio_cents = all
            + laws::pitch_key_follow_cents(i32::from(p.key_follow.0), key, i32::from(p.key_follow.1))
            + laws::random_pitch_cents(i32::from(p.random_pitch), r)
            + f64::from(p.detune_cents);
        let pt = &mut self.pt;
        let ops = slot_ops(s);
        (pt.op_attack, pt.op_decay) = (pt.op_attack & !ops, pt.op_decay & !ops);
        for op in (0..OPS).filter(|&op| has(u32::from(p.plan.live), op)) {
            let at = |a: &[i8; OPS]| i32::from(a.get(op).copied().unwrap_or(0));
            let fixed = has(u32::from(p.fixed), op);
            let cents = if fixed {
                let kf = i32::from(p.fixed_key_follow.get(op).copied().unwrap_or(0));
                all + laws::fixed_key_follow_cents(kf, key) + laws::fixed_pitch_velocity_cents(at(&p.fixed_velocity), v)
            } else {
                ratio_cents
            };
            // the grid's steps, then the frequency (each term far inside the i32 range)
            let steps = p
                .steps
                .get(op)
                .copied()
                .unwrap_or(0)
                .wrapping_add(laws::detune_steps(at(&p.detune), key))
                .wrapping_add(if fixed { 0 } else { laws::key_steps(key) })
                .wrapping_add(laws::cents_steps(cents));
            let hz = laws::steps_hz(steps);
            let hz = if fixed { hz.min(laws::FIXED_MAX_HZ) } else { hz };
            let inc = (hz / rate * 4_294_967_296.0) as u32;
            put(&mut pt.base, op, s, inc);
            put(&mut self.inc, op, s, inc);
            let e = p.opeg.get(op).copied().unwrap_or([0.0; 4]);
            put(
                &mut pt.op,
                op,
                s,
                if has(u32::from(p.opeg_on), op) { e[0] } else { 0.0 },
            );
            if has(u32::from(p.opeg_on), op) {
                pt.op_attack |= 1u32.checked_shl(op.wrapping_mul(4).wrapping_add(s) as u32).unwrap_or(0);
            }
        }
        if common {
            let vs = laws::part_peg_velocity_scale(i32::from(p.peg_velocity), v);
            let ks = laws::part_peg_key_scale(i32::from(p.peg_key), key);
            if let (Some(c), Some(seg), Some(sc), Some(rt)) = (
                pt.c.get_mut(s),
                pt.c_seg.get_mut(s),
                pt.c_scale.get_mut(s),
                pt.c_rate.get_mut(s),
            ) {
                *sc = vs as f32;
                *rt = (vs * ks) as f32;
                (*c, *seg) = if p.peg_on {
                    (p.peg[0] * *sc, ATTACK)
                } else {
                    (0.0, HOLD)
                };
            }
        }
        if let (Some(g), Some(st)) = (pt.glide.get_mut(s), pt.glide_step.get_mut(s)) {
            (*g, *st) = glide;
        }
        let moves =
            pt.op_attack & ops != 0 || glide.0 != 0.0 || pt.c_seg.get(s).is_some_and(|&c| c != HOLD && c != DONE);
        if moves {
            pt.moving |= bit(s);
        } else {
            pt.moving &= !bit(s);
        }
        // the increments hold what moves from the next step on
        pt.jump |= moves || pt.bend != 0.0;
    }

    fn end(&mut self, s: usize, end: &mut impl FnMut(NoteEnd)) {
        if has(u32::from(self.used), s) {
            let (key, id) = (
                self.key.get(s).copied().unwrap_or(0),
                self.id.get(s).copied().unwrap_or(0),
            );
            end(NoteEnd {
                sound: self.sound,
                key,
                id,
            });
        }
        self.free(s);
    }
}

/// A voice's place in the shedding order: held, another sound's, priority, loudness, age.
type Rank = (u8, u8, u8, f32, u32);

/// The engine: every object fixed size, owned by the caller.
pub struct Engine {
    sounds: [Prepared; SOUNDS],
    limits: [Limits; SOUNDS],
    lanes: [Lane; LANES],
    /// Voices the instance may keep (at most `VOICES`).
    limit: usize,
    /// Sounds whose sustain pedal is down, a bit each.
    pedal: u8,
    age: u32,
    /// Each sound's random source (xorshift32), started again when the sound is installed.
    rng: [u32; SOUNDS],
    /// Each sound's pitch bend, -8192..+8191.
    bend: [i16; SOUNDS],
    /// Each sound's latest note, where portamento glides from.
    last: [Option<u32>; SOUNDS],
    /// Frames rendered (at the block's start), and the glide clock's period (frames, 2^-16 a
    /// unit): one clock for every sound.
    clock: u64,
    tick: u64,
    rate: f64,
    shed_step: f32,
    /// The operators' outputs in a step, at each width (each is written before it is read, so
    /// they are never cleared).
    y1: [[[f32; 1]; STEP]; OPS],
    y4: [[[f32; W]; STEP]; OPS],
}

impl Engine {
    /// An engine at `rate` Hz, every sound the default sound.
    pub fn new(rate: f64) -> Engine {
        let p = Prepared::new(&Sound::default());
        let steps = (SHED_MS * rate / 1000.0 / STEP as f64 + 0.5).max(1.0);
        Engine {
            sounds: [p; SOUNDS],
            limits: [Limits::default(); SOUNDS],
            lanes: [LANE0; LANES],
            limit: VOICES,
            pedal: 0,
            age: 0,
            rng: [SEED; SOUNDS],
            bend: [0; SOUNDS],
            last: [None; SOUNDS],
            clock: 0,
            tick: ((laws::GLIDE_TICK_MS * rate / 1000.0 * 65536.0 + 0.5) as u64).max(1),
            rate,
            shed_step: (1.0 / steps) as f32,
            y1: [[[0.0; 1]; STEP]; OPS],
            y4: [[[0.0; W]; STEP]; OPS],
        }
    }

    /// Installs a prepared sound in slot `sound`, which must have no voice sounding (see
    /// [`Engine::choke`]); whether it was installed.
    pub fn set_sound(&mut self, sound: usize, p: &Prepared) -> bool {
        let busy = self.lanes.iter().any(|l| l.used != 0 && usize::from(l.sound) == sound);
        match (self.sounds.get_mut(sound), self.rng.get_mut(sound)) {
            (Some(d), Some(r)) if !busy => {
                (*d, *r) = (*p, SEED);
                true
            }
            _ => false,
        }
    }

    /// Fades out every voice of `sound` (each ends after its fade).
    pub fn choke(&mut self, sound: usize) {
        for l in self.lanes.iter_mut().filter(|l| usize::from(l.sound) == sound) {
            l.ending |= l.used;
        }
    }

    /// The instance's voice limit (0 or above `VOICES`: `VOICES`) and each sound's place in the
    /// order; voices past them are shed at the next block's start.
    pub fn set_limits(&mut self, limit: usize, limits: &[Limits; SOUNDS]) {
        self.limit = if limit == 0 { VOICES } else { limit.min(VOICES) };
        self.limits = *limits;
    }

    /// Voices sounding (fading ones included).
    pub fn active(&self) -> usize {
        self.lanes
            .iter()
            .fold(0usize, |n, l| n.wrapping_add(l.used.count_ones() as usize))
    }

    fn keeping(&self, sound: Option<u8>) -> usize {
        self.lanes
            .iter()
            .filter(|l| sound.is_none_or(|s| s == l.sound))
            .fold(0usize, |n, l| {
                n.wrapping_add((l.used & !l.ending).count_ones() as usize)
            })
    }

    /// Portamento for a note of `sound` at `key` (note shift applied): from the pitch the sound's
    /// latest note has now (if it still sounds), when the switch is on and the note is legato or
    /// the mode full-time; octaves to glide (at most one with time modes rate 2 and time 2) and
    /// the step a tick of the glide clock moves them, straight in semitones; none at time 0.
    fn glide(&self, sound: usize, p: &Prepared, key: i32, legato: bool) -> (f32, f32) {
        let last = self
            .last
            .get(sound)
            .copied()
            .flatten()
            .filter(|_| p.porta && (legato || p.porta_full));
        let from = last.and_then(|id| {
            self.lanes
                .iter()
                .filter(|l| usize::from(l.sound) == sound)
                .find_map(|l| {
                    let s = (0..W).find(|&s| has(u32::from(l.used), s) && l.id.get(s) == Some(&id))?;
                    let k = i32::from(l.key.get(s).copied().unwrap_or(0)) + i32::from(p.note_shift);
                    Some(k as f32 / 12.0 + l.pt.glide.get(s).copied().unwrap_or(0.0))
                })
        });
        let Some(from) = from else { return (0.0, 0.0) };
        let g = from - key as f32 / 12.0;
        let g = if p.porta_mode >= 2 { g.clamp(-1.0, 1.0) } else { g };
        let rate = laws::portamento_rate(
            i32::from(p.porta_time),
            i32::from(p.porta_mode),
            f64::from(g.abs()) * 12.0,
        );
        if rate > 0.0 && g != 0.0 {
            (g, (rate / 12.0 * laws::GLIDE_TICK_MS / 1000.0) as f32)
        } else {
            (0.0, 0.0)
        }
    }

    /// Starts a note: `key` 0-127, `velocity` 1-127 (fractions allowed). A note that gets no voice
    /// ends at once; past the limits the voice-budget order sheds another note (with a fade).
    pub fn note_on(&mut self, sound: usize, id: u32, key: u8, velocity: f32, end: &mut impl FnMut(NoteEnd)) {
        let s8 = sound as u8;
        let refuse = |end: &mut dyn FnMut(NoteEnd)| end(NoteEnd { sound: s8, key, id });
        let Some(p) = self.sounds.get(sound).copied() else {
            return refuse(end);
        };
        let (depth, off) = p.velocity;
        let v = laws::part_velocity(i32::from(depth), i32::from(off), f64::from(velocity));
        if v <= 0.0 || velocity.is_nan() {
            return refuse(end);
        }
        // a legato note: another key of the sound is down
        let legato = self.lanes.iter().any(|l| l.sound == s8 && l.held & !l.ending != 0);
        let glide = self.glide(sound, &p, i32::from(key) + i32::from(p.note_shift), legato);
        let own = usize::from(self.limits.get(sound).map_or(0, |l| l.limit));
        if self.keeping(None) >= self.limit || (own > 0 && self.keeping(Some(s8)) >= own) {
            let only = (own > 0 && self.keeping(Some(s8)) >= own).then_some(s8);
            if !self.shed(Some(s8), only) {
                return refuse(end);
            }
        }
        // a slot in a lane of this sound, else an empty lane
        let found = self
            .lanes
            .iter()
            .position(|l| l.used != 0 && l.used != 0xf && l.sound == s8)
            .or_else(|| self.lanes.iter().position(|l| l.used == 0));
        let Some(lane) = found.and_then(|i| self.lanes.get_mut(i)) else {
            return refuse(end);
        };
        if lane.used == 0 {
            lane.sound = s8;
        }
        let s = (!lane.used).trailing_zeros() as usize;
        self.age = self.age.wrapping_add(1);
        let b = bit(s);
        lane.used |= b;
        lane.held |= b;
        lane.sustained &= !b;
        lane.ending &= !b;
        if let (Some(k), Some(i), Some(a), Some(f), Some(bd)) = (
            lane.key.get_mut(s),
            lane.id.get_mut(s),
            lane.age.get_mut(s),
            lane.fade.get_mut(s),
            lane.bound.get_mut(s),
        ) {
            (*k, *i, *a, *f, *bd) = (key, id, self.age, 1.0, 1.0);
        }
        for h in lane.fb.iter_mut() {
            if let Some(x) = h.get_mut(s) {
                *x = 0.0;
            }
        }
        let rate = self.rate;
        let key = i32::from(key) + i32::from(p.note_shift);
        let r = next_random(self.rng.get_mut(sound));
        if lane.used == b {
            lane.pt.bend = 0.0; // (a lane's first voice: its increments hold no bend)
        }
        lane.tune(&p, s, key, v, r, rate, true, glide);
        if let Some(l) = self.last.get_mut(sound) {
            *l = Some(id);
        }
        for op in 0..OPS {
            put(&mut lane.gain, op, s, 0.0);
            if !has(u32::from(p.plan.live), op) {
                put(&mut lane.seg, op, s, DONE);
                continue;
            }
            let at = |a: &[i8; OPS]| i32::from(a.get(op).copied().unwrap_or(0));
            if has(u32::from(p.reset), op) {
                put(&mut lane.ph, op, s, 0);
            }
            // level for this velocity
            let full =
                p.full.get(op).copied().unwrap_or(FLOOR) + (laws::velocity_db(at(&p.level_velocity), v) * DPD) as f32;
            put(&mut lane.top, op, s, full + (laws::ATTACK_OVER_DB * DPD) as f32);
            let start = (full + p.eg.get(op).map_or(FLOOR, |e| e[3])).max(FLOOR);
            put(&mut lane.lvl, op, s, start);
            let hold = f64::from(p.hold_ms.get(op).copied().unwrap_or(0.0)) * rate / 1000.0 / STEP as f64 + 0.5;
            put(&mut lane.count, op, s, (hold as u32).saturating_mul(STEP as u32));
            lane.enter(&p, op, s, if hold as u32 > 0 { HOLD } else { ATTACK }, rate);
        }
    }

    /// Releases the note `id` of `sound` (or holds it with the pedal down).
    pub fn note_off(&mut self, sound: usize, id: u32) {
        let s8 = sound as u8;
        let pedal = has(u32::from(self.pedal), sound);
        let Some(p) = self.sounds.get(sound).copied() else {
            return;
        };
        for lane in self.lanes.iter_mut().filter(|l| l.sound == s8 && l.held != 0) {
            for s in 0..W {
                if has(u32::from(lane.held), s) && lane.id.get(s) == Some(&id) {
                    if pedal {
                        lane.held &= !bit(s);
                        lane.sustained |= bit(s);
                    } else {
                        lane.release(&p, s, self.rate);
                    }
                }
            }
        }
    }

    /// The pitch bend of `sound`, -8192..+8191 (a 14-bit controller about its centre), played by
    /// the sound's ranges on every operator from the next control step (an event's frame).
    pub fn bend(&mut self, sound: usize, value: i16) {
        if let Some(b) = self.bend.get_mut(sound) {
            *b = value.clamp(-8192, 8191);
        }
    }

    /// The sustain pedal of `sound`: up releases the notes it held.
    pub fn pedal(&mut self, sound: usize, down: bool) {
        let Some(p) = self.sounds.get(sound).copied() else {
            return;
        };
        let b = bit(sound);
        self.pedal = if down { self.pedal | b } else { self.pedal & !b };
        if !down {
            for lane in self
                .lanes
                .iter_mut()
                .filter(|l| usize::from(l.sound) == sound && l.sustained != 0)
            {
                for s in 0..W {
                    if has(u32::from(lane.sustained), s) {
                        lane.release(&p, s, self.rate);
                    }
                }
            }
        }
    }

    /// Starts shedding one note by the platform's voice-budget order, for a new note of `incoming`
    /// (or none), from sound `only` alone when given: never a held voice of a sound at or under
    /// its reserve (unless for that sound's own note); released before held; the new note's own
    /// sound before others; lower priority before higher; the quietest, then the oldest. Whether
    /// one was found.
    fn shed(&mut self, incoming: Option<u8>, only: Option<u8>) -> bool {
        let mut held = [0usize; SOUNDS];
        for l in &self.lanes {
            if let Some(h) = held.get_mut(usize::from(l.sound)) {
                *h = h.saturating_add((l.held & !l.ending).count_ones() as usize);
            }
        }
        // rank: (held, other sound, priority, loudness, age), smallest first
        let mut best: Option<(usize, usize, Rank)> = None;
        for (li, l) in self.lanes.iter().enumerate() {
            let lim = self.limits.get(usize::from(l.sound)).copied().unwrap_or_default();
            for s in 0..W {
                if !has(u32::from(l.used & !l.ending), s) || only.is_some_and(|o| o != l.sound) {
                    continue;
                }
                let is_held = has(u32::from(l.held), s);
                let reserve = held.get(usize::from(l.sound)).copied().unwrap_or(0) <= usize::from(lim.reserve);
                if is_held && incoming != Some(l.sound) && reserve {
                    continue;
                }
                let rank = (
                    u8::from(is_held),
                    u8::from(incoming.is_some_and(|i| i != l.sound)),
                    lim.priority,
                    l.bound.get(s).copied().unwrap_or(0.0),
                    l.age.get(s).copied().unwrap_or(0),
                );
                if best.is_none_or(|(_, _, b)| rank.partial_cmp(&b) == Some(core::cmp::Ordering::Less)) {
                    best = Some((li, s, rank));
                }
            }
        }
        let Some((li, s, _)) = best else { return false };
        if let Some(l) = self.lanes.get_mut(li) {
            l.ending |= bit(s);
        }
        true
    }

    /// Adds one block of every sounding voice, mono, to `out`; reports the notes that ended.
    pub fn render<V: F4>(&mut self, out: &mut [f32; BLOCK], end: &mut impl FnMut(NoteEnd)) {
        self.render_events::<V>(out, &[], end);
    }

    /// [`Engine::render`] with `events` (in frame order) applied at their frames: each sound's
    /// control steps start again at its own events' frames (a step shorter than 16 frames before
    /// one), so a note starts on its frame and no sound's output depends on another's events.
    pub fn render_events<V: F4>(&mut self, out: &mut [f32; BLOCK], events: &[Event], end: &mut impl FnMut(NoteEnd)) {
        while self.keeping(None) > self.limit && self.shed(None, None) {}
        for (sound, lim) in (0..SOUNDS as u8).zip(self.limits).filter(|(_, l)| l.limit > 0) {
            while self.keeping(Some(sound)) > usize::from(lim.limit) && self.shed(None, Some(sound)) {}
        }
        let busy = self
            .lanes
            .iter()
            .filter(|l| l.used != 0)
            .fold(0, |m, l| m | bit(usize::from(l.sound)));
        let busy = events.iter().fold(busy, |m, e| m | bit(usize::from(e.sound)));
        for sound in (0..SOUNDS as u8).filter(|&s| has(u32::from(busy), usize::from(s))) {
            let mut from = 0;
            for e in events.iter().filter(|e| e.sound == sound) {
                let at = e.frame.clamp(from, BLOCK);
                self.render_sound::<V>(sound, out, from, at, end);
                from = at;
                let s = usize::from(sound);
                match e.kind {
                    Kind::On { id, key, velocity } => self.note_on(s, id, key, velocity, end),
                    Kind::Off { id } => self.note_off(s, id),
                    Kind::Pedal { down } => self.pedal(s, down),
                    Kind::Bend { value } => self.bend(s, value),
                }
            }
            self.render_sound::<V>(sound, out, from, BLOCK, end);
        }
        self.clock = self.clock.wrapping_add(BLOCK as u64);
    }

    /// Frames `from..to` of the lanes of `sound`, in control steps of 16 frames from `from` (the
    /// last one shorter).
    fn render_sound<V: F4>(
        &mut self,
        sound: u8,
        out: &mut [f32; BLOCK],
        from: usize,
        to: usize,
        end: &mut impl FnMut(NoteEnd),
    ) {
        let (y1, y4) = (&mut self.y1, &mut self.y4);
        let Some(p) = self.sounds.get(usize::from(sound)) else {
            return;
        };
        let c = Ctx {
            rate: self.rate,
            shed_step: self.shed_step,
            ms: (1000.0 / self.rate) as f32,
            bend: self.bend.get(usize::from(sound)).map_or(0.0, |&b| {
                laws::bend_steps(i32::from(b), i32::from(p.bend.0), i32::from(p.bend.1)) as f32 / 2048.0
            }),
            fixed_max: (laws::FIXED_MAX_HZ / self.rate * 4_294_967_296.0) as f32,
            tick: self.tick,
        };
        let clock = self.clock;
        for lane in self.lanes.iter_mut().filter(|l| l.used != 0 && l.sound == sound) {
            let mut f = from;
            while let Some(chunk) = out.get_mut(f..to.min(f.saturating_add(STEP))) {
                if chunk.is_empty() {
                    break;
                }
                let t = clock.wrapping_add(f as u64);
                f = f.saturating_add(chunk.len());
                lane.compact();
                // the lane's width follows its voices (kept in its first slots)
                match lane.used {
                    0 => break,
                    1 => step::<1, scalar::F<1>>(lane, p, &c, t, end, chunk, y1),
                    _ => step::<W, V>(lane, p, &c, t, end, chunk, y4),
                }
            }
        }
    }
}

/// What a lane's control step takes from the engine: the rate (Hz), the shed fade a step,
/// milliseconds a frame, the bend of the lane's sound (octaves), the highest increment a
/// fixed-mode operator plays, and the glide clock's period (frames, 2^-16 a unit).
struct Ctx {
    rate: f64,
    shed_step: f32,
    ms: f32,
    bend: f32,
    fixed_max: f32,
    tick: u64,
}

impl Ctx {
    /// Whether the glide clock ticks in the step of `n` frames from frame `t`.
    fn ticks(&self, t: u64, n: usize) -> bool {
        let at = |t: u64| t.wrapping_mul(65536).checked_div(self.tick);
        at(t) != at(t.wrapping_add(n as u64))
    }
}

/// An increment (or its ramp) for each operator and voice of a lane at width `L`.
type Incs<const L: usize> = [[u32; L]; OPS];

/// The highest increment (the largest `f32` below 2^31).
const MAX_INC: f32 = 2_147_483_520.0;

/// One control step of a lane at width `L` (its voices in its first `L` slots), added to `out`
/// (1 to 16 frames: a shorter step is rendered whole, its phases and feedback history then set
/// to where its last frame left them), from frame `t` of the engine's clock.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn step<const L: usize, V: F4<L>>(
    lane: &mut Lane,
    p: &Prepared,
    c: &Ctx,
    t: u64,
    end: &mut impl FnMut(NoteEnd),
    out: &mut [f32],
    y: &mut [[[f32; L]; STEP]; OPS],
) {
    let n = out.len().clamp(1, STEP);
    let (g0, dg) = envelopes::<L, V>(lane, p, c, n, end);
    // a lane whose pitch does not move skips this
    let pt = &lane.pt;
    let ramp = if pt.moving != 0 || pt.jump || pt.bend != c.bend {
        pitch::<L, V>(lane, p, c, t, n)
    } else {
        None
    };
    let start = (n < STEP).then(|| (lane.ph, lane.fb[0]));
    let mut vout = [[0.0f32; L]; STEP];
    // a sound with sines only runs the kernels without the forms' branches; the increments ramp
    // only while a pitch envelope moves them
    match (p.plan.forms, &ramp) {
        (true, None) => ops::<L, V, true, false>(lane, p, &g0, &dg, &[[0; L]; OPS], y, &mut vout),
        (false, None) => ops::<L, V, false, false>(lane, p, &g0, &dg, &[[0; L]; OPS], y, &mut vout),
        (true, Some((d, _))) => ops::<L, V, true, true>(lane, p, &g0, &dg, d, y, &mut vout),
        (false, Some((d, _))) => ops::<L, V, false, true>(lane, p, &g0, &dg, d, y, &mut vout),
    }
    if let Some((ph0, fb0)) = start {
        short(lane, p, n, &ph0, &fb0, y, ramp.as_ref().map(|r| &r.0));
    }
    if let Some((_, to)) = ramp {
        for (inc, to) in lane.inc.iter_mut().zip(to.iter()) {
            V::U::load(*to).store4(inc);
        }
    }
    // the voices in pairs, as four-wide (the missing ones' zeros change no bits)
    for (o, v) in out.iter_mut().zip(vout.iter()) {
        *o += match v.as_slice() {
            [a] => *a,
            [a, b] => *a + *b,
            [a, b, c, d] => (*a + *b) + (*c + *d),
            _ => 0.0,
        };
    }
}

/// After a step of `n` frames (fewer than 16) rendered whole: the phases `n` increments on from
/// `ph0` (with the increments' ramp `d` a frame), the feedback history the source's outputs at
/// frames `n - 1` and `n - 2` (`fb0` before).
#[inline(never)]
fn short<const L: usize>(
    lane: &mut Lane,
    p: &Prepared,
    n: usize,
    ph0: &[[u32; W]; OPS],
    fb0: &[f32; W],
    y: &[[[f32; L]; STEP]; OPS],
    d: Option<&[[u32; L]; OPS]>,
) {
    let live = (lane.ph.iter_mut().zip(ph0.iter().zip(lane.inc.iter()))).zip(0..OPS);
    // (n is at most 16)
    let ramp = (n.wrapping_mul(n.saturating_sub(1)) / 2) as u32;
    for ((ph, (p0, inc)), i) in live.filter(|&(_, i)| has(u32::from(p.plan.live), i)) {
        let d = d.and_then(|d| d.get(i)).copied().unwrap_or([0; L]);
        let ds = d.into_iter().chain([0; W]);
        for ((ph, (p0, inc)), ds) in ph.iter_mut().zip(p0.iter().zip(inc.iter())).zip(ds) {
            *ph = p0
                .wrapping_add(inc.wrapping_mul(n as u32))
                .wrapping_add(ds.wrapping_mul(ramp));
        }
    }
    let src = match p.plan.feedback {
        Feedback::Own(i) => usize::from(i),
        Feedback::Loop { from, .. } => usize::from(from),
        Feedback::None => return,
    };
    let Some(ys) = y.get(src) else { return };
    let y1 = ys.get(n.wrapping_sub(1)).copied().unwrap_or([0.0; L]);
    let y2 = ys.get(n.wrapping_sub(2)).copied().unwrap_or_else(|| first(fb0));
    let [f1, f2] = &mut lane.fb;
    f1.iter_mut().zip(y1).for_each(|(f, y)| *f = y);
    f2.iter_mut().zip(y2).for_each(|(f, y)| *f = y);
}

/// A lane's pitch for a control step of `n` frames from frame `t` (docs/design.md, 2.3): what
/// jumps (the bend, a portamento step when the glide clock ticks) is in the increments from the
/// step's start; while a pitch envelope moves them, the envelopes move a step and the increments
/// ramp linearly to their value at the step's end: returns the ramp a frame and that value.
#[inline(never)]
fn pitch<const L: usize, V: F4<L>>(
    lane: &mut Lane,
    p: &Prepared,
    c: &Ctx,
    t: u64,
    n: usize,
) -> Option<(Incs<L>, Incs<L>)> {
    let pt = &mut lane.pt;
    let mut gliding = 0;
    if pt.glide.iter().take(L).any(|&g| g != 0.0) && c.ticks(t, n) {
        for (s, (g, &st)) in (0..L).zip(pt.glide.iter_mut().zip(pt.glide_step.iter())) {
            *g = if *g > 0.0 {
                (*g - st).max(0.0)
            } else {
                (*g + st).min(0.0)
            };
            gliding |= if *g != 0.0 { bit(s) } else { 0 };
        }
        pt.jump = true;
    } else {
        gliding = (0..L)
            .zip(pt.glide.iter())
            .fold(0, |m, (s, &g)| if g != 0.0 { m | bit(s) } else { m });
    }
    if pt.jump || pt.bend != c.bend {
        (pt.jump, pt.bend) = (false, c.bend);
        for (inc, to) in lane.inc.iter_mut().zip(incs::<L, V>(pt, p, c).iter()) {
            V::U::load(*to).store4(inc);
        }
    }
    let moves = |seg: u8| matches!(seg, ATTACK | DECAY1 | DECAY2 | RELEASE);
    if pt.op_attack | pt.op_decay == 0 && !pt.c_seg.iter().take(L).any(|&s| moves(s)) {
        pt.moving = gliding;
        return None;
    }
    // the envelopes move straight in octaves for the step's milliseconds; a segment that ends
    // within the step leaves the rest of them to the next
    let ms = c.ms * n as f32;
    let go = |l: &mut f32, t: f32, r: f32, ms: &mut f32| {
        let (dist, reach) = ((t - *l).abs(), r * *ms);
        if reach < dist {
            *l += if *l < t { reach } else { -reach };
            *ms = 0.0;
            false
        } else {
            (*l, *ms) = (t, *ms - dist / r);
            true
        }
    };
    let mut w = pt.op_attack | pt.op_decay;
    while w != 0 {
        let k = w.trailing_zeros();
        w &= w.wrapping_sub(1);
        let (op, s, b) = ((k / 4) as usize, (k % 4) as usize, 1u32.checked_shl(k).unwrap_or(0));
        let e = p.opeg.get(op).copied().unwrap_or([0.0; 4]);
        let (mut l, mut left) = (get(&pt.op, op, s), ms);
        if pt.op_attack & b != 0 && go(&mut l, e[1], e[2], &mut left) {
            (pt.op_attack, pt.op_decay) = (pt.op_attack & !b, pt.op_decay | b);
        }
        if pt.op_decay & b != 0 && left > 0.0 && go(&mut l, 0.0, e[3], &mut left) {
            pt.op_decay &= !b;
        }
        put(&mut pt.op, op, s, l);
    }
    let mut moving = 0;
    for (s, (l, (seg, (&sc, &rt)))) in (0..L).zip(
        pt.c.iter_mut()
            .zip(pt.c_seg.iter_mut().zip(pt.c_scale.iter().zip(pt.c_rate.iter()))),
    ) {
        let mut left = ms;
        while moves(*seg) && left > 0.0 {
            // the segment's level and rate, and the one after it
            let (i, next) = match *seg {
                ATTACK => (1, DECAY1),
                DECAY1 => (2, DECAY2),
                DECAY2 => (3, HOLD),
                _ => (4, DONE),
            };
            let (t, r) = (at(&p.peg, i) * sc, at(&p.peg_rate, i.wrapping_sub(1)) * rt);
            if go(l, t, r, &mut left) {
                *seg = next;
            }
        }
        if moves(*seg) || (pt.op_attack | pt.op_decay) & slot_ops(s) != 0 {
            moving |= bit(s);
        }
    }
    pt.moving = moving | gliding;
    let to = incs::<L, V>(pt, p, c);
    let mut d = [[0u32; L]; OPS];
    for ((d, to), from) in d.iter_mut().zip(to.iter()).zip(lane.inc.iter()) {
        for ((d, &to), &from) in d.iter_mut().zip(to.iter()).zip(from.iter()) {
            // (both below 2^31)
            *d = (to as i32).wrapping_sub(from as i32).checked_div(n as i32).unwrap_or(0) as u32;
        }
    }
    Some((d, to))
}

/// Each live operator's increment from a lane's pitch state: its base times 2 to the octaves
/// that move it (the common pitch envelope and portamento for ratio-mode operators, the
/// operator's own envelope, the bend), held under Nyquist (fixed-mode ones at their highest
/// frequency); a voice that nothing moves keeps its base exactly.
#[inline(always)]
fn incs<const L: usize, V: F4<L>>(pt: &Pitch, p: &Prepared, c: &Ctx) -> [[u32; L]; OPS] {
    let zero = V::splat(0.0);
    let bend = V::splat(c.bend);
    let common = V::load4(&pt.c) + V::load4(&pt.glide) + bend;
    let mut out = [[0u32; L]; OPS];
    for (op, o) in (0..OPS)
        .zip(out.iter_mut())
        .filter(|&(op, _)| has(u32::from(p.plan.live), op))
    {
        let fixed = has(u32::from(p.fixed), op);
        let mut m = if fixed { bend } else { common };
        if let Some(e) = pt.op.get(op).filter(|_| has(u32::from(p.opeg_on), op)) {
            m = m + V::load4(e);
        }
        let base = V::U::load4(pt.base.get(op).unwrap_or(&[0; W]));
        // 2^m as 2^(m - 16) 2^16, within the vector exp2's range
        let f = base.i32_to_f32() * (m - V::splat(16.0)).exp2() * V::splat(65536.0);
        let f = f.min(V::splat(if fixed { c.fixed_max } else { MAX_INC }));
        let still = m.ge(zero).and(zero.ge(m));
        *o = V::select(still, base.bits_to_f32(), f.to_i32().bits_to_f32())
            .to_bits()
            .store();
    }
    out
}

/// One control step of a lane's envelopes: returns each operator's gain at the step's start and
/// its ramp a frame; moves segments on, fades shed voices, and ends voices (the end rule, a fade's
/// end).
#[inline(always)]
fn envelopes<const L: usize, V: F4<L>>(
    lane: &mut Lane,
    p: &Prepared,
    c: &Ctx,
    n: usize,
    end: &mut impl FnMut(NoteEnd),
) -> ([[f32; L]; OPS], [[f32; L]; OPS]) {
    // a step of `n` frames moves each envelope `k` of a 16-frame step's way
    let (k, per_frame) = (n as f32 / STEP as f32, V::splat(1.0 / n as f32));
    let shed_step = c.shed_step * k;
    // shed voices fade linearly
    for (s, f) in lane.fade.iter_mut().enumerate().take(L) {
        if has(u32::from(lane.ending), s) {
            // (to 0 on the last step, whatever the rounding of the steps before)
            *f = if *f > shed_step * 1.5 { *f - shed_step } else { 0.0 };
        }
    }
    let fade = V::load4(&lane.fade) * V::splat(16.0); // 2^HEAD
    let mut g0 = [[0.0; L]; OPS];
    let mut dg = [[0.0; L]; OPS];
    let mut bound = V::splat(0.0);
    // a shorter step's falls, and rises' shares of the deficit (1 - e^-rate)
    let short;
    let (falls, rises, min_rise) = if n < STEP {
        short = (lane.fall.map(|f| (V::load4(&f) * V::splat(k)).store())).map(|f| {
            let r = V::splat(1.0) - (V::load(f) * V::splat(-core::f32::consts::LOG2_E)).exp2();
            (f, r.store())
        });
        (short.map(|x| x.0), short.map(|x| x.1), MIN_RISE * k)
    } else {
        (lane.fall.map(|f| first(&f)), lane.rise.map(|r| first(&r)), MIN_RISE)
    };
    let ops = (lane.lvl.iter_mut().zip(lane.gain.iter_mut()))
        .zip(g0.iter_mut().zip(dg.iter_mut()))
        .zip((lane.tgt.iter().zip(falls.iter())).zip(rises.iter().zip(lane.top.iter())))
        .zip(0..OPS);
    for ((((lv, gain), (g, d)), ((t, fall), (rise, top))), i) in ops {
        if !has(u32::from(p.plan.live), i) {
            continue;
        }
        let (l, t) = (V::load4(lv), V::load4(t));
        let up = ((V::load4(top) - l) * V::load(*rise)).max(V::splat(min_rise)) + l;
        let l2 = V::select(l.ge(t), (l - V::load(*fall)).max(t), up.min(t));
        l2.store4(lv);
        let target = l2.exp2() * fade;
        let g_last = V::load4(gain);
        *g = g_last.store();
        *d = ((target - g_last) * per_frame).store();
        target.store4(gain);
        if has(u32::from(p.plan.carriers), i) {
            bound = bound + l2.max(t).exp2();
        }
    }
    (bound * V::splat(16.0)).store4(&mut lane.bound);
    // segment changes, read from memory only for the envelopes that can change
    let mut w = lane.watch;
    while w != 0 {
        let k = w.trailing_zeros() as usize;
        w &= w.wrapping_sub(1);
        let (op, s) = (k / 4, k % 4);
        let seg = get(&lane.seg, op, s);
        let reached = get(&lane.lvl, op, s) == get(&lane.tgt, op, s);
        match seg {
            HOLD => {
                let left = get(&lane.count, op, s).saturating_sub(n as u32);
                put(&mut lane.count, op, s, left);
                if left == 0 {
                    lane.enter(p, op, s, ATTACK, c.rate);
                }
            }
            ATTACK | DECAY1 if reached => lane.enter(p, op, s, seg.saturating_add(1), c.rate),
            RELEASE if reached => lane.enter(p, op, s, DONE, c.rate),
            _ => {}
        }
    }
    // the end rule (released voices only) and the end of a fade
    for s in 0..L {
        let released = has(u32::from(lane.used & !(lane.held | lane.sustained)), s);
        let quiet = lane.bound.get(s).is_some_and(|&b| b < END_GAIN);
        let faded = has(u32::from(lane.ending), s) && lane.fade.get(s).is_some_and(|&f| f <= 0.0);
        if (released && quiet) || faded {
            lane.end(s, end);
        }
    }
    (g0, dg)
}

/// One control step of a lane's operators, in ascending order (every modulation edge runs
/// upwards), each a whole step at a time, except a feedback loop through several operators,
/// whose operators are interleaved sample by sample; carriers summed into `vout`.
#[allow(clippy::too_many_arguments)]
fn ops<const L: usize, V: F4<L>, const FORMS: bool, const RAMP: bool>(
    lane: &mut Lane,
    p: &Prepared,
    g0: &[[f32; L]; OPS],
    dg: &[[f32; L]; OPS],
    di: &[[u32; L]; OPS],
    y: &mut [[[f32; L]; STEP]; OPS],
    vout: &mut [[f32; L]; STEP],
) {
    let plan = &p.plan;
    let (lo, hi) = match plan.feedback {
        Feedback::Loop { to, from } => (usize::from(to), usize::from(from)),
        _ => (OPS, OPS),
    };
    for i in (0..OPS).filter(|&i| has(u32::from(plan.live), i)) {
        if FORMS && i == lo && plan.loop_forms {
            interleaved_forms::<L, V, RAMP>(lane, p, g0, dg, di, y, lo, hi);
        } else if i == lo {
            interleaved::<L, V, false, RAMP>(lane, p, g0, dg, di, y, lo, hi);
        } else if i < lo || i > hi {
            single::<L, V, FORMS, RAMP>(lane, p, g0, dg, di, y, i);
        }
        if has(u32::from(plan.carriers), i) {
            if let Some(out) = y.get(i) {
                for (o, s) in vout.iter_mut().zip(out.iter()) {
                    *o = (V::load(*o) + V::load(*s)).store();
                }
            }
        }
    }
}

/// Operator `i` over a whole step: its modulation input summed first, then its samples (with
/// `RAMP`, its increment moving by `di` a frame).
#[allow(clippy::too_many_arguments)]
fn single<const L: usize, V: F4<L>, const FORMS: bool, const RAMP: bool>(
    lane: &mut Lane,
    p: &Prepared,
    g0: &[[f32; L]; OPS],
    dg: &[[f32; L]; OPS],
    di: &[[u32; L]; OPS],
    y: &mut [[[f32; L]; STEP]; OPS],
    i: usize,
) {
    let Some((done, rest)) = y.split_at_mut_checked(i) else {
        return;
    };
    let Some((cur, _)) = rest.split_first_mut() else { return };
    let (Some(ph), Some(&inc), Some(&g), Some(&d), Some(&di), Some(&mods)) = (
        lane.ph.get_mut(i),
        lane.inc.get(i),
        g0.get(i),
        dg.get(i),
        di.get(i),
        p.plan.mods.get(i),
    ) else {
        return;
    };
    let di = V::U::load(di);
    let mut input = [[0.0f32; L]; STEP];
    for (j, src) in done.iter().enumerate() {
        if has(u32::from(mods), j) {
            for (m, s) in input.iter_mut().zip(src.iter()) {
                *m = (V::load(*m) + V::load(*s)).store();
            }
        }
    }
    let (mut inc, d) = (V::U::load4(&inc), V::load(d));
    let (mut ph_v, mut g) = (V::U::load4(ph), V::load(g));
    let wave = p.plan.waves.get(i).copied().unwrap_or(Wave::Sine);
    if FORMS && wave != Wave::Sine {
        let own = (p.plan.feedback == Feedback::Own(i as u8)).then_some((&mut lane.fb, p.fbk));
        form_step::<L, V, RAMP>(wave, own, cur, &input, (inc, di), g, d, &mut ph_v);
    } else if p.plan.feedback == Feedback::Own(i as u8) {
        // self-feedback: sample by sample within this operator
        let fbk = V::splat(p.fbk);
        let [f1, f2] = &mut lane.fb;
        let (mut y1, mut y2) = (V::load4(f1), V::load4(f2));
        for (out, m) in cur.iter_mut().zip(input.iter()) {
            let v = read::<L, V>(&SINE, ph_v.add(offset((y1 + y2).mul_add(fbk, V::load(*m))))) * g;
            *out = v.store();
            (y2, y1) = (y1, v);
            g = g + d;
            ph_v = ph_v.add(inc);
            if RAMP {
                inc = inc.add(di);
            }
        }
        y1.store4(f1);
        y2.store4(f2);
    } else {
        for (out, m) in cur.iter_mut().zip(input.iter()) {
            *out = (read::<L, V>(&SINE, ph_v.add(offset(V::load(*m)))) * g).store();
            g = g + d;
            ph_v = ph_v.add(inc);
            if RAMP {
                inc = inc.add(di);
            }
        }
    }
    ph_v.store4(ph);
}

/// A non-sine operator's step, kept out of line so the sine kernels compile as they do alone.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn form_step<const L: usize, V: F4<L>, const RAMP: bool>(
    wave: Wave,
    own: Option<(&mut [[f32; W]; 2], f32)>,
    cur: &mut [[f32; L]; STEP],
    input: &[[f32; L]; STEP],
    inc: (V::U, V::U),
    g: V,
    d: V,
    ph: &mut V::U,
) {
    let (g, d) = nyquist::<L, V>(wave, inc.0, g, d);
    // one loop for each form, the match outside it
    match wave {
        Wave::Table(t) => run::<L, V, RAMP>(|q, o| read16(t, q.add(o)), own, cur, input, inc, g, d, ph),
        Wave::Odd(t) => run::<L, V, RAMP>(|q, o| odd(t, q, o), own, cur, input, inc, g, d, ph),
        Wave::Resonant(t, c) => run::<L, V, RAMP>(|q, o| resonant(t, c, q, o), own, cur, input, inc, g, d, ph),
        Wave::Sine => run::<L, V, RAMP>(|q, o| read(&SINE, q.add(o)), own, cur, input, inc, g, d, ph),
    }
}

/// An odd form: E(2t + m) sin(t + m).
#[inline(always)]
fn odd<const L: usize, V: F4<L>>(t: &Table16, ph: V::U, off: V::U) -> V {
    read16::<L, V>(t, ph.add(ph).add(off)) * read::<L, V>(&SINE, ph.add(off))
}

/// A resonant form: window(t + m) sin(centre t + m).
#[inline(always)]
fn resonant<const L: usize, V: F4<L>>(t: &Table16, centre: u32, ph: V::U, off: V::U) -> V {
    read16::<L, V>(t, ph.add(off)) * read::<L, V>(&SINE, ph.mul(V::U::splat(centre)).add(off))
}

/// An operator's output at phase `ph` and modulation offset `off`.
#[inline(always)]
fn wave<const L: usize, V: F4<L>>(w: Wave, ph: V::U, off: V::U) -> V {
    match w {
        Wave::Sine => read(&SINE, ph.add(off)),
        Wave::Table(t) => read16(t, ph.add(off)),
        Wave::Odd(t) => odd(t, ph, off),
        Wave::Resonant(t, c) => resonant(t, c, ph, off),
    }
}

/// A resonant form is silent while its centre is at or above Nyquist (its gain and ramp zero for
/// the step); the other forms as they are.
#[inline(always)]
fn nyquist<const L: usize, V: F4<L>>(w: Wave, inc: V::U, g: V, d: V) -> (V, V) {
    let Wave::Resonant(_, c) = w else { return (g, d) };
    // centre x increment >= 2^31, in units of 2^8 (exact below 2^24)
    let above = inc
        .shr::<8>()
        .small_to_f32()
        .ge(V::splat(8_388_608.0 / c.max(1) as f32));
    (V::select(above, V::splat(0.0), g), V::select(above, V::splat(0.0), d))
}

/// One operator's step: `r` reads its form at a phase and modulation offset; with `own`, it
/// feeds back on itself sample by sample (the history and the amount); `inc` is the increment and
/// (with `RAMP`) its move a frame.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn run<const L: usize, V: F4<L>, const RAMP: bool>(
    r: impl Fn(V::U, V::U) -> V,
    own: Option<(&mut [[f32; W]; 2], f32)>,
    cur: &mut [[f32; L]; STEP],
    input: &[[f32; L]; STEP],
    (mut inc, di): (V::U, V::U),
    mut g: V,
    d: V,
    ph: &mut V::U,
) {
    let mut ph_v = *ph;
    if let Some(([f1, f2], k)) = own {
        let fbk = V::splat(k);
        let (mut y1, mut y2) = (V::load4(f1), V::load4(f2));
        for (out, m) in cur.iter_mut().zip(input.iter()) {
            let v = r(ph_v, offset((y1 + y2).mul_add(fbk, V::load(*m)))) * g;
            *out = v.store();
            (y2, y1) = (y1, v);
            g = g + d;
            ph_v = ph_v.add(inc);
            if RAMP {
                inc = inc.add(di);
            }
        }
        y1.store4(f1);
        y2.store4(f2);
    } else {
        for (out, m) in cur.iter_mut().zip(input.iter()) {
            *out = (r(ph_v, offset(V::load(*m))) * g).store();
            g = g + d;
            ph_v = ph_v.add(inc);
            if RAMP {
                inc = inc.add(di);
            }
        }
    }
    *ph = ph_v;
}

/// [`interleaved`] with non-sine forms, out of line (as [`form_step`]).
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn interleaved_forms<const L: usize, V: F4<L>, const RAMP: bool>(
    lane: &mut Lane,
    p: &Prepared,
    g0: &[[f32; L]; OPS],
    dg: &[[f32; L]; OPS],
    di: &[[u32; L]; OPS],
    y: &mut [[[f32; L]; STEP]; OPS],
    lo: usize,
    hi: usize,
) {
    interleaved::<L, V, true, RAMP>(lane, p, g0, dg, di, y, lo, hi);
}

/// The most operators a feedback loop runs through (routing 12's 3 > 4 > 5).
const LOOP: usize = 3;

/// Operators `lo..=hi` (the live ones) sample by sample: each sample's input reads the outputs
/// already made for that sample. Their inputs from below `lo` (written for the whole step before)
/// are summed first, a whole step at a time, in the order a single operator sums its sources, so
/// the bits are the same.
#[allow(clippy::too_many_arguments)]
fn interleaved<const L: usize, V: F4<L>, const MIXED: bool, const RAMP: bool>(
    lane: &mut Lane,
    p: &Prepared,
    g0: &[[f32; L]; OPS],
    dg: &[[f32; L]; OPS],
    di: &[[u32; L]; OPS],
    y: &mut [[[f32; L]; STEP]; OPS],
    lo: usize,
    hi: usize,
) {
    let plan = &p.plan;
    // the loop's live operators in order (OPS past the last)
    let mut ks = [OPS; LOOP];
    for (x, k) in ks.iter_mut().zip((lo..=hi).filter(|&k| has(u32::from(plan.live), k))) {
        *x = k;
    }
    let mods = ks.map(|k| u32::from(plan.mods.get(k).copied().unwrap_or(0)));
    // each one's sources inside the loop, by place in `ks`
    let inner = mods.map(|m| {
        (0..LOOP)
            .filter(|&a| ks.get(a).is_some_and(|&j| has(m, j)))
            .fold(0, |n, a| n | bit(a))
    });
    let mut buf = [[[0.0f32; L]; STEP]; LOOP];
    for (b, &m) in buf.iter_mut().zip(mods.iter()) {
        for (j, src) in y.iter().enumerate().take(lo) {
            if has(m, j) {
                for (x, s) in b.iter_mut().zip(src.iter()) {
                    *x = (V::load(*x) + V::load(*s)).store();
                }
            }
        }
    }
    let mut ph = ks.map(|k| V::U::load4(lane.ph.get(k).unwrap_or(&[0; W])));
    let mut inc = ks.map(|k| V::U::load4(lane.inc.get(k).unwrap_or(&[0; W])));
    let di = ks.map(|k| V::U::load(di.get(k).copied().unwrap_or([0; L])));
    let waves = ks.map(|k| plan.waves.get(k).copied().unwrap_or(Wave::Sine));
    let mut g = ks.map(|k| V::load(g0.get(k).copied().unwrap_or([0.0; L])));
    let mut d = ks.map(|k| V::load(dg.get(k).copied().unwrap_or([0.0; L])));
    if MIXED {
        for ((g, d), (&w, &i)) in g.iter_mut().zip(d.iter_mut()).zip(waves.iter().zip(inc.iter())) {
            (*g, *d) = nyquist::<L, V>(w, i, *g, *d);
        }
    }
    let fbk = V::splat(p.fbk);
    let (mut y1, mut y2) = (V::load4(&lane.fb[0]), V::load4(&lane.fb[1]));
    // (written out for two or three operators, so their state stays in registers)
    let three = ks.get(2).is_some_and(|&k| k < OPS);
    let [n1, n2] = [inner.get(1), inner.get(2)].map(|n| u32::from(n.copied().unwrap_or(0)));
    for s in 0..STEP {
        // operator `a` of the loop, its input from below with `add` applied
        let mut op = |a: usize, add: &dyn Fn(V) -> V| {
            let (Some(b), Some(pa), Some(ia), Some(&w), Some(ga), Some(&da), Some(&dia)) = (
                buf.get_mut(a).and_then(|b| b.get_mut(s)),
                ph.get_mut(a),
                inc.get_mut(a),
                waves.get(a),
                g.get_mut(a),
                d.get(a),
                di.get(a),
            ) else {
                return V::splat(0.0);
            };
            let m = add(V::load(*b));
            let out = if MIXED {
                wave::<L, V>(w, *pa, offset(m))
            } else {
                read::<L, V>(&SINE, pa.add(offset(m)))
            } * *ga;
            *ga = *ga + da;
            *pa = pa.add(*ia);
            if RAMP {
                *ia = ia.add(dia);
            }
            *b = out.store();
            out
        };
        let v0 = op(0, &|m| (y1 + y2).mul_add(fbk, m));
        let v1 = op(1, &|m| if n1 & 1 != 0 { m + v0 } else { m });
        let last = if three {
            op(2, &|m| {
                let m = if n2 & 1 != 0 { m + v0 } else { m };
                if n2 & 2 != 0 {
                    m + v1
                } else {
                    m
                }
            })
        } else {
            v1
        };
        // the loop's last operator is its feedback source
        (y2, y1) = (y1, last);
    }
    for ((&k, b), pa) in ks.iter().zip(buf.iter()).zip(ph.iter()) {
        if let (Some(yk), Some(d)) = (y.get_mut(k), lane.ph.get_mut(k)) {
            *yk = *b;
            pa.store4(d);
        }
    }
    let [f1, f2] = &mut lane.fb;
    y1.store4(f1);
    y2.store4(f2);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::PARAMS;
    use crate::simd::{scalar, Native};
    use std::collections::HashMap;

    const RATE: f64 = 48000.0;

    fn sound(text: &str) -> Prepared {
        Prepared::new(&Sound::from_text(&format!("fm-sound/1\n{text}\n")).unwrap())
    }

    fn engine(p: &Prepared) -> Engine {
        let mut e = Engine::new(RATE);
        assert!(e.set_sound(0, p));
        e
    }

    /// Renders `blocks` blocks of one note on sound 0, released after `hold` blocks.
    fn play<V: F4>(p: &Prepared, key: u8, velocity: f32, hold: usize, blocks: usize) -> Vec<f32> {
        let mut e = engine(p);
        let mut x = Vec::new();
        e.note_on(0, 1, key, velocity, &mut |_| {});
        for b in 0..blocks {
            if b == hold {
                e.note_off(0, 1);
            }
            let mut out = [0.0; BLOCK];
            e.render::<V>(&mut out, &mut |_| {});
            x.extend_from_slice(&out);
        }
        x
    }

    fn peak_db(x: &[f32]) -> f64 {
        20.0 * f64::from(x.iter().fold(0.0f32, |m, v| m.max(v.abs()))).log10()
    }

    /// One lone carrier, operator 1, on `routing`.
    fn carrier(routing: i32, extra: &str) -> Prepared {
        let level = if extra.contains("op1.level=") {
            ""
        } else {
            "op1.level=99 "
        };
        sound(&format!("algorithm={routing}\n{level}{extra}\nop8.level=0"))
    }

    #[test]
    fn carrier_level_and_routing_gain() {
        // a full-level sine carrier on routing 1 peaks at -17.64 dBFS; routing 24 is 13.55 dB higher
        let x = play::<Native>(&carrier(1, ""), 60, 100.0, 100, 40);
        assert!(
            (peak_db(&x[BLOCK * 8..]) - laws::CARRIER_FULL_DBFS).abs() < 0.02,
            "{}",
            peak_db(&x)
        );
        let x = play::<Native>(&carrier(24, ""), 60, 100.0, 100, 40);
        assert!((peak_db(&x[BLOCK * 8..]) - (laws::CARRIER_FULL_DBFS + 13.55)).abs() < 0.02);
        // operator level 70 and velocity sensitivity +7 at velocity 127 (velocity depth 64, offset 64)
        let x = play::<Native>(&carrier(1, "op1.level=70 op1.level_velocity=7"), 60, 127.0, 100, 40);
        let want = laws::CARRIER_FULL_DBFS - 21.99 + 5.27;
        assert!(
            (peak_db(&x[BLOCK * 8..]) - want).abs() < 0.02,
            "{} against {want}",
            peak_db(&x[BLOCK * 8..])
        );
        // envelope level 70 on decay 2 (attack and decay 1 at 99)
        let x = play::<Native>(
            &carrier(1, "op1.eg_level_decay2=70 op1.eg_time_decay2=0"),
            60,
            100.0,
            300,
            300,
        );
        let want = laws::CARRIER_FULL_DBFS - 21.09;
        assert!(
            (peak_db(&x[BLOCK * 200..]) - want).abs() < 0.02,
            "{} against {want}",
            peak_db(&x[BLOCK * 200..])
        );
    }

    /// Goertzel magnitude of `x` at `hz`, Hann-windowed.
    fn magnitude(x: &[f32], hz: f64) -> f64 {
        let n = x.len() as f64;
        let w = 2.0 * std::f64::consts::PI * hz / RATE;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            let hann = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n).cos();
            re += f64::from(v) * hann * (w * i as f64).cos();
            im += f64::from(v) * hann * (w * i as f64).sin();
        }
        (re * re + im * im).sqrt()
    }

    /// Bessel function of the first kind, order `n`, by its series.
    fn bessel(n: i32, x: f64) -> f64 {
        let mut term = (x / 2.0).powi(n) / (1..=n).map(f64::from).product::<f64>();
        let mut sum = term;
        for k in 1..30 {
            term *= -(x * x / 4.0) / (f64::from(k) * f64::from(k + n));
            sum += term;
        }
        sum
    }

    #[test]
    fn modulation_index() {
        // operator 1 (ratio 3) modulating operator 2 (ratio 1): the lines at f and 4 f are J0 and J1
        // of the index, 4 pi radians at 0 dB of the modulator level curve
        for level in [40, 60, 75] {
            let p = sound(&format!(
                "algorithm=7\nop1.level={level} op1.coarse=3\nop2.level=99\nop8.level=0"
            ));
            assert_eq!(p.plan.live, 3);
            let x = play::<Native>(&p, 57, 100.0, 1000, 160);
            let f = laws::steps_hz(laws::key_steps(57));
            let got = magnitude(&x[BLOCK * 16..], 4.0 * f) / magnitude(&x[BLOCK * 16..], f);
            let beta = 4.0 * std::f64::consts::PI * laws::db_to_gain(laws::modulator_level_db(level));
            let want = (bessel(1, beta) / bessel(0, beta)).abs();
            assert!((got / want - 1.0).abs() < 0.01, "level {level}: {got} against {want}");
        }
    }

    /// The level of each 48-sample window, dB.
    fn windows_db(x: &[f32]) -> Vec<f64> {
        x.chunks(48).map(peak_db).collect()
    }

    #[test]
    fn decay_and_release_rates() {
        // the slope between 3 and 35 dB under the peak, ms per dB, against the time tables
        for (t, key_follow, key, want) in [
            (50, 0, 105, laws::eg_ms_per_db(50)),
            (30, 0, 105, laws::eg_ms_per_db(30)),
            (70, 0, 105, laws::eg_ms_per_db(70)),
            (35, 7, 105, laws::eg_ms_per_db(35) * 5632.0 / 32768.0),
        ] {
            let p = carrier(
                1,
                &format!("op1.eg_time_decay1={t} op1.eg_level_decay1=0 op1.eg_level_decay2=0 op1.eg_time_key_follow={key_follow}"),
            );
            let seconds = 0.05 + 40.0 * want / 1000.0;
            let x = play::<Native>(&p, key, 100.0, 100_000, (seconds * RATE / BLOCK as f64) as usize + 1);
            let db = windows_db(&x);
            let top = db.iter().cloned().fold(f64::MIN, f64::max);
            let pts: Vec<(f64, f64)> = db
                .iter()
                .enumerate()
                .skip_while(|(_, &d)| d < top)
                .filter(|(_, &d)| top - d >= 3.0 && top - d <= 35.0)
                .map(|(i, &d)| (i as f64 * 48.0 / RATE * 1000.0, d))
                .collect();
            let n = pts.len() as f64;
            let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
            let (sxx, sxy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 * p.0, a.1 + p.0 * p.1));
            let ms = -1.0 / ((n * sxy - sx * sy) / (n * sxx - sx * sx));
            assert!(
                n > 2.0 && (ms / want - 1.0).abs() < 0.02,
                "time {t} key follow {key_follow}: {ms} ms/dB, table {want}"
            );
        }
    }

    #[test]
    fn hold_then_attack() {
        // the hold keeps the hold level (here silent) for its time, then the attack rises
        for t in [30, 50, 70] {
            let ms = laws::hold_ms(t);
            let p = carrier(1, &format!("op1.eg_time_hold={t}"));
            let x = play::<Native>(
                &p,
                69,
                100.0,
                100_000,
                ((ms / 1000.0 + 0.1) * RATE / BLOCK as f64) as usize,
            );
            let top = peak_db(&x);
            let first = x
                .iter()
                .position(|v| 20.0 * f64::from(v.abs()).log10() > top - 6.0)
                .unwrap();
            let got = first as f64 / RATE * 1000.0;
            assert!(
                (got - ms).abs() < (0.02 * ms).max(STEP as f64 / RATE * 2000.0),
                "hold {t}: {got} ms, table {ms}"
            );
        }
    }

    /// The frequency of `x` from its rising zero crossings, interpolated.
    fn zero_crossing_hz(x: &[f32]) -> f64 {
        let mut c = Vec::new();
        for i in 1..x.len() {
            if x[i - 1] < 0.0 && x[i] >= 0.0 {
                c.push(i as f64 - 1.0 + f64::from(x[i - 1] / (x[i - 1] - x[i])));
            }
        }
        (c.len() - 1) as f64 / (c[c.len() - 1] - c[0]) * RATE
    }

    #[test]
    fn pitch() {
        let cents = |got: f64, want: f64| 1200.0 * (got / want).log2();
        for (text, key, want) in [
            (
                "op1.coarse=2 op1.fine=37",
                57,
                laws::steps_hz(laws::key_steps(57) + laws::ratio_steps(laws::ratio(2, 37))),
            ),
            (
                "op1.coarse=0",
                81,
                laws::steps_hz(laws::key_steps(81) + laws::ratio_steps(0.4999996)),
            ),
            (
                "op1.detune=-15",
                36,
                laws::steps_hz(laws::key_steps(36) + laws::detune_steps(-15, 36)),
            ),
            (
                "op1.mode=1 op1.coarse=18 op1.fine=64",
                60,
                laws::steps_hz(laws::fixed_steps(18, 64)),
            ),
            (
                "op1.mode=1 op1.coarse=15 op1.detune=15",
                48,
                laws::steps_hz(laws::fixed_steps(15, 0) + laws::detune_steps(15, 48)),
            ),
        ] {
            let x = play::<Native>(&carrier(1, text), key, 100.0, 1000, 100);
            let c = cents(zero_crossing_hz(&x[BLOCK * 4..]), want);
            assert!(c.abs() < 0.05, "{text} key {key}: {c} cents");
        }
        // note shift moves the key in whole semitones before the grid
        let p = sound("algorithm=1 note_shift=-12\nop1.level=99\nop8.level=0");
        let x = play::<Native>(&p, 69, 100.0, 1000, 100);
        assert!(cents(zero_crossing_hz(&x[BLOCK * 4..]), laws::steps_hz(laws::key_steps(57))).abs() < 0.05);
        // a fixed frequency holds at 22.04 kHz
        let p = carrier(1, "op1.mode=1 op1.coarse=31 op1.fine=127");
        let mut e = engine(&p);
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        let hz = f64::from(e.lanes[0].inc[0][0]) / 4_294_967_296.0 * RATE;
        assert!((hz - laws::FIXED_MAX_HZ).abs() < 0.01, "{hz}");
    }

    /// The pitch a note keeps: key follow about its centre, the part's velocity sensitivity and
    /// detune, a fixed-mode operator's key follow and velocity, on the grid; random pitch in whole
    /// 18.75-cent steps.
    #[test]
    fn pitch_offsets() {
        let cents = |got: f64, want: f64| 1200.0 * (got / want).log2();
        let ratio = |key: i32, c: f64| laws::steps_hz(laws::key_steps(key) + laws::cents_steps(c));
        let fixed = |c: f64| laws::steps_hz(laws::fixed_steps(16, 0) + laws::cents_steps(c));
        for (text, key, want) in [
            ("pitch_key_follow=64", 72, ratio(72, -1200.0)),
            ("pitch_key_follow=127 pitch_key_centre=48", 60, ratio(60, 1200.0)),
            ("pitch_velocity=63", 69, ratio(69, 1189.4)),
            ("detune=127", 57, ratio(57, 49.63)),
            ("op1.mode=1 op1.coarse=16 op1.fixed_key_follow=99", 72, fixed(1200.0)),
            (
                "op1.mode=1 op1.coarse=16 op1.pitch_velocity=-7 pitch_velocity=-64",
                60,
                fixed(-1504.05 - 1189.46),
            ),
            // fixed-mode operators do not follow key follow or the part's detune
            (
                "op1.mode=1 op1.coarse=16 pitch_key_follow=0 detune=-128",
                40,
                fixed(0.0),
            ),
        ] {
            let x = play::<Native>(&carrier(1, text), key as u8, 127.0, 1000, 100);
            let c = cents(zero_crossing_hz(&x[BLOCK * 4..]), want);
            assert!(c.abs() < 0.05, "{text} key {key}: {c} cents");
        }
        let p = carrier(1, "random_pitch=127");
        let mut e = engine(&p);
        let base = f64::from(e.sounds[0].steps[0] + laws::key_steps(60));
        let mut seen = std::collections::BTreeSet::new();
        for id in 0..200 {
            e.note_on(0, id, 60, 100.0, &mut |_| {});
            let l = e.lanes.iter().find(|l| l.id.contains(&id)).unwrap();
            let s = l.id.iter().position(|&i| i == id).unwrap();
            let hz = f64::from(l.inc[0][s]) / 4_294_967_296.0 * RATE;
            let c = cents(hz, laws::steps_hz(base as i32));
            let k = (c / 18.75).round();
            assert!((c - k * 18.75).abs() < 0.6 && k.abs() <= 31.0, "{c} cents");
            seen.insert(k as i32);
            e.note_off(0, id);
            e.choke(0);
            e.render::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        }
        assert!(seen.len() > 40, "{} values", seen.len());
    }

    /// Both pitch envelopes against their laws, by operator 1's increment at every block's end
    /// against the note's own: straight in cents at their rates, the common one scaled by velocity
    /// (levels and rates alike) and time key follow, released with the key, and not reaching
    /// fixed-mode operators; within 0.2 cents (the vector exp2's accuracy is 0.15 cents).
    #[test]
    fn pitch_envelopes() {
        let (a, d) = (laws::peg_cents_per_ms(50), laws::peg_cents_per_ms(40));
        let (lo, hi) = (laws::peg_cents(-20), laws::peg_cents(10));
        let top = (hi - lo) / a; // the attack's end, ms
        let r = laws::part_peg_cents_per_ms(40);
        let off = 20.0 * BLOCK as f64 / RATE * 1000.0; // the release of the fifth case, ms
        type Law<'a> = &'a dyn Fn(f64) -> f64;
        let laws: [(&str, u8, f32, Law); 6] = [
            (
                "op1.pitch_eg_level_initial=-20 op1.pitch_eg_level_attack=10 op1.pitch_eg_time_attack=50 \
                 op1.pitch_eg_time_decay=40",
                69,
                100.0,
                &|t| {
                    if t < top {
                        lo + a * t
                    } else {
                        (hi - d * (t - top)).max(0.0)
                    }
                },
            ),
            (
                "pitch_eg_level_initial=-32 pitch_eg_time_attack=40 pitch_eg_depth=1",
                69,
                100.0,
                &|t| (-300.0 + r * t).min(0.0),
            ),
            (
                "pitch_eg_level_initial=-32 pitch_eg_time_attack=40 pitch_eg_depth=1 pitch_eg_depth_velocity=3",
                69,
                64.0,
                &|t| (-300.0 + r * t).min(0.0) * 102.0 / 128.0,
            ),
            (
                "pitch_eg_level_initial=-32 pitch_eg_time_attack=40 pitch_eg_depth=1 pitch_eg_time_key_follow=7",
                36,
                100.0,
                &|t| (-300.0 + 8.0 * r * t).min(0.0),
            ),
            ("pitch_eg_level_release=32 pitch_eg_time_release=60", 69, 100.0, &|t| {
                (laws::part_peg_cents_per_ms(60) * (t - off)).clamp(0.0, 1200.0)
            }),
            (
                "op1.mode=1 op1.coarse=16 pitch_eg_level_initial=-32 pitch_eg_time_attack=40",
                69,
                100.0,
                &|_| 0.0,
            ),
        ];
        for (text, key, vel, law) in laws {
            // (a long release, so the note sounds throughout)
            let mut e = engine(&carrier(1, &format!("{text} op1.eg_time_release=99")));
            e.note_on(0, 1, key, vel, &mut |_| {});
            let base = f64::from(e.lanes[0].pt.base[0][0]);
            for b in 1..=300 {
                if b == 21 && text.contains("release") {
                    e.note_off(0, 1);
                }
                e.render::<Native>(&mut [0.0; BLOCK], &mut |_| {});
                let t = (b * BLOCK) as f64 / RATE * 1000.0;
                let got = 1200.0 * (f64::from(e.lanes[0].inc[0][0]) / base).log2();
                assert!(
                    (got - law(t)).abs() < 0.2,
                    "{text} at {t} ms: {got} cents, the law {}",
                    law(t)
                );
            }
        }
    }

    /// The fastest operator pitch envelope (4800 cents at 387.6 cents a millisecond) at control
    /// rate with the increments ramped across each step, against the per-sample law: the phase
    /// at every block's end within 0.005 cycles of the law's.
    #[test]
    fn pitch_ramp_follows_the_law() {
        let p = carrier(1, "op1.pitch_eg_level_initial=-50 op1.pitch_eg_time_attack=0");
        let mut e = engine(&p);
        e.note_on(0, 1, 93, 100.0, &mut |_| {});
        let base = f64::from(e.lanes[0].pt.base[0][0]) / 4_294_967_296.0;
        let (lo, rate) = (laws::peg_cents(-50), laws::peg_cents_per_ms(0) * 1000.0 / RATE);
        let (mut law, mut worst) = (0.0f64, 0.0f64);
        for b in 0..12 {
            e.render::<Native>(&mut [0.0; BLOCK], &mut |_| {});
            for k in b * BLOCK..(b + 1) * BLOCK {
                law += base * ((lo + rate * k as f64).min(0.0) / 1200.0).exp2();
            }
            let got = f64::from(e.lanes[0].ph[0][0]) / 4_294_967_296.0;
            let err = (got - law.fract() + 1.5).fract() - 0.5;
            worst = worst.max(err.abs());
        }
        assert!(worst < 0.005, "{worst} cycles");
    }

    #[test]
    fn feedback_amounts() {
        // a modulator (routing 2's operator 1) feeds back its own amount; a carrier (routing 1's
        // operator 1) its measured share, before the routing's output level
        let p = sound("algorithm=2 feedback=7\nop1.level=99 op2.level=99 op3.level=99 op4.level=99\nop8.level=0");
        assert_eq!((p.plan.feedback, p.fbk), (Feedback::Own(0), 0.25));
        let p = sound("algorithm=1 feedback=5\nop1.level=99\nop8.level=0");
        let a = laws::db_to_gain(laws::CARRIER_FULL_DBFS);
        assert_eq!(p.fbk, (0.0625 * 0.18446 * 2.0 / a) as f32);
        // and the output stays finite and bounded at the strongest feedback on every routing
        for r in 1..=88 {
            let text: String = (1..=8).map(|o| format!("op{o}.level=99\n")).collect();
            let p = sound(&format!("algorithm={r} feedback=7\n{text}"));
            let x = play::<Native>(&p, 60, 127.0, 20, 40);
            assert!(x.iter().all(|v| v.is_finite() && v.abs() < 12.0), "routing {r}");
        }
    }

    #[test]
    fn velocity_and_release() {
        // a part velocity of 0 or below plays no note: it ends at once
        let p = sound("algorithm=1 velocity_depth=0 velocity_offset=0\nop1.level=99\nop8.level=0");
        let mut e = engine(&p);
        let mut ends = Vec::new();
        e.note_on(0, 7, 60, 100.0, &mut |n| ends.push(n));
        assert_eq!(
            (ends, e.active()),
            (
                vec![NoteEnd {
                    sound: 0,
                    key: 60,
                    id: 7
                }],
                0
            )
        );
        // a held note never ends by the end rule, even silent; released, it ends once
        let p = carrier(1, "op1.eg_level_decay1=0 op1.eg_level_decay2=0 op1.eg_time_decay1=0");
        let mut e = engine(&p);
        let mut ends = Vec::new();
        e.note_on(0, 3, 60, 100.0, &mut |n| ends.push(n));
        for b in 0..2000 {
            if b == 1000 {
                e.note_off(0, 3);
            }
            let mut out = [0.0; BLOCK];
            e.render::<Native>(&mut out, &mut |n| ends.push(n));
            assert_eq!(e.active(), usize::from(b < 1000), "block {b}");
        }
        assert_eq!(ends.len(), 1);
    }

    #[test]
    fn pedal_holds_released_notes() {
        let p = carrier(1, "op1.eg_time_release=0");
        let mut e = engine(&p);
        let mut ends = 0;
        e.pedal(0, true);
        e.note_on(0, 1, 60, 100.0, &mut |_| ends += 1);
        e.note_off(0, 1);
        for _ in 0..200 {
            e.render::<Native>(&mut [0.0; BLOCK], &mut |_| ends += 1);
        }
        assert_eq!((e.active(), ends), (1, 0));
        e.pedal(0, false);
        for _ in 0..200 {
            e.render::<Native>(&mut [0.0; BLOCK], &mut |_| ends += 1);
        }
        assert_eq!((e.active(), ends), (0, 1));
    }

    /// Which notes survive at the limit: the voice-budget order.
    #[test]
    fn shedding_order() {
        let quiet = carrier(1, "op1.level=50");
        let loud = carrier(1, "");
        let run = |e: &mut Engine, ends: &mut Vec<u32>| {
            for _ in 0..4 {
                e.render::<Native>(&mut [0.0; BLOCK], &mut |n| ends.push(n.id));
            }
        };
        let mut e = Engine::new(RATE);
        e.set_sound(0, &loud);
        e.set_sound(1, &quiet);
        e.set_limits(3, &[Limits::default(); SOUNDS]);
        let mut ends = Vec::new();
        let on = |e: &mut Engine, s, id, ends: &mut Vec<u32>| {
            e.note_on(s, id, 60, 100.0, &mut |n| ends.push(n.id));
            run(e, ends);
        };
        on(&mut e, 0, 1, &mut ends);
        on(&mut e, 0, 2, &mut ends);
        on(&mut e, 0, 3, &mut ends);
        // released before held: 2 is released, so the fourth note sheds it, with a fade
        e.note_off(0, 2);
        on(&mut e, 0, 4, &mut ends);
        assert_eq!(ends, vec![2]);
        // all held: the quietest goes, then the oldest
        on(&mut e, 1, 5, &mut ends);
        assert_eq!(ends, vec![2, 1], "the oldest of equally loud held notes");
        // the new note's own sound before others
        on(&mut e, 0, 6, &mut ends);
        assert_eq!(ends, vec![2, 1, 3], "the oldest of the new note's own sound");
        // a sound's reserve keeps its held voices from other sounds' notes
        let mut lim = [Limits::default(); SOUNDS];
        lim[0].reserve = 2;
        e.set_limits(3, &lim);
        on(&mut e, 1, 7, &mut ends);
        assert_eq!(
            ends,
            vec![2, 1, 3, 5],
            "sound 0 at its reserve: sound 1 gives up its own"
        );
        lim[1].reserve = 1;
        e.set_limits(3, &lim);
        on(&mut e, 2, 8, &mut ends);
        assert_eq!(
            ends,
            vec![2, 1, 3, 5, 8],
            "every voice protected: the new note gets none and ends at once"
        );
        // the lower priority gives way first
        lim = [Limits::default(); SOUNDS];
        lim[0].priority = 9;
        e.set_limits(2, &lim);
        run(&mut e, &mut ends);
        assert_eq!(ends, vec![2, 1, 3, 5, 8, 7]);
        // a sound's own limit
        lim[0].limit = 1;
        e.set_limits(0, &lim);
        run(&mut e, &mut ends);
        assert_eq!(ends, vec![2, 1, 3, 5, 8, 7, 4]);
        assert_eq!(e.active(), 1);
        // the quietest before the oldest
        let mut e = Engine::new(RATE);
        e.set_sound(0, &loud);
        e.set_sound(1, &quiet);
        e.set_limits(2, &[Limits::default(); SOUNDS]);
        ends.clear();
        on(&mut e, 0, 10, &mut ends);
        on(&mut e, 1, 11, &mut ends);
        on(&mut e, 2, 12, &mut ends);
        assert_eq!(ends, vec![11]);
    }

    #[test]
    fn shed_notes_fade_out_in_3_ms() {
        let mut e = engine(&carrier(1, ""));
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        for _ in 0..8 {
            e.render::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        }
        e.choke(0);
        let mut x = [0.0; BLOCK * 2];
        let mut ended_at = None;
        for (b, out) in x.chunks_exact_mut(BLOCK).enumerate() {
            e.render::<Native>(out.try_into().unwrap(), &mut |_| ended_at = Some(b));
        }
        assert_eq!(ended_at, Some(1));
        // a linear fade: no step larger than the fade's slope allows, silent from 3 ms on
        let a = laws::db_to_gain(laws::CARRIER_FULL_DBFS) as f32;
        assert!(x[150..].iter().all(|&v| v == 0.0));
        assert!(x[..120].iter().any(|&v| v.abs() > a * 0.5));
    }

    /// A random sound: every parameter M1 plays at a random value in its range.
    fn random_sound(r: &mut impl FnMut() -> u32) -> Prepared {
        let mut s = Sound::default();
        let played = [30u16, 31, 209, 212, 213];
        for p in PARAMS.iter() {
            let k = p.id % 100;
            let op_played = p.op.is_some()
                && ((40..=44).contains(&k) || (47..=49).contains(&k) || (54..=64).contains(&k) || k == 70);
            if played.contains(&p.id) || op_played {
                let span = (i32::from(p.max) - i32::from(p.min) + 1) as u32;
                s.set(p.id, (i32::from(p.min) + (r() % span) as i32) as i16).unwrap();
            }
        }
        Prepared::new(&s)
    }

    /// Random notes, releases, pedals, limits and sheds on random sounds: the output is finite
    /// and bounded, NEON and scalar agree bit for bit, and every note ends exactly once.
    #[test]
    fn fuzz() {
        let mut seed = 0x2545_f491u32;
        let mut r = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut a = Engine::new(RATE);
        let mut b = Engine::new(RATE);
        let mut started: HashMap<u32, u32> = HashMap::new();
        let mut ended: HashMap<u32, u32> = HashMap::new();
        let mut held: Vec<(usize, u32)> = Vec::new();
        let mut next = 0u32;
        let record = |n: NoteEnd, ended: &mut HashMap<u32, u32>| *ended.entry(n.id).or_default() += 1;
        for block in 0..3000 {
            if block % 500 == 0 {
                for s in 0..SOUNDS {
                    let p = random_sound(&mut r);
                    a.choke(s);
                    b.choke(s);
                    for _ in 0..4 {
                        let mut o = [0.0; BLOCK];
                        a.render::<Native>(&mut o, &mut |n| record(n, &mut ended));
                        b.render::<scalar::F>(&mut o, &mut |_| {});
                    }
                    held.retain(|h| h.0 != s);
                    assert!(a.set_sound(s, &p) && b.set_sound(s, &p), "sound {s} still sounding");
                }
            }
            // every other block, the notes and pedals as timed events
            let mut ev = Vec::new();
            for _ in 0..r() % 6 {
                let (timed, frame) = (block % 2 == 1, (r() % BLOCK as u32) as usize);
                let mut event = |sound: usize, kind| {
                    ev.push(Event {
                        frame,
                        sound: sound as u8,
                        kind,
                    })
                };
                match r() % 10 {
                    0..=4 => {
                        let (s, key, vel) = ((r() % 3) as usize, (r() % 128) as u8, (r() % 128) as f32);
                        next += 1;
                        *started.entry(next).or_default() += 1;
                        if timed {
                            event(
                                s,
                                Kind::On {
                                    id: next,
                                    key,
                                    velocity: vel,
                                },
                            );
                        } else {
                            a.note_on(s, next, key, vel, &mut |n| record(n, &mut ended));
                            b.note_on(s, next, key, vel, &mut |_| {});
                        }
                        held.push((s, next));
                    }
                    5..=7 if !held.is_empty() => {
                        let (s, id) = held.swap_remove(r() as usize % held.len());
                        if timed {
                            event(s, Kind::Off { id });
                        } else {
                            a.note_off(s, id);
                            b.note_off(s, id);
                        }
                    }
                    8 => {
                        let (s, down) = ((r() % 3) as usize, r() % 2 == 0);
                        if timed {
                            event(s, Kind::Pedal { down });
                        } else {
                            a.pedal(s, down);
                            b.pedal(s, down);
                        }
                    }
                    _ => {
                        let mut lim = [Limits::default(); SOUNDS];
                        for l in lim.iter_mut() {
                            *l = Limits {
                                reserve: (r() % 4) as u8,
                                priority: (r() % 3) as u8,
                                limit: (r() % 12) as u8,
                            };
                        }
                        let n = (r() % 40) as usize;
                        a.set_limits(n, &lim);
                        b.set_limits(n, &lim);
                    }
                }
            }
            ev.sort_by_key(|e| e.frame);
            let (mut oa, mut ob) = ([0.0; BLOCK], [0.0; BLOCK]);
            a.render_events::<Native>(&mut oa, &ev, &mut |n| record(n, &mut ended));
            b.render_events::<scalar::F>(&mut ob, &ev, &mut |_| {});
            assert!(
                oa.iter().zip(&ob).all(|(x, y)| x.to_bits() == y.to_bits()),
                "block {block}: back ends differ"
            );
            assert!(oa.iter().all(|v| v.is_finite() && v.abs() < 2000.0), "block {block}");
            assert_eq!(a.active(), b.active());
        }
        // everything released: every note rings out and ends
        for s in 0..SOUNDS {
            a.pedal(s, false);
            a.choke(s);
        }
        for _ in 0..100 {
            a.render::<Native>(&mut [0.0; BLOCK], &mut |n| record(n, &mut ended));
        }
        assert_eq!(a.active(), 0);
        assert!(started.len() > 3000, "{} notes", started.len());
        assert!(ended.values().all(|&c| c == 1), "a note ended twice");
        assert_eq!(started, ended, "every note ends exactly once");
    }

    /// Hann-windowed magnitudes of harmonics 1..=n of `x` at `f0`.
    fn harmonics(x: &[f32], f0: f64, n: usize) -> Vec<f64> {
        (1..=n).map(|h| magnitude(x, f0 * h as f64)).collect()
    }

    /// A lone carrier's held part at key 48 (operator 1 on routing 1), and its frequency.
    fn held(extra: &str) -> (Vec<f32>, f64) {
        let x = play::<Native>(&carrier(1, extra), 48, 100.0, 1000, 170);
        (x[BLOCK * 20..].to_vec(), laws::steps_hz(laws::key_steps(48)))
    }

    /// Every harmonic form at every spread as a carrier: its first 16 harmonics against the measured
    /// amplitudes relative to a sine at the same level, and its loudness (rms) against the sine's
    /// by the measured difference: the level law holds across forms.
    #[test]
    fn forms_against_a_sine() {
        use crate::forms_data::{DC, HARMONIC, WINDOW};
        let (sine, f0) = held("");
        let unit = magnitude(&sine, f0);
        let rms = |x: &[f32]| (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt();
        let (mut worst, mut worst_rms) = (0.0f64, 0.0f64);
        for form in 1..=6 {
            for spread in 0..8 {
                let set = (form - 1) * 8 + spread;
                let (x, _) = held(&format!("op1.form={form} op1.spread={spread} op1.formant=49"));
                let got = harmonics(&x, f0, 66);
                // (harmonic, measured amplitude) and the power the data says the form has
                let (want, power): (Vec<(usize, f64)>, f64) = if form <= 4 {
                    let h = HARMONIC[set];
                    let dc = if form <= 2 { f64::from(DC[set]) } else { 0.0 };
                    let w = h
                        .iter()
                        .filter(|c| c.0 <= 16)
                        .map(|c| (usize::from(c.0), f64::from(c.1)))
                        .collect();
                    (w, dc * dc + h.iter().map(|c| f64::from(c.1).powi(2) / 2.0).sum::<f64>())
                } else {
                    // the window times a sine at harmonic 50: a cos(d t + p) sin(50 t) is
                    // a/2 (sin((50 + d) t + p) + sin((50 - d) t - p)), folded below harmonic 1
                    let w = WINDOW[set - 32];
                    let mut z = [(0.0f64, 0.0f64); 160];
                    let mut put = |n: i32, a: f64, p: f64| {
                        let (a, p) = if n < 0 { (-a, -p) } else { (a, p) };
                        let e = &mut z[n.unsigned_abs() as usize];
                        *e = (e.0 + a * p.cos(), e.1 + a * p.sin());
                    };
                    for &(d, a, p) in w {
                        let (d, a, p) = (i32::from(d), f64::from(a), f64::from(p));
                        if d == 0 {
                            put(50, a, p);
                        } else {
                            put(50 + d, a / 2.0, p);
                            put(50 - d, a / 2.0, -p);
                        }
                    }
                    let v = (35..=65).map(|h| (h, z[h].0.hypot(z[h].1))).collect();
                    let p = (1..160).map(|h| z[h].0.hypot(z[h].1).powi(2) / 2.0).sum();
                    (v, p)
                };
                let top = want.iter().fold(0.0f64, |m, w| m.max(w.1));
                for &(h, a) in want.iter().filter(|w| w.1 >= top * 0.01) {
                    let db = 20.0 * (got[h - 1] / unit / a).log10();
                    assert!(db.abs() < 0.06, "form {form} spread {spread} harmonic {h}: {db:.3} dB");
                    worst = worst.max(db.abs());
                }
                let db = 20.0 * (rms(&x) / rms(&sine) / (2.0 * power).sqrt()).log10();
                assert!(
                    db.abs() < 0.05,
                    "form {form} spread {spread}: loudness {db:.3} dB from the measured"
                );
                worst_rms = worst_rms.max(db.abs());
            }
        }
        println!("forms against a sine: harmonics within {worst:.3} dB, loudness within {worst_rms:.3} dB");
    }

    /// The resonant forms' centre is harmonic formant + 1 at formants 9-99 at spreads 0 and 3, 18-99
    /// at spread 7 (lower, the window folds back below harmonic 1: `resonant_fold_back`),
    /// and their sidebands give the window's width (`forms_against_a_sine`).
    #[test]
    fn resonant_centre_follows_the_formant() {
        for form in [5, 6] {
            for spread in [0, 3, 7] {
                for formant in (if spread == 7 { 18 } else { 9 }..=99).step_by(9) {
                    let (x, f0) = held(&format!("op1.form={form} op1.spread={spread} op1.formant={formant}"));
                    let got = harmonics(&x, f0, 110);
                    let strongest = (0..got.len()).fold(0, |m, i| if got[i] > got[m] { i } else { m }) + 1;
                    assert_eq!(strongest, formant + 1, "form {form} spread {spread} formant {formant}");
                }
            }
        }
    }

    /// The reference's renders: resonant 2 at spread 5, formant 0: harmonic 15, where the window's
    /// components below harmonic 1 fold back, -75.75 dB against a sine (the existing profile 1.5 dB).
    #[test]
    fn resonant_fold_back() {
        let (sine, f0) = held("");
        let (x, _) = held("op1.form=6 op1.spread=5");
        let db = 20.0 * (magnitude(&x, 15.0 * f0) / magnitude(&sine, f0)).log10();
        assert!((db + 75.75).abs() < 1.5, "{db:.2} dB");
    }

    /// A resonant form whose centre is at or above Nyquist is silent (a known difference: the
    /// reference plays a tone near Nyquist there).
    #[test]
    fn resonant_above_nyquist_is_silent() {
        let below = play::<Native>(&carrier(1, "op1.form=5 op1.formant=99"), 48, 100.0, 1000, 20);
        let above = play::<Native>(&carrier(1, "op1.form=5 op1.formant=99"), 60, 100.0, 1000, 20);
        assert!(peak_db(&below) > -30.0 && above.iter().all(|&v| v == 0.0));
    }

    /// Non-sine carriers modulated by a sine, against the reference's renders (note 48, operator 1
    /// a sine at ratio 1 and level 65 modulating operator 2): harmonics 1-8 relative to the
    /// strongest within 1 dB where the reference's are within 40 dB (the existing profile's gate).
    #[test]
    fn modulated_forms_against_the_reference() {
        let cases = [
            (2, 4, 0, [0.0, -5.2, -16.5, -30.9, -28.1, -42.3, -47.0, -55.2]),
            (4, 2, 0, [0.0, -6.4, -6.2, -6.1, -31.0, -13.5, -21.7, -32.7]),
            (5, 0, 5, [-89.3, -67.1, -45.6, -26.4, -2.7, 0.0, -24.6, -15.2]),
        ];
        for (form, spread, formant, reference) in cases {
            let p = sound(&format!(
                "algorithm=7\nop1.level=65\nop2.level=99 op2.form={form} op2.spread={spread} op2.formant={formant}\nop8.level=0"
            ));
            assert_eq!(p.plan.live, 3);
            let x = play::<Native>(&p, 48, 100.0, 1000, 170);
            let h = harmonics(&x[BLOCK * 20..], laws::steps_hz(laws::key_steps(48)), 8);
            let top = h.iter().fold(0.0f64, |m, &v| m.max(v));
            for (k, &r) in reference.iter().enumerate().filter(|(_, &r)| r > -40.0) {
                let db = 20.0 * (h[k] / top).log10();
                assert!(
                    (db - r).abs() < 1.0,
                    "form {form} harmonic {}: {db:.2} dB, the reference {r}",
                    k + 1
                );
            }
        }
    }

    /// The first-level data cache is 32 KB: a lane at work (its state, the step's scratch and the
    /// sine table) must leave half of it for the rest of the block path, and fit with eight form
    /// tables.
    #[test]
    fn hot_set_fits_half_the_l1() {
        use core::mem::size_of;
        let scratch = size_of::<[[[f32; W]; STEP]; OPS]>() // operator outputs, four-wide
            + size_of::<[[[f32; 1]; STEP]; OPS]>() // and one-wide
            + size_of::<[[f32; W]; STEP]>() // modulation input
            + 4 * size_of::<[[f32; W]; OPS]>() // gains, ramps, falls and rises
            + size_of::<[[f32; W]; STEP]>(); // the lane's output in a step
        let hot = size_of::<Lane>() + scratch + size_of::<crate::tables::Table>();
        assert!(hot <= 16 * 1024, "{hot} B");
        // eight operators on eight different form tables: within the first-level cache by size
        // (conflicts measured by tools/cost/cg.sh, cache-mixed-N)
        let forms = hot + 8 * size_of::<crate::tables::Table16>();
        assert!(forms <= 28 * 1024, "{forms} B with eight form tables");
        assert!(size_of::<Lane>() <= 2048, "{} B a lane", size_of::<Lane>());
    }

    /// A hash of the output of six notes on routings 12 and 80 with feedback, every operator,
    /// released halfway; `forms` adds each operator's form, spread and formant line.
    fn golden(forms: impl Fn(i32) -> String) -> u64 {
        let mut h = 0u64;
        for r in [12, 80] {
            let text: String = (1..=8)
                .map(|o| {
                    format!(
                        "op{o}.level={} op{o}.coarse={o} op{o}.detune={} {}\n",
                        60 + 4 * o,
                        o - 4,
                        forms(o)
                    )
                })
                .collect();
            let p = sound(&format!("algorithm={r} feedback=6\n{text}"));
            let mut e = engine(&p);
            for v in 0..6 {
                e.note_on(0, v, 40 + 7 * v as u8, 30.0 + 15.0 * v as f32, &mut |_| {});
            }
            for b in 0..24 {
                if b == 12 {
                    (0..6).for_each(|v| e.note_off(0, v));
                }
                let mut out = [0.0; BLOCK];
                e.render::<Native>(&mut out, &mut |_| {});
                h = out.iter().fold(h, |h, x| h.rotate_left(5) ^ u64::from(x.to_bits()));
            }
        }
        h
    }

    /// The same bits on every target and back end (checked on every CI target), sines and forms.
    #[test]
    fn golden_render() {
        let h = golden(|_| String::new());
        assert_eq!(h, GOLDEN, "render hash {h:#x}");
        let h = golden(|o| format!("op{o}.form={} op{o}.spread={} op{o}.formant={}", o % 7, o % 8, 5 * o));
        assert_eq!(h, GOLDEN_FORMS, "render hash with forms {h:#x}");
    }

    /// A voice's state, read from whichever slot holds note `id`.
    fn voice(e: &Engine, id: u32) -> Vec<u32> {
        let (l, s) = e
            .lanes
            .iter()
            .find_map(|l| (0..W).find(|&s| l.used >> s & 1 != 0 && l.id[s] == id).map(|s| (l, s)))
            .expect("the voice sounds");
        let mut v: Vec<u32> = (0..OPS)
            .flat_map(|o| [l.ph[o][s], l.inc[o][s], l.gain[o][s].to_bits(), l.lvl[o][s].to_bits()])
            .collect();
        v.extend([l.fb[0][s].to_bits(), l.fb[1][s].to_bits(), l.bound[s].to_bits()]);
        v
    }

    /// Note 0 (key 36) played alone, in a pair, in a full lane, and while neighbours (key 96, fast
    /// release) start and end around it, so it moves to another slot and between the narrow and
    /// four-wide paths: its state after every block, and the output, are the same bits whichever
    /// path renders it. With `silent`, the neighbours are silent (a resonant form centred above
    /// Nyquist), so the output is the voice's alone.
    fn lane_widths(name: &str, forms: impl Fn(i32) -> &'static str, silent: bool) {
        let text: String = (1..=8)
            .map(|o| {
                format!(
                    "op{o}.detune={} {} op{o}.eg_time_release=0 op{o}.level=90\n",
                    o - 4,
                    forms(o)
                )
                .replace('#', &o.to_string())
            })
            .collect();
        let p = sound(&format!("algorithm=12 feedback=6\n{text}"));
        // (block, note id, on): note 0 is the voice followed
        let scenes: [&[(usize, u32, bool)]; 4] = [
            &[(0, 0, true)],
            &[(0, 1, true), (0, 0, true)],
            &[(0, 1, true), (0, 2, true), (0, 0, true), (0, 3, true)],
            &[
                (0, 1, true),
                (0, 0, true),
                (0, 2, true),
                (2, 1, false),
                (2, 2, false),
                (8, 3, true),
                (12, 3, false),
                (13, 4, true),
                (14, 5, true),
                (17, 4, false),
            ],
        ];
        let mut want: Option<(Vec<Vec<u32>>, Vec<f32>)> = None;
        let mut widths = Vec::new();
        for scene in scenes {
            let mut e = engine(&p);
            let (mut states, mut out) = (Vec::new(), Vec::new());
            for b in 0..24 {
                for &(_, id, on) in scene.iter().filter(|x| x.0 == b) {
                    if on {
                        e.note_on(0, id, if id == 0 { 36 } else { 96 }, 100.0, &mut |_| {});
                    } else {
                        e.note_off(0, id);
                    }
                }
                let mut o = [0.0; BLOCK];
                e.render::<Native>(&mut o, &mut |_| {});
                out.extend_from_slice(&o);
                states.push(voice(&e, 0));
                widths.push(e.lanes[0].used);
            }
            assert!(out.iter().any(|x| x.abs() > 1e-3), "{name}: the voice sounds");
            match &want {
                None => want = Some((states, out)),
                Some((s, o)) => {
                    assert_eq!(&states, s, "voice state, {name}");
                    if silent {
                        assert!(
                            out.iter().zip(o).all(|(a, b)| a.to_bits() == b.to_bits()),
                            "output, {name}"
                        );
                    }
                }
            }
        }
        // the moving scene went narrow (the voice alone, moved to the first slot) and back
        let moving = &widths[72..];
        assert!(
            moving.contains(&1) && moving.contains(&3) && moving.contains(&7),
            "{moving:?}"
        );
    }

    #[test]
    fn a_voice_is_the_same_at_every_lane_width() {
        lane_widths("sines", |_| "", false);
        lane_widths("one resonant", |o| if o == 1 { "op#.form=5" } else { "" }, false);
        // every operator resonant, formant 60: the neighbours' centres are above Nyquist
        lane_widths("resonant", |_| "op#.form=5 op#.formant=60", true);
    }

    fn on(frame: usize, sound: u8, id: u32, key: u8) -> Event {
        let kind = Kind::On {
            id,
            key,
            velocity: 100.0,
        };
        Event { frame, sound, kind }
    }

    /// A note starts on its frame: silence before it, then, for its whole steps in the block, the
    /// same bits as the note started at a block's start.
    #[test]
    fn a_note_starts_on_its_frame() {
        let p = carrier(12, "");
        let mut a = engine(&p);
        let mut x = [0.0; BLOCK];
        a.render_events::<Native>(&mut x, &[on(37, 0, 1, 60)], &mut |_| {});
        let mut b = engine(&p);
        let mut y = [0.0; BLOCK];
        b.render_events::<Native>(&mut y, &[on(0, 0, 1, 60)], &mut |_| {});
        assert!(x[..38].iter().all(|&v| v == 0.0) && x[38] != 0.0);
        let whole = (BLOCK - 37) / STEP * STEP;
        assert!(x[37..37 + whole]
            .iter()
            .zip(&y)
            .all(|(a, b)| a.to_bits() == b.to_bits()));
        // and the step cut short at the block's end leaves the note where 128 - 37 frames take it
        let (mut c, mut z) = (engine(&p), [0.0; BLOCK]);
        c.render_events::<Native>(&mut z, &[on(0, 0, 1, 60)], &mut |_| {});
        assert!(x[37..].iter().zip(&z).all(|(a, b)| (a - b).abs() < 1e-3));
    }

    /// Two notes of one sound (keys 60 then `to`, the first released first when `release`): the
    /// second's pitch (cents against its own) at every block's end, and the blocks it moved in.
    fn glide_track(text: &str, to: u8, release: bool) -> (Vec<f64>, usize) {
        let mut e = engine(&carrier(1, text));
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        e.render::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        if release {
            e.note_off(0, 1);
        }
        e.note_on(0, 2, to, 100.0, &mut |_| {});
        let (mut track, mut moves) = (Vec::new(), 0);
        for _ in 0..200 {
            e.render::<Native>(&mut [0.0; BLOCK], &mut |_| {});
            let l = e.lanes.iter().find(|l| l.used != 0 && l.id.contains(&2)).unwrap();
            let s = l.id.iter().position(|&i| i == 2).unwrap();
            let c = 1200.0 * (f64::from(l.inc[0][s]) / f64::from(l.pt.base[0][s])).log2();
            moves += usize::from(track.last().is_some_and(|&x| x != c));
            track.push(c);
        }
        (track, moves)
    }

    /// Portamento: straight in semitones from the sound's latest note, in steps of the glide clock
    /// (9.977 ms) at the time value's rate; rate 1 keeps the rate, time 1 the time; rate 2 starts
    /// at most an octave away; fingered glides only from a held key; off or time 0, no glide.
    #[test]
    fn portamento() {
        let block_ms = BLOCK as f64 / RATE * 1000.0;
        let ends = |t: &[f64]| t.iter().position(|&c| c == 0.0).unwrap() as f64 * block_ms;
        let rate = laws::portamento_rate(64, 0, 12.0); // semitones a second
        let (t, moves) = glide_track("portamento=1 portamento_mode=1", 72, true);
        // from an octave down; 12 / rate seconds in whole steps of the clock
        assert!((t[0] + 1200.0).abs() < 40.0, "{}", t[0]);
        let want = (12.0 / (rate * laws::GLIDE_TICK_MS / 1000.0)).ceil() * laws::GLIDE_TICK_MS;
        assert!(
            (ends(&t) - want).abs() < 2.0 * laws::GLIDE_TICK_MS,
            "{} ms against {want}",
            ends(&t)
        );
        assert!(
            moves as f64 <= want / laws::GLIDE_TICK_MS + 1.0,
            "{moves} moves: steps, not a slide"
        );
        // each step the rate's share of a tick
        let step = rate * laws::GLIDE_TICK_MS / 1000.0 * 100.0;
        let diffs: Vec<f64> = t.windows(2).map(|w| w[1] - w[0]).filter(|&d| d > 0.0).collect();
        assert!(
            diffs.iter().take(diffs.len() - 1).all(|&d| (d - step).abs() < 0.5),
            "{diffs:?}"
        );
        // time 1: twice the distance in the same time; rate 2: from an octave at most
        let (t1, _) = glide_track("portamento=1 portamento_mode=1 portamento_time_mode=1", 84, true);
        assert!(
            (ends(&t1) - want).abs() < 2.0 * laws::GLIDE_TICK_MS,
            "time 1: {} ms",
            ends(&t1)
        );
        let (t2, _) = glide_track("portamento=1 portamento_mode=1 portamento_time_mode=2", 84, true);
        assert!((t2[0] + 1200.0).abs() < 40.0 && (ends(&t2) - want).abs() < 2.0 * laws::GLIDE_TICK_MS);
        // fingered: from a held key only
        assert!(glide_track("portamento=1 portamento_mode=0", 72, true).0[0] == 0.0);
        assert!(glide_track("portamento=1 portamento_mode=0", 72, false).0[0] < -1100.0);
        // off, or time 0
        assert!(glide_track("portamento=0 portamento_mode=1", 72, true).0[0] == 0.0);
        assert!(glide_track("portamento=1 portamento_mode=1 portamento_time=0", 72, true).0[0] == 0.0);
    }

    /// Pitch bend: from its event's frame on, every operator (fixed-mode ones too) at the law's
    /// steps for the sound's ranges; the frames before it the same bits as without it; back at 0,
    /// the note's own increments exactly.
    #[test]
    fn bend_on_its_frame() {
        let p = sound(
            "algorithm=1 bend_up=12 bend_down=-24\nop1.level=99\nop2.level=99 op2.mode=1 op2.coarse=15\nop8.level=0",
        );
        let (mut a, mut b) = (engine(&p), engine(&p));
        let (mut x, mut y) = ([0.0; BLOCK], [0.0; BLOCK]);
        for e in [&mut a, &mut b] {
            e.note_on(0, 1, 60, 100.0, &mut |_| {});
            e.render::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        }
        let bend = |frame, value| Event {
            frame,
            sound: 0,
            kind: Kind::Bend { value },
        };
        a.render_events::<Native>(&mut x, &[bend(50, -8192)], &mut |_| {});
        b.render::<Native>(&mut y, &mut |_| {});
        let same = x.iter().zip(&y).take_while(|(a, b)| a.to_bits() == b.to_bits()).count();
        assert_eq!(
            same, 51,
            "the bend reaches the output at frame 51 (frame 50's phase is set before it)"
        );
        let ratio = |e: &Engine, op: usize| f64::from(e.lanes[0].inc[op][0]) / f64::from(e.lanes[0].pt.base[op][0]);
        let want = (f64::from(laws::bend_steps(-8192, 12, -24)) / 2048.0).exp2();
        for op in 0..2 {
            assert!(
                (ratio(&a, op) / want - 1.0).abs() < 1e-4,
                "operator {op}: {}",
                ratio(&a, op)
            );
        }
        a.render_events::<Native>(&mut x, &[bend(0, 0)], &mut |_| {});
        assert_eq!(a.lanes[0].inc, b.lanes[0].inc);
    }

    /// A sound's events cut its own control steps only: another sound plays the same bits; a voice
    /// of the same sound plays on with its steps cut, close to its bits without the cut (the same
    /// bits while its envelopes stand still; moving, their steps' shares differ by rounding).
    #[test]
    fn events_split_their_own_sound_only() {
        let text: String = (1..=8)
            .map(|o| {
                format!(
                    "op{o}.form=5 op{o}.formant=60 op{o}.eg_level_decay1=0 op{o}.eg_time_decay1=60 op{o}.level=90\n"
                )
            })
            .collect();
        let p = sound(&format!("algorithm=12 feedback=6\n{text}"));
        let quiet = sound(&(1..=8).map(|o| format!("op{o}.level=0\n")).collect::<String>());
        let render = |events: &[Event], other: bool| {
            let mut e = engine(&p);
            assert!(e.set_sound(1, if other { &p } else { &quiet }));
            let mut x = Vec::new();
            for b in 0..12 {
                let mut o = [0.0; BLOCK];
                let ev: Vec<Event> = events
                    .iter()
                    .filter(|e| e.frame / BLOCK == b)
                    .map(|e| Event {
                        frame: e.frame % BLOCK,
                        ..*e
                    })
                    .collect();
                e.render_events::<Native>(&mut o, &ev, &mut |_| {});
                x.extend_from_slice(&o);
            }
            x
        };
        let base = [on(0, 0, 1, 36)];
        let alone = render(&base, false);
        // a silent sound's note at frame 3 * 128 + 37: sound 0's bits unchanged
        let other = render(&[base[0], on(421, 1, 2, 60)], false);
        assert!(alone.iter().zip(&other).all(|(a, b)| a.to_bits() == b.to_bits()));
        // a silent note of the same sound (centred above Nyquist) cuts the voice's steps
        let same = render(&[base[0], on(421, 0, 2, 96)], false);
        let peak = alone.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let diff = alone.iter().zip(&same).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(diff > 0.0 && diff < peak * 1e-3, "{diff} against a peak of {peak}");
    }

    const GOLDEN: u64 = 0xee81_582b_c559_a5fe;
    const GOLDEN_FORMS: u64 = 0x6308_0b08_ff84_df29;

    #[test]
    fn silence_in_silence_out() {
        let mut e = Engine::new(RATE);
        let mut out = [0.0; BLOCK];
        e.render::<Native>(&mut out, &mut |_| {});
        assert!(out.iter().all(|&x| x == 0.0));
    }
}
