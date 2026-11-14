// SPDX-License-Identifier: AGPL-3.0-or-later
//! The voice engine (docs/design.md, section 2): up to eight prepared sounds, a pool of lanes of
//! four voices each, a lane holding one sound's voices only; notes on and off, the operator
//! envelopes at control rate, sine operators on the routings, the end rule and whole-note shedding.
//! Fixed size; nothing here allocates, and the block path (`note_on`, `note_off`, `pedal`,
//! `render`) has no panic path (tools/check-no-panic.sh).

use crate::laws::{self, DOUBLINGS_PER_DB as DPD};
use crate::params::{op_index, Common, Op, Part, OPS};
use crate::plan::{Feedback, Plan};
use crate::routing::routing;
use crate::simd::{F4, U4, W};
use crate::sound::Sound;
use crate::tables::{offset, read, SINE};

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
}

impl Prepared {
    pub fn new(s: &Sound) -> Prepared {
        let r = routing(s.get(Common::Algorithm as usize));
        let feedback = i32::from(s.get(Common::Feedback as usize));
        let o = |op: usize, k: Op, i: usize| i32::from(s.get(op_index(op, k, i)));
        let mut p = Prepared {
            plan: Plan::new(&r, 0, false),
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
        p.plan = Plan::new(&r, audible, feedback > 0);
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
    /// Read only when a segment changes: the segment, steps left of the hold, each segment's rate.
    seg: [[u8; W]; OPS],
    count: [[u32; W]; OPS],
    rates: [[[f32; W]; 4]; OPS],
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
    rates: [[[0.0; W]; 4]; OPS],
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
    /// Enters segment `seg` of operator `op`'s envelope in slot `s`.
    fn enter(&mut self, p: &Prepared, op: usize, s: usize, seg: u8) {
        let top = get(&self.top, op, s);
        let full = top - (laws::ATTACK_OVER_DB * DPD) as f32;
        let k = usize::from(seg.saturating_sub(1)).min(3);
        let (mut tgt, mut fall) = (get(&self.lvl, op, s), 0.0);
        if (ATTACK..=RELEASE).contains(&seg) {
            tgt = (full + p.eg.get(op).and_then(|e| e.get(k)).copied().unwrap_or(FLOOR)).max(FLOOR);
            fall = self
                .rates
                .get(op)
                .and_then(|r| r.get(k))
                .and_then(|r| r.get(s))
                .copied()
                .unwrap_or(0.0);
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
        self.watch &= !(0x1111_1111u32.checked_shl(s as u32).unwrap_or(0));
    }

    fn release(&mut self, p: &Prepared, s: usize) {
        self.held &= !bit(s);
        self.sustained &= !bit(s);
        for op in (0..OPS).filter(|&op| has(u32::from(p.plan.live), op)) {
            self.enter(p, op, s, RELEASE);
        }
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
    rate: f64,
    shed_step: f32,
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
            rate,
            shed_step: (1.0 / steps) as f32,
        }
    }

    /// Installs a prepared sound in slot `sound`, which must have no voice sounding (see
    /// [`Engine::choke`]); whether it was installed.
    pub fn set_sound(&mut self, sound: usize, p: &Prepared) -> bool {
        let busy = self.lanes.iter().any(|l| l.used != 0 && usize::from(l.sound) == sound);
        match self.sounds.get_mut(sound) {
            Some(d) if !busy => {
                *d = *p;
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
        for op in 0..OPS {
            put(&mut lane.gain, op, s, 0.0);
            if !has(u32::from(p.plan.live), op) {
                put(&mut lane.seg, op, s, DONE);
                continue;
            }
            let at = |a: &[i8; OPS]| i32::from(a.get(op).copied().unwrap_or(0));
            // pitch: the grid's steps, then the frequency
            let fixed = has(u32::from(p.fixed), op);
            // (each term far inside the i32 range)
            let steps = p
                .steps
                .get(op)
                .copied()
                .unwrap_or(0)
                .wrapping_add(laws::detune_steps(at(&p.detune), key))
                .wrapping_add(if fixed { 0 } else { laws::key_steps(key) });
            let hz = laws::steps_hz(steps);
            let hz = if fixed { hz.min(laws::FIXED_MAX_HZ) } else { hz };
            put(&mut lane.inc, op, s, (hz / rate * 4_294_967_296.0) as u32);
            if has(u32::from(p.reset), op) {
                put(&mut lane.ph, op, s, 0);
            }
            // level and the envelope's rates for this key and velocity
            let full =
                p.full.get(op).copied().unwrap_or(FLOOR) + (laws::velocity_db(at(&p.level_velocity), v) * DPD) as f32;
            put(&mut lane.top, op, s, full + (laws::ATTACK_OVER_DB * DPD) as f32);
            let ks = laws::time_key_follow_steps(i32::from(p.time_key_follow.get(op).copied().unwrap_or(0)), key);
            for k in 0..4 {
                let ms = f64::from(p.ms.get(op).and_then(|m| m.get(k)).copied().unwrap_or(1.0));
                let from = i32::from(p.rate_step.get(op).and_then(|r| r.get(k)).copied().unwrap_or(0));
                let scale = laws::rate(from) / laws::rate(from.saturating_add(ks).clamp(0, 127));
                let r = STEP as f64 * 1000.0 / (ms * scale * rate) * DPD;
                if let Some(x) = lane
                    .rates
                    .get_mut(op)
                    .and_then(|r| r.get_mut(k))
                    .and_then(|r| r.get_mut(s))
                {
                    *x = r as f32;
                }
            }
            let start = (full + p.eg.get(op).map_or(FLOOR, |e| e[3])).max(FLOOR);
            put(&mut lane.lvl, op, s, start);
            let hold = f64::from(p.hold_ms.get(op).copied().unwrap_or(0.0)) * rate / 1000.0 / STEP as f64 + 0.5;
            put(&mut lane.count, op, s, hold as u32);
            lane.enter(&p, op, s, if hold as u32 > 0 { HOLD } else { ATTACK });
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
                        lane.release(&p, s);
                    }
                }
            }
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
                        lane.release(&p, s);
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
        while self.keeping(None) > self.limit && self.shed(None, None) {}
        for (sound, lim) in (0..SOUNDS as u8).zip(self.limits).filter(|(_, l)| l.limit > 0) {
            while self.keeping(Some(sound)) > usize::from(lim.limit) && self.shed(None, Some(sound)) {}
        }
        let shed_step = self.shed_step;
        for lane in self.lanes.iter_mut().filter(|l| l.used != 0) {
            let Some(p) = self.sounds.get(usize::from(lane.sound)) else {
                continue;
            };
            let mut vout = [[0.0f32; W]; BLOCK];
            for chunk in vout.chunks_exact_mut(STEP) {
                let Ok(chunk) = <&mut [[f32; W]; STEP]>::try_from(chunk) else {
                    continue;
                };
                if lane.used == 0 {
                    break;
                }
                let (g0, dg) = envelopes::<V>(lane, p, shed_step, end);
                ops::<V>(lane, p, &g0, &dg, chunk);
            }
            for (o, v) in out.iter_mut().zip(vout.iter()) {
                *o += (v[0] + v[1]) + (v[2] + v[3]);
            }
        }
    }
}

/// One control step of a lane's envelopes: returns each operator's gain at the step's start and
/// its ramp a frame; moves segments on, fades shed voices, and ends voices (the end rule, a fade's
/// end).
#[inline(always)]
fn envelopes<V: F4>(
    lane: &mut Lane,
    p: &Prepared,
    shed_step: f32,
    end: &mut impl FnMut(NoteEnd),
) -> ([[f32; W]; OPS], [[f32; W]; OPS]) {
    // shed voices fade linearly
    for (s, f) in lane.fade.iter_mut().enumerate() {
        if has(u32::from(lane.ending), s) {
            // (to 0 on the last step, whatever the rounding of the steps before)
            *f = if *f > shed_step * 1.5 { *f - shed_step } else { 0.0 };
        }
    }
    let fade = V::load(lane.fade) * V::splat(16.0); // 2^HEAD
    let mut g0 = [[0.0; W]; OPS];
    let mut dg = [[0.0; W]; OPS];
    let mut bound = V::splat(0.0);
    for i in (0..OPS).filter(|&i| has(u32::from(p.plan.live), i)) {
        let (Some(lv), Some(&t), Some(&fall), Some(&rise), Some(&top), Some(gain), Some(g), Some(d)) = (
            lane.lvl.get_mut(i),
            lane.tgt.get(i),
            lane.fall.get(i),
            lane.rise.get(i),
            lane.top.get(i),
            lane.gain.get_mut(i),
            g0.get_mut(i),
            dg.get_mut(i),
        ) else {
            continue;
        };
        let (l, t) = (V::load(*lv), V::load(t));
        let up = ((V::load(top) - l) * V::load(rise)).max(V::splat(MIN_RISE)) + l;
        let l2 = V::select(l.ge(t), (l - V::load(fall)).max(t), up.min(t));
        *lv = l2.store();
        let target = l2.exp2() * fade;
        *g = *gain;
        *d = ((target - V::load(*gain)) * V::splat(1.0 / STEP as f32)).store();
        *gain = target.store();
        if has(u32::from(p.plan.carriers), i) {
            bound = bound + l2.max(t).exp2();
        }
    }
    lane.bound = (bound * V::splat(16.0)).store();
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
                let c = get(&lane.count, op, s).saturating_sub(1);
                put(&mut lane.count, op, s, c);
                if c == 0 {
                    lane.enter(p, op, s, ATTACK);
                }
            }
            ATTACK | DECAY1 if reached => lane.enter(p, op, s, seg.saturating_add(1)),
            RELEASE if reached => lane.enter(p, op, s, DONE),
            _ => {}
        }
    }
    // the end rule (released voices only) and the end of a fade
    for s in 0..W {
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
fn ops<V: F4>(lane: &mut Lane, p: &Prepared, g0: &[[f32; W]; OPS], dg: &[[f32; W]; OPS], vout: &mut [[f32; W]; STEP]) {
    let plan = &p.plan;
    let mut y = [[[0.0f32; W]; STEP]; OPS];
    let (lo, hi) = match plan.feedback {
        Feedback::Loop { to, from } => (usize::from(to), usize::from(from)),
        _ => (OPS, OPS),
    };
    for i in (0..OPS).filter(|&i| has(u32::from(plan.live), i)) {
        if i == lo {
            interleaved::<V>(lane, p, g0, dg, &mut y, lo, hi);
        } else if i < lo || i > hi {
            single::<V>(lane, p, g0, dg, &mut y, i);
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

/// Operator `i` over a whole step: its modulation input summed first, then its samples.
fn single<V: F4>(
    lane: &mut Lane,
    p: &Prepared,
    g0: &[[f32; W]; OPS],
    dg: &[[f32; W]; OPS],
    y: &mut [[[f32; W]; STEP]; OPS],
    i: usize,
) {
    let Some((done, rest)) = y.split_at_mut_checked(i) else {
        return;
    };
    let Some((cur, _)) = rest.split_first_mut() else { return };
    let (Some(ph), Some(&inc), Some(&g), Some(&d), Some(&mods)) = (
        lane.ph.get_mut(i),
        lane.inc.get(i),
        g0.get(i),
        dg.get(i),
        p.plan.mods.get(i),
    ) else {
        return;
    };
    let mut input = [[0.0f32; W]; STEP];
    for (j, src) in done.iter().enumerate() {
        if has(u32::from(mods), j) {
            for (m, s) in input.iter_mut().zip(src.iter()) {
                *m = (V::load(*m) + V::load(*s)).store();
            }
        }
    }
    let (inc, d) = (V::U::load(inc), V::load(d));
    let (mut ph_v, mut g) = (V::U::load(*ph), V::load(g));
    if p.plan.feedback == Feedback::Own(i as u8) {
        // self-feedback: sample by sample within this operator
        let fbk = V::splat(p.fbk);
        let (mut y1, mut y2) = (V::load(lane.fb[0]), V::load(lane.fb[1]));
        for (out, m) in cur.iter_mut().zip(input.iter()) {
            let v = read::<V>(&SINE, ph_v.add(offset((y1 + y2).mul_add(fbk, V::load(*m))))) * g;
            *out = v.store();
            (y2, y1) = (y1, v);
            g = g + d;
            ph_v = ph_v.add(inc);
        }
        lane.fb = [y1.store(), y2.store()];
    } else {
        for (out, m) in cur.iter_mut().zip(input.iter()) {
            *out = (read::<V>(&SINE, ph_v.add(offset(V::load(*m)))) * g).store();
            g = g + d;
            ph_v = ph_v.add(inc);
        }
    }
    *ph = ph_v.store();
}

/// Operators `lo..=hi` (the live ones) sample by sample: each sample's input reads the outputs
/// already written for that sample (sources below `lo` were written for the whole step before).
fn interleaved<V: F4>(
    lane: &mut Lane,
    p: &Prepared,
    g0: &[[f32; W]; OPS],
    dg: &[[f32; W]; OPS],
    y: &mut [[[f32; W]; STEP]; OPS],
    lo: usize,
    hi: usize,
) {
    let plan = &p.plan;
    let mut ph = lane.ph.map(V::U::load);
    let inc = lane.inc.map(V::U::load);
    let mut g = g0.map(V::load);
    let dg = dg.map(V::load);
    let fbk = V::splat(p.fbk);
    let (mut y1, mut y2) = (V::load(lane.fb[0]), V::load(lane.fb[1]));
    for s in 0..STEP {
        for k in (lo..=hi).filter(|&k| has(u32::from(plan.live), k)) {
            let mods = plan.mods.get(k).copied().unwrap_or(0);
            let mut m = V::splat(0.0);
            for (j, yj) in y.iter().enumerate().take(k) {
                if has(u32::from(mods), j) {
                    m = m + V::load(yj.get(s).copied().unwrap_or_default());
                }
            }
            if k == lo {
                m = (y1 + y2).mul_add(fbk, m);
            }
            let (Some(pk), Some(&inck), Some(gk), Some(&dgk)) = (ph.get_mut(k), inc.get(k), g.get_mut(k), dg.get(k))
            else {
                continue;
            };
            let v = read::<V>(&SINE, pk.add(offset(m))) * *gk;
            *gk = *gk + dgk;
            *pk = pk.add(inck);
            if k == hi {
                (y2, y1) = (y1, v);
            }
            if let Some(out) = y.get_mut(k).and_then(|o| o.get_mut(s)) {
                *out = v.store();
            }
        }
    }
    for k in lo..=hi {
        if let (Some(d), Some(v)) = (lane.ph.get_mut(k), ph.get(k)) {
            *d = v.store();
        }
    }
    lane.fb = [y1.store(), y2.store()];
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
            let op_played = p.op.is_some() && ((40..=44).contains(&k) || (54..=64).contains(&k) || k == 70);
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
            for _ in 0..r() % 6 {
                match r() % 10 {
                    0..=4 => {
                        let (s, key, vel) = ((r() % 3) as usize, (r() % 128) as u8, (r() % 128) as f32);
                        next += 1;
                        *started.entry(next).or_default() += 1;
                        a.note_on(s, next, key, vel, &mut |n| record(n, &mut ended));
                        b.note_on(s, next, key, vel, &mut |_| {});
                        held.push((s, next));
                    }
                    5..=7 if !held.is_empty() => {
                        let (s, id) = held.swap_remove(r() as usize % held.len());
                        a.note_off(s, id);
                        b.note_off(s, id);
                    }
                    8 => {
                        let (s, down) = ((r() % 3) as usize, r() % 2 == 0);
                        a.pedal(s, down);
                        b.pedal(s, down);
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
            let (mut oa, mut ob) = ([0.0; BLOCK], [0.0; BLOCK]);
            a.render::<Native>(&mut oa, &mut |n| record(n, &mut ended));
            b.render::<scalar::F>(&mut ob, &mut |_| {});
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

    /// The first-level data cache is 32 KB: a lane at work (its state, the step's scratch and the
    /// sine table) must leave half of it for the rest of the block path.
    #[test]
    fn hot_set_fits_half_the_l1() {
        use core::mem::size_of;
        let scratch = size_of::<[[[f32; W]; STEP]; OPS]>() // operator outputs
            + size_of::<[[f32; W]; STEP]>() // modulation input
            + 2 * size_of::<[[f32; W]; OPS]>() // gains and ramps
            + size_of::<[[f32; W]; BLOCK]>(); // the lane's output
        let hot = size_of::<Lane>() + scratch + size_of::<crate::tables::Table>();
        assert!(hot <= 16 * 1024, "{hot} B");
        assert!(size_of::<Lane>() <= 2048, "{} B a lane", size_of::<Lane>());
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = Engine::new(RATE);
        let mut out = [0.0; BLOCK];
        e.render::<Native>(&mut out, &mut |_| {});
        assert!(out.iter().all(|&x| x == 0.0));
    }
}
