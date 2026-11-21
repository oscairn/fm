// SPDX-License-Identifier: AGPL-3.0-or-later
//! Every parameter of the model (docs/design.md, section 1) with a stable id, its range and its
//! default, in the model's raw units (signed where the model shows a signed value). Ids never
//! change and are never reused: a new parameter takes a new id, and its default is the behaviour
//! sounds had before it existed. `docs/coverage.md` says, for each, whether the engine plays it.
//!
//! Id ranges: 0-40 part common, 100-134 filter and filter envelope, 200-266 part parameters
//! (256-266 the controller boxes' own sources), 300-555 the controller boxes (box n's eight fields
//! at 300 + 8 (n - 1)), 1000 + 100 op + k for operator `op` (0-7): switches at k 0-38, parameters
//! at k 40-77.

/// One parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    pub id: u16,
    /// The name, without the operator prefix (`op3.` in the text form).
    pub name: &'static str,
    /// Operator 0-7, or `None` for a part-wide parameter.
    pub op: Option<u8>,
    pub min: i16,
    pub max: i16,
    pub default: i16,
}

type Row = (&'static str, i16, i16, i16);

const COMMON: [Row; 41] = [
    ("random_pan", 0, 127, 0),
    ("alternate_pan", -64, 63, 0),
    ("scaling_pan", -64, 63, 0),
    ("key_on_delay", 0, 127, 0),
    ("key_on_delay_sync", 0, 1, 0),
    ("key_on_delay_length", 5, 21, 14),
    ("pitch_velocity", -64, 63, 0),
    ("random_pitch", 0, 127, 0),
    ("pitch_key_follow", 0, 127, 96),
    ("pitch_key_centre", 0, 127, 60),
    ("pitch_eg_level_initial", -50, 50, 0),
    ("pitch_eg_level_attack", -50, 50, 0),
    ("pitch_eg_level_decay1", -50, 50, 0),
    ("pitch_eg_level_decay2", -50, 50, 0),
    ("pitch_eg_level_release", -50, 50, 0),
    ("pitch_eg_time_attack", 0, 99, 0),
    ("pitch_eg_time_decay1", 0, 99, 0),
    ("pitch_eg_time_decay2", 0, 99, 0),
    ("pitch_eg_time_release", 0, 99, 0),
    ("pitch_eg_depth_velocity", 0, 7, 0),
    ("pitch_eg_depth", 0, 3, 0),
    ("pitch_eg_time_key_follow", 0, 7, 0),
    ("lfo_wave", 0, 5, 0),
    ("lfo_speed", 0, 99, 30),
    ("lfo_phase", 0, 3, 0),
    ("lfo_delay", 0, 99, 0),
    ("lfo_key_on_reset", 0, 1, 0),
    ("lfo_pitch_depth", 0, 99, 0),
    ("lfo_amp_depth", 0, 99, 0),
    ("lfo_filter_depth", 0, 99, 0),
    ("algorithm", 1, 88, 69),
    ("feedback", 0, 7, 0),
    ("lfo_speed_range", 0, 1, 1),
    ("lfo_extended_speed", 0, 415, 60),
    ("macro_depth", -99, 99, 0),
    ("macro_ratio", -99, 99, 0),
    ("macro_attack", -99, 99, 0),
    ("macro_decay", -99, 99, 0),
    ("macro_sustain", -99, 99, 0),
    ("macro_release", -99, 99, 0),
    ("macro_texture", -99, 99, 0),
];

const FILTER: [Row; 35] = [
    ("filter_type", 0, 21, 21),
    ("cutoff", 0, 1023, 1023),
    ("cutoff_velocity", -64, 63, 0),
    ("resonance", 0, 127, 10),
    ("resonance_velocity", -64, 63, 0),
    ("hpf_cutoff", 0, 1023, 0),
    ("distance", -128, 127, 0),
    ("filter_gain", 0, 255, 255),
    ("filter_eg_time_hold", 0, 127, 0),
    ("filter_eg_time_attack", 0, 127, 0),
    ("filter_eg_time_decay1", 0, 127, 0),
    ("filter_eg_time_decay2", 0, 127, 0),
    ("filter_eg_time_release", 0, 127, 0),
    ("filter_eg_level_hold", -128, 127, 0),
    ("filter_eg_level_attack", -128, 127, 0),
    ("filter_eg_level_decay1", -128, 127, 0),
    ("filter_eg_level_decay2", -128, 127, 0),
    ("filter_eg_level_release", -128, 127, 0),
    ("filter_eg_depth", -64, 63, 40),
    ("filter_eg_time_velocity_segment", 0, 4, 4),
    ("filter_eg_time_velocity", -64, 63, 0),
    ("filter_eg_depth_velocity", -64, 63, 0),
    ("filter_eg_depth_velocity_curve", 0, 4, 2),
    ("filter_eg_time_key_follow", -64, 63, 0),
    ("filter_eg_time_key_centre", 0, 127, 24),
    ("cutoff_key_follow", -64, 63, 10),
    ("cutoff_break1", 0, 124, 36),
    ("cutoff_break2", 1, 125, 48),
    ("cutoff_break3", 2, 126, 60),
    ("cutoff_break4", 3, 127, 72),
    ("cutoff_offset1", -128, 127, 0),
    ("cutoff_offset2", -128, 127, 0),
    ("cutoff_offset3", -128, 127, 0),
    ("cutoff_offset4", -128, 127, 0),
    ("hpf_key_follow", -64, 63, 0),
];

const PART: [Row; 67] = [
    ("mono_poly", 0, 1, 1),
    ("key_assign", 0, 1, 1),
    ("portamento", 0, 1, 1),
    ("portamento_time", 0, 127, 64),
    ("portamento_mode", 0, 1, 1),
    ("portamento_time_mode", 0, 3, 0),
    ("bend_up", -48, 24, 2),
    ("bend_down", -48, 24, -2),
    ("detune", -128, 127, 0),
    ("note_shift", -48, 48, 0),
    ("tuning_scale", 0, 212, 0),
    ("tuning_root", 0, 11, 0),
    ("velocity_depth", 0, 127, 64),
    ("velocity_offset", 0, 127, 64),
    ("slfo_phase", 0, 5, 0),
    ("slfo_wave", 0, 12, 0),
    ("slfo_speed", 0, 63, 32),
    ("slfo_tempo_sync", 0, 1, 0),
    ("slfo_tempo_speed", 5, 24, 11),
    ("slfo_delay", 0, 127, 0),
    ("slfo_fade_in", 0, 127, 0),
    ("slfo_hold", 0, 127, 127),
    ("slfo_fade_out", 0, 127, 64),
    ("slfo_key_on_reset", 0, 2, 2),
    ("slfo_dest1", 0, 78, 2),
    ("slfo_depth1", 0, 127, 0),
    ("slfo_dest2", 0, 78, 4),
    ("slfo_depth2", 0, 127, 0),
    ("slfo_dest3", 0, 78, 4),
    ("slfo_depth3", 0, 127, 0),
    ("slfo_user_cycle", 0, 6, 6),
    ("slfo_user_slope", 0, 3, 0),
    ("slfo_step1", -64, 63, 0),
    ("slfo_step2", -64, 63, 0),
    ("slfo_step3", -64, 63, 0),
    ("slfo_step4", -64, 63, 0),
    ("slfo_step5", -64, 63, 0),
    ("slfo_step6", -64, 63, 0),
    ("slfo_step7", -64, 63, 0),
    ("slfo_step8", -64, 63, 0),
    ("slfo_step9", -64, 63, 0),
    ("slfo_step10", -64, 63, 0),
    ("slfo_step11", -64, 63, 0),
    ("slfo_step12", -64, 63, 0),
    ("slfo_step13", -64, 63, 0),
    ("slfo_step14", -64, 63, 0),
    ("slfo_step15", -64, 63, 0),
    ("slfo_step16", -64, 63, 0),
    ("slfo_random_speed", 0, 127, 0),
    ("amp_eg_attack_offset", -64, 63, 0),
    ("amp_eg_decay_offset", -64, 63, 0),
    ("amp_eg_sustain_offset", -64, 63, 0),
    ("amp_eg_release_offset", -64, 63, 0),
    ("filter_eg_depth_offset", -64, 63, 0),
    ("cutoff_offset", -64, 63, 0),
    ("resonance_offset", -64, 63, 0),
    // the controller boxes' sources the host sends as parameters: the assignable knobs (raw),
    // the ribbon (as its controller) and the two assignable switches
    ("knob1", 0, 1023, 512),
    ("knob2", 0, 1023, 512),
    ("knob3", 0, 1023, 512),
    ("knob4", 0, 1023, 512),
    ("knob5", 0, 1023, 512),
    ("knob6", 0, 1023, 512),
    ("knob7", 0, 1023, 512),
    ("knob8", 0, 1023, 512),
    ("ribbon", 0, 127, 0),
    ("switch1", 0, 1, 0),
    ("switch2", 0, 1, 0),
];

/// The controller boxes 1-32 (the model's controller sets), raw: source, destination (the
/// reference's number; 0: off), ratio, polarity (bipolar), curve bank, curve type and its two
/// parameters.
macro_rules! boxes {
    ($($n:literal)*) => {[$(
        (concat!("box", $n, "_src"), 0, 41, 1),
        (concat!("box", $n, "_dst"), 0, 413, 0),
        (concat!("box", $n, "_ratio"), -128, 127, 0),
        (concat!("box", $n, "_pol"), 0, 1, 0),
        (concat!("box", $n, "_bank"), 0, 25, 0),
        (concat!("box", $n, "_curve"), 0, 31, 0),
        (concat!("box", $n, "_p1"), 0, 127, 5),
        (concat!("box", $n, "_p2"), 0, 127, 0),
    )*]};
}
const BOX: [Row; 256] = boxes!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32);
/// Fields a box has, in [`BOX`]'s order.
pub const BOX_FIELDS: usize = 8;

