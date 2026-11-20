// SPDX-License-Identifier: AGPL-3.0-or-later
//! The voice engine (docs/design.md, section 2): up to eight prepared sounds, a pool of lanes of
//! four voices each, a lane holding one sound's voices only and rendered four-wide, or one-wide
//! while it holds one voice; notes on and off, the operator
//! envelopes at control rate, sine operators on the routings, the end rule and whole-note shedding.
//! Fixed size; nothing here allocates, and the block path (`note_on`, `note_off`, `pedal`,
//! `render`) has no panic path (tools/check-no-panic.sh).

use crate::boxes::{self, SOURCES};
use crate::filter::{Laws, Voices, VOICES0};
use crate::forms::Wave;
use crate::laws::{self, DOUBLINGS_PER_DB as DPD};
use crate::params::{index_of, op_index, slfo_box_index, Common, Op, Part, OPS};
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
// (the lanes in use are a bit each of a u64)
const _: () = assert!(LANES <= 64);

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
/// A retriggered note (mono without portamento, a key played again with key assign single) fades
/// out over this long, by the square of the share left.
const RETRIGGER_MS: f64 = 13.3;
/// The keys a mono sound keeps to return to (more: the oldest is let go).
const KEYS: usize = 16;
/// Notes an instance keeps waiting out a key-on delay (more: a note ends at once).
const DELAYED: usize = 32;

/// Takes key `i` out of a mono sound's kept keys, the later ones moving down.
fn remove(keys: &mut [Held; KEYS], i: usize) {
    for j in i..KEYS {
        let next = keys.get(j.wrapping_add(1)).copied().unwrap_or_default();
        if let Some(k) = keys.get_mut(j) {
            *k = next;
        }
    }
}

/// A mono sound's key still down whose note another took over.
#[derive(Clone, Copy, Default)]
struct Held {
    id: u32,
    key: u8,
    velocity: f32,
}

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
    /// [`Engine::param`]: a parameter's raw value (its id in the sound format)
    Param { id: u16, value: i16 },
    /// [`Engine::choke_note`]
    Choke { id: u32 },
    /// [`Engine::control`]: a controller (its MIDI number, or [`PRESSURE`]) at a value 0-127
    Control { cc: u8, value: u8 },
    /// [`Engine::volume`]: a note's volume, linear
    Volume { id: u32, gain: f32 },
}

/// The controller number [`Engine::control`] takes for channel or key pressure (aftertouch).
pub const PRESSURE: u8 = 128;

/// The channel controllers a sound keeps, by their place in [`Engine`]'s `ctl`: the mod wheel
/// (CC 1), aftertouch, foot controller 1 (CC 11), foot controller 2 (CC 4), breath (CC 2) and
/// expression (CC 11 too), and their values at rest.
const CTLS: usize = 6;
const EXPRESSION: usize = 5;
const CTL_REST: [u8; CTLS] = [0, 0, 0, 0, 0, 127];

/// What a sound's controllers do to it now, made again when one moves (or the sound is
/// prepared) and read every control step: whether anything moves, the output gain (expression);
/// from the controller boxes (docs/design.md, 3.9) each operator's level against the level it was
/// prepared at (log2, at most 0) and whether any operator has one, the cutoff's values and the
/// resonance's steps, the voice LFO's pitch and amplitude depths (as [`Prepared::lfo_depths`])
/// and speed value, the sound's pitch and each operator's (octaves).
#[derive(Clone, Copy)]
struct Mods {
    on: bool,
    gain: f32,
    lvl_on: bool,
    lvl: [f32; OPS],
    cut: f32,
    res: f32,
    lfo_depths: (f32, f32),
    lfo_speed: i16,
    pitch: f32,
    op_pitch: [f32; OPS],
}

const MODS0: Mods = Mods {
    on: false,
    gain: 1.0,
    lvl_on: false,
    lvl: [0.0; OPS],
    cut: 0.0,
    res: 0.0,
    lfo_depths: (1.0, 1.0),
    lfo_speed: 0,
    pitch: 0.0,
    op_pitch: [0.0; OPS],
};

/// A sound's place in the voice budget's order (oscairn.voice-budget): voices it keeps from other
/// sounds' new notes, its priority (lower gives way first) and its own limit (0: none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub reserve: u8,
    pub priority: u8,
    pub limit: u8,
}

/// The channels a render wrote ([`Engine::render_buses`]), a bit an output pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub left: u16,
    pub right: u16,
}

/// A sound as the engine plays it: the laws applied once, off the audio thread.
#[derive(Clone, Copy)]
pub struct Prepared {
    /// The raw values it was prepared from (a parameter event changes one and prepares again).
    sound: Sound,
    pub plan: Plan,
    /// Feedback: the phase offset in cycles per unit of the source's last two outputs (as stored).
    fbk: f32,
    /// Per operator, log2 of its gain at envelope level 99 and velocity sensitivity 0, less HEAD.
    full: [f32; OPS],
    level_velocity: [i8; OPS],
    /// Level scaling's raw values (break point, low and high depths, low and high curves), the
    /// operators it acts on, and the most it may raise each level (log2: up to level 99's).
    scaling: [[u8; 5]; OPS],
    scaling_on: u8,
    cap: [f32; OPS],
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
    /// Mono (else poly), and key assign single (else multi).
    mono: bool,
    single: bool,
    /// The voice LFO: rate (Hz), its delay's hold (seconds; the fade in as long), wave, start
    /// phase, key-on reset; per operator its pitch swing at the peak (octaves; ratio-mode live
    /// operators) and its straight amplitude loss at the bottom (in the bend's steps); whether
    /// either reaches an operator.
    lfo_hz: f32,
    lfo_hold: f32,
    lfo_wave: u8,
    lfo_start: u32,
    lfo_reset: bool,
    lfo_pitch: [f32; OPS],
    lfo_amp: [f32; OPS],
    lfo_pitch_on: bool,
    lfo_amp_on: bool,
    /// What `lfo_pitch` and `lfo_amp` are multiplied by: 1, or where a box moves the depth, its
    /// stored depth (in the swing's units; straight loss steps) and they per unit of it.
    lfo_depths: (f32, f32),
    /// Whether the voice LFO reaches anything (pitch, amplitude, the cutoff).
    lfo_on: bool,
    /// The voice LFO's speed value and range (for the sound LFO's speed destination).
    lfo_speed: (bool, i16),
    /// The sound LFO, one a sound (docs/design.md, 3.6): what it moves (`SLFO_*` bits, 0: it does
    /// not run); rate (Hz); wave; start phase; key-on reset (off, each, first); delay, fade in,
    /// hold (infinite: for ever) and fade out (seconds); the user wave's steps, step count and
    /// slope. Per operator at the LFO's top: pitch (octaves; ratio-mode operators), detune steps
    /// (ratio-mode operators), form steps, and half the level lost at its bottom (log2); the
    /// feedback and voice LFO speed steps; the base feedback and each operator's form, spread and
    /// formant (the stepped destinations' starting points), and the feedback amount at 0-7.
    slfo: u8,
    slfo_hz: f32,
    slfo_wave: u8,
    slfo_start: u32,
    slfo_reset: u8,
    slfo_env: [f32; 4],
    slfo_user: [f32; 16],
    slfo_steps: u8,
    slfo_slope: u8,
    slfo_pitch: [f32; OPS],
    slfo_detune: [f32; OPS],
    slfo_form: [f32; OPS],
    slfo_amp: [f32; OPS],
    slfo_fb: f32,
    slfo_speed: f32,
    feedback: u8,
    forms: [(u8, u8, u8); OPS],
    slfo_fbk: [f32; 8],
    /// The filter, its envelope and pan (docs/design.md, 3.7).
    filter: Laws,
    /// Key-on delay, ms (0: none).
    delay_ms: f32,
    /// Each operator's level (with the depth macro) and the level it is prepared at (the top its
    /// controller boxes reach); whether the sound has a box the engine plays.
    levels: [(u8, u8); OPS],
    boxed: bool,
    /// Whether a box moves an operator's frequency, and each operator's pitch controller
    /// sensitivity / 7 (ratio-mode live operators).
    op_pitch_box: bool,
    op_sens: [f32; OPS],
}

/// What the sound LFO moves: pitch (frequency or detune), level, what the kernels play (form or
/// feedback), the voice LFO's speed, the filter or pan.
const SLFO_PITCH: u8 = 1;
const SLFO_AMP: u8 = 2;
const SLFO_KERN: u8 = 4;
const SLFO_SPEED: u8 = 8;
const SLFO_POST: u8 = 16;

/// The sound LFO's running state, one a sound: phase (2^32 a cycle), frames since its envelope
/// started, frames it still stands still after a reset, its random source and the sample-and-hold
/// value.
#[derive(Clone, Copy)]
struct Slfo {
    ph: u32,
    t: u32,
    wait: u32,
    rnd: u32,
    held: f32,
}

const SLFO0: Slfo = Slfo {
    ph: 0,
    t: 0,
    wait: 0,
    rnd: SEED,
    held: 0.0,
};

impl Prepared {
    pub fn new(s: &Sound) -> Prepared {
        let r = routing(s.get(Common::Algorithm as usize));
        let feedback = i32::from(s.get(Common::Feedback as usize));
        let o = |op: usize, k: Op, i: usize| i32::from(s.get(op_index(op, k, i)));
        // the sound LFO's destinations with depth (the reference's numbers: 70-78 this part's)
        let slfo_part = |i: usize| i32::from(s.get((Part::SlfoDest as usize).wrapping_add(i)));
        let dests = [0, 2, 4].map(|i| (slfo_part(i), slfo_part(i + 1)));
        let mut p = Prepared {
            sound: *s,
            plan: Plan::new(&r, 0, false, [Wave::Sine; OPS]),
            fbk: 0.0,
            full: [0.0; OPS],
            level_velocity: [0; OPS],
            scaling: [[0; 5]; OPS],
            scaling_on: 0,
            cap: [0.0; OPS],
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
            mono: s.get(Part::MonoPoly as usize) == 0,
            single: s.get(Part::KeyAssign as usize) == 0,
            lfo_hz: 0.0,
            lfo_hold: 0.0,
            lfo_wave: 0,
            lfo_start: 0,
            lfo_reset: false,
            lfo_pitch: [0.0; OPS],
            lfo_amp: [0.0; OPS],
            lfo_pitch_on: false,
            lfo_amp_on: false,
            lfo_depths: (1.0, 1.0),
            lfo_on: false,
            lfo_speed: (false, 0),
            slfo: 0,
            slfo_hz: 0.0,
            slfo_wave: 0,
            slfo_start: 0,
            slfo_reset: 0,
            slfo_env: [0.0; 4],
            slfo_user: [0.0; 16],
            slfo_steps: 16,
            slfo_slope: 0,
            slfo_pitch: [0.0; OPS],
            slfo_detune: [0.0; OPS],
            slfo_form: [0.0; OPS],
            slfo_amp: [0.0; OPS],
            slfo_fb: 0.0,
            slfo_speed: 0.0,
            feedback: feedback.clamp(0, 7) as u8,
            forms: [(0, 0, 0); OPS],
            slfo_fbk: [0.0; 8],
            filter: Laws::new(s, &dests),
            levels: [(0, 0); OPS],
            boxed: false,
            op_pitch_box: false,
            op_sens: [0.0; OPS],
            // shortcut: tempo sync plays the time (no tempo input; the coverage report names it)
            delay_ms: laws::key_on_delay_ms(i32::from(s.get(Common::KeyOnDelay as usize))) as f32,
        };
        let carrier_gain_db =
            laws::CARRIER_FULL_DBFS + laws::routing_gain_db(i32::from(s.get(Common::Algorithm as usize)));
        let c = |k: Common| i32::from(s.get(k as usize));
        let part = |k: Part| i32::from(s.get(k as usize));
        let attack_scale = laws::aeg_attack_scale(part(Part::AttackOffset));
        let (decay_off, release_off) = (
            laws::aeg_offset_steps(part(Part::DecayOffset)),
            laws::aeg_offset_steps(part(Part::ReleaseOffset)),
        );
        let sustain_off = laws::aeg_sustain_steps(part(Part::SustainOffset));
        let mut audible = 0u8;
        let mut forms = [(0, 0, 0); OPS];
        p.boxed = boxes::boxes(s).next().is_some();
        let up = if p.boxed { boxes::reach(s) } else { [0; OPS] };
        for op in 0..OPS {
            let bit = 1u8 << op;
            let carrier = r.carriers & bit != 0;
            let m = |k: Common| if carrier { 0 } else { c(k).clamp(-99, 99) };
            // the depth macro on modulators, divided by 1 + feedback on the operator that feeds back
            let fb_div = if usize::from(r.fb_from) == op {
                1 + feedback.max(0)
            } else {
                1
            };
            let level = (o(op, Op::Level, 0) + m(Common::MacroDepth) / fb_div).clamp(0, 99);
            // an operator that boxes can raise is prepared at the top they reach; the boxes' steps
            // lower it from there (docs/design.md, 3.9)
            let top = level.saturating_add(up.get(op).copied().unwrap_or(0)).min(99);
            if let Some(l) = p.levels.get_mut(op) {
                *l = (level as u8, top as u8);
            }
            let level = top;
            let (form, spread, formant) = (o(op, Op::Form, 0), o(op, Op::Spread, 0), o(op, Op::Formant, 0));
            let (form, spread, formant, texture_db) =
                laws::texture(c(Common::MacroTexture), !carrier, form, spread, formant)
                    .unwrap_or((form, spread, formant, 0.0));
            if let Some(f) = forms.get_mut(op) {
                *f = (form, spread, formant);
            }
            // a modulator's full deviation is 2 cycles (4 pi radians) at 0 dB
            let (db, base) = if carrier {
                (laws::carrier_level_db(level), carrier_gain_db * DPD)
            } else {
                (laws::modulator_level_db(level), 1.0)
            };
            let db = db + texture_db;
            let scaling = [0, 1, 2, 3, 4].map(|i| o(op, Op::Scaling, i) as u8);
            if scaling[1] | scaling[2] != 0 {
                p.scaling_on |= bit;
            }
            if let (Some(sc), Some(cap)) = (p.scaling.get_mut(op), p.cap.get_mut(op)) {
                // level scaling raises a level to level 99's at most
                (*sc, *cap) = (scaling, (-db * DPD) as f32);
            }
            let mut levels = [0, 1, 2, 3].map(|i| o(op, Op::EgLevel, i));
            levels[2] += sustain_off + m(Common::MacroSustain);
            let levels = levels.map(|l| l.clamp(0, 99));
            let mut times = [0, 1, 2, 3].map(|i| o(op, Op::EgTime, i));
            times[0] += m(Common::MacroAttack);
            times[1] += decay_off + m(Common::MacroDecay);
            times[2] += decay_off + m(Common::MacroDecay);
            times[3] += release_off + m(Common::MacroRelease);
            let times = times.map(|t| t.clamp(0, 99));
            let eg = levels.map(|l| {
                (if carrier {
                    laws::carrier_eg_db(l)
                } else {
                    laws::modulator_eg_db(l)
                }) * DPD
            });
            if db > laws::SILENT_DB && eg.iter().any(|&e| e > laws::SILENT_DB * DPD) {
                audible |= bit;
            }
            // the attack offset scales attack times within times 0-99's (raising them on carriers only)
            let attack = laws::attack_ms_per_db(times[0]) * if carrier { attack_scale } else { attack_scale.min(1.0) };
            let attack = attack.max(laws::attack_ms_per_db(0)).min(laws::attack_ms_per_db(99));
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
                    attack,
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
                    let ratio = laws::ratio(coarse, fine);
                    laws::ratio_steps(if carrier {
                        ratio
                    } else {
                        laws::shifted_ratio(ratio, c(Common::MacroRatio))
                    })
                };
                *d = o(op, Op::Detune, 0) as i8;
            }
            p.fixed |= if fixed { bit } else { 0 };
            p.reset |= if o(op, Op::KeyOnReset, 0) != 0 { bit } else { 0 };
        }
        let waves = forms.map(|(f, s, n)| Wave::new(f, s, n));
        p.forms = forms.map(|(f, s, n)| (f as u8, s as u8, n as u8));
        // (the feedback destination needs the feedback path, at amount 0 where the sound has none)
        let fb_dest = dests.iter().any(|&(x, depth)| x == 74 && depth > 0);
        p.plan = Plan::new(&r, audible, feedback > 0 || fb_dest, waves);
        // the voice LFO
        p.lfo_hz = laws::lfo_hz(c(Common::LfoSpeedRange) != 0, {
            if c(Common::LfoSpeedRange) != 0 {
                c(Common::LfoExtendedSpeed)
            } else {
                c(Common::LfoSpeed)
            }
        }) as f32;
        p.lfo_hold = laws::lfo_delay_s(c(Common::LfoDelay)) as f32;
        p.lfo_wave = c(Common::LfoWave).clamp(0, 5) as u8;
        p.lfo_start = (c(Common::LfoPhase).clamp(0, 3) as u32) << 30;
        p.lfo_reset = c(Common::LfoKeyOnReset) != 0;
        // a box on a depth: the operators' shares a unit of it, the stored depth in lfo_depths
        let (pm_box, am_box) = (
            boxes::moves(s, boxes::Dest::LfoPitch),
            boxes::moves(s, boxes::Dest::LfoAmp),
        );
        let amp = laws::lfo_amp_steps(c(Common::LfoAmpDepth));
        if pm_box {
            p.lfo_depths.0 = laws::lfo_pitch_d(c(Common::LfoPitchDepth)) as f32;
        }
        if am_box {
            p.lfo_depths.1 = amp as f32;
        }
        for op in (0..OPS).filter(|&op| has(u32::from(p.plan.live), op)) {
            let pitch = if has(u32::from(p.fixed), op) {
                0.0
            } else if pm_box {
                laws::lfo_pitch_offset(o(op, Op::LfoPitchOffset, 0)) / 32768.0
            } else {
                laws::lfo_pitch_cents(c(Common::LfoPitchDepth), o(op, Op::LfoPitchOffset, 0)) / 1200.0
            };
            let share = laws::lfo_amp_share(o(op, Op::LfoAmpOffset, 0));
            if let (Some(lp), Some(la)) = (p.lfo_pitch.get_mut(op), p.lfo_amp.get_mut(op)) {
                *lp = pitch as f32;
                *la = (if am_box { share } else { amp * share }) as f32;
            }
            if let Some(k) = p.op_sens.get_mut(op).filter(|_| !has(u32::from(p.fixed), op)) {
                *k = o(op, Op::PitchCtrlSens, 0).clamp(-7, 7) as f32 / 7.0;
            }
        }
        p.op_pitch_box = boxes::moves(s, boxes::Dest::OpPitch) && p.op_sens.iter().any(|&k| k != 0.0);
        p.lfo_pitch_on = p.lfo_pitch.iter().any(|&x| x != 0.0);
        p.lfo_amp_on = p.lfo_amp.iter().any(|&x| x != 0.0);
        p.lfo_on = p.lfo_pitch_on || p.lfo_amp_on || p.filter.uses_lfo();
        p.lfo_speed = (c(Common::LfoSpeedRange) != 0, s.get(Common::LfoSpeed as usize));
        if p.lfo_speed.0 {
            p.lfo_speed.1 = s.get(Common::LfoExtendedSpeed as usize);
        }
        // a carrier feeds back its measured share of a modulator's amount, of its signal before the
        // routing's output level
        let fb_carrier = has(u32::from(r.carriers), usize::from(r.fb_from));
        let fbk = |feedback: i32| {
            (laws::feedback_amount(feedback)
                * if fb_carrier {
                    laws::carrier_feedback_scale(feedback) * 2.0 / laws::db_to_gain(carrier_gain_db)
                } else {
                    1.0
                }) as f32
        };
        p.fbk = fbk(feedback);
        p.slfo_fbk = core::array::from_fn(|f| fbk(f as i32));
        p.prepare_slfo(s, &dests);
        p
    }

    /// The sound LFO's laws (docs/design.md, 3.6), from the sound `s` and its destinations
    /// (number, depth).
    fn prepare_slfo(&mut self, s: &Sound, dests: &[(i32, i32); 3]) {
        let part = |k: Part, i: usize| i32::from(s.get((k as usize).wrapping_add(i)));
        for (d, &(dest, depth)) in (0..3).zip(dests.iter()).filter(|(_, x)| x.1 > 0) {
            match dest {
                71 => self.slfo_speed += (f64::from(depth) * laws::SLFO_SPEED_PER_DEPTH) as f32,
                74 => self.slfo_fb += (f64::from(depth) * laws::SLFO_FEEDBACK_PER_DEPTH) as f32,
                75..=78 => {
                    let live = (0..OPS).filter(|&op| has(u32::from(self.plan.live), op));
                    for op in live.filter(|&op| s.get(slfo_box_index(op, d)) != 0) {
                        let ratio = s.get(op_index(op, Op::SlfoRatio, 0).wrapping_add(d));
                        let k = laws::slfo_depth(depth, i32::from(ratio));
                        let fixed = has(u32::from(self.fixed), op);
                        let (x, add) = match dest {
                            75 if !fixed => (&mut self.slfo_pitch, laws::slfo_pitch_cents(k) / 1200.0),
                            76 => (&mut self.slfo_form, f64::from(k) * laws::SLFO_FORM_PER_DEPTH),
                            77 if !fixed => (&mut self.slfo_detune, f64::from(k) * laws::SLFO_DETUNE_PER_DEPTH),
                            78 => (&mut self.slfo_amp, laws::slfo_level_db(k) * DPD / 2.0),
                            _ => continue,
                        };
                        if let Some(x) = x.get_mut(op) {
                            *x += add as f32;
                        }
                    }
                }
                // pan, cutoff and resonance (70, 72, 73) are the filter's (`Laws`); the effects'
                // parameters are not played (the coverage report names them)
                _ => {}
            }
        }
        let any = |a: &[f32; OPS]| a.iter().any(|&x| x != 0.0);
        self.slfo = if any(&self.slfo_pitch) || any(&self.slfo_detune) {
            SLFO_PITCH
        } else {
            0
        } | if any(&self.slfo_amp) { SLFO_AMP } else { 0 }
            | if any(&self.slfo_form) || self.slfo_fb != 0.0 {
                SLFO_KERN
            } else {
                0
            }
            | if self.slfo_speed != 0.0 && self.lfo_on {
                SLFO_SPEED
            } else {
                0
            }
            | if self.filter.uses_slfo() { SLFO_POST } else { 0 };
        // shortcut: tempo sync and random speed are not played (the coverage report names them)
        self.slfo_hz = laws::slfo_hz(part(Part::SlfoSpeed, 0)) as f32;
        self.slfo_wave = part(Part::SlfoWave, 0).clamp(0, 12) as u8;
        // phase 0, 90, 120, 180, 240, 270 degrees
        const PHASES: [u32; 6] = [0, 1 << 30, 0x5555_5555, 1 << 31, 0xaaaa_aaab, 3 << 30];
        self.slfo_start = PHASES
            .get(part(Part::SlfoPhase, 0).clamp(0, 5) as usize)
            .copied()
            .unwrap_or(0);
        self.slfo_reset = part(Part::SlfoKeyOnReset, 0).clamp(0, 2) as u8;
        self.slfo_env = core::array::from_fn(|i| laws::slfo_time_s(part(Part::SlfoTime, i)) as f32);
        if let (true, Some(h)) = (part(Part::SlfoTime, 2) >= 127, self.slfo_env.get_mut(2)) {
            *h = f32::INFINITY; // hold for ever
        }
        // the user wave: 2-16 steps, +63 playing as full scale
        const CYCLES: [u8; 7] = [2, 3, 4, 6, 8, 12, 16];
        self.slfo_steps = CYCLES
            .get(part(Part::SlfoUserCycle, 0).clamp(0, 6) as usize)
            .copied()
            .unwrap_or(16);
        self.slfo_slope = part(Part::SlfoUserSlope, 0).clamp(0, 3) as u8;
        self.slfo_user = core::array::from_fn(|i| {
            let v = part(Part::SlfoStep, i).clamp(-64, 63);
            (if v == 63 { 64 } else { v }) as f32 / 64.0
        });
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
    /// Each voice's gain on everything: 1, falling to 0 while it is shed; 0 in a free slot. While
    /// it fades, `left` is the share of the fade left (the gain, or its square for a retrigger).
    fade: [f32; W],
    left: [f32; W],
    /// Read only when a segment changes: the segment, frames left of the hold.
    seg: [[u8; W]; OPS],
    count: [[u32; W]; OPS],
    /// Envelopes that can change segment, bit 4 op + slot.
    watch: u32,
    /// Read every step (beside `watch`, so a lane whose pitch stands still touches nothing of
    /// `pt`): voices whose pitch moves; the bend (octaves) the increments hold; whether the
    /// increments are to be made again at the next step's start (something jumped).
    moving: u8,
    bend: f32,
    jump: bool,
    sound: u8,
    /// Slots in use; keys down; released keys the pedal holds; voices being shed; those fading as
    /// a retrigger (13.3 ms, the square of the share left); those whose note lives on without
    /// them (a mono key still down: its end is not theirs to report).
    used: u8,
    held: u8,
    sustained: u8,
    /// Voices whose keys were down when the sostenuto pedal went down (it holds them).
    latch: u8,
    ending: u8,
    retrig: u8,
    orphan: u8,
    key: [u8; W],
    /// The note's velocity as played (for a mono note taken over and returned to).
    vel: [f32; W],
    id: [u32; W],
    age: [u32; W],
    /// The carriers' gain bound at the last step: the end rule's measure, and loudness for shedding.
    bound: [f32; W],
    pt: Pitch,
    /// The voice LFO, read only when its sound has one: phase, frames since the note started,
    /// and the sample-and-hold value (a random source drawn again each cycle).
    lfo_ph: [u32; W],
    lfo_t: [u32; W],
    lfo_rnd: [u32; W],
    /// The sound LFO's detune destination, read only when its sound has one: per operator the
    /// detune whose move (octaves, per voice) `sdt` holds, made again when it changes or a voice
    /// starts.
    sdt_d: [i8; OPS],
    sdt: [[f32; W]; OPS],
    /// The filter and pan, read only when its sound has them.
    post: Voices,
    /// Each note's volume (its expression) and the output gain each voice had at the last step
    /// (with the sound's expression), read only while one is not 1.
    vol: [f32; W],
    og: [f32; W],
    vol_on: bool,
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
    left: [0.0; W],
    seg: [[DONE; W]; OPS],
    count: [[0; W]; OPS],
    watch: 0,
    sound: 0,
    used: 0,
    held: 0,
    sustained: 0,
    latch: 0,
    ending: 0,
    retrig: 0,
    orphan: 0,
    key: [0; W],
    vel: [0.0; W],
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
    },
    moving: 0,
    bend: 0.0,
    jump: false,
    lfo_ph: [0; W],
    lfo_t: [0; W],
    lfo_rnd: [0; W],
    sdt_d: [SDT_NONE; OPS],
    sdt: [[0.0; W]; OPS],
    post: VOICES0,
    vol: [1.0; W],
    og: [1.0; W],
    vol_on: false,
};