/// Per-operator switches (k 0-38): 32 controller-set switches, 4 key-controller-set switches,
/// 3 sound-LFO destination switches; all on by default.
const SWITCH_NAMES: [&str; 39] = [
    "ctrl_set1",
    "ctrl_set2",
    "ctrl_set3",
    "ctrl_set4",
    "ctrl_set5",
    "ctrl_set6",
    "ctrl_set7",
    "ctrl_set8",
    "ctrl_set9",
    "ctrl_set10",
    "ctrl_set11",
    "ctrl_set12",
    "ctrl_set13",
    "ctrl_set14",
    "ctrl_set15",
    "ctrl_set16",
    "ctrl_set17",
    "ctrl_set18",
    "ctrl_set19",
    "ctrl_set20",
    "ctrl_set21",
    "ctrl_set22",
    "ctrl_set23",
    "ctrl_set24",
    "ctrl_set25",
    "ctrl_set26",
    "ctrl_set27",
    "ctrl_set28",
    "ctrl_set29",
    "ctrl_set30",
    "ctrl_set31",
    "ctrl_set32",
    "key_ctrl_set1",
    "key_ctrl_set2",
    "key_ctrl_set3",
    "key_ctrl_set4",
    "slfo_box1",
    "slfo_box2",
    "slfo_box3",
];

/// Per-operator parameters (k 40-77).
const OP: [Row; 38] = [
    ("key_on_reset", 0, 1, 1),
    ("mode", 0, 1, 0),
    ("coarse", 0, 31, 1),
    ("fine", 0, 127, 0),
    ("detune", -15, 15, 0),
    ("fixed_key_follow", 0, 99, 0),
    ("pitch_velocity", -7, 7, 0),
    ("form", 0, 6, 0),
    ("spread", 0, 7, 0),
    ("formant", 0, 99, 0),
    ("pitch_eg_level_initial", -50, 50, 0),
    ("pitch_eg_level_attack", -50, 50, 0),
    ("pitch_eg_time_attack", 0, 99, 0),
    ("pitch_eg_time_decay", 0, 99, 0),
    ("eg_level_attack", 0, 99, 99),
    ("eg_level_decay1", 0, 99, 99),
    ("eg_level_decay2", 0, 99, 99),
    ("eg_level_release", 0, 99, 0),
    ("eg_time_attack", 0, 99, 0),
    ("eg_time_decay1", 0, 99, 0),
    ("eg_time_decay2", 0, 99, 0),
    ("eg_time_release", 0, 99, 40),
    ("eg_time_hold", 0, 99, 0),
    ("eg_time_key_follow", 0, 7, 0),
    ("level", 0, 99, 0),
    ("scaling_break", 0, 99, 39),
    ("scaling_low_depth", 0, 99, 0),
    ("scaling_high_depth", 0, 99, 0),
    ("scaling_low_curve", 0, 3, 0),
    ("scaling_high_curve", 0, 3, 0),
    ("level_velocity", -7, 7, 0),
    ("lfo_pitch_offset", 0, 7, 3),
    ("lfo_amp_offset", 0, 7, 3),
    ("pitch_ctrl_sens", -7, 7, 0),
    ("level_ctrl_sens", -7, 7, 0),
    ("slfo_ratio1", 0, 127, 127),
    ("slfo_ratio2", 0, 127, 127),
    ("slfo_ratio3", 0, 127, 127),
];