/// No sound LFO detune move held (`Lane::sdt_d`).
const SDT_NONE: i8 = i8::MIN;

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

fn bit16(i: usize) -> u16 {
    u32::try_from(i).ok().and_then(|i| 1u16.checked_shl(i)).unwrap_or(0)
}

fn bit64(i: usize) -> u64 {
    u32::try_from(i).ok().and_then(|i| 1u64.checked_shl(i)).unwrap_or(0)
}

/// The set bits of `m`, lowest first.
fn bits(mut m: u64) -> impl Iterator<Item = usize> {
    core::iter::from_fn(move || {
        let i = m.trailing_zeros() as usize;
        m &= m.wrapping_sub(1);
        (i < 64).then_some(i)
    })
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
        for m in [
            &mut self.used,
            &mut self.held,
            &mut self.sustained,
            &mut self.latch,
            &mut self.ending,
            &mut self.retrig,
            &mut self.orphan,
        ] {
            *m &= !b;
        }
        if let (Some(f), Some(l)) = (self.fade.get_mut(s), self.left.get_mut(s)) {
            (*f, *l) = (0.0, 0.0);
        }
        if let (Some(v), Some(g)) = (self.vol.get_mut(s), self.og.get_mut(s)) {
            (*v, *g) = (1.0, 1.0);
        }
        for op in 0..OPS {
            put(&mut self.gain, op, s, 0.0);
        }
        let ops = !slot_ops(s);
        (self.watch, self.pt.op_attack, self.pt.op_decay) =
            (self.watch & ops, self.pt.op_attack & ops, self.pt.op_decay & ops);
        self.moving &= !b;
        if let (Some(c), Some(g)) = (self.pt.c_seg.get_mut(s), self.pt.glide.get_mut(s)) {
            (*c, *g) = (DONE, 0.0);
        }
        self.post.clear(s);
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
        col(&mut self.sdt, from, to);
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
        for x in [
            &mut self.fade,
            &mut self.left,
            &mut self.vel,
            &mut self.bound,
            &mut self.vol,
            &mut self.og,
        ] {
            one(x, from, to);
        }
        one(&mut self.key, from, to);
        for x in [
            &mut self.id,
            &mut self.age,
            &mut self.lfo_ph,
            &mut self.lfo_t,
            &mut self.lfo_rnd,
        ] {
            one(x, from, to);
        }
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
        self.post.mv(from, to);
        let (f, t) = (bit(from), bit(to));
        for m in [
            &mut self.used,
            &mut self.held,
            &mut self.sustained,
            &mut self.latch,
            &mut self.ending,
            &mut self.retrig,
            &mut self.orphan,
            &mut self.moving,
        ] {
            *m = if *m & f != 0 { *m | t } else { *m & !t };
        }
        for m in [&mut self.watch, &mut pt.op_attack, &mut pt.op_decay] {
            let w = (*m & slot_ops(from)).checked_shr(from.wrapping_sub(to) as u32);
            *m = (*m & !slot_ops(to)) | w.unwrap_or(0);
        }
        self.free(from);
    }

    /// The output of a step of `n` frames times each voice's gain (its note's volume times
    /// `want`), ramped linearly from the last step's. (Not inlined: inlined, it cost a voice alone
    /// 0.9 % with no controller at work, the lane gate's margin.)
    #[inline(never)]
    fn out_gain<const L: usize, V: F4<L>>(&mut self, want: f32, n: usize, y: &mut [[f32; L]; STEP]) {
        let to = V::load4(&self.vol) * V::splat(want);
        let mut g = V::load4(&self.og);
        let d = (to - g) * V::splat(1.0 / n as f32);
        for x in y.iter_mut().take(n) {
            g = g + d;
            *x = (V::load(*x) * g).store();
        }
        to.store4(&mut self.og);
        self.vol_on = want != 1.0 || self.vol.iter().any(|&v| v != 1.0);
    }

    fn release(&mut self, p: &Prepared, s: usize, rate: f64) {
        self.held &= !bit(s);
        self.sustained &= !bit(s);
        for op in (0..OPS).filter(|&op| has(u32::from(p.plan.live), op)) {
            self.enter(p, op, s, RELEASE, rate);
        }
        self.post.release(&p.filter, s, rate);
        if p.peg_on {
            if let Some(c) = self.pt.c_seg.get_mut(s) {
                *c = RELEASE;
            }
            self.moving |= bit(s);
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
            self.moving |= bit(s);
        } else {
            self.moving &= !bit(s);
        }
        // the increments hold what moves from the next step on
        self.jump |= moves || self.bend != 0.0;
    }

    fn end(&mut self, s: usize, end: &mut impl FnMut(NoteEnd)) {
        if has(u32::from(self.used & !self.orphan), s) {
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

/// The voices a sound's importance report lists at most (oscairn.voice-budget draft 2).
pub const LEAST: usize = 16;
/// The voice budget's scale (RFC 0046, the same in every plugin): a voice struck this recently is
/// not judged by its level and goes last in its tier; below this level a voice counts as released;
/// a voice counts this much quieter a second of its age, up to this many seconds.
const STRUCK_MS: f64 = 50.0;
const INAUDIBLE_DB: f32 = -60.0;
const AGED_DB_A_SECOND: f64 = 1.0;
const AGED_SECONDS_MOST: f64 = 10.0;

/// A voice's place in the voice budget's order, as the platform SDK's helper works it out
/// (voice-budget-plugin.hpp): its tier (0 released or below -60 dBFS unless struck under 50 ms
/// ago, 1 held only by a pedal, 2 key held), whether it was struck under 50 ms ago, its level at
/// the output (dBFS: the carriers' envelope levels with velocity and output levels, the fade, the
/// note's volume and the expression; -120 to +24) and its age; then what it is ranked by within
/// its tier (its level less a dB a second of age, at most ten, then its note-on serial) and where
/// it is (lane, slot).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rank {
    pub tier: u8,
    pub struck: bool,
    pub db: f32,
    pub age_ms: f64,
    held: bool,
    audibility: f64,
    serial: u64,
    lane: u8,
    slot: u8,
}

const RANK0: Rank = Rank {
    tier: 0,
    struck: false,
    db: 0.0,
    age_ms: 0.0,
    held: false,
    audibility: 0.0,
    serial: 0,
    lane: 0,
    slot: 0,
};

impl Rank {
    /// A voice's rank from its key (`held`: down; `pedaled`: up, a pedal holding it), its gain at
    /// the output (linear), its age and its note-on serial.
    pub fn new(held: bool, pedaled: bool, gain: f64, age_ms: f64, serial: u64) -> Rank {
        let db = level_db(gain);
        let struck = age_ms < STRUCK_MS;
        let tier = if !struck && db < INAUDIBLE_DB {
            0
        } else if held {
            2
        } else {
            u8::from(pedaled)
        };
        let aged = (age_ms / 1000.0 * AGED_DB_A_SECOND).min(AGED_SECONDS_MOST * AGED_DB_A_SECOND);
        Rank {
            tier,
            struck,
            db,
            age_ms,
            held,
            audibility: f64::from(db) - if age_ms > 0.0 { aged } else { 0.0 },
            serial,
            lane: 0,
            slot: 0,
        }
    }

    /// Whether it goes before `b` across sounds, `other` (another sound's than the new note's) and
    /// `priority` being each one's: by tier, then the new note's own sound, then the lower
    /// priority, then [`Rank::before`].
    fn first(&self, other: bool, priority: u8, b: &Rank, b_other: bool, b_priority: u8) -> bool {
        if self.tier != b.tier {
            self.tier < b.tier
        } else if other != b_other {
            !other
        } else if priority != b_priority {
            priority < b_priority
        } else {
            self.before(b)
        }
    }

    /// Whether it goes before `b` within one sound: the lower tier, one not just struck, the less
    /// audible, the older.
    fn before(&self, b: &Rank) -> bool {
        if self.tier != b.tier {
            return self.tier < b.tier;
        }
        if self.struck != b.struck {
            return b.struck;
        }
        if self.audibility != b.audibility {
            self.audibility < b.audibility
        } else {
            self.serial < b.serial
        }
    }
}

/// What only the voice budget reads of a voice, kept beside the lanes so that rendering never
/// touches it: its note's serial (whole; its lane keeps the low half, `Lane::age`) and the frame
/// the note started at. A lane's four are found by that low half, since a lane moves its voices
/// between slots and these stay where they were written (shortcut: two notes 2^32 note-ons apart
/// in one lane would share it; a voice does not live that long).
#[derive(Clone, Copy, Default)]
struct Born {
    serial: u64,
    on: u64,
}

/// Writes the note-on of `serial` at frame `on` into one of `lane`'s records that no voice of the
/// lane holds (four records, at most three other voices).
fn bear(records: &mut [Born; W], lane: &Lane, serial: u64, on: u64) {
    let free = records
        .iter_mut()
        .find(|b| !(0..W).any(|s| has(u32::from(lane.used), s) && lane.age.get(s) == Some(&(b.serial as u32))));
    if let Some(b) = free {
        *b = Born { serial, on };
    }
}

/// 20 log10 of `gain`, -120 to +24 (-120 for silence).
fn level_db(gain: f64) -> f32 {
    if gain > 1e-6 {
        (laws::log2(gain) / DPD).clamp(-120.0, 24.0) as f32
    } else {
        -120.0
    }
}

/// Puts `r` in its place in the first `n` of `list` (least important first), the last falling off
/// when all `LEAST` are taken and it goes before the last; else drops it.
fn insert(list: &mut [Rank; LEAST], n: &mut u8, r: Rank) {
    let full = usize::from(*n) >= LEAST;
    if full && !list.last().is_some_and(|last| r.before(last)) {
        return;
    }
    let mut at = if full { LEAST - 1 } else { usize::from(*n) };
    while let Some(prev) = at.checked_sub(1).and_then(|i| list.get(i)).copied() {
        if !r.before(&prev) {
            break;
        }
        if let Some(x) = list.get_mut(at) {
            *x = prev;
        }
        at = at.wrapping_sub(1);
    }
    if let Some(x) = list.get_mut(at) {
        *x = r;
    }
    if !full {
        *n = n.saturating_add(1);
    }
}

/// The engine: every object fixed size, owned by the caller.
pub struct Engine {
    sounds: [Prepared; SOUNDS],
    limits: [Limits; SOUNDS],
    lanes: [Lane; LANES],
    /// The lanes with a voice, a bit each (and maybe some emptied since their last step), so that
    /// the per-block scans touch only those (a lane is several cache lines apart from the next).
    live: u64,
    /// Voices the instance may keep (at most `VOICES`).
    limit: usize,
    /// Sounds whose sustain pedal is down, a bit each.
    pedal: u8,
    /// Sounds with a parameter change waiting until none of their voices sounds, a bit each.
    pending: u8,
    /// Sounds with parameter values set by [`Engine::stage`] and not yet prepared, a bit each.
    staged: u8,
    age: u32,
    /// Each sound's random source (xorshift32), started again when the sound is installed.
    rng: [u32; SOUNDS],
    /// Each sound's pitch bend, -8192..+8191.
    bend: [i16; SOUNDS],
    /// Each sound's channel controllers (`CTLS`, 0-127), sounds whose sostenuto pedal is down (a
    /// bit each), and what the controllers do to each sound now.
    ctl: [[u8; CTLS]; SOUNDS],
    sost: u8,
    mods: [Mods; SOUNDS],
    /// Each sound's latest note, where portamento glides from.
    last: [Option<u32>; SOUNDS],
    /// Frames rendered (at the block's start), and the glide clock's period (frames, 2^-16 a
    /// unit): one clock for every sound.
    clock: u64,
    tick: u64,
    /// The frame of the block the event being applied is at (0 between blocks).
    at: usize,
    /// Each sound's sound LFO.
    slfo: [Slfo; SOUNDS],
    /// A mono sound's keys still down whose notes another took over, oldest first.
    keys: [[Held; KEYS]; SOUNDS],
    keys_n: [u8; SOUNDS],
    rate: f64,
    /// A shed fade's and a retrigger fade's share a 16-frame step.
    shed_step: f32,
    retrig_step: f32,
    /// The operators' outputs in a step, at each width (each is written before it is read, so
    /// they are never cleared).
    y1: [[[f32; 1]; STEP]; OPS],
    y4: [[[f32; W]; STEP]; OPS],
    /// Sounds whose next note takes alternate pan's other side, a bit each.
    alt: u8,
    /// The block's sum of the sounds with pan, left and right (added at its end to the sum of the
    /// others, which is both channels'), and whether any added to it.
    pan: [[f32; BLOCK]; 2],
    pan_on: bool,
    /// Notes waiting out their sound's key-on delay: the sound, the note and the frame it starts at
    /// (from the block's start); the first `delayed_n` are in use.
    delayed: [(u8, Held, u32); DELAYED],
    delayed_n: usize,
    budget: Budget,
}

/// What only the voice budget reads, which rendering never touches (one field of the engine, so
/// that it lies beside the rendering's data, not among it): the latest note-on's serial, whole (the
/// engine's `age` is its low half); each lane's voices' records ([`Born`]); each sound's voices
/// [`Engine::importance`] listed, least important first (the first `listed_n`), given up first at
/// the next block's start past a limit.
struct Budget {
    serial: u64,
    born: [[Born; W]; LANES],
    listed: [[Listed; LEAST]; SOUNDS],
    listed_n: [u8; SOUNDS],
    /// To a whole way of the device's first-level cache (32 KB, 4-way: 8 KB), so that everything
    /// after it in memory falls in the cache sets it did without it (measured: the shared hosted
    /// case's misses moved 10 % with the budget's state unpadded).
    _pad: [u8; BUDGET_PAD],
}
const BUDGET_PAD: usize = 2544;
const _: () = assert!(core::mem::size_of::<Budget>() == 8192);

/// A listed voice as [`Engine::shed`] reads it: its lane, its serial and its place in the order
/// (integers and floats alone, no field with a niche, so that the compiler leaves [`Budget`] after
/// the rendering's data).
#[derive(Clone, Copy)]
struct Listed {
    serial: u64,
    audibility: f64,
    lane: u8,
    tier: u8,
    struck: u8,
}

impl Listed {
    fn of(r: &Rank) -> Listed {
        Listed {
            serial: r.serial,
            audibility: r.audibility,
            lane: r.lane,
            tier: r.tier,
            struck: u8::from(r.struck),
        }
    }
    fn rank(&self) -> Rank {
        Rank {
            tier: self.tier,
            struck: self.struck != 0,
            audibility: self.audibility,
            serial: self.serial,
            lane: self.lane,
            ..RANK0
        }
    }
}

const BUDGET0: Budget = Budget {
    serial: 0,
    born: [[Born { serial: 0, on: 0 }; W]; LANES],
    listed: [[Listed {
        serial: 0,
        audibility: 0.0,
        lane: 0,
        tier: 0,
        struck: 0,
    }; LEAST]; SOUNDS],
    listed_n: [0; SOUNDS],
    _pad: [0; BUDGET_PAD],
};

impl Engine {
    /// An engine at `rate` Hz, every sound the default sound.
    pub fn new(rate: f64) -> Engine {
        let p = Prepared::new(&Sound::default());
        let steps = (SHED_MS * rate / 1000.0 / STEP as f64 + 0.5).max(1.0);
        Engine {
            sounds: [p; SOUNDS],
            limits: [Limits::default(); SOUNDS],
            lanes: [LANE0; LANES],
            live: 0,
            limit: VOICES,
            pedal: 0,
            pending: 0,
            staged: 0,
            age: 0,
            rng: [SEED; SOUNDS],
            bend: [0; SOUNDS],
            ctl: [CTL_REST; SOUNDS],
            sost: 0,
            mods: [MODS0; SOUNDS],
            last: [None; SOUNDS],
            clock: 0,
            tick: ((laws::GLIDE_TICK_MS * rate / 1000.0 * 65536.0 + 0.5) as u64).max(1),
            at: 0,
            slfo: [SLFO0; SOUNDS],
            keys: [[Held::default(); KEYS]; SOUNDS],
            keys_n: [0; SOUNDS],
            rate,
            shed_step: (1.0 / steps) as f32,
            retrig_step: (STEP as f64 * 1000.0 / (RETRIGGER_MS * rate)) as f32,
            y1: [[[0.0; 1]; STEP]; OPS],
            y4: [[[0.0; W]; STEP]; OPS],
            alt: 0,
            pan: [[0.0; BLOCK]; 2],
            pan_on: false,
            delayed: [(0, Held::default(), 0); DELAYED],
            delayed_n: 0,
            budget: BUDGET0,
        }
    }

    /// Installs a prepared sound in slot `sound`, which must have no voice sounding (see
    /// [`Engine::choke`]); whether it was installed.
    pub fn set_sound(&mut self, sound: usize, p: &Prepared) -> bool {
        let busy = self.busy(sound);
        match (
            self.sounds.get_mut(sound),
            self.rng.get_mut(sound),
            self.slfo.get_mut(sound),
        ) {
            (Some(d), Some(r), Some(l)) if !busy => {
                (*d, *r, *l) = (*p, SEED, SLFO0);
                self.pending &= !bit(sound);
                self.alt &= !bit(sound);
                self.refresh(sound);
                true
            }
            _ => false,
        }
    }

    /// The lanes with a voice, in order.
    fn live(&self) -> impl Iterator<Item = &Lane> {
        bits(self.live)
            .filter_map(|i| self.lanes.get(i))
            .filter(|l| l.used != 0)
    }

    /// The sounds with a voice or a note waiting out its key-on delay, a bit each: with the sounds
    /// of a block's events, the sounds [`Engine::render_buses`] renders.
    pub fn playing(&self) -> u8 {
        let m = self.live().fold(0, |m, l| m | bit(usize::from(l.sound)));
        (self.delayed.iter().take(self.delayed_n)).fold(m, |m, d| m | bit(usize::from(d.0)))
    }

    /// Whether a voice, a kept mono key or a delayed note of `sound` sounds.
    fn busy(&self, sound: usize) -> bool {
        self.live().any(|l| usize::from(l.sound) == sound)
            || self.keys_n.get(sound).is_some_and(|&n| n > 0)
            || self
                .delayed
                .iter()
                .take(self.delayed_n)
                .any(|d| usize::from(d.0) == sound)
    }

    /// Sets parameter `id` of `sound` to the raw `value` (unknown ids and values out of range are
    /// ignored) and prepares the sound again: from the next control step what is evaluated a step
    /// follows it (the LFO, the bend's ranges), from the next note what a note keeps. A change of
    /// what is rendered (the operators heard, their forms, the feedback path) or of mono or key
    /// assign waits until no voice of the sound sounds.
    pub fn param(&mut self, sound: usize, id: u16, value: i16) {
        if self.stage(sound, id, value) && has(u32::from(self.staged), sound) {
            self.apply(sound);
        }
    }

    /// Sets parameter `id` of `sound` to the raw `value` as [`Engine::param`] does, but prepares
    /// the sound only at the start of the next render, once for every value staged before it;
    /// whether the value was taken. A value the sound already has prepares nothing; a box's source
    /// (a knob, the ribbon, a switch) is read only by what the boxes do, so with nothing else
    /// waiting only that is made again, now.
    pub fn stage(&mut self, sound: usize, id: u16, value: i16) -> bool {
        let Some(d) = self.sounds.get_mut(sound) else {
            return false;
        };
        let i = index_of(id);
        let was = i.map(|i| d.sound.get(i));
        if d.sound.set(id, value).is_err() {
            return false;
        }
        let source = i.is_some_and(|i| (Part::Knob as usize..=Part::Switch as usize + 1).contains(&i));
        if was == Some(value) {
            return true;
        }
        if source && (self.staged | self.pending) & bit(sound) == 0 {
            self.refresh(sound);
        } else {
            self.staged |= bit(sound);
        }
        true
    }

    /// The sounds with staged values, a bit each (each is prepared once at the next render).
    pub fn staged(&self) -> u8 {
        self.staged
    }

    /// Prepares `sound` from its raw values: now, or when none of its voices sounds if what is
    /// rendered, mono or key assign changed.
    fn apply(&mut self, sound: usize) {
        self.staged &= !bit(sound);
        let busy = self.busy(sound);
        if let Some(d) = self.sounds.get_mut(sound) {
            let n = Prepared::new(&d.sound);
            if !busy || (n.plan == d.plan && n.mono == d.mono && n.single == d.single) {
                *d = n;
                self.pending &= !bit(sound);
                self.refresh(sound);
            } else {
                self.pending |= bit(sound);
            }
        }
    }

    /// Every voice ends at once, silently (no note end is reported), and the sounds' running state
    /// (random sources, sound LFOs, pedal, bend, the clock) starts again: the engine then plays as
    /// a new one with the same sounds and limits.
    pub fn reset(&mut self) {
        for s in 0..SOUNDS {
            if has(u32::from(self.staged | self.pending), s) {
                if let Some(d) = self.sounds.get_mut(s) {
                    *d = Prepared::new(&d.sound);
                }
            }
        }
        (self.lanes, self.live) = ([LANE0; LANES], 0);
        (self.pedal, self.pending, self.staged, self.age, self.alt) = (0, 0, 0, 0, 0);
        (self.rng, self.bend, self.last) = ([SEED; SOUNDS], [0; SOUNDS], [None; SOUNDS]);
        (self.ctl, self.sost) = ([CTL_REST; SOUNDS], 0);
        (0..SOUNDS).for_each(|s| self.refresh(s));
        (self.clock, self.at, self.slfo, self.keys_n) = (0, 0, [SLFO0; SOUNDS], [0; SOUNDS]);
        (self.pan_on, self.delayed_n, self.budget) = (false, 0, BUDGET0);
    }

    /// Moves the engine's clock on by `frames` not rendered (blocks a host skipped while nothing
    /// sounded): the glide clock's phase then is what rendering them would have left. Nothing else
    /// runs free while no voice sounds (a sound LFO moves only while its sound plays).
    pub fn skip(&mut self, frames: u64) {
        self.clock = self.clock.wrapping_add(frames);
    }

    /// Fades out the note `id` of `sound` as a shed note (a note still waiting out its key-on
    /// delay, or a mono key kept to return to, ends at once).
    pub fn choke_note(&mut self, sound: usize, id: u32, end: &mut impl FnMut(NoteEnd)) {
        let s8 = sound as u8;
        if let Some((_, h, _)) = self
            .waiting(|d| d.0 == s8 && d.1.id == id)
            .and_then(|i| self.undelay(i))
        {
            return end(NoteEnd {
                sound: s8,
                key: h.key,
                id,
            });
        }
        if let (Some(keys), Some(n)) = (self.keys.get_mut(sound), self.keys_n.get_mut(sound)) {
            let kept = keys.get(..usize::from(*n)).unwrap_or(&[]);
            if let Some(i) = kept.iter().position(|h| h.id == id) {
                let key = kept.get(i).map_or(0, |h| h.key);
                remove(keys, i);
                *n = n.saturating_sub(1);
                return end(NoteEnd { sound: s8, key, id });
            }
        }
        for l in self.lanes.iter_mut().filter(|l| l.sound == s8) {
            for s in (0..W).filter(|&s| l.id.get(s) == Some(&id)) {
                l.ending |= l.used & bit(s);
            }
        }
    }

    /// Each voice sounding: its sound, its note and its loudness now (the end rule's bound on its
    /// carriers' gain, times its fade).
    pub fn voices(&self, mut f: impl FnMut(u8, u32, f32)) {
        for l in self.live() {
            for s in (0..W).filter(|&s| has(u32::from(l.used), s)) {
                let (id, b, g) = (l.id.get(s), l.bound.get(s), l.fade.get(s));
                f(l.sound, id.copied().unwrap_or(0), b.zip(g).map_or(0.0, |(b, g)| b * g));
            }
        }
    }

    /// The estimated cost of a voice of `sound` a block, in instructions (its plan's).
    pub fn cost(&self, sound: usize) -> u32 {
        self.sounds.get(sound).map_or(0, |p| p.plan.cost())
    }

    /// Fades out every voice of `sound` (each ends after its fade); a mono sound's keys kept to
    /// return to end at once.
    pub fn choke(&mut self, sound: usize, end: &mut impl FnMut(NoteEnd)) {
        while let Some(i) = self.waiting(|d| usize::from(d.0) == sound) {
            if let Some((s, h, _)) = self.undelay(i) {
                end(NoteEnd {
                    sound: s,
                    key: h.key,
                    id: h.id,
                });
            }
        }
        for l in self.lanes.iter_mut().filter(|l| usize::from(l.sound) == sound) {
            l.ending |= l.used;
        }
        if let (Some(keys), Some(n)) = (self.keys.get(sound), self.keys_n.get_mut(sound)) {
            for h in keys.iter().take(usize::from(*n)) {
                end(NoteEnd {
                    sound: sound as u8,
                    key: h.key,
                    id: h.id,
                });
            }
            *n = 0;
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
        self.live()
            .fold(0usize, |n, l| n.wrapping_add(l.used.count_ones() as usize))
    }

    fn keeping(&self, sound: Option<u8>) -> usize {
        self.live()
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
    /// A mono sound plays one note: a new one takes over the sounding note's voice when
    /// portamento is on and its key is down (legato: the envelopes go on), else the sounding
    /// note fades out as a retrigger and the new one starts; a key still down is returned to when
    /// the newer one is released (last-note priority). With key assign single, a key played again
    /// retriggers its sounding note. With the sound's key-on delay the note waits that long, to the
    /// frame (released before then, it never sounds).
    pub fn note_on(&mut self, sound: usize, id: u32, key: u8, velocity: f32, end: &mut impl FnMut(NoteEnd)) {
        let s8 = sound as u8;
        let delay = self.sounds.get(sound).map_or(0, |p| {
            (f64::from(p.delay_ms) * self.rate / 1000.0 + 0.5).min(f64::from(u32::MAX / 2)) as u32
        });
        if delay == 0 {
            return self.play(sound, id, key, velocity, end);
        }
        let h = Held { id, key, velocity };
        match self.delayed.get_mut(self.delayed_n) {
            Some(d) => {
                *d = (s8, h, delay.saturating_add(self.at as u32));
                self.delayed_n = self.delayed_n.wrapping_add(1);
            }
            None => end(NoteEnd { sound: s8, key, id }),
        }
    }

    /// A delayed note of `sound` whose start is in this block (of `frames`) at or after frame
    /// `from`, the earliest: its place in the list and its frame.
    fn due(&self, sound: u8, from: usize, frames: usize) -> Option<(usize, usize)> {
        // (a range, not `enumerate`, whose counter would be an overflow check)
        (0..self.delayed_n)
            .filter_map(|i| self.delayed.get(i).map(|d| (i, d)))
            .filter(|(_, d)| d.0 == sound && (d.2 as usize) < frames)
            .map(|(i, d)| (i, (d.2 as usize).max(from)))
            .min_by_key(|x| x.1)
    }

    /// The first delayed note for which `f` holds.
    fn waiting(&self, f: impl Fn(&(u8, Held, u32)) -> bool) -> Option<usize> {
        (0..self.delayed_n).find(|&i| self.delayed.get(i).is_some_and(&f))
    }

    /// Takes delayed note `i` out of the list (the last one moves into its place).
    fn undelay(&mut self, i: usize) -> Option<(u8, Held, u32)> {
        let last = self.delayed_n.checked_sub(1)?;
        let d = *self.delayed.get(i)?;
        let l = *self.delayed.get(last)?;
        *self.delayed.get_mut(i)? = l;
        self.delayed_n = last;
        Some(d)
    }

    /// [`Engine::note_on`] now.
    fn play(&mut self, sound: usize, id: u32, key: u8, velocity: f32, end: &mut impl FnMut(NoteEnd)) {
        let s8 = sound as u8;
        let Some(p) = self.sounds.get(sound).copied() else {
            return end(NoteEnd { sound: s8, key, id });
        };
        let v = laws::part_velocity(i32::from(p.velocity.0), i32::from(p.velocity.1), f64::from(velocity));
        if v <= 0.0 || velocity.is_nan() {
            return end(NoteEnd { sound: s8, key, id });
        }
        // a legato note: another key of the sound is down
        let legato = self.lanes.iter().any(|l| l.sound == s8 && l.held & !l.ending != 0);
        if let Some(l) = self.slfo.get_mut(sound) {
            let now = self.clock.wrapping_add(self.at as u64);
            l.key_on(&p, !legato, now, self.rate);
        }
        let new = Held { id, key, velocity };
        if p.mono {
            if let Some((li, s)) = self.sounding(s8) {
                let Some(lane) = self.lanes.get_mut(li) else { return };
                let (b, down) = (bit(s), has(u32::from(lane.held), s));
                let old = Held {
                    id: lane.id.get(s).copied().unwrap_or(0),
                    key: lane.key.get(s).copied().unwrap_or(0),
                    velocity: lane.vel.get(s).copied().unwrap_or(0.0),
                };
                if p.porta && down {
                    // legato: the voice goes on with the new note, the old key is kept
                    let glide = self.glide(sound, &p, i32::from(key) + i32::from(p.note_shift), true);
                    self.push(sound, old, end);
                    return self.retarget(sound, li, s, new, v, glide);
                }
                // the sounding note fades out (a key still down is kept, its end not the voice's)
                (lane.ending, lane.retrig, lane.held) = (lane.ending | b, lane.retrig | b, lane.held & !b);
                if down {
                    lane.orphan |= b;
                    self.push(sound, old, end);
                }
            }
        } else if p.single {
            for l in self.lanes.iter_mut().filter(|l| l.sound == s8) {
                let same = (0..W)
                    .filter(|&s| l.key.get(s) == Some(&key))
                    .fold(0, |m, s| m | bit(s));
                let on = same & l.used & !l.ending;
                (l.ending, l.retrig) = (l.ending | on, l.retrig | on);
            }
        }
        self.start(sound, new, legato, end);
    }

    /// The voice of a mono sound that sounds (not fading), the newest: its lane and slot.
    fn sounding(&self, sound: u8) -> Option<(usize, usize)> {
        let voices = (0..LANES).zip(self.lanes.iter()).filter(|(_, l)| l.sound == sound);
        voices
            .flat_map(|(li, l)| {
                (0..W)
                    .filter(|&s| has(u32::from(l.used & !l.ending), s))
                    .map(move |s| (li, s, l))
            })
            .max_by_key(|&(_, s, l)| l.age.get(s).copied().unwrap_or(0))
            .map(|(li, s, _)| (li, s))
    }

    /// Keeps a mono sound's key that another took over (the oldest let go, its note ending, when
    /// there are `KEYS` already).
    fn push(&mut self, sound: usize, h: Held, end: &mut impl FnMut(NoteEnd)) {
        let (Some(keys), Some(n)) = (self.keys.get_mut(sound), self.keys_n.get_mut(sound)) else {
            return;
        };
        if usize::from(*n) >= KEYS {
            if let Some(&o) = keys.first() {
                end(NoteEnd {
                    sound: sound as u8,
                    key: o.key,
                    id: o.id,
                });
            }
            remove(keys, 0);
            *n = n.saturating_sub(1);
        }
        if let Some(k) = keys.get_mut(usize::from(*n)) {
            *k = h;
            *n = n.saturating_add(1);
        }
    }

    /// A mono sound's voice goes on with another note `h` (velocity `v` after the part's law):
    /// its pitch for that note (the common pitch envelope and the amplitude envelopes go on).
    fn retarget(&mut self, sound: usize, li: usize, s: usize, h: Held, v: f64, glide: (f32, f32)) {
        let Some(p) = self.sounds.get(sound).copied() else {
            return;
        };
        let (rate, r) = (self.rate, next_random(self.rng.get_mut(sound)));
        self.age = self.age.wrapping_add(1);
        self.budget.serial = self.budget.serial.wrapping_add(1);
        let now = self.clock.wrapping_add(self.at as u64);
        let Some(lane) = self.lanes.get_mut(li) else { return };
        if let (Some(k), Some(i), Some(a), Some(vl)) = (
            lane.key.get_mut(s),
            lane.id.get_mut(s),
            lane.age.get_mut(s),
            lane.vel.get_mut(s),
        ) {
            (*k, *i, *a, *vl) = (h.key, h.id, self.age, h.velocity);
        }
        if let Some(b) = self.budget.born.get_mut(li) {
            bear(b, lane, self.budget.serial, now);
        }
        (lane.held, lane.sustained) = (lane.held | bit(s), lane.sustained & !bit(s));
        // shortcut: the voice keeps its pan (unmeasured)
        lane.post
            .retune(&p.filter, s, i32::from(h.key) + i32::from(p.note_shift), v);
        lane.tune(
            &p,
            s,
            i32::from(h.key) + i32::from(p.note_shift),
            v,
            r,
            rate,
            false,
            glide,
        );
        if let Some(l) = self.last.get_mut(sound) {
            *l = Some(h.id);
        }
    }

    /// A note in a voice of its own (`legato`: another key of the sound was down).
    fn start(&mut self, sound: usize, h: Held, legato: bool, end: &mut impl FnMut(NoteEnd)) {
        let (s8, Held { id, key, velocity }) = (sound as u8, h);
        let refuse = |end: &mut dyn FnMut(NoteEnd)| end(NoteEnd { sound: s8, key, id });
        let Some(p) = self.sounds.get(sound).copied() else {
            return refuse(end);
        };
        let v = laws::part_velocity(i32::from(p.velocity.0), i32::from(p.velocity.1), f64::from(velocity));
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
        self.live |= found.map_or(0, bit64);
        if lane.used == 0 {
            lane.sound = s8;
        }
        let s = (!lane.used).trailing_zeros() as usize;
        self.age = self.age.wrapping_add(1);
        self.budget.serial = self.budget.serial.wrapping_add(1);
        let b = bit(s);
        lane.used |= b;
        lane.held |= b;
        lane.sustained &= !b;
        lane.latch &= !b;
        lane.ending &= !b;
        lane.retrig &= !b;
        lane.orphan &= !b;
        if let Some(v) = lane.vol.get_mut(s) {
            *v = 1.0;
        }
        if let (Some(k), Some(i), Some(a), Some(f), Some(bd)) = (
            lane.key.get_mut(s),
            lane.id.get_mut(s),
            lane.age.get_mut(s),
            lane.fade.get_mut(s),
            lane.bound.get_mut(s),
        ) {
            (*k, *i, *a, *f, *bd) = (key, id, self.age, 1.0, 1.0);
        }
        if let (Some(l), Some(vl)) = (lane.left.get_mut(s), lane.vel.get_mut(s)) {
            (*l, *vl) = (1.0, velocity);
        }
        if let Some(b) = found.and_then(|i| self.budget.born.get_mut(i)) {
            bear(b, lane, self.budget.serial, self.clock.wrapping_add(self.at as u64));
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
            lane.bend = 0.0; // (a lane's first voice: its increments hold no bend)
        }
        lane.tune(&p, s, key, v, r, rate, true, glide);
        // pan: alternate pan's side changes every note of the sound; a random draw only for random
        // pan (so the random pitch's sequence is as it was without it)
        let other = has(u32::from(self.alt), sound);
        self.alt ^= bit(sound);
        let pr = p.filter.random_pan().then(|| next_random(self.rng.get_mut(sound)));
        let pos = p.filter.position(key, other, pr);
        lane.post.start(&p.filter, s, key, v, pos, lane.used == b, rate);
        lane.sdt_d = [SDT_NONE; OPS]; // (the new voice's detune moves are not made yet)
        if let (Some(ph), Some(t), Some(rnd)) = (lane.lfo_ph.get_mut(s), lane.lfo_t.get_mut(s), lane.lfo_rnd.get_mut(s))
        {
            // without key-on reset a note takes the sound's free-running phase
            let free = (self.clock as u32).wrapping_mul(lfo_inc(p.lfo_hz, rate));
            *ph = p.lfo_start.wrapping_add(if p.lfo_reset { 0 } else { free });
            (*t, *rnd) = (0, r.rotate_left(16) | 1);
        }
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
            let mut full =
                p.full.get(op).copied().unwrap_or(FLOOR) + (laws::velocity_db(at(&p.level_velocity), v) * DPD) as f32;
            if let (true, Some(sc)) = (has(u32::from(p.scaling_on), op), p.scaling.get(op)) {
                let x = sc.map(i32::from);
                let db = laws::level_scaling_db(key, x[0], (x[1], x[3]), (x[2], x[4]));
                full += ((db * DPD) as f32).min(p.cap.get(op).copied().unwrap_or(0.0));
            }
            put(&mut lane.top, op, s, full + (laws::ATTACK_OVER_DB * DPD) as f32);
            let start = (full + p.eg.get(op).map_or(FLOOR, |e| e[3])).max(FLOOR);
            put(&mut lane.lvl, op, s, start);
            let hold = f64::from(p.hold_ms.get(op).copied().unwrap_or(0.0)) * rate / 1000.0 / STEP as f64 + 0.5;
            put(&mut lane.count, op, s, (hold as u32).saturating_mul(STEP as u32));
            lane.enter(&p, op, s, if hold as u32 > 0 { HOLD } else { ATTACK }, rate);
        }
    }

    /// Releases the note `id` of `sound` (or holds it with the pedal down). A mono sound's
    /// sounding note released while older keys are down returns to the newest of them: legato
    /// with portamento (the released note ends at once), else as a retrigger.
    pub fn note_off(&mut self, sound: usize, id: u32, end: &mut impl FnMut(NoteEnd)) {
        let s8 = sound as u8;
        // a note still waiting out its key-on delay never sounds
        let waiting = self.waiting(|d| d.0 == s8 && d.1.id == id);
        if let Some((_, h, _)) = waiting.and_then(|i| self.undelay(i)) {
            return end(NoteEnd {
                sound: s8,
                key: h.key,
                id,
            });
        }
        let pedal = has(u32::from(self.pedal), sound);
        let Some(p) = self.sounds.get(sound).copied() else {
            return;
        };
        if p.mono {
            let (Some(keys), Some(n)) = (self.keys.get_mut(sound), self.keys_n.get_mut(sound)) else {
                return;
            };
            let kept = keys.get(..usize::from(*n)).unwrap_or(&[]);
            if let Some(i) = kept.iter().position(|h| h.id == id) {
                // a key another took over: its note ends
                let h = kept.get(i).copied().unwrap_or_default();
                remove(keys, i);
                *n = n.saturating_sub(1);
                return end(NoteEnd {
                    sound: s8,
                    key: h.key,
                    id,
                });
            }
            let top = usize::from(*n).checked_sub(1).and_then(|i| keys.get(i)).copied();
            let voice = self.sounding(s8).filter(|&(li, s)| {
                self.lanes
                    .get(li)
                    .is_some_and(|l| has(u32::from(l.held), s) && l.id.get(s) == Some(&id))
            });
            if let (Some(h), Some((li, s))) = (top, voice) {
                if let Some(n) = self.keys_n.get_mut(sound) {
                    *n = n.saturating_sub(1);
                }
                let v = laws::part_velocity(i32::from(p.velocity.0), i32::from(p.velocity.1), f64::from(h.velocity));
                if p.porta {
                    let glide = self.glide(sound, &p, i32::from(h.key) + i32::from(p.note_shift), true);
                    let key = self.lanes.get(li).and_then(|l| l.key.get(s)).copied().unwrap_or(0);
                    end(NoteEnd { sound: s8, key, id });
                    return self.retarget(sound, li, s, h, v, glide);
                }
                if let Some(l) = self.lanes.get_mut(li) {
                    let b = bit(s);
                    (l.ending, l.retrig, l.held) = (l.ending | b, l.retrig | b, l.held & !b);
                }
                return self.start(sound, h, true, end);
            }
        }
        for lane in self.lanes.iter_mut().filter(|l| l.sound == s8 && l.held != 0) {
            for s in 0..W {
                if has(u32::from(lane.held), s) && lane.id.get(s) == Some(&id) {
                    if pedal || has(u32::from(lane.latch), s) {
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
        // the bend is a box source too
        if self.sounds.get(sound).is_some_and(|p| p.boxed) {
            self.refresh(sound);
        }
    }

    /// The sustain pedal of `sound`: up releases the notes it held (not those the sostenuto
    /// pedal still holds).
    pub fn pedal(&mut self, sound: usize, down: bool) {
        let b = bit(sound);
        self.pedal = if down { self.pedal | b } else { self.pedal & !b };
        if !down {
            self.let_go(sound, |l| l.sustained & !l.latch);
        }
    }

    /// Releases the voices of `sound` that `which` names in each lane.
    fn let_go(&mut self, sound: usize, which: impl Fn(&Lane) -> u8) {
        let Some(p) = self.sounds.get(sound).copied() else {
            return;
        };
        for lane in self.lanes.iter_mut().filter(|l| usize::from(l.sound) == sound) {
            let m = which(lane);
            for s in (0..W).filter(|&s| has(u32::from(m), s)) {
                lane.release(&p, s, self.rate);
            }
        }
    }

    /// A channel controller of `sound` at `value` (0-127), by its MIDI number: 1 the mod wheel, 2
    /// breath, 4 foot controller 2, 11 foot controller 1 and expression, 66 the sostenuto pedal,
    /// [`PRESSURE`] aftertouch; any other is ignored (67, the soft pedal, as the reference does).
    /// It acts from the next control step (an event's frame).
    pub fn control(&mut self, sound: usize, cc: u8, value: u8) {
        let value = value.min(127);
        if cc == 66 {
            return self.sostenuto(sound, value >= 64);
        }
        let Some(c) = self.ctl.get_mut(sound) else {
            return;
        };
        match cc {
            1 => c[0] = value,
            PRESSURE => c[1] = value,
            11 => (c[2], c[EXPRESSION]) = (value, value),
            4 => c[3] = value,
            2 => c[4] = value,
            _ => return,
        }
        self.refresh(sound);
    }

    /// The sostenuto pedal of `sound`: down holds the notes whose keys are down then until it goes
    /// up (a pedal already down holds no more); up releases those it held whose keys are up,
    /// unless the sustain pedal holds them.
    fn sostenuto(&mut self, sound: usize, down: bool) {
        let b = bit(sound);
        if down == (self.sost & b != 0) {
            return;
        }
        self.sost ^= b;
        if down {
            for lane in self.lanes.iter_mut().filter(|l| usize::from(l.sound) == sound) {
                lane.latch = lane.held;
            }
            return;
        }
        let pedal = self.pedal & b != 0;
        self.let_go(sound, |l| if pedal { 0 } else { l.sustained & l.latch });
        for lane in self.lanes.iter_mut().filter(|l| usize::from(l.sound) == sound) {
            lane.latch = 0;
        }
    }

    /// The volume of note `id` of `sound` (its expression), linear 0-4, from the next control
    /// step, ramped across it.
    pub fn volume(&mut self, sound: usize, id: u32, gain: f32) {
        if !(0.0..=4.0).contains(&gain) {
            return;
        }
        for lane in self.lanes.iter_mut().filter(|l| usize::from(l.sound) == sound) {
            for s in (0..W).filter(|&s| has(u32::from(lane.used), s) && lane.id.get(s) == Some(&id)) {
                if let Some(v) = lane.vol.get_mut(s) {
                    (*v, lane.vol_on) = (gain, true);
                }
            }
        }
    }

    /// What the controllers of `sound` do to it now ([`Mods`]).
    fn refresh(&mut self, sound: usize) {
        let (Some(c), Some(m), Some(p), Some(&b)) = (
            self.ctl.get(sound),
            self.mods.get_mut(sound),
            self.sounds.get(sound),
            self.bend.get(sound),
        ) else {
            return;
        };
        // expression: assumed to follow the part volume's measured law, about 40 log10(v / 127) dB
        let e = f32::from(c[EXPRESSION]) / 127.0;
        *m = Mods {
            gain: e * e,
            lfo_depths: p.lfo_depths,
            lfo_speed: p.lfo_speed.1,
            ..MODS0
        };
        if p.boxed {
            let mut src = [0.0; SOURCES];
            boxes::own(&p.sound, &mut src);
            src[0] = boxes::bend(b);
            for (&k, &v) in [1usize, 2, 3, 4, 7].iter().zip(c.iter()) {
                if let Some(x) = src.get_mut(k) {
                    *x = boxes::controller(v);
                }
            }
            let t = boxes::steps(&p.sound, &src);
            let carriers = u32::from(p.plan.carriers);
            let ops = (0..OPS).zip(p.levels.iter().zip(t.level.iter()).zip(m.lvl.iter_mut()));
            for (op, ((&(level, top), &st), l)) in ops {
                if (level == top && st == 0) || !has(u32::from(p.plan.live), op) {
                    continue;
                }
                let db = |l: i32| {
                    if has(carriers, op) {
                        laws::carrier_level_db(l)
                    } else {
                        laws::modulator_level_db(l)
                    }
                };
                let now = i32::from(level).saturating_add(st).clamp(0, 99);
                *l = ((db(now) - db(i32::from(top))) * DPD).max(f64::from(FLOOR) * 2.0) as f32;
                m.lvl_on = true;
            }
            (m.cut, m.res) = (t.cutoff as f32, t.resonance as f32);
            // the voice LFO's values moved by whole steps within 0-99 (measured)
            let moved = |k: Common, st: i32| boxes::lfo_value(p.sound.get(k as usize), st);
            if t.lfo_pitch != 0 {
                m.lfo_depths.0 = laws::lfo_pitch_d(moved(Common::LfoPitchDepth, t.lfo_pitch)) as f32;
            }
            if t.lfo_amp != 0 {
                m.lfo_depths.1 = laws::lfo_amp_steps(moved(Common::LfoAmpDepth, t.lfo_amp)) as f32;
            }
            if t.lfo_speed != 0 {
                m.lfo_speed = moved(Common::LfoSpeed, t.lfo_speed) as i16;
            }
            m.pitch = t.pitch as f32 / 1200.0;
            if p.op_pitch_box {
                for ((x, &st), &k) in m.op_pitch.iter_mut().zip(t.op_pitch.iter()).zip(p.op_sens.iter()) {
                    *x = st as f32 * k / 1200.0;
                }
            }
        }
        m.on = m.gain != 1.0
            || m.lvl_on
            || m.cut != 0.0
            || m.res != 0.0
            || m.lfo_depths != p.lfo_depths
            || m.lfo_speed != p.lfo_speed.1
            || m.pitch != 0.0
            || (p.op_pitch_box && m.op_pitch.iter().any(|&x| x != 0.0));
    }

    /// Each sound's held voices (not fading).
    fn held(&self) -> [usize; SOUNDS] {
        let mut held = [0usize; SOUNDS];
        for l in &self.lanes {
            if let Some(h) = held.get_mut(usize::from(l.sound)) {
                *h = h.saturating_add((l.held & !l.ending).count_ones() as usize);
            }
        }
        held
    }

    /// Whether the order never gives up voice `s` of lane `l` for a new note of `incoming` (or
    /// none): a held voice of a mono sound, or of a sound at or under its reserve (unless for that
    /// sound's own note).
    fn kept(&self, l: &Lane, s: usize, held: &[usize; SOUNDS], incoming: Option<u8>) -> bool {
        let lim = self.limits.get(usize::from(l.sound)).copied().unwrap_or_default();
        let reserve = held.get(usize::from(l.sound)).copied().unwrap_or(0) <= usize::from(lim.reserve);
        let mono = self.sounds.get(usize::from(l.sound)).is_some_and(|p| p.mono);
        has(u32::from(l.held), s) && ((incoming != Some(l.sound) && reserve) || mono)
    }

    /// The record of the voice in slot `s` of lane `li` ([`Born`]).
    fn born(&self, li: usize, s: usize) -> Born {
        let low = self.lanes.get(li).and_then(|l| l.age.get(s)).copied().unwrap_or(0);
        let records = self.budget.born.get(li).map_or(&[][..], |b| &b[..]);
        records
            .iter()
            .find(|b| b.serial as u32 == low)
            .copied()
            .unwrap_or_default()
    }

    /// Voice `s` of lane `li` as the order sees it now (at the frame of the event being applied).
    fn rank(&self, li: usize, s: usize) -> Rank {
        let Some(l) = self.lanes.get(li) else {
            return Rank::default();
        };
        let at = |a: &[f32; W]| f64::from(a.get(s).copied().unwrap_or(0.0));
        let b = self.born(li, s);
        let age_ms = self.clock.wrapping_add(self.at as u64).wrapping_sub(b.on) as f64 * 1000.0 / self.rate;
        Rank {
            lane: li as u8,
            slot: s as u8,
            ..Rank::new(
                has(u32::from(l.held), s),
                has(u32::from(l.sustained), s),
                at(&l.bound) * at(&l.fade) * at(&l.og),
                age_ms,
                b.serial,
            )
        }
    }

    /// Starts shedding one voice by the platform's voice-budget order, for a new note of
    /// `incoming` (or none), from sound `only` alone when given: never a voice [`Engine::kept`]
    /// keeps; by tier (released or below -60 dBFS unless struck under 50 ms ago, then held only by
    /// a pedal, then key held); the new note's own sound before others; lower priority before
    /// higher; then [`Rank`]'s order. With none incoming (a block's start, past a limit) the voices
    /// [`Engine::importance`] listed go first, in its order, each while it is still the same note.
    /// Whether one was found.
    fn shed(&mut self, incoming: Option<u8>, only: Option<u8>) -> bool {
        let held = self.held();
        let best = match incoming {
            None => self.listed(&held, only),
            Some(_) => None,
        };
        let best = best.or_else(|| self.victim(&held, incoming, only));
        let Some((li, s)) = best else { return false };
        if let Some(l) = self.lanes.get_mut(li) {
            l.ending |= bit(s);
        }
        true
    }

    /// The voice [`Engine::shed`] gives up by the order alone: its lane and slot.
    fn victim(&self, held: &[usize; SOUNDS], incoming: Option<u8>, only: Option<u8>) -> Option<(usize, usize)> {
        // the best so far: its rank, whether it is another sound's than the new note's, its priority
        let mut best: Option<(Rank, bool, u8)> = None;
        for (li, l) in (0..LANES).zip(self.lanes.iter()) {
            let priority = self.limits.get(usize::from(l.sound)).map_or(0, |l| l.priority);
            let other = incoming.is_some_and(|i| i != l.sound);
            for s in 0..W {
                if !has(u32::from(l.used & !l.ending), s)
                    || only.is_some_and(|o| o != l.sound)
                    || self.kept(l, s, held, incoming)
                {
                    continue;
                }
                let r = self.rank(li, s);
                if best.is_none_or(|(b, bo, bp)| r.first(other, priority, &b, bo, bp)) {
                    best = Some((r, other, priority));
                }
            }
        }
        best.map(|(r, _, _)| (usize::from(r.lane), usize::from(r.slot)))
    }

    /// The first voice the last [`Engine::importance`] listed that still sounds in its lane as the
    /// same note (its serial) and that [`Engine::kept`] does not keep now, from sound `only` alone
    /// when given: a sound's in its listed order, across sounds the first of each by tier, then
    /// priority, then [`Rank`]'s order (as listed). Its lane and slot.
    fn listed(&self, held: &[usize; SOUNDS], only: Option<u8>) -> Option<(usize, usize)> {
        let mut best: Option<(Rank, u8, (usize, usize))> = None;
        for (sound, (list, &n)) in (0..SOUNDS as u8).zip(self.budget.listed.iter().zip(&self.budget.listed_n)) {
            if only.is_some_and(|o| o != sound) {
                continue;
            }
            let priority = self.limits.get(usize::from(sound)).map_or(0, |l| l.priority);
            for r in list.iter().take(usize::from(n)).map(Listed::rank) {
                let li = usize::from(r.lane);
                // (a lane keeps its voices in its first slots: the voice may have moved in it since)
                let same = self.lanes.get(li).filter(|l| l.sound == sound).and_then(|l| {
                    (0..W)
                        .find(|&s| has(u32::from(l.used & !l.ending), s) && self.born(li, s).serial == r.serial)
                        .filter(|&s| !self.kept(l, s, held, None))
                });
                let Some(s) = same else {
                    continue; // ended, replaced or kept since
                };
                if best.is_none_or(|(b, bp, _)| r.first(false, priority, &b, false, bp)) {
                    best = Some((r, priority, (li, s)));
                }
                break; // a sound's first still sounding
            }
        }
        best.map(|(_, _, at)| at)
    }

    /// The voice budget's importance report (draft 2): for each sound with a voice sounding (not
    /// fading), in sound order, `f(sound, voices, least)`: its voices sounding and its `LEAST`
    /// least important ones, least important first, by the order [`Engine::shed`] gives voices up
    /// in with no note incoming (those [`Engine::kept`] keeps left out; past its reserve, only the
    /// held voices beyond it). One pass over the voices, no sorting; the lists are kept, so that a
    /// limit lowered before the next block gives up those voices first.
    pub fn importance(&mut self, mut f: impl FnMut(u8, u16, &[Rank])) {
        let held = self.held();
        let mut voices = [0u16; SOUNDS];
        let mut lists = [[RANK0; LEAST]; SOUNDS];
        let mut ns = [0u8; SOUNDS];
        for li in bits(self.live) {
            for s in 0..W {
                let Some(l) = self.lanes.get(li).filter(|l| has(u32::from(l.used & !l.ending), s)) else {
                    continue;
                };
                let (sound, kept) = (usize::from(l.sound), self.kept(l, s, &held, None));
                if let Some(v) = voices.get_mut(sound) {
                    *v = v.saturating_add(1);
                }
                if kept {
                    continue;
                }
                let r = self.rank(li, s);
                if let (Some(list), Some(n)) = (lists.get_mut(sound), ns.get_mut(sound)) {
                    insert(list, n, r);
                }
            }
        }
        self.budget.listed_n = [0; SOUNDS];
        let kept = self.budget.listed.iter_mut().zip(&mut self.budget.listed_n);
        for (sound, ((list, n), ((kept, kept_n), &v))) in
            (0..SOUNDS as u8).zip(lists.iter_mut().zip(&mut ns).zip(kept.zip(&voices)))
        {
            if v == 0 {
                continue;
            }
            // past its reserve a sound lists only the held voices beyond it, the least important of them
            let lim = self.limits.get(usize::from(sound)).copied().unwrap_or_default();
            let spare = held
                .get(usize::from(sound))
                .copied()
                .unwrap_or(0)
                .saturating_sub(usize::from(lim.reserve));
            let (mut k, mut held_listed) = (0usize, 0usize);
            for i in 0..usize::from(*n) {
                let Some(r) = list.get(i).copied() else { break };
                if r.held {
                    if held_listed >= spare {
                        continue;
                    }
                    held_listed = held_listed.saturating_add(1);
                }
                if let Some(x) = list.get_mut(k) {
                    *x = r;
                }
                k = k.saturating_add(1);
            }
            *n = k as u8;
            let least = list.get(..k).unwrap_or(&[]);
            for (x, r) in kept.iter_mut().zip(least) {
                *x = Listed::of(r);
            }
            *kept_n = *n;
            f(sound, v, least);
        }
    }

    /// Writes one block of every sounding voice to `out` (left, right); reports the notes that
    /// ended. A sound without pan plays the same in both channels.
    pub fn render<V: F4>(&mut self, out: &mut [[f32; BLOCK]; 2], end: &mut impl FnMut(NoteEnd)) {
        self.render_events::<V>(out, &[], end);
    }

    /// [`Engine::render`] with `events` (in frame order) applied at their frames: each sound's
    /// control steps start again at its own events' frames (a step shorter than 16 frames before
    /// one), so a note starts on its frame and no sound's output depends on another's events.
    pub fn render_events<V: F4>(
        &mut self,
        out: &mut [[f32; BLOCK]; 2],
        events: &[Event],
        end: &mut impl FnMut(NoteEnd),
    ) {
        let w = self.render_buses::<V>(core::slice::from_mut(out), &[0; SOUNDS], BLOCK, events, end);
        let [l, r] = out;
        if w.left & 1 == 0 {
            *l = [0.0; BLOCK];
        }
        if w.right & 1 == 0 {
            *r = *l;
        }
    }

    /// [`Engine::render_events`] over the first `frames` frames of the block (at most `BLOCK`;
    /// the clock moves on by as many), each sound into its own output pair: sound `s` into
    /// `out[bus[s]]` (into `out[0]` when that is past the end). Only what sounds is written: an
    /// output's left channel only when a voice played into it (bit `o` of `left`), its right only
    /// when a sound with pan did (bit `o` of `right`), else the right is the left; the caller
    /// copies or flags it. The sounds of one output are summed in the order of their numbers.
    pub fn render_buses<V: F4>(
        &mut self,
        out: &mut [[[f32; BLOCK]; 2]],
        bus: &[u8; SOUNDS],
        frames: usize,
        events: &[Event],
        end: &mut impl FnMut(NoteEnd),
    ) -> Written {
        let frames = frames.min(BLOCK);
        let mut w = Written::default();
        // values staged since the last block, each sound prepared once
        for sound in 0..if self.staged != 0 { SOUNDS } else { 0 } {
            if has(u32::from(self.staged), sound) {
                self.apply(sound);
            }
        }
        // parameter changes that waited for their sound to fall silent
        for sound in 0..if self.pending != 0 { SOUNDS } else { 0 } {
            if !has(u32::from(self.pending), sound) || self.busy(sound) {
                continue;
            }
            if let Some(d) = self.sounds.get_mut(sound) {
                *d = Prepared::new(&d.sound);
            }
            self.pending &= !bit(sound);
            self.refresh(sound);
        }
        let busy = self.live().fold(0, |m, l| m | bit(usize::from(l.sound)));
        // (without a voice there is nothing to shed; a note starts only within the limit, so at the
        // pool's own nothing is over it)
        if busy != 0 {
            while self.limit < VOICES && self.keeping(None) > self.limit && self.shed(None, None) {}
            for (sound, lim) in (0..SOUNDS as u8).zip(self.limits).filter(|(_, l)| l.limit > 0) {
                while self.keeping(Some(sound)) > usize::from(lim.limit) && self.shed(None, Some(sound)) {}
            }
        }
        let busy = events.iter().fold(busy, |m, e| m | bit(usize::from(e.sound)));
        let busy = (self.delayed.iter().take(self.delayed_n)).fold(busy, |m, d| m | bit(usize::from(d.0)));
        let outs = out.len().min(16);
        let port = |s: u8| {
            bus.get(usize::from(s))
                .map(|&b| usize::from(b))
                .filter(|&b| b < outs)
                .unwrap_or(0)
        };
        for (o, pair) in (0..outs).zip(out.iter_mut()) {
            // the sounds without pan add to the left channel (cleared when first written); those
            // with pan to the engine's pair, added to both channels at the output's end
            let (mut left, b) = (false, bit16(o));
            self.pan_on = false;
            for sound in (0..SOUNDS as u8).filter(|&s| has(u32::from(busy), usize::from(s)) && port(s) == o) {
                let mut from = 0;
                let mut ext = events.iter().filter(|e| e.sound == sound).peekable();
                loop {
                    // the next event, or the next delayed note to start (before an event at its frame)
                    let next = ext.peek().map(|e| e.frame.max(from).min(frames));
                    let due = self.due(sound, from, frames).filter(|d| next.is_none_or(|f| d.1 < f));
                    let Some(at) = due.map(|d| d.1).or(next) else { break };
                    self.render_sound::<V>(sound, pair, &mut left, from, at, end);
                    (from, self.at) = (at, at);
                    let s = usize::from(sound);
                    if let Some((_, h, _)) = due.and_then(|d| self.undelay(d.0)) {
                        self.play(s, h.id, h.key, h.velocity, end);
                        continue;
                    }
                    let Some(e) = ext.next() else { break };
                    match e.kind {
                        Kind::On { id, key, velocity } => self.note_on(s, id, key, velocity, end),
                        Kind::Off { id } => self.note_off(s, id, end),
                        Kind::Pedal { down } => self.pedal(s, down),
                        Kind::Bend { value } => self.bend(s, value),
                        Kind::Param { id, value } => self.param(s, id, value),
                        Kind::Choke { id } => self.choke_note(s, id, end),
                        Kind::Control { cc, value } => self.control(s, cc, value),
                        Kind::Volume { id, gain } => self.volume(s, id, gain),
                    }
                }
                self.render_sound::<V>(sound, pair, &mut left, from, frames, end);
            }
            let [l, r] = pair;
            if self.pan_on {
                if !left {
                    *l = [0.0; BLOCK];
                }
                let [pl, pr] = &self.pan;
                for (((l, r), pl), pr) in l.iter_mut().zip(r.iter_mut()).zip(pl).zip(pr) {
                    (*l, *r) = (*l + *pl, *l + *pr);
                }
                (w.left, w.right) = (w.left | b, w.right | b);
            } else if left {
                w.left |= b;
            }
        }
        for d in self.delayed.iter_mut().take(self.delayed_n) {
            d.2 = d.2.saturating_sub(frames as u32);
        }
        // the lanes emptied in the block leave the bits
        let lanes = &self.lanes;
        self.live = bits(self.live)
            .filter(|&i| lanes.get(i).is_some_and(|l| l.used != 0))
            .fold(0, |m, i| m | bit64(i));
        (self.clock, self.at) = (self.clock.wrapping_add(frames as u64), 0);
        w
    }

    /// Frames `from..to` of the lanes of `sound`, in control steps of 16 frames from `from` (the
    /// last one shorter).
    fn render_sound<V: F4>(
        &mut self,
        sound: u8,
        out: &mut [[f32; BLOCK]; 2],
        written: &mut bool,
        from: usize,
        to: usize,
        end: &mut impl FnMut(NoteEnd),
    ) {
        let (y1, y4) = (&mut self.y1, &mut self.y4);
        let Some(p) = self.sounds.get(usize::from(sound)) else {
            return;
        };
        // a sound with pan adds to the block's pair for those, the others to the left channel,
        // each cleared when a lane first plays into it
        let panned = p.filter.pan_on;
        let (pan_on, [left, right]) = (&mut self.pan_on, &mut self.pan);
        let mono = &mut out[0];
        // what the controllers do (a sound they leave alone skips it)
        let m = self.mods.get(usize::from(sound)).filter(|m| m.on);
        let lfo_speed = m.map_or(p.lfo_speed.1, |m| m.lfo_speed);
        let c = Ctx {
            rate: self.rate,
            shed_step: self.shed_step,
            retrig_step: self.retrig_step,
            ms: (1000.0 / self.rate) as f32,
            bend: self.bend.get(usize::from(sound)).map_or(0.0, |&b| {
                laws::bend_steps(i32::from(b), i32::from(p.bend.0), i32::from(p.bend.1)) as f32 / 2048.0
            }) + m.map_or(0.0, |m| m.pitch),
            fixed_max: (laws::FIXED_MAX_HZ / self.rate * 4_294_967_296.0) as f32,
            tick: self.tick,
            lfo_inc: if lfo_speed == p.lfo_speed.1 {
                lfo_inc(p.lfo_hz, self.rate)
            } else {
                lfo_inc(laws::lfo_hz(false, i32::from(lfo_speed)) as f32, self.rate)
            },
            lfo_hold: (f64::from(p.lfo_hold) * self.rate) as f32,
            lfo_speed,
            lfo_depths: m.map_or(p.lfo_depths, |m| m.lfo_depths),
            op_pitch: p.op_pitch_box.then(|| m.map_or(NO_PITCH, |m| &m.op_pitch)),
        };
        let clock = self.clock;
        // the sound LFO at each step's end, once for all the sound's lanes
        let mut sv = [0.0f32; BLOCK / STEP];
        if let Some(l) = self.slfo.get_mut(usize::from(sound)).filter(|_| p.slfo != 0) {
            l.steps(p, self.rate, to.saturating_sub(from), &mut sv);
        }
        // only the lanes in use: a filter over every lane, the bit tested first, still touched about
        // 40 lines more a block (measured); clearing bits in this loop changed the operator kernel's code
        // (32 voices of a sound 1.1 % dearer), so the emptied lanes leave at the block's end
        for i in bits(self.live) {
            let Some(lane) = self.lanes.get_mut(i).filter(|l| l.used != 0 && l.sound == sound) else {
                continue;
            };
            if panned && !*pan_on {
                (*left, *right, *pan_on) = ([0.0; BLOCK], [0.0; BLOCK], true);
            } else if !panned && !*written {
                (*mono, *written) = ([0.0; BLOCK], true);
            }
            let mut f = from;
            let mut k = 0usize;
            while f < to {
                let e = to.min(f.saturating_add(STEP));
                let o = if panned {
                    left.get_mut(f..e).zip(right.get_mut(f..e)).map(|(l, r)| Out::Pan(l, r))
                } else {
                    mono.get_mut(f..e).map(Out::Mono)
                };
                let Some(o) = o else { break };
                let t = clock.wrapping_add(f as u64);
                f = e;
                let v = sv.get(k).copied().unwrap_or(0.0);
                k = k.wrapping_add(1);
                lane.compact();
                // the lane's width follows its voices (kept in its first slots)
                match lane.used {
                    0 => break,
                    1 => step::<1, scalar::F<1>>(lane, p, &c, t, end, o, y1, v, m),
                    _ => step::<W, V>(lane, p, &c, t, end, o, y4, v, m),
                }
            }
        }
    }
}

/// What a lane's control step takes from the engine: the rate (Hz), the shed fade a step,
/// milliseconds a frame, the bend of the lane's sound (octaves), the highest increment a
/// fixed-mode operator plays, the glide clock's period (frames, 2^-16 a unit), and the voice
/// LFO's increment a frame and its delay's hold (frames).
struct Ctx<'a> {
    rate: f64,
    shed_step: f32,
    retrig_step: f32,
    ms: f32,
    bend: f32,
    fixed_max: f32,
    tick: u64,
    lfo_inc: u32,
    lfo_hold: f32,
    /// The voice LFO's speed value and depths' multipliers now ([`Mods`]); each operator's
    /// pitch from the boxes (a sound without a box on it: none, its pitch not recomputed).
    lfo_speed: i16,
    lfo_depths: (f32, f32),
    op_pitch: Option<&'a [f32; OPS]>,
}

const NO_PITCH: &[f32; OPS] = &[0.0; OPS];

/// The voice LFO's phase increment a frame at `hz` (2^32 a cycle).
fn lfo_inc(hz: f32, rate: f64) -> u32 {
    (f64::from(hz) / rate * 4_294_967_296.0) as u32
}

/// The voice LFO of a lane's voices over a step of `n` frames (`inc` a frame), at its end: per voice its pitch
/// share (-1 to 1, the swing's) and its amplitude share (0 at its top to 1 at its bottom), each
/// times the delay's fade in. The waves from phase 0: triangle and saw down from their top, saw up
/// and square from their bottom, sine from its middle going down, sample and hold a new value each
/// cycle.
#[inline(never)]
fn voice_lfo<const L: usize>(lane: &mut Lane, p: &Prepared, c: &Ctx, n: usize, inc: u32) -> ([f32; W], [f32; W]) {
    let (mut v, mut a) = ([0.0f32; W], [0.0f32; W]);
    let step = inc.wrapping_mul(n as u32);
    let live = (lane
        .lfo_ph
        .iter_mut()
        .zip(lane.lfo_t.iter_mut())
        .zip(lane.lfo_rnd.iter_mut()))
    .take(L);
    for (((ph, t), r), (v, a)) in live.zip(v.iter_mut().zip(a.iter_mut())) {
        let next = ph.wrapping_add(step);
        if next < *ph {
            *r = next_random(Some(r));
        }
        (*ph, *t) = (next, t.saturating_add(n as u32));
        let x = next as f32 * (1.0 / 4_294_967_296.0);
        let u = match p.lfo_wave {
            0 => (1.0 - 2.0 * x).abs(),
            1 => 1.0 - x,
            2 => x,
            3 => f32::from(u8::from(next >= 1 << 31)),
            4 => 0.5 - 0.5 * sine(next),
            _ => *r as f32 * (1.0 / 4_294_967_296.0),
        };
        let fade = if c.lfo_hold > 0.0 {
            ((*t as f32 - c.lfo_hold) / c.lfo_hold).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (*v, *a) = ((2.0 * u - 1.0) * fade, (1.0 - u) * fade);
    }
    (v, a)
}

/// The sine table at phase `p` (2^32 a cycle), straight between its points.
fn sine(p: u32) -> f32 {
    let i = (p >> 22) as usize;
    let (a, b) = (
        SINE.get(i).copied().unwrap_or(0.0),
        SINE.get(i.wrapping_add(1)).copied().unwrap_or(0.0),
    );
    a + (b - a) * ((p >> 6) & 0xffff) as f32 * (1.0 / 65536.0)
}

impl Slfo {
    /// A key-on of its sound at frame `now` (`first`: no other key of the sound is down): with
    /// key-on reset each, or first on a first key, the phase starts again (standing still for the
    /// measured onset first) with a new sample-and-hold value; without reset a first key takes the
    /// free-running phase. The envelope starts again with the phase, and on a first key without
    /// reset (shortcut: unmeasured).
    fn key_on(&mut self, p: &Prepared, first: bool, now: u64, rate: f64) {
        if p.slfo_reset == 1 || (p.slfo_reset == 2 && first) {
            self.ph = p.slfo_start;
            self.wait = (laws::SLFO_START_S * rate) as u32;
            self.held = random_bipolar(&mut self.rnd);
            self.t = 0;
        } else if first {
            let inc = u64::from(lfo_inc(p.slfo_hz, rate));
            self.ph = p.slfo_start.wrapping_add(inc.wrapping_mul(now) as u32);
            self.t = 0;
        }
    }

    /// Runs `frames` frames on in control steps of 16 (the last shorter), its value at each step's
    /// end into `out`: the wave (-1 to 1) times the envelope.
    fn steps(&mut self, p: &Prepared, rate: f64, frames: usize, out: &mut [f32; BLOCK / STEP]) {
        let inc = lfo_inc(p.slfo_hz, rate);
        let env = p.slfo_env.map(|s| s * rate as f32);
        let mut left = frames;
        for v in out.iter_mut() {
            if left == 0 {
                break;
            }
            let n = left.min(STEP) as u32;
            left = left.saturating_sub(STEP);
            let still = self.wait.min(n);
            self.wait = self.wait.saturating_sub(still);
            let run = n.saturating_sub(still);
            let next = self.ph.wrapping_add(inc.wrapping_mul(run));
            if next < self.ph {
                self.held = random_bipolar(&mut self.rnd);
            }
            (self.ph, self.t) = (next, self.t.saturating_add(run));
            *v = slfo_wave(p, next, self.held) * slfo_env(self.t as f32, &env);
        }
    }
}

/// A uniform value in [-1, 1) from the random source `r`.
fn random_bipolar(r: &mut u32) -> f32 {
    next_random(Some(r)) as f32 * (1.0 / 2_147_483_648.0) - 1.0
}

/// The sound LFO's envelope at `t` frames from its start: 0 through the delay, rising through the
/// fade in, 1 through the hold (for ever when infinite), falling through the fade out, then 0
/// (each stage in frames).
fn slfo_env(t: f32, &[delay, fade_in, hold, fade_out]: &[f32; 4]) -> f32 {
    let (up, down) = (delay + fade_in, delay + fade_in + hold);
    if t < delay {
        0.0
    } else if t < up {
        (t - delay) / fade_in
    } else if t < down {
        1.0
    } else if t < down + fade_out {
        1.0 - (t - down) / fade_out
    } else {
        0.0
    }
}

/// The sound LFO's wave at phase `ph` (2^32 a cycle; `held`: the sample-and-hold value), as
/// measured: triangle from 0 up; triangle 0 to 1; saw up 0 to 1; saw down 0 to -1; squares from
/// their top for 1/4, 1/3, 1/2, 2/3, 3/4 of the cycle; the trapezoid (the triangle doubled and
/// clipped); two sample and holds (shortcut: the second plays as the first, its difference not
/// resolved); the user wave's steps with their slopes (up: towards a higher next step, down:
/// towards a lower one, or both).
fn slfo_wave(p: &Prepared, ph: u32, held: f32) -> f32 {
    let x = ph as f32 * (1.0 / 4_294_967_296.0);
    let tri = if x < 0.25 {
        4.0 * x
    } else if x < 0.75 {
        2.0 - 4.0 * x
    } else {
        4.0 * x - 4.0
    };
    let top = |duty: u32| if ph < duty { 1.0 } else { -1.0 };
    match p.slfo_wave {
        0 => tri,
        1 => 1.0 - (1.0 - 2.0 * x).abs(),
        2 => x,
        3 => -x,
        4 => top(1 << 30),
        5 => top(0x5555_5555),
        6 => top(1 << 31),
        7 => top(0xaaaa_aaab),
        8 => top(3 << 30),
        9 => (2.0 * tri).clamp(-1.0, 1.0),
        10 | 11 => held,
        _ => {
            let n = u64::from(p.slfo_steps.clamp(2, 16));
            let pos = u64::from(ph).wrapping_mul(n);
            let i = (pos >> 32) as usize;
            let f = (pos & 0xffff_ffff) as f32 * (1.0 / 4_294_967_296.0);
            let a = p.slfo_user.get(i).copied().unwrap_or(0.0);
            let b = if i.wrapping_add(1) as u64 >= n {
                0
            } else {
                i.wrapping_add(1)
            };
            let b = p.slfo_user.get(b).copied().unwrap_or(0.0);
            let ramp = p.slfo_slope == 3 || (p.slfo_slope == 1 && b > a) || (p.slfo_slope == 2 && b < a);
            if ramp {
                a + (b - a) * f
            } else {
                a
            }
        }
    }
}

impl Ctx<'_> {
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

/// Where a step of a lane goes: the sum of the sounds without pan, or left and right.
enum Out<'a> {
    Mono(&'a mut [f32]),
    Pan(&'a mut [f32], &'a mut [f32]),
}

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
    out: Out,
    y: &mut [[[f32; L]; STEP]; OPS],
    sv: f32,
    m: Option<&Mods>,
) {
    let n = match &out {
        Out::Mono(m) => m.len(),
        Out::Pan(l, _) => l.len(),
    }
    .clamp(1, STEP);
    // a sound without a voice LFO skips this; the sound LFO's speed destination moves its rate
    let lfo = p.lfo_on.then(|| {
        let inc = if p.slfo & SLFO_SPEED != 0 {
            let speed = i32::from(c.lfo_speed).saturating_add((sv * p.slfo_speed) as i32);
            lfo_inc(laws::lfo_hz(p.lfo_speed.0, speed) as f32, c.rate)
        } else {
            c.lfo_inc
        };
        voice_lfo::<L>(lane, p, c, n, inc)
    });
    let bx = m.filter(|m| m.lvl_on).map(|m| &m.lvl);
    let (g0, dg) = envelopes::<L, V>(lane, p, c, n, end, lfo.as_ref().map(|l| &l.1), sv, bx);
    // the sound LFO's pitch and the boxes' on the operators (a sound without either skips this)
    let sp = (p.slfo & SLFO_PITCH != 0 || c.op_pitch.is_some()).then(|| {
        let mut sp = if p.slfo & SLFO_PITCH != 0 {
            slfo_pitch::<L>(lane, p, sv)
        } else {
            [[0.0; W]; OPS]
        };
        if let Some(o) = c.op_pitch {
            for (s, &b) in sp.iter_mut().zip(o.iter()) {
                s.iter_mut().for_each(|x| *x += b);
            }
        }
        sp
    });
    // a lane whose pitch does not move skips this
    let ramp = if lane.moving != 0 || lane.jump || lane.bend != c.bend || p.lfo_pitch_on || sp.is_some() {
        pitch::<L, V>(lane, p, c, t, n, lfo.as_ref().map_or(&[0.0; W], |l| &l.0), sp.as_ref())
    } else {
        None
    };
    // what the kernels play: the sound's plan, or its forms and feedback where the sound LFO
    // moves them
    let kern = (p.slfo & SLFO_KERN != 0).then(|| slfo_kern(p, sv));
    let (plan, fbk) = kern.as_ref().map_or((&p.plan, p.fbk), |k| (&k.0, k.1));
    let start = (n < STEP).then(|| (lane.ph, lane.fb[0]));
    let mut vout = [[0.0f32; L]; STEP];
    // a sound with sines only runs the kernels without the forms' branches; the increments ramp
    // only while a pitch envelope moves them
    let k = Kern { plan, fbk };
    match (plan.forms, &ramp) {
        (true, None) => ops::<L, V, true, false>(lane, k, &g0, &dg, &[[0; L]; OPS], y, &mut vout),
        (false, None) => ops::<L, V, false, false>(lane, k, &g0, &dg, &[[0; L]; OPS], y, &mut vout),
        (true, Some((d, _))) => ops::<L, V, true, true>(lane, k, &g0, &dg, d, y, &mut vout),
        (false, Some((d, _))) => ops::<L, V, false, true>(lane, k, &g0, &dg, d, y, &mut vout),
    }
    if let Some((ph0, fb0)) = start {
        short(lane, p, n, &ph0, &fb0, y, ramp.as_ref().map(|r| &r.0));
    }
    if let Some((_, to)) = ramp {
        for (inc, to) in lane.inc.iter_mut().zip(to.iter()) {
            V::U::load(*to).store4(inc);
        }
    }
    // the filter and pan (docs/design.md, 3.7)
    if p.filter.on {
        let lfo = lfo.as_ref().map_or(&[0.0; W], |l| &l.0);
        let bx = m.map_or((0.0, 0.0), |m| (m.cut, m.res));
        lane.post.filter::<L, V>(&p.filter, c.rate, sv, lfo, n, &mut vout, bx);
    }
    // expression and the notes' volumes (a lane without either skips this)
    let want = m.map_or(1.0, |m| m.gain);
    if want != 1.0 || lane.vol_on {
        lane.out_gain::<L, V>(want, n, &mut vout);
    }
    match out {
        Out::Pan(l, r) => lane.post.mix::<L, V>(&p.filter, sv, &vout, l, r),
        Out::Mono(m) => {
            for (o, v) in m.iter_mut().zip(vout.iter()) {
                *o += crate::filter::sum(v);
            }
        }
    }
}

/// What the operator kernels play: a plan and its feedback amount.
#[derive(Clone, Copy)]
struct Kern<'a> {
    plan: &'a Plan,
    fbk: f32,
}

/// The sound LFO's pitch at its value `sv`, per operator and voice (octaves): the frequency
/// destination's share of its swing, and the detune destination's move (steps rounded towards 0,
/// the detune held within -15..+15) at each voice's key.
#[inline(never)]
fn slfo_pitch<const L: usize>(lane: &mut Lane, p: &Prepared, sv: f32) -> [[f32; W]; OPS] {
    let mut out = [[0.0f32; W]; OPS];
    let ops = (0..OPS).zip(out.iter_mut().zip(lane.sdt.iter_mut().zip(lane.sdt_d.iter_mut())));
    for (op, (o, (moved, held))) in ops {
        let swing = sv * at(&p.slfo_pitch, op);
        let d0 = i32::from(p.detune.get(op).copied().unwrap_or(0));
        let d = d0.saturating_add((sv * at(&p.slfo_detune, op)) as i32).clamp(-15, 15);
        // the move is made again only when the detune changes (or a voice started)
        if d != d0 && i32::from(*held) != d {
            for (m, &key) in moved.iter_mut().zip(lane.key.iter()).take(L) {
                let k = i32::from(key).wrapping_add(i32::from(p.note_shift));
                *m = laws::detune_steps(d, k).wrapping_sub(laws::detune_steps(d0, k)) as f32 / 2048.0;
            }
            *held = d as i8;
        }
        for (x, &m) in o.iter_mut().zip(moved.iter()).take(L) {
            *x = swing + if d == d0 { 0.0 } else { m };
        }
    }
    out
}

/// What the kernels play while the sound LFO (at its value `sv`) moves operators' forms or the
/// feedback: the plan with the moved forms (steps rounded towards 0, within 0-6) and the feedback
/// amount at the moved feedback (within 0-7).
#[inline(never)]
fn slfo_kern(p: &Prepared, sv: f32) -> (Plan, f32) {
    let fb = i32::from(p.feedback)
        .saturating_add((sv * p.slfo_fb) as i32)
        .clamp(0, 7);
    let waves = core::array::from_fn(|op| {
        let k = at(&p.slfo_form, op);
        match (p.forms.get(op), p.plan.waves.get(op)) {
            (Some(&(f, s, n)), _) if k != 0.0 => Wave::new(
                i32::from(f).saturating_add((sv * k) as i32).clamp(0, 6),
                i32::from(s),
                i32::from(n),
            ),
            (_, w) => w.copied().unwrap_or(Wave::Sine),
        }
    });
    (
        p.plan.with_waves(waves),
        p.slfo_fbk.get(fb as usize).copied().unwrap_or(0.0),
    )
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
    lfo: &[f32; W],
    sp: Option<&[[f32; W]; OPS]>,
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
        lane.jump = true;
    } else {
        gliding = (0..L)
            .zip(pt.glide.iter())
            .fold(0, |m, (s, &g)| if g != 0.0 { m | bit(s) } else { m });
    }
    if lane.jump || lane.bend != c.bend {
        (lane.jump, lane.bend) = (false, c.bend);
        for (inc, to) in lane.inc.iter_mut().zip(incs::<L, V>(pt, p, c, lfo, sp).iter()) {
            V::U::load(*to).store4(inc);
        }
    }
    let moves = |seg: u8| matches!(seg, ATTACK | DECAY1 | DECAY2 | RELEASE);
    if pt.op_attack | pt.op_decay == 0 && !pt.c_seg.iter().take(L).any(|&s| moves(s)) && !p.lfo_pitch_on && sp.is_none()
    {
        lane.moving = gliding;
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
    lane.moving = moving | gliding;
    let to = incs::<L, V>(pt, p, c, lfo, sp);
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
/// operator's own envelope, the bend, the voice LFO's share `lfo` of each operator's swing, the
/// sound LFO's pitch `sp`), held under Nyquist (fixed-mode ones at their highest frequency); a
/// voice that nothing moves keeps its base exactly.
#[inline(always)]
fn incs<const L: usize, V: F4<L>>(
    pt: &Pitch,
    p: &Prepared,
    c: &Ctx,
    lfo: &[f32; W],
    sp: Option<&[[f32; W]; OPS]>,
) -> [[u32; L]; OPS] {
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
        if p.lfo_pitch_on && !fixed {
            // (times the depth a box moves to, or 1)
            m = V::load4(lfo).mul_add(V::splat(at(&p.lfo_pitch, op) * c.lfo_depths.0), m);
        }
        if let Some(s) = sp.and_then(|s| s.get(op)) {
            m = m + V::load4(s);
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
/// its ramp a frame (with the voice LFO's amplitude share `lfo`, each operator's loss through the
/// bend, and the sound LFO's at its value `sv`, straight in dB); moves segments on, fades shed
/// voices, and ends voices (the end rule, a fade's end: the bound leaves the LFOs' losses out);
/// `bx` the controller boxes' levels, when they move any.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn envelopes<const L: usize, V: F4<L>>(
    lane: &mut Lane,
    p: &Prepared,
    c: &Ctx,
    n: usize,
    end: &mut impl FnMut(NoteEnd),
    lfo: Option<&[f32; W]>,
    sv: f32,
    bx: Option<&[f32; OPS]>,
) -> ([[f32; L]; OPS], [[f32; L]; OPS]) {
    let amp = lfo.filter(|_| p.lfo_amp_on);
    // the sound LFO's level destination: none lost at its top, all at its bottom
    let slfo_amp = (p.slfo & SLFO_AMP != 0).then_some(1.0 - sv);
    // a step of `n` frames moves each envelope `k` of a 16-frame step's way
    let (k, per_frame) = (n as f32 / STEP as f32, V::splat(1.0 / n as f32));
    let (shed, retrig) = (c.shed_step * k, c.retrig_step * k);
    // shed voices fade linearly, a retrigger's by the square of the share left
    for (s, (f, left)) in lane.fade.iter_mut().zip(lane.left.iter_mut()).enumerate().take(L) {
        if has(u32::from(lane.ending), s) {
            let sq = has(u32::from(lane.retrig), s);
            let step = if sq { retrig } else { shed };
            // (to 0 on the last step, whatever the rounding of the steps before)
            *left = if *left > step * 1.5 { *left - step } else { 0.0 };
            *f = if sq { *left * *left } else { *left };
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
        let heard = match amp {
            Some(a) => {
                let k = at(&p.lfo_amp, i) * c.lfo_depths.1;
                let mut loss = [0.0f32; L];
                for (l, &a) in loss.iter_mut().zip(a.iter()) {
                    *l = laws::amp_loss_log2(a * k);
                }
                (l2 + V::load(loss)).max(V::splat(FLOOR))
            }
            None => l2,
        };
        let heard = match slfo_amp {
            Some(u) => (heard - V::splat(u * at(&p.slfo_amp, i))).max(V::splat(FLOOR)),
            None => heard,
        };
        // the controller boxes' level
        let heard = match bx {
            Some(b) => (heard + V::splat(at(b, i))).max(V::splat(FLOOR)),
            None => heard,
        };
        let target = heard.exp2() * fade;
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
    p: Kern,
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
    p: Kern,
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
            let v = read::<L, V>(SINE, ph_v.add(offset((y1 + y2).mul_add(fbk, V::load(*m))))) * g;
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
            *out = (read::<L, V>(SINE, ph_v.add(offset(V::load(*m)))) * g).store();
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
        Wave::Sine => run::<L, V, RAMP>(|q, o| read(SINE, q.add(o)), own, cur, input, inc, g, d, ph),
    }
}

/// An odd form: E(2t + m) sin(t + m).
#[inline(always)]
fn odd<const L: usize, V: F4<L>>(t: &Table16, ph: V::U, off: V::U) -> V {
    read16::<L, V>(t, ph.add(ph).add(off)) * read::<L, V>(SINE, ph.add(off))
}

/// A resonant form: window(t + m) sin(centre t + m).
#[inline(always)]
fn resonant<const L: usize, V: F4<L>>(t: &Table16, centre: u32, ph: V::U, off: V::U) -> V {
    read16::<L, V>(t, ph.add(off)) * read::<L, V>(SINE, ph.mul(V::U::splat(centre)).add(off))
}

/// An operator's output at phase `ph` and modulation offset `off`.
#[inline(always)]
fn wave<const L: usize, V: F4<L>>(w: Wave, ph: V::U, off: V::U) -> V {
    match w {
        Wave::Sine => read(SINE, ph.add(off)),
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
    p: Kern,
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
    p: Kern,
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
                read::<L, V>(SINE, pa.add(offset(m)))
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

    impl Engine {
        /// [`Engine::render`] of sounds without pan: one channel, checked to be both, added to
        /// `out`.
        fn render1<V: F4>(&mut self, out: &mut [f32; BLOCK], end: &mut impl FnMut(NoteEnd)) {
            self.render1_events::<V>(out, &[], end);
        }

        fn render1_events<V: F4>(&mut self, out: &mut [f32; BLOCK], events: &[Event], end: &mut impl FnMut(NoteEnd)) {
            let mut o = [[f32::NAN; BLOCK]; 2]; // (written, whatever it held)
            self.render_events::<V>(&mut o, events, end);
            assert!(
                o[0].iter()
                    .zip(o[1].iter())
                    .all(|(l, r)| l.to_bits() == r.to_bits() && !l.is_nan()),
                "left and right differ, or a frame was not written"
            );
            out.iter_mut().zip(o[0]).for_each(|(a, b)| *a += b);
        }
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
                e.note_off(0, 1, &mut |_| {});
            }
            let mut out = [0.0; BLOCK];
            e.render1::<V>(&mut out, &mut |_| {});
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

    /// Level scaling on a lone carrier at level 70 (+ linear above break point 39, key 60 one
    /// group up: 7 steps), capped at level 99's; the macros and part offsets as the laws move the
    /// operators' values.
    #[test]
    fn scaling_macros_and_offsets() {
        let step = 32.0 * 6.020_599_913_279_624 / 256.0;
        let at = |extra: &str, key: u8| {
            let x = play::<Native>(&carrier(1, &format!("op1.level=70 {extra}")), key, 100.0, 100, 40);
            peak_db(&x[BLOCK * 8..])
        };
        let base = at("", 60);
        let up = "op1.scaling_high_depth=99 op1.scaling_high_curve=3";
        assert!((at(up, 60) - base - 7.0 * step).abs() < 0.02, "{}", at(up, 60) - base);
        assert!((at(up, 96) - base - 21.99).abs() < 0.02); // capped at level 99
        let down = "op1.scaling_low_depth=99 op1.scaling_low_curve=0";
        assert!((at(down, 52) - base + 7.0 * step).abs() < 0.02);
        assert!((at(down, 60) - base).abs() < 1e-6); // the high side has no depth
                                                     // routing 2: operators 1-3 a stack of modulators (operator 1 feeding back), 4-8 carriers
        let r = routing(2);
        assert!(r.carriers == 0xf8 && r.fb_from == 0);
        let p = |t: &str| sound(&format!("algorithm=2 feedback=3 op4.level=99 {t}"));
        let full = |t: &str, op: usize| p(t).full[op];
        // depth: added to modulators' levels, divided by 1 + feedback on the one feeding back
        assert_eq!(full("op2.level=50 macro_depth=10", 1), full("op2.level=60", 1));
        assert_eq!(full("op1.level=50 macro_depth=10", 0), full("op1.level=52", 0));
        assert_eq!(full("op5.level=50 macro_depth=10", 4), full("op5.level=50", 4));
        // the envelope macros on modulators: time and level values; the part's offsets on all
        let m = p("macro_attack=10 macro_decay=-5 macro_sustain=20 macro_release=3 op2.eg_level_decay2=50");
        let want = p("op2.eg_time_attack=10 op2.eg_time_release=43 op2.eg_level_decay2=70");
        assert_eq!((m.ms[1], m.eg[1]), (want.ms[1], want.eg[1]));
        assert_eq!((m.ms[3], m.eg[3]), (p("").ms[3], p("").eg[3]));
        let o = p("amp_eg_decay_offset=2 amp_eg_release_offset=-2 amp_eg_sustain_offset=3 op4.eg_time_decay1=10");
        let want = p("op4.eg_time_decay1=13 op4.eg_time_release=37 op4.eg_level_decay2=99");
        assert_eq!((o.ms[3][1], o.ms[3][3]), (want.ms[3][1], want.ms[3][3]));
        let s = p("amp_eg_sustain_offset=-3 op4.eg_level_decay2=50");
        assert_eq!(s.eg[3], p("op4.eg_level_decay2=45").eg[3]);
        // the attack offset doubles attack times every 4 steps, raising them on carriers only
        let a = p("amp_eg_attack_offset=4 op4.eg_time_attack=50 op2.eg_time_attack=50");
        assert_eq!(a.ms[3][0], (laws::attack_ms_per_db(50) * 2.0) as f32);
        assert_eq!(a.ms[1][0], laws::attack_ms_per_db(50) as f32);
        // the ratio macro on ratio-mode modulators; texture: a carrier's harmonic form at spread
        // 3 plays a sine at -25
        let q = p("macro_ratio=99 op2.coarse=1 op4.coarse=1");
        assert_eq!(q.steps[1], laws::ratio_steps(laws::shifted_ratio(1.0, 99)));
        assert_eq!(q.steps[3], laws::ratio_steps(1.0));
        let tex = "op5.level=99 op5.form=1 op5.spread=3";
        assert_eq!(p(&format!("{tex} macro_texture=-25")).plan.waves[4], Wave::Sine);
        assert_eq!(
            p(&format!("{tex} macro_texture=25")).plan.waves[4],
            p(tex).plan.waves[4]
        );
    }

    /// The voice LFO on a lone carrier against its per-sample law: the pitch route (triangle,
    /// 1 Hz, 93 cents, key-on reset) within 0.2 cent of the law's mean frequency over every eight
    /// cycles; the waves' starts; the amplitude route's swing; the delay's hold and fade in.
    #[test]
    fn voice_lfo() {
        let lfo = "lfo_speed_range=0 lfo_speed=6 lfo_key_on_reset=1 op1.lfo_pitch_offset=3";
        let f0 = zero_crossing_hz(&play::<Native>(&carrier(1, ""), 69, 100.0, 1000, 40)[BLOCK * 4..]);
        let x = play::<Native>(&carrier(1, &format!("{lfo} lfo_pitch_depth=30")), 69, 100.0, 1000, 800);
        let swing = laws::lfo_pitch_cents(30, 3) / 1200.0;
        let hz = laws::lfo_hz(false, 6);
        let law = |k: f64| {
            let u = (hz * k / RATE).fract();
            f0 * (swing * ((1.0 - 2.0 * u).abs() * 2.0 - 1.0)).exp2()
        };
        let mut c = Vec::new();
        for i in 1..x.len() {
            if x[i - 1] < 0.0 && x[i] >= 0.0 {
                c.push(i as f64 - 1.0 + f64::from(x[i - 1] / (x[i - 1] - x[i])));
            }
        }
        let mut worst = 0.0f64;
        for w in c.windows(9).step_by(8) {
            let (a, b) = (w[0], w[8]);
            let got = 8.0 / (b - a) * RATE;
            let want = (a.ceil() as usize..b.floor() as usize)
                .map(|k| law(k as f64))
                .sum::<f64>()
                / (b.floor() - a.ceil());
            worst = worst.max((1200.0 * (got / want).log2()).abs());
        }
        // (the vector exp2 alone is within 0.15 cent)
        assert!(worst < 0.2, "worst {worst} cents");
        // the square starts low, the saw up low, the saw down high (a quarter of a cycle in)
        let quarter = |wave: i32| {
            let s = carrier(1, &format!("{lfo} lfo_pitch_depth=30 lfo_wave={wave}"));
            let x = play::<Native>(&s, 69, 100.0, 1000, 150);
            1200.0 * (zero_crossing_hz(&x[9000..15000]) / f0).log2()
        };
        assert!(quarter(3) < -0.99 * swing * 1200.0, "{}", quarter(3));
        assert!(quarter(2) < -0.3 * swing * 1200.0 && quarter(1) > 0.3 * swing * 1200.0);
        // amplitude: depth 30 at offset 7 swings 6 dB, none at the LFO's top
        let amp = "lfo_speed_range=0 lfo_speed=6 lfo_key_on_reset=1 lfo_amp_depth=30 op1.lfo_amp_offset=7";
        let x = play::<Native>(&carrier(1, amp), 69, 100.0, 1000, 400);
        let blocks: Vec<f64> = x.chunks(BLOCK * 4).skip(2).map(peak_db).collect();
        let (hi, lo) = blocks
            .iter()
            .fold((-200.0f64, 0.0f64), |(h, l), &b| (h.max(b), l.min(b)));
        assert!(
            (hi - laws::CARRIER_FULL_DBFS).abs() < 0.05 && (hi - lo - 6.0).abs() < 0.1,
            "{hi} {lo}"
        );
        // delay 50: nothing during the hold, the full swing after the fade in
        let hold = laws::lfo_delay_s(50) * RATE;
        let s = carrier(1, &format!("{amp} lfo_delay=50"));
        let x = play::<Native>(&s, 69, 100.0, 1000, (2.0 * hold + 1.2 * RATE) as usize / BLOCK);
        let early = peak_db(&x[BLOCK * 4..(hold as usize - BLOCK)]) - peak_db(&x[BLOCK * 4..BLOCK * 8]);
        assert!(early.abs() < 1e-3, "{early}");
        let late: Vec<f64> = x[(2.0 * hold) as usize + BLOCK..].chunks(BLOCK).map(peak_db).collect();
        let lo = late.iter().fold(0.0f64, |l, &b| l.min(b));
        assert!((laws::CARRIER_FULL_DBFS - lo - 6.0).abs() < 0.1, "{lo}");
    }

    /// The sound LFO's waves, phases, user wave, envelope and onset against the laws written out
    /// here, at each step's end: its value after a key-on reset at frame 0.
    #[test]
    fn sound_lfo_waves_and_envelope() {
        let tri = |x: f64| {
            if x < 0.25 {
                4.0 * x
            } else if x < 0.75 {
                2.0 - 4.0 * x
            } else {
                4.0 * x - 4.0
            }
        };
        let sq = |x: f64, d: f64| if x < d { 1.0 } else { -1.0 };
        let user = [40, -64, 63, 0, 10, -20, 30, -40, 50, -60, 5, 15, 25, 35, 45, 55];
        let steps: String = (0..16).map(|i| format!(" slfo_step{}={}", i + 1, user[i])).collect();
        let step_v = move |i: usize| f64::from(if user[i] == 63 { 64 } else { user[i] }) / 64.0;
        // wave 12 at user cycle 4 (8 steps) and each slope
        let user_law = move |x: f64, slope: i32| {
            let pos = x * 8.0;
            let i = (pos as usize).min(7);
            let (a, b) = (step_v(i), step_v((i + 1) % 8));
            let ramp = slope == 3 || (slope == 1 && b > a) || (slope == 2 && b < a);
            if ramp {
                a + (b - a) * pos.fract()
            } else {
                a
            }
        };
        type Law = Box<dyn Fn(f64) -> f64>;
        let mut cases: Vec<(String, Law)> = vec![
            ("slfo_wave=0".into(), Box::new(tri)),
            ("slfo_wave=1".into(), Box::new(|x: f64| 1.0 - (1.0 - 2.0 * x).abs())),
            ("slfo_wave=2".into(), Box::new(|x| x)),
            ("slfo_wave=3".into(), Box::new(|x: f64| -x)),
            ("slfo_wave=4".into(), Box::new(move |x| sq(x, 0.25))),
            ("slfo_wave=5".into(), Box::new(move |x| sq(x, 1.0 / 3.0))),
            ("slfo_wave=6".into(), Box::new(move |x| sq(x, 0.5))),
            ("slfo_wave=7".into(), Box::new(move |x| sq(x, 2.0 / 3.0))),
            ("slfo_wave=8".into(), Box::new(move |x| sq(x, 0.75))),
            ("slfo_wave=9".into(), Box::new(move |x| (2.0 * tri(x)).clamp(-1.0, 1.0))),
            (
                "slfo_wave=0 slfo_phase=2".into(),
                Box::new(move |x| tri((x + 1.0 / 3.0).fract())),
            ),
            (
                "slfo_wave=6 slfo_phase=5".into(),
                Box::new(move |x| sq((x + 0.75).fract(), 0.5)),
            ),
        ];
        for slope in 0..4 {
            let text = format!("slfo_wave=12 slfo_user_cycle=4 slfo_user_slope={slope}{steps}");
            cases.push((text, Box::new(move |x| user_law(x, slope))));
        }
        let (hz, onset) = (laws::slfo_hz(40), (laws::SLFO_START_S * RATE) as u32);
        for (text, law) in &cases {
            let p = sound(&format!("slfo_speed=40 slfo_dest1=75 slfo_depth1=8 {text}"));
            let mut l = SLFO0;
            l.key_on(&p, true, 0, RATE);
            let inc = lfo_inc(p.slfo_hz, RATE);
            let mut out = [0.0f32; BLOCK / STEP];
            for b in 0..300u32 {
                l.steps(&p, RATE, BLOCK, &mut out);
                for (k, &v) in out.iter().enumerate() {
                    let run = (b * BLOCK as u32 + 16 * (k as u32 + 1)).saturating_sub(onset);
                    let ph = inc.wrapping_mul(run);
                    let x = f64::from(ph) / 4_294_967_296.0;
                    // (single precision; a square's edge may fall either side of a step's end)
                    let want = law(x);
                    let edge = (law((x + 1e-6).fract()) - law((x + 1.0 - 1e-6).fract())).abs() > 0.1;
                    assert!(edge || (f64::from(v) - want).abs() < 1e-5, "{text}: {v} {want} at {x}");
                }
            }
            assert!((f64::from(inc) / 4_294_967_296.0 * RATE / hz - 1.0).abs() < 1e-5);
        }
        // sample and hold: one uniform value a cycle, new at each wrap (both waves)
        for w in [10, 11] {
            let p = sound(&format!("slfo_speed=63 slfo_dest1=78 slfo_depth1=8 slfo_wave={w}"));
            let mut l = SLFO0;
            l.key_on(&p, true, 0, RATE);
            let (mut out, mut held) = ([0.0f32; BLOCK / STEP], Vec::new());
            for _ in 0..3000 {
                let before = l.ph;
                l.steps(&p, RATE, BLOCK, &mut out);
                assert!(out.iter().all(|&v| (-1.0..1.0).contains(&v)));
                if l.ph < before || held.is_empty() {
                    held.push(out[7]);
                }
            }
            let mean = held.iter().map(|&v| f64::from(v)).sum::<f64>() / held.len() as f64;
            assert!(held.len() > 90 && mean.abs() < 0.15, "{} {mean}", held.len());
        }
        // the envelope: delay 40 (151 ms), fade in 64 (452 ms), hold 48 (211 ms), fade out 80 (1.024 s)
        let p = sound("slfo_wave=4 slfo_speed=0 slfo_delay=40 slfo_fade_in=64 slfo_hold=48 slfo_fade_out=80 slfo_dest1=78 slfo_depth1=8");
        let [d, fi, h, fo] = [0.151, 0.452, 0.211, 1.024].map(|s| s * RATE);
        let env = |t: f64| (((t - d) / fi).min(1.0).min(1.0 - (t - d - fi - h) / fo)).clamp(0.0, 1.0);
        let mut l = SLFO0;
        l.key_on(&p, true, 0, RATE);
        let mut out = [0.0f32; BLOCK / STEP];
        for b in 0..(2.0 * RATE) as usize / BLOCK {
            l.steps(&p, RATE, BLOCK, &mut out);
            for (k, &v) in out.iter().enumerate() {
                let t = ((b * BLOCK + 16 * (k + 1)) as f64 - f64::from(onset)).max(0.0);
                assert!((f64::from(v) - env(t)).abs() < 1e-4, "{t}: {v} {}", env(t));
            }
        }
        // hold 127 holds for ever
        let p = sound("slfo_wave=4 slfo_speed=0 slfo_fade_in=10 slfo_hold=127 slfo_dest1=78 slfo_depth1=8");
        let mut l = SLFO0;
        l.key_on(&p, true, 0, RATE);
        for _ in 0..(30.0 * RATE) as usize / BLOCK {
            l.steps(&p, RATE, BLOCK, &mut out);
        }
        assert_eq!(out[7], 1.0);
    }

    /// The sound LFO's destinations through the engine: frequency and detune (pitch), level, and
    /// the stepped ones (feedback, form, the voice LFO's speed: at the LFO's top the same bits as
    /// the sound with the moved value); key-on reset modes; the pitch route against the
    /// per-sample law.
    #[test]
    fn sound_lfo_destinations() {
        // a square from its top, slow (speed 10: 3.7 s a half cycle) so the first second is the top
        let slow = "slfo_wave=6 slfo_speed=10 slfo_key_on_reset=1";
        let f0 = zero_crossing_hz(&play::<Native>(&carrier(1, ""), 69, 100.0, 1000, 40)[BLOCK * 4..]);
        let cents = |x: &[f32]| 1200.0 * (zero_crossing_hz(x) / f0).log2();
        let x = play::<Native>(
            &carrier(1, &format!("{slow} slfo_dest1=75 slfo_depth1=8")),
            69,
            100.0,
            1000,
            300,
        );
        assert!((cents(&x[2000..]) - laws::slfo_pitch_cents(8)).abs() < 0.2);
        // the depth ratio (64: depth 4) and the box (off: none)
        let x = play::<Native>(
            &carrier(1, &format!("{slow} slfo_dest1=75 slfo_depth1=8 op1.slfo_ratio1=64")),
            69,
            100.0,
            1000,
            40,
        );
        assert!((cents(&x[2000..]) - laws::slfo_pitch_cents(4)).abs() < 0.2);
        let x = play::<Native>(
            &carrier(1, &format!("{slow} slfo_dest1=75 slfo_depth1=8 op1.slfo_box1=0")),
            69,
            100.0,
            1000,
            40,
        );
        assert!(cents(&x[2000..]).abs() < 0.05);
        // detune: depth 64 is 16 steps at the top, held at +15
        let x = play::<Native>(
            &carrier(1, &format!("{slow} slfo_dest1=77 slfo_depth1=64")),
            69,
            100.0,
            1000,
            40,
        );
        let want = 1200.0
            * (laws::steps_hz(laws::key_steps(69) + laws::detune_steps(15, 69)) / laws::steps_hz(laws::key_steps(69)))
                .log2();
        assert!((cents(&x[2000..]) - want).abs() < 0.2, "{} {want}", cents(&x[2000..]));
        // a voice joining the lane while the detune holds gets its own key's move
        let mut e = engine(&carrier(1, "slfo_wave=6 slfo_speed=10 slfo_dest1=77 slfo_depth1=64"));
        let mut out = [0.0; BLOCK];
        e.note_on(0, 1, 36, 100.0, &mut |_| {});
        e.render1::<Native>(&mut out, &mut |_| {});
        e.note_on(0, 2, 96, 100.0, &mut |_| {});
        e.render1::<Native>(&mut out, &mut |_| {});
        let law = |k: i32| (laws::detune_steps(15, k) - laws::detune_steps(0, k)) as f32 / 2048.0;
        assert_eq!((e.lanes[0].sdt[0][0], e.lanes[0].sdt[0][1]), (law(36), law(96)));
        // level: depth 32 loses 18.85 dB at the bottom, nothing at the top (a fast square)
        let s = carrier(
            1,
            "slfo_wave=6 slfo_speed=40 slfo_key_on_reset=1 slfo_dest1=78 slfo_depth1=32",
        );
        let x = play::<Native>(&s, 69, 100.0, 1000, 400);
        let blocks: Vec<f64> = x.chunks(BLOCK * 4).skip(2).map(peak_db).collect();
        let (hi, lo) = blocks
            .iter()
            .fold((-200.0f64, 0.0f64), |(h, l), &b| (h.max(b), l.min(b)));
        assert!(
            (hi - laws::CARRIER_FULL_DBFS).abs() < 0.05 && (hi - lo - laws::slfo_level_db(32)).abs() < 0.1,
            "{hi} {lo}"
        );
        // stepped destinations at the top: the same bits as the moved value
        let same = |lfo: &str, moved: &str| {
            let base = "algorithm=1 op1.level=99 op2.level=80 op8.level=0 portamento=0";
            let a = play::<Native>(&sound(&format!("{base} {slow} {lfo}")), 60, 100.0, 1000, 300);
            let b = play::<Native>(&sound(&format!("{base} {moved}")), 60, 100.0, 1000, 300);
            assert!(a.iter().zip(&b).all(|(a, b)| a.to_bits() == b.to_bits()), "{lfo}");
        };
        same("slfo_dest1=74 slfo_depth1=112", "feedback=7");
        same("feedback=2 slfo_dest2=74 slfo_depth2=40", "feedback=4");
        same(
            "slfo_dest1=76 slfo_depth1=96 op2.spread=3 op1.slfo_box1=0",
            "op2.form=6 op2.spread=3",
        );
        same("slfo_dest3=76 slfo_depth3=127 op2.form=3 op1.slfo_box3=0", "op2.form=6");
        let vlfo = "lfo_speed_range=0 lfo_pitch_depth=30 op2.lfo_pitch_offset=7";
        same(
            &format!("{vlfo} lfo_speed=20 slfo_dest1=71 slfo_depth1=64"),
            &format!("{vlfo} lfo_speed=70"),
        );
        // the pitch route against the per-sample law: a triangle from 0 up, 8 cycles at a time
        let s = carrier(
            1,
            "slfo_wave=0 slfo_speed=40 slfo_key_on_reset=1 slfo_dest1=75 slfo_depth1=8",
        );
        let x = play::<Native>(&s, 69, 100.0, 1000, 800);
        let (hz, swing, onset) = (
            laws::slfo_hz(40),
            laws::slfo_pitch_cents(8) / 1200.0,
            laws::SLFO_START_S * RATE,
        );
        let law = |k: f64| {
            let u = (hz * (k - onset).max(0.0) / RATE).fract();
            let t = if u < 0.25 {
                4.0 * u
            } else if u < 0.75 {
                2.0 - 4.0 * u
            } else {
                4.0 * u - 4.0
            };
            f0 * (swing * t).exp2()
        };
        let mut c = Vec::new();
        for i in 1..x.len() {
            if x[i - 1] < 0.0 && x[i] >= 0.0 {
                c.push(i as f64 - 1.0 + f64::from(x[i - 1] / (x[i - 1] - x[i])));
            }
        }
        let mut worst = 0.0f64;
        for w in c.windows(9).step_by(8) {
            let (a, b) = (w[0], w[8]);
            let want = (a.ceil() as usize..b.floor() as usize)
                .map(|k| law(k as f64))
                .sum::<f64>()
                / (b.floor() - a.ceil());
            worst = worst.max((1200.0 * (8.0 / (b - a) * RATE / want).log2()).abs());
        }
        assert!(worst < 0.2, "worst {worst} cents");
    }

    /// Key-on reset: each note starts the phase again; first only when no key of the sound is
    /// down; off keeps it free-running from the engine's clock (a first key starts the envelope).
    #[test]
    fn sound_lfo_key_on_reset() {
        let run = |reset: i32, events: &[Event]| {
            let p = sound(&format!(
                "slfo_wave=2 slfo_speed=40 slfo_dest1=75 slfo_depth1=8 slfo_key_on_reset={reset}"
            ));
            let mut e = engine(&p);
            let mut out = [0.0; BLOCK];
            e.render1_events::<Native>(&mut out, events, &mut |_| {});
            e.slfo[0]
        };
        let inc = lfo_inc(laws::slfo_hz(40) as f32, RATE);
        let onset = (laws::SLFO_START_S * RATE) as u32;
        let two = [on(10, 0, 1, 60), on(100, 0, 2, 64)];
        // each: the second note at 100 starts it again (still at the block's end, 28 frames on)
        let l = run(1, &two);
        assert_eq!((l.ph, l.wait, l.t), (0, onset - 28, 0));
        // first: the second note is legato, the phase runs on from the first note's start
        let l = run(2, &two);
        assert_eq!((l.ph, l.wait), (0, onset - 118));
        // off: free-running from frame 0 of the clock
        let l = run(0, &two);
        assert_eq!((l.ph, l.t, l.wait), (inc.wrapping_mul(128), 118, 0));
        // first after a release: starts again
        let l = run(
            2,
            &[
                on(10, 0, 1, 60),
                Event {
                    frame: 50,
                    sound: 0,
                    kind: Kind::Off { id: 1 },
                },
                on(100, 0, 2, 64),
            ],
        );
        assert_eq!((l.ph, l.wait), (0, onset - 28));
    }

    /// A parameter event takes effect at its frame (the same bits as the change made between two
    /// blocks when the frame is a block's start); a change of what is rendered waits for silence.
    #[test]
    fn parameter_events() {
        let p = carrier(
            1,
            "lfo_speed_range=0 lfo_speed=40 lfo_key_on_reset=1 op1.lfo_pitch_offset=7",
        );
        let run = |events: &[Event], direct: bool| {
            let mut e = engine(&p);
            e.note_on(0, 1, 60, 100.0, &mut |_| {});
            let mut x = Vec::new();
            for b in 0..6 {
                let mut out = [0.0; BLOCK];
                if b == 3 && direct {
                    e.param(0, 27, 50);
                }
                e.render1_events::<Native>(&mut out, if b == 3 { events } else { &[] }, &mut |_| {});
                x.extend_from_slice(&out);
            }
            x
        };
        let ev = |frame| Event {
            frame,
            sound: 0,
            kind: Kind::Param { id: 27, value: 50 },
        };
        let (none, direct) = (run(&[], false), run(&[], true));
        assert_eq!(run(&[ev(0)], false), direct);
        let mid = run(&[ev(64)], false);
        assert_eq!(mid[..BLOCK * 3 + 64], none[..BLOCK * 3 + 64]);
        assert_ne!(mid[BLOCK * 4..], none[BLOCK * 4..]);
        // out of range: ignored
        assert_eq!(
            run(
                &[Event {
                    kind: Kind::Param { id: 27, value: 100 },
                    ..ev(0)
                }],
                false
            ),
            none
        );
        // operator 2 heard from now: waits while the note sounds, then plays
        let mut e = engine(&p);
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        e.param(0, 1164, 99);
        assert!(e.pending == 1 && e.sounds[0].plan == p.plan);
        e.note_off(0, 1, &mut |_| {});
        let mut out = [0.0; BLOCK];
        for _ in 0..400 {
            e.render1::<Native>(&mut out, &mut |_| {});
        }
        assert!(e.pending == 0 && e.sounds[0].plan != p.plan);
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
            e.note_off(0, id, &mut |_| {});
            e.choke(0, &mut |_| {});
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
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
                    e.note_off(0, 1, &mut |_| {});
                }
                e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
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
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
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
                e.note_off(0, 3, &mut |_| {});
            }
            let mut out = [0.0; BLOCK];
            e.render1::<Native>(&mut out, &mut |n| ends.push(n));
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
        e.note_off(0, 1, &mut |_| {});
        for _ in 0..200 {
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| ends += 1);
        }
        assert_eq!((e.active(), ends), (1, 0));
        e.pedal(0, false);
        for _ in 0..200 {
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| ends += 1);
        }
        assert_eq!((e.active(), ends), (0, 1));
    }

    /// `blocks` blocks of `e` with `first` applied in the first.
    fn blocks(e: &mut Engine, first: &[Event], blocks: usize) -> Vec<f32> {
        let mut out = vec![0.0; blocks * BLOCK];
        for (b, o) in out.chunks_mut(BLOCK).enumerate() {
            let o: &mut [f32; BLOCK] = o.try_into().unwrap();
            e.render1_events::<Native>(o, if b == 0 { first } else { &[] }, &mut |_| {});
        }
        out
    }

    fn control(cc: u8, value: u8) -> Event {
        let kind = Kind::Control { cc, value };
        Event {
            frame: 0,
            sound: 0,
            kind,
        }
    }

    /// Expression scales the output by (v / 127)^2 (ramped over one step), a note's volume
    /// expression its own voice; the soft pedal and controllers with no route leave the bits as
    /// they were, and expression back at 127 plays as never moved.
    #[test]
    fn expression_and_note_volume() {
        // (no portamento: the second note would glide from the first's pitch)
        let p = carrier(1, "portamento=0 op1.eg_time_decay1=60 op1.eg_level_decay1=0");
        let held = |events: &[Event]| {
            let mut e = engine(&p);
            e.note_on(0, 1, 60, 100.0, &mut |_| {});
            e.note_on(0, 2, 67, 100.0, &mut |_| {});
            blocks(&mut e, &[], 2);
            blocks(&mut e, events, 4)
        };
        let plain = held(&[]);
        let quiet = held(&[control(11, 64)]);
        let k = (64.0f32 / 127.0).powi(2);
        // after the first step's ramp, every frame scaled
        for (a, b) in plain.iter().zip(&quiet).skip(STEP) {
            assert!((a * k - b).abs() <= 1e-6 * a.abs().max(1e-3), "{a} {b}");
        }
        let untouched = [control(1, 90), control(2, 30), control(4, 127), control(67, 127)];
        let bits = |x: &[f32]| x.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&held(&untouched)), bits(&plain));
        // back to 127: the same bits once the ramp is over
        let mut e = engine(&p);
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        e.note_on(0, 2, 67, 100.0, &mut |_| {});
        blocks(&mut e, &[control(11, 0)], 2);
        let back = blocks(&mut e, &[control(11, 127)], 4);
        assert_eq!(bits(&back[STEP..]), bits(&plain[STEP..]));
        // a note's volume: half of note 2 alone
        let alone = |id: u32, gain: f32| {
            let mut e = engine(&p);
            e.note_on(0, id, if id == 1 { 60 } else { 67 }, 100.0, &mut |_| {});
            blocks(&mut e, &[], 2);
            let kind = Kind::Volume { id, gain };
            blocks(
                &mut e,
                &[Event {
                    frame: 0,
                    sound: 0,
                    kind,
                }],
                4,
            )
        };
        let (one, two) = (alone(1, 1.0), alone(2, 0.5));
        let both = held(&[Event {
            frame: 0,
            sound: 0,
            kind: Kind::Volume { id: 2, gain: 0.5 },
        }]);
        for ((a, b), c) in one.iter().zip(&two).zip(&both).skip(STEP) {
            assert!((a + b - c).abs() < 1e-5, "{a} + {b} against {c}");
        }
    }

    /// A controller box plays its destination as the static value it moves it to: the mod wheel
    /// through a +16 box on operator 1's level (24 steps at full travel, 12 at 64) and a +8 box on
    /// the cutoff (128 values), set before the note or moved under a held one; an operator whose
    /// switch for the box is off keeps its level; the knobs and the bend reach their boxes.
    #[test]
    fn boxes_play_their_destinations() {
        let at = |l1: i32, l2: i32, cut: i32| {
            format!(
                "algorithm=24 portamento=0 filter_type=0 resonance=40 filter_eg_depth=0 cutoff={cut} \
                 op1.level={l1} op2.level={l2} op2.coarse=2"
            )
        };
        let base = at(60, 70, 300);
        // box 1 reaches operator 1 only (every operator's switch is on by default)
        let off = |ops: &[usize]| ops.iter().map(|o| format!(" op{o}.ctrl_set1=0")).collect::<String>();
        let only1 = off(&[2, 3, 4, 5, 6, 7, 8]);
        let boxed = |extra: &str| {
            sound(&format!(
                "{base} {extra} box1_dst=106 box1_ratio=16 box2_dst=102 box2_ratio=8{only1}"
            ))
        };
        let fixed = |l1: i32, l2: i32, cut: i32| sound(&at(l1, l2, cut));
        let play = |p: &Prepared, before: &[Event], during: &[Event]| {
            let mut e = engine(p);
            blocks(&mut e, before, 1);
            e.note_on(0, 1, 60, 100.0, &mut |_| {});
            blocks(&mut e, during, 1);
            blocks(&mut e, &[], 6)
        };
        let near = |a: &[f32], b: &[f32]| {
            let peak = a.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.01);
            let worst = a.iter().zip(b).fold(0.0f32, |m, (x, y)| m.max((x - y).abs())) / peak;
            assert!(worst <= 2e-5, "{worst} of the peak {peak}");
            true
        };
        let reference = |l1, cut| play(&fixed(l1, 70, cut), &[], &[]);
        let wheel = |v| Event {
            frame: 0,
            sound: 0,
            kind: Kind::Control { cc: 1, value: v },
        };
        // the wheel before the note: at rest, at 64 and at the top
        assert!(near(&play(&boxed(""), &[], &[]), &reference(60, 300)));
        assert!(near(&play(&boxed(""), &[wheel(64)], &[]), &reference(72, 364)));
        assert!(near(&play(&boxed(""), &[wheel(127)], &[]), &reference(84, 428)));
        // moved under a held note: the same from the step after its ramp
        let moved = play(&boxed(""), &[], &[wheel(127)]);
        assert!(near(&moved[STEP..], &reference(84, 428)[STEP..]));
        // operator 2's switch on instead: it moves operator 2 and leaves operator 1
        let two = sound(&format!(
            "{base} box1_dst=106 box1_ratio=16{}",
            off(&[1, 3, 4, 5, 6, 7, 8])
        ));
        assert!(near(
            &play(&two, &[wheel(127)], &[]),
            &play(&fixed(60, 94, 300), &[], &[])
        ));
        // a knob's stored value, the bend at its top (a bipolar box: 11.99 steps at 8191, toward 0)
        assert!(near(
            &play(&boxed("box1_src=8 knob1=1023"), &[], &[]),
            &reference(84, 300)
        ));
        let bend = Event {
            frame: 0,
            sound: 0,
            kind: Kind::Bend { value: 8191 },
        };
        let p = sound(&format!(
            "{base} box1_src=0 box1_dst=106 box1_ratio=16 box1_pol=1 bend_up=0{only1}"
        ));
        assert!(near(&play(&p, &[bend], &[]), &reference(71, 300)));
    }

    /// The boxes on the voice LFO's speed and depths play as the stored values they move to, and
    /// the box on the sound's pitch as that many cents; moved under a held note, from the next
    /// step on.
    #[test]
    fn boxes_move_the_voice_lfo_and_the_pitch() {
        let lfo = "lfo_speed_range=0 lfo_key_on_reset=1 op1.lfo_pitch_offset=5 op1.lfo_amp_offset=7";
        let s = |extra: &str| carrier(1, &format!("{lfo} {extra}"));
        let play = |p: &Prepared, before: &[Event]| {
            let mut e = engine(p);
            blocks(&mut e, before, 1);
            e.note_on(0, 1, 69, 100.0, &mut |_| {});
            blocks(&mut e, &[], 30)
        };
        let wheel = |v| control(1, v);
        let near = |a: &[f32], b: &[f32]| {
            let worst = a.iter().zip(b).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
            assert!(worst < 1e-3, "{worst}");
            true
        };
        // the pitch depth: 40 + 25 steps (ratio 16 at the top), 40 - 12 (ratio -16 at 64)
        let pm = |r: i32| s(&format!("lfo_pitch_depth=40 box1_dst=90 box1_ratio={r}"));
        assert!(near(
            &play(&pm(16), &[wheel(127)]),
            &play(&s("lfo_pitch_depth=65"), &[])
        ));
        assert!(near(
            &play(&pm(-16), &[wheel(64)]),
            &play(&s("lfo_pitch_depth=28"), &[])
        ));
        assert!(near(&play(&pm(16), &[]), &play(&s("lfo_pitch_depth=40"), &[])));
        // the amplitude depth from 0, the speed (with a pitch depth to hear it)
        let am = s("box1_dst=91 box1_ratio=32");
        assert!(near(&play(&am, &[wheel(127)]), &play(&s("lfo_amp_depth=50"), &[])));
        let sp = s("lfo_pitch_depth=30 lfo_speed=20 box1_dst=89 box1_ratio=32");
        assert!(near(
            &play(&sp, &[wheel(64)]),
            &play(&s("lfo_pitch_depth=30 lfo_speed=45"), &[])
        ));
        // the pitch: +1024 cents at the wheel's 64 (ratio 16), moved under the note (the vector
        // exp2 within 0.15 cent)
        let f0 = zero_crossing_hz(&play(&s(""), &[])[BLOCK * 4..]);
        let pitch = s("box1_dst=54 box1_ratio=16");
        let up = zero_crossing_hz(&play(&pitch, &[wheel(64)])[BLOCK * 4..]);
        assert!((1200.0 * (up / f0).log2() - 1024.0).abs() < 0.15, "{f0} {up}");
        let mut e = engine(&pitch);
        e.note_on(0, 1, 69, 100.0, &mut |_| {});
        blocks(&mut e, &[], 2);
        let x = blocks(&mut e, &[wheel(64)], 8);
        assert!((1200.0 * (zero_crossing_hz(&x[BLOCK..]) / f0).log2() - 1024.0).abs() < 0.15);
        // an operator's frequency: 450 cents at the wheel's 64 (ratio 16) by sensitivity +7, -3
        // sevenths of it at -3, and back where it was with the wheel at 0
        let op = |sens: i32| s(&format!("box1_dst=105 box1_ratio=16 op1.pitch_ctrl_sens={sens}"));
        for (sens, cents) in [(7, 450.0), (-3, -450.0 * 3.0 / 7.0), (0, 0.0)] {
            let hz = zero_crossing_hz(&play(&op(sens), &[wheel(64)])[BLOCK * 4..]);
            assert!((1200.0 * (hz / f0).log2() - cents).abs() < 0.15, "{sens}: {hz}");
        }
        let mut e = engine(&op(7));
        e.note_on(0, 1, 69, 100.0, &mut |_| {});
        blocks(&mut e, &[wheel(64)], 2);
        let x = blocks(&mut e, &[wheel(0)], 8);
        assert!((1200.0 * (zero_crossing_hz(&x[BLOCK..]) / f0).log2()).abs() < 0.15);
    }

    /// The sostenuto pedal holds the notes whose keys were down when it went down, not later ones;
    /// up, it lets them go unless the sustain pedal holds them.
    #[test]
    fn sostenuto_holds_the_notes_down_when_it_went_down() {
        let p = carrier(1, "op1.eg_time_release=0");
        let run = |e: &mut Engine, n: usize| {
            let mut ends = 0;
            for _ in 0..n {
                e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| ends += 1);
            }
            ends
        };
        let mut e = engine(&p);
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        e.control(0, 66, 127);
        e.note_on(0, 2, 64, 100.0, &mut |_| {});
        e.note_off(0, 1, &mut |_| {});
        e.note_off(0, 2, &mut |_| {});
        assert_eq!((run(&mut e, 200), e.active()), (1, 1));
        e.pedal(0, true);
        e.control(0, 66, 0);
        assert_eq!((run(&mut e, 200), e.active()), (0, 1));
        e.pedal(0, false);
        assert_eq!((run(&mut e, 200), e.active()), (1, 0));
        // a note whose key is still down when the pedal goes up keeps sounding until its note off
        e.note_on(0, 3, 60, 100.0, &mut |_| {});
        e.control(0, 66, 127);
        e.control(0, 66, 0);
        assert_eq!((run(&mut e, 200), e.active()), (0, 1));
        e.note_off(0, 3, &mut |_| {});
        assert_eq!((run(&mut e, 200), e.active()), (1, 0));
    }

    /// Which notes survive at the limit: the voice-budget order.
    #[test]
    fn shedding_order() {
        let quiet = carrier(1, "op1.level=50");
        let loud = carrier(1, "");
        let run = |e: &mut Engine, ends: &mut Vec<u32>| {
            for _ in 0..4 {
                e.render1::<Native>(&mut [0.0; BLOCK], &mut |n| ends.push(n.id));
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
        e.note_off(0, 2, &mut |_| {});
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

    /// The order, case by case, as the platform SDK's helper orders them (cases from its self-test,
    /// examples/voice-budget/selftest.cpp in third_party/oscairn-clap, cited by line; the others
    /// worked by hand from voice-budget-plugin.hpp's `tier()`, `before()` and `victim()`).
    #[test]
    fn the_order_is_the_sdk_helpers() {
        let v = Rank::new;
        // selftest.cpp 28-36: a released voice of a priority-5 part before a held key of a
        // priority-0 part (tier before priority)
        let (held, released) = (v(true, false, 0.5, 3000.0, 1), v(false, false, 0.5, 3000.0, 2));
        assert!(released.first(false, 5, &held, false, 0) && !held.first(false, 0, &released, false, 5));
        // selftest.cpp 40-47: B released at -30 dB, 2 s old, before A released at -40 dB struck 48 ms
        // ago; 2.7 ms on, A is no longer struck and goes first
        let (b, a) = (v(false, false, 0.0316, 2000.0, 1), v(false, false, 0.01, 48.0, 2));
        assert!(a.struck && b.before(&a));
        let (b, a) = (v(false, false, 0.0316, 2002.7, 1), v(false, false, 0.01, 50.7, 2));
        assert!(!a.struck && a.before(&b));
        // selftest.cpp 51 and 69-81: released voices quietest first; twenty, the sixteen least
        // important listed in order
        let mut list = [Rank::default(); LEAST];
        let mut n = 0u8;
        for i in [7u64, 19, 0, 3, 12, 15, 1, 18, 9, 4, 11, 2, 16, 6, 14, 5, 13, 8, 17, 10] {
            insert(
                &mut list,
                &mut n,
                v(false, false, 0.001 * (i + 1) as f64, 5000.0, i + 1),
            );
        }
        assert_eq!(list.map(|r| r.serial), core::array::from_fn(|i| i as u64 + 1));
        // a released quiet voice before a held loud one
        assert!(v(false, false, 0.01, 3000.0, 2).before(&v(true, false, 1.0, 3000.0, 1)));
        // released, then held only by a pedal, then key held
        let (r, p, k) = (
            v(false, false, 0.5, 3000.0, 1),
            v(false, true, 0.5, 3000.0, 2),
            v(true, false, 0.5, 3000.0, 3),
        );
        assert_eq!((r.tier, p.tier, k.tier), (0, 1, 2));
        assert!(r.before(&p) && p.before(&k));
        // below -60 dBFS a held key counts as released, unless struck under 50 ms ago
        let (quiet, struck) = (v(true, false, 0.0005, 3000.0, 1), v(true, false, 0.0005, 10.0, 2));
        assert_eq!((quiet.tier, struck.tier, struck.struck), (0, 2, true));
        assert!(
            quiet.before(&v(false, false, 0.0316, 3000.0, 3)),
            "then by level: -66 before -30"
        );
        // a voice just struck goes after the others of its tier, however quiet
        assert!(v(true, false, 1.0, 3000.0, 1).before(&v(true, false, 0.1, 20.0, 2)));
        // within a tier: the new note's own sound first, then the lower priority, then audibility,
        // a dB a second of age (at most ten), then the older
        let (loud, soft) = (v(true, false, 1.0, 3000.0, 1), v(true, false, 0.5, 3000.0, 2));
        assert!(loud.first(false, 5, &soft, true, 0) && loud.first(false, 0, &soft, false, 5));
        assert!(soft.first(false, 0, &loud, false, 0));
        assert!(
            v(true, false, 1.0, 9000.0, 1).before(&v(true, false, 0.5, 1000.0, 2)),
            "0 - 9 dB before -6 - 1 dB"
        );
        assert!(
            v(true, false, 1.0, 20_000.0, 1).before(&v(true, false, 1.0, 60_000.0, 2)),
            "ten dB at most: the same, so the older note"
        );
        // the level: 20 log10 of the gain, -120 to +24
        assert!((v(true, false, 0.5, 0.0, 1).db - -6.0206).abs() < 1e-3);
        assert_eq!(
            (v(true, false, 0.0, 0.0, 1).db, v(true, false, 100.0, 0.0, 1).db),
            (-120.0, 24.0)
        );
    }

    /// Sound 0 with notes `ids` held (a block apart, each `key` 40 + its id), then aged to 60 ms past
    /// the last.
    fn held_notes(ids: core::ops::Range<u32>) -> Engine {
        let mut e = engine(&carrier(1, ""));
        for id in ids {
            e.note_on(0, id, 40 + id as u8, 100.0, &mut |_| {});
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        }
        for _ in 0..23 {
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        }
        e
    }

    /// Each sound's list: (sound, voices, the listed notes' ids, their ranks).
    fn lists(e: &mut Engine) -> Vec<(u8, u16, Vec<u32>, Vec<Rank>)> {
        let mut out = Vec::new();
        e.importance(|s, v, least| out.push((s, v, Vec::new(), least.to_vec())));
        for (_, _, ids, least) in out.iter_mut() {
            *ids = least
                .iter()
                .map(|r| e.lanes[usize::from(r.lane)].id[usize::from(r.slot)])
                .collect();
        }
        out
    }

    #[test]
    fn importance_lists_each_sounds_least_important_in_the_order() {
        let mut e = held_notes(0..20);
        assert!(e.set_sound(1, &carrier(1, "op1.level=80")));
        e.note_on(1, 20, 60, 100.0, &mut |_| {});
        e.note_off(0, 7, &mut |_| {});
        e.pedal(0, true);
        e.note_off(0, 9, &mut |_| {});
        e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        let l = lists(&mut e);
        assert_eq!(
            l.iter().map(|x| (x.0, x.1, x.2.len())).collect::<Vec<_>>(),
            vec![(0, 20, LEAST), (1, 1, 1)]
        );
        // the released note, the pedaled one, then the held keys oldest first (equally loud)
        let mut want = vec![7, 9];
        want.extend((0..20).filter(|i| ![7, 9].contains(i)).take(LEAST - 2));
        assert_eq!(l[0].2, want);
        let r = &l[0].3;
        assert_eq!(r.iter().map(|r| r.tier).collect::<Vec<_>>()[..3], [0, 1, 2]);
        assert!(r.windows(2).all(|w| !w[1].before(&w[0])), "least important first");
        assert!(r
            .iter()
            .all(|r| !r.struck && r.db > -60.0 && r.db <= 24.0 && r.age_ms >= 50.0 && r.age_ms < 200.0));
        assert!(r[2..].windows(2).all(|w| w[0].age_ms > w[1].age_ms));
        // sound 1's one note: struck under 50 ms ago, quieter
        let s1 = &l[1].3[0];
        assert!(s1.struck && s1.tier == 2 && s1.age_ms < 50.0 && s1.db < r[2].db);
        // a sound at its reserve lists none of its held keys
        let mut lim = [Limits::default(); SOUNDS];
        lim[0].reserve = 18;
        e.set_limits(0, &lim);
        assert_eq!(lists(&mut e)[0].2, vec![7, 9]);
        lim[0].reserve = 16;
        e.set_limits(0, &lim);
        assert_eq!(
            lists(&mut e)[0].2,
            vec![7, 9, 0, 1],
            "past it, the two held keys beyond it"
        );
    }

    /// The ids of the notes ending (fading out) in sound 0.
    fn ending(e: &Engine) -> Vec<u32> {
        let mut v: Vec<u32> = e
            .lanes
            .iter()
            .filter(|l| l.sound == 0)
            .flat_map(|l| (0..W).filter(|&s| (l.ending & l.used) >> s & 1 != 0).map(|s| l.id[s]))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_lowered_limit_gives_up_the_voices_listed_first() {
        // the order alone would now take another: the listed ones go, each checked by its serial
        let first_n = |n: usize, between: &dyn Fn(&mut Engine, &[u32])| {
            let mut e = held_notes(0..24);
            let listed = lists(&mut e)[0].2.clone();
            between(&mut e, &listed);
            let keep = e.keeping(Some(0)).saturating_sub(n);
            let mut lim = [Limits::default(); SOUNDS];
            lim[0].limit = keep as u8;
            e.set_limits(0, &lim);
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
            (ending(&e), listed, e)
        };
        let (ends, listed, _) = first_n(3, &|_, _| {});
        let mut want = listed[..3].to_vec();
        want.sort();
        assert_eq!(ends, want, "exactly the first three listed");
        // the oldest notes listed first, then released: the order alone takes the released ones
        let (ends, listed, _) = first_n(2, &|e, _| {
            e.note_off(0, 23, &mut |_| {});
            e.note_off(0, 22, &mut |_| {});
        });
        assert_eq!(
            (ends, &listed[..2]),
            (vec![0, 1], &[0, 1][..]),
            "listed first, not the released"
        );
        // a listed note that ended by itself in between is skipped, and its slot's new note kept
        let (ends, listed, e) = first_n(2, &|e, listed| {
            let has_id = |l: &Lane, id: u32| (0..W).any(|s| l.used >> s & 1 != 0 && l.id[s] == id);
            let lane = e.lanes.iter().position(|l| has_id(l, listed[0])).unwrap();
            e.choke_note(0, listed[0], &mut |_| {});
            for _ in 0..3 {
                e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
            }
            assert_eq!(e.lanes[lane].used.count_ones(), 3, "it ended");
            e.note_on(0, 99, 100, 100.0, &mut |_| {});
            assert!(
                has_id(&e.lanes[lane], 99),
                "a new note in its lane, in a slot a listed note had"
            );
        });
        assert_eq!(ends, {
            let mut w = listed[1..3].to_vec();
            w.sort();
            w
        });
        assert!(e
            .lanes
            .iter()
            .any(|l| (0..W).any(|s| l.used >> s & 1 != 0 && l.id[s] == 99 && l.ending >> s & 1 == 0)));
        // with nothing listed left, the order: the first listed after the list's end
        let (ends, listed, _) = first_n(LEAST + 2, &|_, _| {});
        assert_eq!(listed, (0..LEAST as u32).collect::<Vec<_>>());
        assert_eq!(ends, (0..LEAST as u32 + 2).collect::<Vec<_>>());
    }

    #[test]
    fn shed_notes_fade_out_in_3_ms() {
        let mut e = engine(&carrier(1, ""));
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        for _ in 0..8 {
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        }
        e.choke(0, &mut |_| {});
        let mut x = [0.0; BLOCK * 2];
        let mut ended_at = None;
        for (b, out) in x.chunks_exact_mut(BLOCK).enumerate() {
            e.render1::<Native>(out.try_into().unwrap(), &mut |_| ended_at = Some(b));
        }
        assert_eq!(ended_at, Some(1));
        // a linear fade: no step larger than the fade's slope allows, silent from 3 ms on
        let a = laws::db_to_gain(laws::CARRIER_FULL_DBFS) as f32;
        assert!(x[150..].iter().all(|&v| v == 0.0));
        assert!(x[..120].iter().any(|&v| v.abs() > a * 0.5));
    }

    /// A random sound: every parameter the engine plays at a random value in its range.
    fn random_sound(r: &mut impl FnMut() -> u32) -> Prepared {
        let mut s = Sound::default();
        // (and the pitch parameters: both envelopes, key follow, random pitch, mono, key assign,
        // portamento, bend ranges, detune)
        // and the voice LFO, the macros, the part's envelope offsets and level scaling; the sound
        // LFO; the filter, its envelope and pan
        let played = |id: u16| {
            [30u16, 31, 209, 212, 213].contains(&id)
                || (0..=3).contains(&id)
                || (6..=29).contains(&id)
                || (32..=40).contains(&id)
                || (100..=134).contains(&id)
                || (200..=208).contains(&id)
                || (214..=216).contains(&id)
                || (219..=247).contains(&id)
                || (249..=255).contains(&id)
        };
        for p in PARAMS.iter() {
            let k = p.id % 100;
            let op_played = p.op.is_some() && ((40..=70).contains(&k) || k == 71 || k == 72);
            if played(p.id) || op_played {
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
                    a.choke(s, &mut |n| record(n, &mut ended));
                    b.choke(s, &mut |_| {});
                    for _ in 0..4 {
                        let mut o = [[0.0; BLOCK]; 2];
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
                            a.note_off(s, id, &mut |n| record(n, &mut ended));
                            b.note_off(s, id, &mut |_| {});
                        }
                    }
                    8 if r() % 2 == 0 => {
                        let (s, down) = ((r() % 3) as usize, r() % 2 == 0);
                        if timed {
                            event(s, Kind::Pedal { down });
                        } else {
                            a.pedal(s, down);
                            b.pedal(s, down);
                        }
                    }
                    // a controller (the sostenuto pedal among them) or a note's volume
                    8 => {
                        let s = (r() % 3) as usize;
                        let (cc, value) = ([1, 2, 4, 11, 66, 67, PRESSURE][r() as usize % 7], (r() % 128) as u8);
                        let (id, gain) = (r() % (next + 1), (r() % 500) as f32 / 100.0);
                        if timed {
                            event(s, Kind::Control { cc, value });
                            event(s, Kind::Volume { id, gain });
                        } else {
                            a.control(s, cc, value);
                            b.control(s, cc, value);
                            a.volume(s, id, gain);
                            b.volume(s, id, gain);
                        }
                    }
                    9 if r() % 2 == 0 => {
                        let (s, value) = ((r() % 3) as usize, (r() % 16384) as i16 - 8192);
                        if timed {
                            event(s, Kind::Bend { value });
                        } else {
                            a.bend(s, value);
                            b.bend(s, value);
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
            // (every frame is written: a NaN left would fail below)
            let (mut oa, mut ob) = ([[f32::NAN; BLOCK]; 2], [[f32::NAN; BLOCK]; 2]);
            a.render_events::<Native>(&mut oa, &ev, &mut |n| record(n, &mut ended));
            b.render_events::<scalar::F>(&mut ob, &ev, &mut |_| {});
            let (oa, ob) = (oa.as_flattened(), ob.as_flattened());
            assert!(
                oa.iter().zip(ob).all(|(x, y)| x.to_bits() == y.to_bits()),
                "block {block}: back ends differ"
            );
            // finite and bounded, and no subnormal (the filter's states are flushed)
            let ok = |v: &f32| v.is_finite() && v.abs() < 2000.0 && (*v == 0.0 || v.abs() >= f32::MIN_POSITIVE);
            assert!(oa.iter().all(ok), "block {block}");
            assert_eq!(a.active(), b.active());
        }
        // everything released: every note rings out and ends
        for s in 0..SOUNDS {
            a.pedal(s, false);
            a.choke(s, &mut |n| record(n, &mut ended));
        }
        for _ in 0..100 {
            a.render::<Native>(&mut [[0.0; BLOCK]; 2], &mut |n| record(n, &mut ended));
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
        // the filter and pan state is read only when the sound has them
        let post = size_of::<Voices>();
        assert!(size_of::<Lane>() - post <= 2048, "{} B a lane", size_of::<Lane>());
        assert!(post <= 384, "{post} B of filter and pan a lane");
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
            let p = sound(&format!("algorithm={r} feedback=6 portamento=0\n{text}"));
            let mut e = engine(&p);
            for v in 0..6 {
                e.note_on(0, v, 40 + 7 * v as u8, 30.0 + 15.0 * v as f32, &mut |_| {});
            }
            for b in 0..24 {
                if b == 12 {
                    (0..6).for_each(|v| e.note_off(0, v, &mut |_| {}));
                }
                let mut out = [0.0; BLOCK];
                e.render1::<Native>(&mut out, &mut |_| {});
                h = out.iter().fold(h, |h, x| h.rotate_left(5) ^ u64::from(x.to_bits()));
            }
        }
        h
    }

    /// Renders one note of `p` at key 48 for `blocks` blocks, held: left and right.
    fn stereo(e: &mut Engine, id: u32, blocks: usize) -> (Vec<f32>, Vec<f32>) {
        e.note_on(0, id, 48, 100.0, &mut |_| {});
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for _ in 0..blocks {
            let mut o = [[0.0; BLOCK]; 2];
            e.render::<Native>(&mut o, &mut |_| {});
            l.extend_from_slice(&o[0]);
            r.extend_from_slice(&o[1]);
        }
        e.note_off(0, id, &mut |_| {});
        (l, r)
    }

    /// The filter after the operators: high-pass 12 at resonance 0 (Q 0.5) at cutoff value 512
    /// (448.9 Hz) takes a 130.9 Hz carrier down by the second-order high-pass's 22.1 dB (the
    /// section is close to the analog one far below a sixth of the rate); the gain 0.375 dB a step,
    /// through thru too.
    #[test]
    fn filter_in_the_engine() {
        // (the text form takes a line's parameters in id order)
        let f = |lfo: &str, filter: &str, slfo: &str| {
            held(&format!("{lfo} {filter} filter_eg_depth=0 cutoff_key_follow=0 {slfo}"))
        };
        let (thru, f0) = f("", "", "");
        let (hp, _) = f("", "filter_type=7 cutoff=512 resonance=0", "");
        let (w, wc) = (f0, f64::from(crate::filter::cutoff_hz(512.0)));
        let want = 20.0 * (w * w / ((wc * wc - w * w).powi(2) + (2.0 * w * wc).powi(2)).sqrt()).log10();
        let got = peak_db(&hp[BLOCK * 20..]) - peak_db(&thru[BLOCK * 20..]);
        assert!((got - want).abs() < 0.3, "{got:.2} dB, the law {want:.2}");
        let (quiet, _) = f("", "filter_gain=191", "");
        let got = peak_db(&quiet) - peak_db(&thru);
        assert!((got + 24.0).abs() < 0.01, "{got:.3} dB");
        // the voice LFO and the sound LFO move the cutoff, and the sound LFO the resonance: the
        // filtered level swings
        for (lfo, filter, slfo) in [
            (
                "lfo_wave=3 lfo_speed=40 lfo_filter_depth=40",
                "filter_type=7 cutoff=512 resonance=0",
                "",
            ),
            (
                "",
                "filter_type=7 cutoff=512 resonance=0",
                "slfo_wave=6 slfo_speed=50 slfo_dest1=72 slfo_depth1=40",
            ),
            (
                "",
                "filter_type=11 cutoff=400 resonance=0",
                "slfo_wave=6 slfo_speed=50 slfo_dest1=73 slfo_depth1=100",
            ),
        ] {
            let m = format!("{lfo} {filter} {slfo}");
            let (x, _) = f(lfo, filter, slfo);
            let w = x.chunks(1200).map(peak_db).collect::<Vec<_>>();
            let (lo, hi) = w.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
            assert!(hi - lo > 3.0, "{m}: {lo:.1} to {hi:.1} dB");
        }
    }

    /// Pan: alternate pan puts the first note on its side and the next on the other (the far side
    /// silent at 63); a centred position through the pan path plays the mono bits in both
    /// channels; the sound LFO's pan moves it.
    #[test]
    fn pan_in_the_engine() {
        let mut e = engine(&carrier(1, "alternate_pan=63"));
        let (l, r) = stereo(&mut e, 1, 8);
        assert!(peak_db(&l) < -200.0 && peak_db(&r) > -30.0);
        e.choke(0, &mut |_| {});
        for _ in 0..3 {
            e.render::<Native>(&mut [[0.0; BLOCK]; 2], &mut |_| {});
        }
        let (l, r) = stereo(&mut e, 2, 8);
        assert!(peak_db(&l) > -30.0 && peak_db(&r) < -200.0);
        // scaling pan has no positions between keys 60 and 64 (key 48 here: centre at depth 0
        // otherwise); random pan at depth 1 draws within half a position
        let mono = play::<Native>(&carrier(1, ""), 48, 100.0, 8, 8);
        for extra in ["random_pan=1", "scaling_pan=63 note_shift=14"] {
            let mut e = engine(&carrier(1, extra));
            let (l, r) = stereo(&mut e, 1, 8);
            let shifted = play::<Native>(&carrier(1, extra.replace("random_pan=1", "").as_str()), 48, 100.0, 8, 8);
            let want = if extra.contains("note_shift") { &shifted } else { &mono };
            assert!(l.iter().zip(want).all(|(a, b)| a.to_bits() == b.to_bits()), "{extra}");
            assert!(l.iter().zip(&r).all(|(a, b)| a.to_bits() == b.to_bits()), "{extra}");
        }
        let mut e = engine(&carrier(1, "slfo_wave=6 slfo_speed=50 slfo_dest1=70 slfo_depth1=127"));
        let (l, r) = stereo(&mut e, 1, 60);
        let w = |x: &[f32]| x.chunks(1200).map(peak_db).collect::<Vec<_>>();
        let (wl, wr) = (w(&l[BLOCK * 8..]), w(&r[BLOCK * 8..]));
        assert!(
            wl.iter().any(|&v| v < -60.0) && wr.iter().any(|&v| v < -60.0),
            "{wl:?} {wr:?}"
        );
    }

    /// Key-on delay: a note with delay 1 (10 blocks of 64 at 44.1 kHz, 697 frames at 48 kHz) plays
    /// the bits of the same note started at that frame; released before then it never sounds and
    /// ends at once.
    #[test]
    fn key_on_delay() {
        let at = (10.0 * 64.0 / 44.1 * RATE / 1000.0 + 0.5) as usize;
        assert_eq!(at, 697);
        let render = |p: &Prepared, timed: bool| {
            let mut e = engine(p);
            let mut x = Vec::new();
            if !timed {
                e.note_on(0, 1, 60, 100.0, &mut |_| {});
            }
            for b in 0..12 {
                let ev = [Event {
                    frame: at % BLOCK,
                    sound: 0,
                    kind: Kind::On {
                        id: 1,
                        key: 60,
                        velocity: 100.0,
                    },
                }];
                let mut o = [0.0; BLOCK];
                let ev: &[Event] = if timed && b == at / BLOCK { &ev } else { &[] };
                e.render1_events::<Native>(&mut o, ev, &mut |_| {});
                x.extend_from_slice(&o);
            }
            x
        };
        let delayed = render(&carrier(1, "key_on_delay=1"), false);
        assert!(delayed[..at].iter().all(|&v| v == 0.0) && delayed[at + 1..].iter().any(|&v| v != 0.0));
        let timed = render(&carrier(1, ""), true);
        assert!(delayed.iter().zip(&timed).all(|(a, b)| a.to_bits() == b.to_bits()));
        let mut e = engine(&carrier(1, "key_on_delay=1"));
        let mut ends = Vec::new();
        e.note_on(0, 7, 60, 100.0, &mut |n| ends.push(n.id));
        e.render1::<Native>(&mut [0.0; BLOCK], &mut |n| ends.push(n.id));
        e.note_off(0, 7, &mut |n| ends.push(n.id));
        assert_eq!(ends, [7]);
        for _ in 0..8 {
            let mut o = [0.0; BLOCK];
            e.render1::<Native>(&mut o, &mut |_| {});
            assert!(o.iter().all(|&v| v == 0.0));
        }
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
        let p = sound(&format!("algorithm=12 feedback=6 portamento=0\n{text}"));
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
                        e.note_off(0, id, &mut |_| {});
                    }
                }
                let mut o = [0.0; BLOCK];
                e.render1::<Native>(&mut o, &mut |_| {});
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
        // the sound LFO free-running on detune and frequency (the detune moves a lane holds)
        let slfo = "slfo_key_on_reset=0 slfo_speed=56 slfo_dest1=77 slfo_depth1=64 slfo_dest2=75 slfo_depth2=8";
        lane_widths("sound LFO", |o| if o == 1 { slfo } else { "" }, false);
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
        a.render1_events::<Native>(&mut x, &[on(37, 0, 1, 60)], &mut |_| {});
        let mut b = engine(&p);
        let mut y = [0.0; BLOCK];
        b.render1_events::<Native>(&mut y, &[on(0, 0, 1, 60)], &mut |_| {});
        assert!(x[..38].iter().all(|&v| v == 0.0) && x[38] != 0.0);
        let whole = (BLOCK - 37) / STEP * STEP;
        assert!(x[37..37 + whole]
            .iter()
            .zip(&y)
            .all(|(a, b)| a.to_bits() == b.to_bits()));
        // and the step cut short at the block's end leaves the note where 128 - 37 frames take it
        let (mut c, mut z) = (engine(&p), [0.0; BLOCK]);
        c.render1_events::<Native>(&mut z, &[on(0, 0, 1, 60)], &mut |_| {});
        assert!(x[37..].iter().zip(&z).all(|(a, b)| (a - b).abs() < 1e-3));
    }

    /// Mono: one voice sounds; with portamento a held key's note takes over legato (one voice,
    /// the envelopes going on) and the older key is returned to on release; without it the sounding
    /// note fades out as a retrigger (13.3 ms, the square of the share left) while the new one
    /// starts; every note ends exactly once, a key taken over at its own release; a held mono key
    /// is never shed. Key assign single retriggers a key played again.
    #[test]
    fn mono_and_key_assign() {
        let ended = std::cell::RefCell::new(Vec::new());
        let mut end = |n: NoteEnd| ended.borrow_mut().push(n.id);
        let ids = |e: &Engine| -> Vec<u32> {
            let mut v: Vec<u32> = e
                .lanes
                .iter()
                .flat_map(|l| {
                    (0..W)
                        .filter(|&s| has(u32::from(l.used & !l.ending), s))
                        .map(|s| l.id[s])
                })
                .collect();
            v.sort();
            v
        };
        let blocks = |e: &mut Engine, n: usize, end: &mut dyn FnMut(NoteEnd)| {
            for _ in 0..n {
                e.render1::<Native>(&mut [0.0; BLOCK], &mut |x| end(x));
            }
        };
        // legato with portamento
        let mut e = engine(&carrier(1, "mono_poly=0 portamento=1"));
        e.note_on(0, 1, 60, 100.0, &mut end);
        blocks(&mut e, 4, &mut end);
        let level = e.lanes[0].lvl[0][0];
        e.note_on(0, 2, 67, 100.0, &mut end);
        assert_eq!((ids(&e), e.active()), (vec![2], 1));
        assert_eq!(e.lanes[0].lvl[0][0], level, "the envelope goes on");
        blocks(&mut e, 2, &mut end);
        e.note_off(0, 2, &mut end);
        assert_eq!(
            (ids(&e), ended.borrow().clone()),
            (vec![1], vec![2]),
            "back to the held key"
        );
        e.note_off(0, 1, &mut end);
        blocks(&mut e, 400, &mut end);
        assert_eq!((e.active(), ended.borrow().clone()), (0, vec![2, 1]));
        // retrigger without portamento: the old note fades by the square of what is left
        ended.borrow_mut().clear();
        let mut e = engine(&carrier(1, "mono_poly=0 portamento=0"));
        e.note_on(0, 1, 60, 100.0, &mut end);
        blocks(&mut e, 4, &mut end);
        e.note_on(0, 2, 64, 100.0, &mut end);
        assert_eq!((ids(&e), e.active()), (vec![2], 2));
        e.render1::<Native>(&mut [0.0; BLOCK], &mut end);
        let l = e.lanes.iter().find(|l| l.id.contains(&1) && l.used != 0).unwrap();
        let s = l.id.iter().position(|&i| i == 1).unwrap();
        let share = 1.0 - 8.0 * e.retrig_step;
        assert!(
            (l.fade[s] - share * share).abs() < 1e-6,
            "{} against {}",
            l.fade[s],
            share * share
        );
        blocks(&mut e, 5, &mut end); // 13.3 ms: the fade is over
        assert_eq!(
            (e.active(), ended.borrow().len()),
            (1, 0),
            "a held key's note does not end with its voice"
        );
        e.note_off(0, 2, &mut end); // back to key 60, a new voice; key 64 fades out
        assert_eq!(ids(&e), vec![1]);
        blocks(&mut e, 6, &mut end);
        assert_eq!(ended.borrow().clone(), vec![2]);
        // a key taken over and released ends at its release; choke ends the kept keys
        e.note_on(0, 3, 70, 100.0, &mut end);
        e.note_on(0, 4, 72, 100.0, &mut end);
        e.note_off(0, 3, &mut end);
        assert_eq!(ended.borrow().clone(), vec![2, 3]);
        e.choke(0, &mut end);
        blocks(&mut e, 10, &mut end);
        let mut got = ended.borrow().clone();
        got.sort();
        assert_eq!((e.active(), got), (0, vec![1, 2, 3, 4]));
        // a held mono key is never shed for another sound's note
        let mut e = engine(&carrier(1, "mono_poly=0"));
        assert!(e.set_sound(1, &carrier(1, "")));
        e.set_limits(1, &[Limits::default(); SOUNDS]);
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        let mut refused = vec![];
        e.note_on(1, 2, 60, 100.0, &mut |n| refused.push(n.id));
        assert_eq!((ids(&e), refused), (vec![1], vec![2]));
        // key assign single: the same key again retriggers; multi keeps both
        for (assign, want) in [(0, 1), (1, 2)] {
            let mut e = engine(&carrier(1, &format!("key_assign={assign}")));
            e.note_on(0, 1, 60, 100.0, &mut |_| {});
            e.note_on(0, 2, 60, 100.0, &mut |_| {});
            assert_eq!(ids(&e).len(), want, "key assign {assign}");
        }
    }

    /// Two notes of one sound (keys 60 then `to`, the first released first when `release`): the
    /// second's pitch (cents against its own) at every block's end, and the blocks it moved in.
    fn glide_track(text: &str, to: u8, release: bool) -> (Vec<f64>, usize) {
        let mut e = engine(&carrier(1, text));
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        if release {
            e.note_off(0, 1, &mut |_| {});
        }
        e.note_on(0, 2, to, 100.0, &mut |_| {});
        let (mut track, mut moves) = (Vec::new(), 0);
        for _ in 0..200 {
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
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
            e.render1::<Native>(&mut [0.0; BLOCK], &mut |_| {});
        }
        let bend = |frame, value| Event {
            frame,
            sound: 0,
            kind: Kind::Bend { value },
        };
        a.render1_events::<Native>(&mut x, &[bend(50, -8192)], &mut |_| {});
        b.render1::<Native>(&mut y, &mut |_| {});
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
        a.render1_events::<Native>(&mut x, &[bend(0, 0)], &mut |_| {});
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
        let p = sound(&format!("algorithm=12 feedback=6 portamento=0\n{text}"));
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
                e.render1_events::<Native>(&mut o, &ev, &mut |_| {});
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
        e.render1::<Native>(&mut out, &mut |_| {});
        assert!(out.iter().all(|&x| x == 0.0));
    }

    /// The lanes in use are kept a bit each (a block scans only those): a lane joins with its first
    /// voice and leaves in the step its last one ends.
    #[test]
    fn live_lanes_follow_the_voices() {
        let mut e = Engine::new(RATE);
        let p = carrier(1, "op8.eg_time_release=99");
        for s in 0..3 {
            e.set_sound(s, &p);
            e.note_on(s, s as u32, 60, 100.0, &mut |_| {});
        }
        e.note_on(0, 9, 64, 100.0, &mut |_| {});
        assert_eq!((e.live.count_ones(), e.active()), (3, 4));
        for (s, id) in [(0, 0), (1, 1), (2, 2), (0, 9)] {
            e.note_off(s, id, &mut |_| {});
        }
        let mut out = [0.0; BLOCK];
        for _ in 0..400 {
            e.render1::<Native>(&mut out, &mut |_| {});
            assert_eq!(
                e.live.count_ones() as usize,
                e.lanes.iter().filter(|l| l.used != 0).count()
            );
        }
        assert_eq!((e.live, e.active()), (0, 0));
    }

    /// Two sounds, one with pan, playing a note each: (sound, pan) settings.
    fn two_sounds() -> Engine {
        let mut e = Engine::new(RATE);
        e.set_sound(0, &carrier(1, ""));
        e.set_sound(1, &carrier(5, "alternate_pan=40"));
        e.note_on(0, 1, 60, 100.0, &mut |_| {});
        e.note_on(1, 2, 64, 90.0, &mut |_| {});
        e
    }

    #[test]
    fn buses_take_each_sound_as_it_plays_alone() {
        // together on one pair, as render_events
        let mut a = two_sounds();
        let mut both = [[0.0; BLOCK]; 2];
        a.render_events::<Native>(&mut both, &[], &mut |_| {});
        // sound 0 on output 2, sound 1 on output 1, output 0 untouched; the outputs add to the pair
        let mut b = two_sounds();
        let mut outs = [[[f32::NAN; BLOCK]; 2]; 3];
        let w = b.render_buses::<Native>(&mut outs, &[2, 1, 0, 0, 0, 0, 0, 0], BLOCK, &[], &mut |_| {});
        assert_eq!(
            w,
            Written {
                left: 0b110,
                right: 0b010
            }
        );
        assert!(outs[0].as_flattened().iter().all(|x| x.is_nan()));
        assert!(
            outs[2][1].iter().all(|x| x.is_nan()),
            "a channel without pan is left for the caller"
        );
        for i in 0..BLOCK {
            assert_eq!((outs[2][0][i] + outs[1][0][i]).to_bits(), both[0][i].to_bits());
            assert_eq!((outs[2][0][i] + outs[1][1][i]).to_bits(), both[1][i].to_bits());
        }
        // a bus past the outputs is output 0; nothing sounding writes nothing
        let mut c = Engine::new(RATE);
        assert_eq!(
            c.render_buses::<Native>(&mut outs, &[9; SOUNDS], BLOCK, &[], &mut |_| {}),
            Written::default()
        );
    }

    #[test]
    fn a_block_in_parts_plays_its_frames() {
        // two halves of a block, on the step grid, play as the whole block
        let ev = |frame, kind| Event { frame, sound: 0, kind };
        let events = [
            ev(
                0,
                Kind::On {
                    id: 1,
                    key: 60,
                    velocity: 100.0,
                },
            ),
            ev(96, Kind::Off { id: 1 }),
        ];
        let mut a = engine(&carrier(1, ""));
        let mut whole = Vec::new();
        for b in 0..4 {
            let mut o = [[0.0; BLOCK]; 2];
            a.render_events::<Native>(&mut o, if b == 1 { &events } else { &[] }, &mut |_| {});
            whole.extend_from_slice(&o[0]);
        }
        let mut b = engine(&carrier(1, ""));
        let mut parts = Vec::new();
        for blk in 0..4 {
            for half in 0..2 {
                let mut o = [[[0.0; BLOCK]; 2]];
                let ev: Vec<Event> = if blk == 1 {
                    events
                        .iter()
                        .filter(|e| e.frame / 64 == half)
                        .map(|e| Event {
                            frame: e.frame % 64,
                            ..*e
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                b.render_buses::<Native>(&mut o, &[0; SOUNDS], 64, &ev, &mut |_| {});
                parts.extend_from_slice(&o[0][0][..64]);
            }
        }
        assert_eq!(whole.len(), parts.len());
        assert!(whole.iter().zip(&parts).all(|(a, b)| a.to_bits() == b.to_bits()));
        assert!(whole.iter().any(|&x| x != 0.0));
    }

    #[test]
    fn staged_values_prepare_once_as_events_at_the_block_start() {
        let ids = [(1042u16, 2i16), (1042, 3), (1043, 40), (2, 40)];
        let mut a = engine(&carrier(1, ""));
        let mut b = engine(&carrier(1, ""));
        for e in [&mut a, &mut b] {
            e.note_on(0, 1, 60, 100.0, &mut |_| {});
            e.render::<Native>(&mut [[0.0; BLOCK]; 2], &mut |_| {});
        }
        let events: Vec<Event> = ids
            .iter()
            .map(|&(id, value)| Event {
                frame: 0,
                sound: 0,
                kind: Kind::Param { id, value },
            })
            .collect();
        for &(id, value) in &ids {
            b.stage(0, id, value);
        }
        assert_eq!(b.staged(), 1);
        assert!(!b.stage(0, 1042, 1000), "a value out of range is refused");
        let (mut oa, mut ob) = ([[0.0; BLOCK]; 2], [[0.0; BLOCK]; 2]);
        for blk in 0..3 {
            a.render_events::<Native>(&mut oa, if blk == 0 { &events } else { &[] }, &mut |_| {});
            b.render_events::<Native>(&mut ob, &[], &mut |_| {});
            assert_eq!(b.staged(), 0);
            assert!(oa
                .as_flattened()
                .iter()
                .zip(ob.as_flattened())
                .all(|(x, y)| x.to_bits() == y.to_bits()));
        }
    }

    /// A box's source moved (knob 3 on the cutoff here) makes only the boxes again, the same bits
    /// as the sound prepared again; a value the sound already has prepares nothing.
    #[test]
    fn a_box_source_or_an_unchanged_value_prepares_nothing() {
        let p = carrier(
            1,
            "filter_type=0 cutoff=300 box1_src=10 box1_dst=102 box1_ratio=60 box1_pol=1",
        );
        let [mut a, mut b, mut c] = [engine(&p), engine(&p), engine(&p)];
        for e in [&mut a, &mut b, &mut c] {
            e.note_on(0, 1, 60, 100.0, &mut |_| {});
            e.render::<Native>(&mut [[0.0; BLOCK]; 2], &mut |_| {});
        }
        assert!(a.stage(0, 258, 900) && a.staged() == 0, "a knob: no preparing");
        assert!(b.stage(0, 258, 900));
        b.apply(0);
        assert!(c.stage(0, 1042, c.sounds[0].sound.get(index_of(1042).unwrap())) && c.staged() == 0);
        let [mut oa, mut ob, mut oc] = [[[0.0; BLOCK]; 2]; 3];
        let mut d = engine(&p);
        d.note_on(0, 1, 60, 100.0, &mut |_| {});
        let mut od = [[0.0; BLOCK]; 2];
        d.render::<Native>(&mut od, &mut |_| {});
        for _ in 0..3 {
            a.render::<Native>(&mut oa, &mut |_| {});
            b.render::<Native>(&mut ob, &mut |_| {});
            c.render::<Native>(&mut oc, &mut |_| {});
            d.render::<Native>(&mut od, &mut |_| {});
            let bits = |x: &[[f32; BLOCK]; 2]| x.as_flattened().iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            assert_eq!(bits(&oa), bits(&ob));
            assert_eq!(bits(&oc), bits(&od));
            assert_ne!(bits(&oa), bits(&od), "the knob moved the cutoff");
        }
    }

    #[test]
    fn a_choked_note_fades_and_a_reset_engine_plays_as_new() {
        let mut e = two_sounds();
        e.note_on(0, 3, 67, 100.0, &mut |_| {});
        let mut ended = Vec::new();
        e.choke_note(0, 3, &mut |n| ended.push(n.id));
        for _ in 0..2 {
            e.render::<Native>(&mut [[0.0; BLOCK]; 2], &mut |n| ended.push(n.id));
        }
        assert_eq!(ended, vec![3], "only the choked note ends, after its fade");
        // the same as an event at its frame
        e.note_on(0, 4, 69, 100.0, &mut |_| {});
        let choke = Event {
            frame: 40,
            sound: 0,
            kind: Kind::Choke { id: 4 },
        };
        for b in 0..2 {
            let ev = if b == 0 { vec![choke] } else { vec![] };
            e.render_events::<Native>(&mut [[0.0; BLOCK]; 2], &ev, &mut |n| ended.push(n.id));
        }
        assert_eq!(ended, vec![3, 4]);
        let mut n = 0;
        e.voices(|s, id, loud| {
            n += 1;
            assert!((s, id) == (1, 2) || (s, id) == (0, 1) && loud > 0.1, "{s} {id} {loud}");
        });
        assert_eq!(n, 2);
        // reset: no voice, no end reported, and the next notes play as on a new engine
        e.render::<Native>(&mut [[0.0; BLOCK]; 2], &mut |_| {});
        e.reset();
        assert_eq!(e.active(), 0);
        let mut f = Engine::new(RATE);
        f.set_sound(0, &carrier(1, ""));
        f.set_sound(1, &carrier(5, "alternate_pan=40"));
        for g in [&mut e, &mut f] {
            g.note_on(1, 9, 50, 70.0, &mut |_| {});
        }
        let (mut a, mut b) = ([[0.0; BLOCK]; 2], [[0.0; BLOCK]; 2]);
        e.render::<Native>(&mut a, &mut |_| panic!("no end"));
        f.render::<Native>(&mut b, &mut |_| {});
        assert!(a
            .as_flattened()
            .iter()
            .zip(b.as_flattened())
            .all(|(x, y)| x.to_bits() == y.to_bits()));
        assert_eq!(e.cost(0), carrier(1, "").plan.cost());
    }
}