pub const OPS: usize = 8;
/// Parameters an operator has: switches and parameters.
pub const PER_OP: usize = SWITCH_NAMES.len() + OP.len();
/// Index of operator 0's first switch in [`PARAMS`].
pub const OP_BASE: usize = BOX_BASE + BOX.len();
/// Index of box 1's first field in [`PARAMS`].
pub const BOX_BASE: usize = COMMON.len() + FILTER.len() + PART.len();
/// All parameters.
pub const COUNT: usize = OP_BASE + OPS * PER_OP;

/// The level default of the last operator (the others are silent): the model's init sound.
const LAST_OP_LEVEL: i16 = 85;

#[allow(clippy::indexing_slicing)] // compile-time evaluation: an out-of-range index fails the build
const fn build() -> [Param; COUNT] {
    let zero = Param {
        id: 0,
        name: "",
        op: None,
        min: 0,
        max: 0,
        default: 0,
    };
    let mut t = [zero; COUNT];
    let mut n = 0;
    let groups: [(&[Row], u16); 4] = [(&COMMON, 0), (&FILTER, 100), (&PART, 200), (&BOX, 300)];
    let mut g = 0;
    while g < groups.len() {
        let (rows, base) = groups[g];
        let mut k = 0;
        while k < rows.len() {
            let (name, min, max, default) = rows[k];
            t[n] = Param {
                id: base + k as u16,
                name,
                op: None,
                min,
                max,
                default,
            };
            n += 1;
            k += 1;
        }
        g += 1;
    }
    let mut op = 0;
    while op < OPS {
        let base = 1000 + 100 * op as u16;
        let mut k = 0;
        while k < SWITCH_NAMES.len() {
            t[n] = Param {
                id: base + k as u16,
                name: SWITCH_NAMES[k],
                op: Some(op as u8),
                min: 0,
                max: 1,
                default: 1,
            };
            n += 1;
            k += 1;
        }
        k = 0;
        while k < OP.len() {
            let (name, min, max, mut default) = OP[k];
            if k == Op::Level as usize && op == OPS - 1 {
                default = LAST_OP_LEVEL;
            }
            t[n] = Param {
                id: base + 40 + k as u16,
                name,
                op: Some(op as u8),
                min,
                max,
                default,
            };
            n += 1;
            k += 1;
        }
        op += 1;
    }
    t
}

/// Every parameter, sorted by id.
pub static PARAMS: [Param; COUNT] = build();

/// The index in [`PARAMS`] of the parameter with `id`.
pub fn index_of(id: u16) -> Option<usize> {
    PARAMS.binary_search_by_key(&id, |p| p.id).ok()
}

/// Part-wide parameters the engine reads, by index.
#[derive(Clone, Copy)]
pub enum Common {
    RandomPan = 0,
    AlternatePan = 1,
    ScalingPan = 2,
    KeyOnDelay = 3,
    KeyOnDelaySync = 4,
    KeyOnDelayLength = 5,
    PitchVelocity = 6,
    RandomPitch = 7,
    PitchKeyFollow = 8,
    PitchKeyCentre = 9,
    PegLevel = 10, // five levels from here: initial, attack, decay 1, decay 2, release
    PegTime = 15,  // four times from here: attack, decay 1, decay 2, release
    PegVelocity = 19,
    PegDepth = 20,
    PegTimeKeyFollow = 21,
    LfoWave = 22,
    LfoSpeed = 23,
    LfoPhase = 24,
    LfoDelay = 25,
    LfoKeyOnReset = 26,
    LfoPitchDepth = 27,
    LfoAmpDepth = 28,
    LfoFilterDepth = 29,
    Algorithm = 30,
    Feedback = 31,
    LfoSpeedRange = 32,
    LfoExtendedSpeed = 33,
    MacroDepth = 34,
    MacroRatio = 35,
    MacroAttack = 36,
    MacroDecay = 37,
    MacroSustain = 38,
    MacroRelease = 39,
    MacroTexture = 40,
}

#[derive(Clone, Copy)]
pub enum Part {
    MonoPoly = COMMON.len() as isize + FILTER.len() as isize,
    KeyAssign = COMMON.len() as isize + FILTER.len() as isize + 1,
    Portamento = COMMON.len() as isize + FILTER.len() as isize + 2,
    PortamentoTime = COMMON.len() as isize + FILTER.len() as isize + 3,
    PortamentoMode = COMMON.len() as isize + FILTER.len() as isize + 4,
    PortamentoTimeMode = COMMON.len() as isize + FILTER.len() as isize + 5,
    BendUp = COMMON.len() as isize + FILTER.len() as isize + 6,
    BendDown = COMMON.len() as isize + FILTER.len() as isize + 7,
    Detune = COMMON.len() as isize + FILTER.len() as isize + 8,
    NoteShift = COMMON.len() as isize + FILTER.len() as isize + 9,
    VelocityDepth = COMMON.len() as isize + FILTER.len() as isize + 12,
    VelocityOffset = COMMON.len() as isize + FILTER.len() as isize + 13,
    SlfoPhase = COMMON.len() as isize + FILTER.len() as isize + 14,
    SlfoWave = COMMON.len() as isize + FILTER.len() as isize + 15,
    SlfoSpeed = COMMON.len() as isize + FILTER.len() as isize + 16,
    SlfoTempoSync = COMMON.len() as isize + FILTER.len() as isize + 17,
    SlfoTime = COMMON.len() as isize + FILTER.len() as isize + 19, // delay, fade in, hold, fade out
    SlfoKeyOnReset = COMMON.len() as isize + FILTER.len() as isize + 23,
    SlfoDest = COMMON.len() as isize + FILTER.len() as isize + 24, // destination d at 2 d, its depth after it
    SlfoUserCycle = COMMON.len() as isize + FILTER.len() as isize + 30,
    SlfoUserSlope = COMMON.len() as isize + FILTER.len() as isize + 31,
    SlfoStep = COMMON.len() as isize + FILTER.len() as isize + 32, // 16 steps from here
    AttackOffset = COMMON.len() as isize + FILTER.len() as isize + 49,
    DecayOffset = COMMON.len() as isize + FILTER.len() as isize + 50,
    SustainOffset = COMMON.len() as isize + FILTER.len() as isize + 51,
    ReleaseOffset = COMMON.len() as isize + FILTER.len() as isize + 52,
    FegDepthOffset = COMMON.len() as isize + FILTER.len() as isize + 53,
    CutoffOffset = COMMON.len() as isize + FILTER.len() as isize + 54,
    ResonanceOffset = COMMON.len() as isize + FILTER.len() as isize + 55,
    Knob = COMMON.len() as isize + FILTER.len() as isize + 56, // eight knobs from here
    Ribbon = COMMON.len() as isize + FILTER.len() as isize + 64,
    Switch = COMMON.len() as isize + FILTER.len() as isize + 65, // two switches from here
}

/// The filter and its envelope's parameters, by index (ids 100-134, in order).
#[derive(Clone, Copy)]
pub enum Filter {
    Type = COMMON.len() as isize,
    Cutoff,
    CutoffVelocity,
    Resonance,
    ResonanceVelocity,
    HpfCutoff,
    Distance,
    Gain,
    EgTime,                               // five times from here: hold, attack, decay 1, decay 2, release
    EgLevel = COMMON.len() as isize + 13, // five levels from here, in the same order
    EgDepth = COMMON.len() as isize + 18,
    EgTimeVelocitySegment,
    EgTimeVelocity,
    EgDepthVelocity,
    EgDepthVelocityCurve,
    EgTimeKeyFollow,
    EgTimeKeyCentre,
    KeyFollow,
    Break,                               // four break points from here
    Offset = COMMON.len() as isize + 30, // four offsets from here
    HpfKeyFollow = COMMON.len() as isize + 34,
}

/// Operator parameters the engine reads, by their place in the operator's parameter list.
#[derive(Clone, Copy)]
pub enum Op {
    KeyOnReset = 0,
    Mode = 1,
    Coarse = 2,
    Fine = 3,
    Detune = 4,
    FixedKeyFollow = 5,
    PitchVelocity = 6,
    Form = 7,
    Spread = 8,
    Formant = 9,
    PegLevel = 10, // initial, attack
    PegTime = 12,  // attack, decay
    EgLevel = 14,  // four levels from here: attack, decay 1, decay 2, release (hold)
    EgTime = 18,   // four times from here, then the hold time
    EgHold = 22,
    EgTimeKeyFollow = 23,
    Level = 24,
    Scaling = 25, // break point, low depth, high depth, low curve, high curve
    LevelVelocity = 30,
    LfoPitchOffset = 31,
    LfoAmpOffset = 32,
    PitchCtrlSens = 33,
    SlfoRatio = 35, // destinations 1-3
}

/// The index of operator `op`'s parameter `k` (+ `offset` for the multi-value ones).
pub const fn op_index(op: usize, k: Op, offset: usize) -> usize {
    OP_BASE + op * PER_OP + SWITCH_NAMES.len() + k as usize + offset
}

/// The index of operator `op`'s controller set switch `n` (0-31).
pub const fn ctrl_set_index(op: usize, n: usize) -> usize {
    OP_BASE.wrapping_add(op.wrapping_mul(PER_OP)).wrapping_add(n)
}

/// The index of operator `op`'s sound LFO destination box `d` (0-2).
pub const fn slfo_box_index(op: usize, d: usize) -> usize {
    (OP_BASE + 36).wrapping_add(op.wrapping_mul(PER_OP)).wrapping_add(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_unique_and_sane() {
        assert_eq!(COUNT, 41 + 35 + 67 + 256 + 8 * 77);
        assert_eq!(
            (PARAMS[Part::Knob as usize].id, PARAMS[Part::Knob as usize].name),
            (256, "knob1")
        );
        assert_eq!(PARAMS[Part::Ribbon as usize].name, "ribbon");
        assert_eq!(PARAMS[Part::Switch as usize + 1].name, "switch2");
        assert_eq!((PARAMS[BOX_BASE].id, PARAMS[BOX_BASE].name), (300, "box1_src"));
        let last = &PARAMS[OP_BASE - 1];
        assert_eq!((last.id, last.name), (555, "box32_p2"));
        assert_eq!(PARAMS[ctrl_set_index(2, 31)].name, "ctrl_set32");
        for w in PARAMS.windows(2) {
            assert!(w[0].id < w[1].id, "{} then {}", w[0].id, w[1].id);
        }
        for p in &PARAMS {
            assert!(p.min <= p.default && p.default <= p.max, "{}", p.name);
            assert_eq!(index_of(p.id).map(|i| PARAMS[i].id), Some(p.id));
        }
        assert_eq!(PARAMS[Common::Algorithm as usize].name, "algorithm");
        assert_eq!(PARAMS[Common::KeyOnDelaySync as usize].name, "key_on_delay_sync");
        assert_eq!(PARAMS[Common::KeyOnDelayLength as usize].name, "key_on_delay_length");
        assert_eq!(PARAMS[Part::NoteShift as usize].name, "note_shift");
        assert_eq!(PARAMS[Part::VelocityOffset as usize].name, "velocity_offset");
        assert_eq!(PARAMS[Part::MonoPoly as usize].name, "mono_poly");
        assert_eq!(PARAMS[Part::Detune as usize].name, "detune");
        assert_eq!(PARAMS[Common::PegLevel as usize].name, "pitch_eg_level_initial");
        assert_eq!(
            PARAMS[Common::PegTimeKeyFollow as usize].name,
            "pitch_eg_time_key_follow"
        );
        assert_eq!(PARAMS[op_index(1, Op::FixedKeyFollow, 0)].name, "fixed_key_follow");
        assert_eq!(PARAMS[op_index(1, Op::PegTime, 1)].name, "pitch_eg_time_decay");
        let level7 = &PARAMS[op_index(7, Op::Level, 0)];
        assert_eq!((level7.id, level7.name, level7.default), (1764, "level", 85));
        assert_eq!(PARAMS[op_index(2, Op::EgHold, 0)].name, "eg_time_hold");
        assert_eq!(PARAMS[op_index(0, Op::LevelVelocity, 0)].name, "level_velocity");
        assert_eq!(PARAMS[op_index(3, Op::SlfoRatio, 2)].name, "slfo_ratio3");
        assert_eq!(PARAMS[slfo_box_index(3, 2)].name, "slfo_box3");
        for (k, name) in [
            (Part::SlfoPhase, "slfo_phase"),
            (Part::SlfoWave, "slfo_wave"),
            (Part::SlfoSpeed, "slfo_speed"),
            (Part::SlfoTempoSync, "slfo_tempo_sync"),
            (Part::SlfoTime, "slfo_delay"),
            (Part::SlfoKeyOnReset, "slfo_key_on_reset"),
            (Part::SlfoDest, "slfo_dest1"),
            (Part::SlfoUserCycle, "slfo_user_cycle"),
            (Part::SlfoUserSlope, "slfo_user_slope"),
            (Part::SlfoStep, "slfo_step1"),
        ] {
            assert_eq!(PARAMS[k as usize].name, name);
        }
        assert_eq!(PARAMS[Part::SlfoDest as usize + 5].name, "slfo_depth3");
        assert_eq!(index_of(41), None);
    }
}
