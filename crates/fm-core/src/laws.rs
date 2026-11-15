// SPDX-License-Identifier: AGPL-3.0-or-later
//! The measured laws M1 plays: raw parameter values to physical ones, evaluated once per note or
//! per sound, never per sample. The numbers are measurements of the reference's audio output
//! (docs/coverage.md states each law's accuracy). Double precision, and no libm: `exp2` and
//! `log2` are here.

/// 2^x, |x| < 1000; relative error below 1e-15.
pub const fn exp2(x: f64) -> f64 {
    let x = if x < -1000.0 {
        -1000.0
    } else if x > 1000.0 {
        1000.0
    } else {
        x
    };
    let mut k = x as i64; // towards 0
    k = if k < -1000 {
        -1000
    } else if k > 1000 {
        1000
    } else {
        k
    };
    if (k as f64) > x {
        k -= 1;
    }
    let f = (x - k as f64) * core::f64::consts::LN_2; // [0, ln 2)
    let (mut term, mut sum, mut n) = (1.0, 1.0, 1);
    while n < 20 {
        term *= f / n as f64;
        sum += term;
        n += 1;
    }
    // 2^k in two halves, each within the exponent range
    let half = k / 2;
    sum * pow2i(half) * pow2i(k - half)
}

const fn pow2i(k: i64) -> f64 {
    let k = if k < -1022 {
        -1022
    } else if k > 1023 {
        1023
    } else {
        k
    };
    f64::from_bits(((k + 1023) as u64) << 52)
}

/// log2(x) for x > 0 and finite (0 or less gives -1100).
pub const fn log2(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 {
        return -1100.0;
    }
    let mut b = x.to_bits();
    let mut e = ((b >> 52) & 0x7ff) as i64 - 1023;
    if e == -1023 {
        // subnormal: scale up first
        b = (x * 4_503_599_627_370_496.0).to_bits(); // 2^52
        e = ((b >> 52) & 0x7ff) as i64 - 1023 - 52;
    }
    let m = f64::from_bits((b & 0x000f_ffff_ffff_ffff) | (1023u64 << 52)); // [1, 2)
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let (mut term, mut sum, mut n) = (z, z, 1);
    while n < 25 {
        term *= z2;
        sum += term / (2 * n + 1) as f64;
        n += 1;
    }
    e as f64 + 2.0 * sum / core::f64::consts::LN_2
}

/// Amplitude of `db` decibels.
pub fn db_to_gain(db: f64) -> f64 {
    exp2(db * DOUBLINGS_PER_DB)
}

pub const DOUBLINGS_PER_DB: f64 = 1.0 / 6.020_599_913_279_624;

/// Straight lines through `(x, y)`, clamped at both ends; `x` ascending.
fn interpolate(x: &[f64], y: &[f64], v: f64) -> f64 {
    let mut prev: Option<(f64, f64)> = None;
    for (&xi, &yi) in x.iter().zip(y) {
        if v <= xi {
            return match prev {
                Some((x0, y0)) if v < xi => y0 + (yi - y0) * (v - x0) / (xi - x0),
                _ => yi,
            };
        }
        prev = Some((xi, yi));
    }
    prev.map_or(0.0, |p| p.1)
}

fn at(t: &[f64], i: i32) -> f64 {
    t.get(i.clamp(0, t.len() as i32 - 1) as usize).copied().unwrap_or(0.0)
}

// ---- levels ----

/// Below this a carrier's level, and its envelope's levels, are silent.
pub const SILENT_BELOW: i32 = 12;
/// Silence, in dB.
pub const SILENT_DB: f64 = -200.0;

/// Operator level 0-99 of a carrier: dB against level 99.
pub fn carrier_level_db(l: i32) -> f64 {
    if l < SILENT_BELOW {
        SILENT_DB
    } else {
        interpolate(&LEVEL_X, &CARRIER_LEVEL_DB, f64::from(l))
    }
}

/// Operator level 0-99 of a modulator: dB of its modulation index against 4 pi radians.
pub fn modulator_level_db(l: i32) -> f64 {
    if l < 6 {
        SILENT_DB
    } else {
        interpolate(&MOD_LEVEL_X, &MOD_LEVEL_DB, f64::from(l.min(99)))
    }
}

/// A carrier's envelope level 0-99: dB against 99. From 20 up in steps of two (2k plays as
/// 2k + 1).
pub fn carrier_eg_db(l: i32) -> f64 {
    let l = l.clamp(0, 99);
    if l >= 20 {
        interpolate(&CARRIER_EG_X, &CARRIER_EG_DB, f64::from(l | 1))
    } else if l < SILENT_BELOW {
        SILENT_DB
    } else {
        interpolate(&LEVEL_X, &EG_LEVEL_DB, f64::from(l))
    }
}

/// A modulator's envelope level 0-99: dB against 99, on the modulator level curve in steps of two.
pub fn modulator_eg_db(l: i32) -> f64 {
    let l = l.clamp(0, 99) | 1;
    if l < 6 {
        SILENT_DB
    } else {
        modulator_level_db(l) - at(&MOD_LEVEL_DB, 31)
    }
}

/// The carriers' output level by routing 1-88, dB against routing 1.
pub fn routing_gain_db(r: i32) -> f64 {
    at(&ROUTING_GAIN_DB, r - 1)
}

/// The peak level of a lone full-level sine carrier on routing 1 (operator and envelope levels
/// 99, velocity sensitivity 0), in dBFS.
pub const CARRIER_FULL_DBFS: f64 = -17.64;

/// Level velocity sensitivity -7..+7 at a velocity (fractional, as the part's velocity law
/// leaves it): dB against sensitivity 0.
pub fn velocity_db(sens: i32, velocity: f64) -> f64 {
    let row = LEVEL_VELOCITY_DB
        .get((sens.clamp(-7, 7) + 7) as usize)
        .copied()
        .unwrap_or([0.0; 17]);
    if sens > 0 && velocity < 8.0 {
        let v = velocity.max(0.25);
        return row[0] + interpolate(&LOW_VELOCITY, &LOW_VELOCITY_DB, v) * f64::from(sens) / 7.0;
    }
    interpolate(&LEVEL_VELOCITIES, &row, velocity.clamp(1.0, 127.0))
}

/// The part's velocity sensitivity depth and offset (0-127 each) applied to a key velocity:
/// 0 or below plays no note.
pub fn part_velocity(depth: i32, offset: i32, velocity: f64) -> f64 {
    (velocity * f64::from(depth) / 64.0 + 2.0 * f64::from(offset - 64)).min(127.0)
}

// ---- envelope ----

/// Decay 1, decay 2 and release time 0-99: ms per dB.
pub fn eg_ms_per_db(t: i32) -> f64 {
    at(&DECAY_MS_PER_DB, t)
}

/// Attack time 0-99: ms per dB (its rate, as a decay of that time).
pub fn attack_ms_per_db(t: i32) -> f64 {
    at(&ATTACK_MS_PER_DB, t)
}

/// Hold time 0-99: ms.
pub fn hold_ms(t: i32) -> f64 {
    at(&HOLD_MS, t)
}

/// A rising segment approaches this far above the operator's full level, exponentially in dB.
pub const ATTACK_OVER_DB: f64 = 1.52;
/// The attack starts at least this far below the operator's full level.
pub const ATTACK_START_DB: f64 = 56.3;

/// A time's place on the envelope's rate scale: eight steps a doubling of the rate, the decay
/// table's times on it; time 0 is step 119, 93-99 step 0.
pub fn rate_step(t: i32) -> i32 {
    let v = at(&DECAY_MS_PER_DB, 99) * 8.0 / eg_ms_per_db(t);
    let e = (log2(v / 8.0 + 1e-9) as i32).clamp(0, 20); // floor (of a non-negative number)
    (8 * e + ((v / pow2i(i64::from(e)) + 0.5) as i32).clamp(0, 64) - 8).min(127)
}

pub fn rate(step: i32) -> f64 {
    f64::from(8 + (step & 7)) * pow2i(i64::from(step >> 3))
}

/// Time key follow 0-7 at a key: rate steps (keys in groups of three counted from key 1; keys
/// 64-69 do not move; at most 15 groups either way; truncated towards 0).
pub fn time_key_follow_steps(sens: i32, key: i32) -> i32 {
    let group = (key.clamp(-256, 383) + 2).div_euclid(3) - 1;
    let keys = if group <= 21 {
        -(21 - group).min(15)
    } else {
        (group - 22).min(15)
    };
    sens.clamp(0, 7) * keys / 4
}

/// A segment of time `t` moved by `steps`: the factor on its time (the moved step held in 0-127).
pub fn time_key_follow_scale(steps: i32, t: i32) -> f64 {
    let from = rate_step(t);
    rate(from) / rate((from + steps).clamp(0, 127))
}

// ---- pitch: the grid of 1/2048 octave ----

/// A4's step on the grid (440.169 Hz).
pub const A4_STEPS: i32 = 13012;

/// A key's pitch in steps (equal temperament, rounded up to the 1/1024-octave grid from A).
pub fn key_steps(key: i32) -> i32 {
    let num = (key.clamp(-256, 383) - 69) * 1024;
    A4_STEPS + 2 * num.div_euclid(12) + if num.rem_euclid(12) != 0 { 2 } else { 0 }
}

/// A step's frequency: 44100 m 2^(octave - 25) Hz, m = 4096 x 2^(step / 2048) rounded.
pub fn steps_hz(steps: i32) -> f64 {
    let m = (4096.0 * exp2(f64::from(steps & 2047) / 2048.0) + 0.5) as i32;
    44100.0 * f64::from(m) * exp2(f64::from((steps >> 11) - 25))
}

/// Ratio mode: coarse 0-31 and fine 0-127 (fine 100 and above carries into the next coarse
/// value; past coarse 31 the reference plays 0.5 at fine 100, and a fixed ratio above it).
pub fn ratio(coarse: i32, fine: i32) -> f64 {
    let (mut c, mut f) = (coarse.clamp(0, 31), fine.clamp(0, 127));
    if f >= 100 {
        c += 1;
        f -= 100;
        if c == 32 {
            return if f == 0 { at(&COARSE_RATIO, 0) } else { RATIO_PAST_TOP };
        }
    }
    at(&COARSE_RATIO, c) * at(&FINE_FACTOR, f)
}

/// A ratio's steps, on the 1/1024-octave grid.
pub fn ratio_steps(r: f64) -> i32 {
    2 * round(1024.0 * log2(r))
}

/// Detune -15..+15 at a key: cents (the measured table, straight between its keys).
pub fn detune_cents(d: i32, key: i32) -> f64 {
    let col = (d.clamp(-15, 15) + 15) as usize;
    let y = DETUNE_CENTS.map(|row| row.get(col).copied().unwrap_or(0.0));
    interpolate(&DETUNE_KEYS, &y, f64::from(key))
}

pub fn detune_steps(d: i32, key: i32) -> i32 {
    2 * round(detune_cents(d, key) * 1024.0 / 1200.0)
}

/// Fixed mode: coarse 0-31 and fine 0-127 in whole 1/128-octave steps from A4's step.
pub fn fixed_steps(coarse: i32, fine: i32) -> i32 {
    A4_STEPS + 2048 * (coarse.clamp(0, 31) - 16) + 16 * fine.clamp(0, 127)
}

/// The highest frequency a fixed-mode operator plays: higher values hold here.
pub const FIXED_MAX_HZ: f64 = 22_040.0;

/// To the nearest integer, held within +-2^24 (so callers' integer arithmetic cannot overflow).
fn round(x: f64) -> i32 {
    ((if x < 0.0 { x - 0.5 } else { x + 0.5 }) as i32).clamp(-1 << 24, 1 << 24)
}

// ---- pitch: offsets a note keeps, envelopes, bend and portamento ----

/// Cents to the nearest step of the grid.
pub fn cents_steps(c: f64) -> i32 {
    round(c * 2048.0 / 1200.0)
}

/// Pitch key follow 0-127 (-200 % to +200 %; 96 is 100 %) at a key, about a centre key: cents off
/// the key's own pitch, straight in cents.
pub fn pitch_key_follow_cents(v: i32, key: i32, centre: i32) -> f64 {
    let pct = KEY_FOLLOW_PERCENT.get(v.clamp(0, 127) as usize).copied().unwrap_or(100);
    f64::from((i32::from(pct) - 100) * (key.clamp(-256, 383) - centre.clamp(0, 127)))
}

/// Random pitch 0-127 with a uniform 32-bit draw: whole steps of 18.75 cents within depth / 4
/// steps either way, cut towards 0.
pub fn random_pitch_cents(depth: i32, r: u32) -> f64 {
    let u = f64::from(r >> 8) * (2.0 / 16_777_216.0) - 1.0;
    f64::from((u * f64::from(depth.clamp(0, 127)) / 4.0) as i32) * 18.75
}

/// The part's pitch velocity sensitivity -64..+63 at a velocity: cents, every operator.
pub fn pitch_velocity_cents(depth: i32, velocity: f64) -> f64 {
    interpolate(&PV_DEPTH, &PV_DEPTH_CENTS, f64::from(depth.clamp(-64, 63)))
        * interpolate(&PV_X, &PV_FACTOR, velocity.clamp(0.0, 127.0))
}

/// An operator's pitch velocity sensitivity -7..+7 at a velocity: cents, fixed-frequency operators
/// only (straight in velocity from 64; the value at 127 by sensitivity).
pub fn fixed_pitch_velocity_cents(sens: i32, velocity: f64) -> f64 {
    let top = FIXED_PV_TOP
        .get(sens.unsigned_abs().min(7) as usize)
        .copied()
        .unwrap_or(0.0);
    let c = top * (velocity.clamp(1.0, 127.0) - 64.0) / 63.0;
    if sens < 0 {
        -c
    } else {
        c
    }
}

/// Fixed-mode key follow 0-99 at a key: cents off its frequency, about key 60.
pub fn fixed_key_follow_cents(sens: i32, key: i32) -> f64 {
    interpolate(&FKF_X, &FKF_CENTS, f64::from(sens.clamp(0, 99))) * f64::from(key.clamp(-256, 383) - 60)
}

/// The part's detune -128..+127 (documented as tenths of a hertz): cents, the same at every key;
/// nothing within 2 of 0, straight between the measured values.
pub fn part_detune_cents(d: i32) -> f64 {
    let raw = d.clamp(-128, 127) + 128;
    let t = |i: i32| at(&PART_DETUNE_CENTS, i);
    match raw {
        126..=130 => 0.0,
        121..=125 => t(15) * f64::from(126 - raw) / 6.0,
        131..=135 => t(17) * f64::from(raw - 130) / 6.0,
        255 => t(32),
        _ => {
            let i = raw / 8;
            t(i) + (t(i + 1) - t(i)) * f64::from(raw - 8 * i) / if i == 31 { 7.0 } else { 8.0 }
        }
    }
}

/// An operator's pitch envelope level -50..+50: cents.
pub fn peg_cents(level: i32) -> f64 {
    at(&PEG_CENTS, level.clamp(-50, 50) + 50)
}

/// An operator's pitch envelope time 0-99: its rate, cents a millisecond, straight in cents.
pub fn peg_cents_per_ms(t: i32) -> f64 {
    at(&PEG_CENTS_PER_MS, t)
}

/// The common pitch envelope's level -50..+50 at depth 0-3 (8, 2, 1 and 0.5 octaves across):
/// cents, steps of 37.5 cents at depth 0, wider beyond +-32.
pub fn part_peg_cents(level: i32, depth: i32) -> f64 {
    let steps = PART_PEG_STEPS
        .get((level.clamp(-50, 50) + 50) as usize)
        .copied()
        .unwrap_or(0);
    f64::from(steps) * 37.5 * at(&[1.0, 0.25, 0.125, 0.0625], depth)
}

/// The common pitch envelope's time 0-99: cents a millisecond, a whole multiple of 0.05619077;
/// 0 is instant (returned as 0).
pub fn part_peg_cents_per_ms(t: i32) -> f64 {
    f64::from(PART_PEG_RATE_STEPS.get(t.clamp(0, 99) as usize).copied().unwrap_or(0)) * 0.056_190_77
}

/// The common pitch envelope's depth velocity sensitivity 0-7 at a velocity: the factor on its
/// levels and rates alike (so its times do not change): of its 128 steps at +50 it loses this
/// many at velocity 1, in proportion to 127 - velocity, rounded down.
pub fn part_peg_velocity_scale(sens: i32, velocity: f64) -> f64 {
    let lost = f64::from(PART_PEG_LOST.get(sens.clamp(0, 7) as usize).copied().unwrap_or(0));
    // (truncation of a non-negative number is its floor)
    f64::from(128 - (lost * (127.0 - velocity.clamp(1.0, 127.0)) / 126.0) as i32) / 128.0
}

/// The common pitch envelope's time key follow 0-7 at a key: the factor on its rates,
/// 1 + s (key - 3) / 33 (keys below 3 as key 3).
pub fn part_peg_key_scale(sens: i32, key: i32) -> f64 {
    1.0 + f64::from(sens.clamp(0, 7) * (key.clamp(3, 383) - 3)) / 33.0
}

/// Pitch bend -8192..+8191 with the part's ranges (the pitch at full bend up and at full bend
/// down, semitones -48..+24 each): whole cents, then whole steps, each cut towards 0.
pub fn bend_steps(bend: i32, up: i32, down: i32) -> i32 {
    let b = bend.clamp(-8192, 8191);
    let (range, full) = if b >= 0 { (up, 8191) } else { (-down, 8192) };
    let cents = b * range.clamp(-48, 48) * 100 / full;
    cents * 2048 / 1200
}

/// Portamento moves in steps this far apart (440 samples at 44.1 kHz).
pub const GLIDE_TICK_MS: f64 = 440.0 / 44.1;

/// Portamento time 0-127 and time mode 0-3 (rate 1, time 1, rate 2, time 2) over a glide of
/// `semitones`: semitones a second, straight in semitones (time 0: instant, returned as 0). The
/// rates are straight in log rate between the measured times; time 1 takes as long for any glide
/// as rate 1 takes for an octave, time 2 the same up to an octave; rate 2 and time 2 start at most
/// an octave away (the caller's part).
pub fn portamento_rate(time: i32, mode: i32, semitones: f64) -> f64 {
    let t = f64::from(time.clamp(0, 127));
    if t == 0.0 {
        return 0.0;
    }
    let i = PORTA_TIMES.iter().rposition(|&x| x <= t).unwrap_or(0);
    let (x0, y0) = (at(&PORTA_TIMES, i as i32), at(&PORTA_RATE, i as i32));
    let (x1, y1) = (at(&PORTA_TIMES, i as i32 + 1), at(&PORTA_RATE, i as i32 + 1));
    let rate = if x1 > x0 {
        y0 * exp2(log2(y1 / y0) * (t - x0) / (x1 - x0))
    } else {
        y0
    };
    match mode {
        1 => rate * semitones / 12.0,
        3 => rate * semitones.min(12.0) / 12.0,
        _ => rate,
    }
}

// ---- feedback ----

/// Feedback 0-7 on a carrier: its amount against a modulator's at the same value.
pub fn carrier_feedback_scale(f: i32) -> f64 {
    at(&CARRIER_FEEDBACK, f)
}

/// Feedback 1-7: the phase offset in cycles a unit of the sum of the operator's last two outputs
/// (in cycles of a modulator's deviation) gives; 0 is off.
pub fn feedback_amount(f: i32) -> f64 {
    if f <= 0 {
        0.0
    } else {
        pow2i(i64::from(f.min(7)) - 9)
    }
}

// ---- the measured data ----

const DECAY_MS_PER_DB: [f64; 100] = [
    0.06422956, 0.07406888, 0.07406888, 0.08756147, 0.1070121, 0.1070121, 0.1283906, 0.1482076, 0.1482076, 0.1751299,
    0.1751299, 0.2140819, 0.256875, 0.256875, 0.2963951, 0.3502656, 0.3502656, 0.4283256, 0.5138304, 0.5138304,
    0.5927736, 0.7002282, 0.7002282, 0.8561943, 0.8561943, 1.027078, 1.185271, 1.185271, 1.401477, 1.713092, 1.713092,
    2.054005, 2.375587, 2.375587, 2.799806, 2.799807, 3.428563, 4.108464, 4.108464, 4.739116, 5.597653, 5.597654,
    6.846748, 8.217957, 8.217957, 9.487787, 11.20562, 11.20561, 13.70369, 13.70375, 16.44134, 18.96404, 18.9639,
    22.41762, 27.41311, 27.41344, 32.88795, 37.94203, 37.90653, 44.84388, 54.75025, 54.81438, 65.78719, 65.77894,
    75.90866, 89.67639, 89.70719, 109.6494, 131.5843, 131.5732, 151.6223, 179.4332, 179.4286, 219.3055, 219.2955,
    262.8264, 303.6603, 303.6446, 358.8679, 438.6262, 438.0034, 526.3527, 607.3285, 607.3083, 717.7403, 877.1923,
    877.2531, 1052.734, 1052.734, 1214.698, 1435.544, 1435.599, 1754.708, 1974.003, 1974.005, 1974.006, 1974.012,
    1973.984, 1973.986, 1974.037,
];
const ATTACK_MS_PER_DB: [f64; 100] = [
    0.01834473, 0.02731587, 0.02961298, 0.04582566, 0.06430807, 0.07484329, 0.08674849, 0.09591333, 0.1066609,
    0.118664, 0.1331587, 0.1685054, 0.2150772, 0.2331177, 0.2545983, 0.2793993, 0.3086193, 0.3808669, 0.472306,
    0.5093471, 0.5515246, 0.6018775, 0.6593584, 0.7320307, 0.8151857, 0.9650131, 1.144944, 1.24707, 1.360854, 1.507503,
    1.672916, 1.973991, 2.332272, 2.537975, 2.763652, 3.058465, 3.387127, 3.713423, 4.073421, 4.765772, 5.569314,
    6.197589, 6.888162, 7.655683, 8.508726, 9.470861, 10.56016, 11.79223, 13.16805, 14.70438, 16.34483, 18.18838,
    20.14725, 22.31709, 24.72062, 27.37535, 32.85475, 37.96294, 37.96305, 44.87017, 55.70303, 54.75394, 65.77849,
    65.77849, 75.90835, 89.63767, 99.59409, 110.6506, 122.9346, 136.5823, 151.6269, 169.3109, 188.91, 210.778,
    235.1773, 262.3062, 290.4979, 321.6031, 356.039, 394.1621, 436.3489, 500.7175, 574.5573, 659.2862, 756.5099,
    868.202, 956.6754, 1054.324, 1161.939, 1280.539, 1411.84, 1506.817, 1608.862, 1717.818, 1834.152, 2560.441,
    3040.555, 3040.429, 3727.686, 3727.752,
];
const HOLD_MS: [f64; 100] = [
    0.0, 1.354045, 1.343859, 1.354045, 1.343859, 1.355369, 1.343859, 1.353269, 1.343859, 1.748507, 1.738291, 2.28813,
    2.689951, 2.687773, 3.237282, 3.977392, 3.98297, 5.093089, 5.910362, 5.911806, 6.975654, 8.463499, 8.465149,
    10.71769, 10.72775, 12.31553, 14.4818, 14.47666, 17.47608, 21.9567, 21.96104, 25.17932, 29.473, 29.46389, 35.46647,
    35.47967, 44.44431, 50.90362, 50.89823, 59.46865, 71.46654, 71.69602, 89.37909, 101.6755, 101.6847, 118.8117,
    144.3118, 144.3118, 179.4245, 179.4245, 205.439, 239.8464, 239.8464, 286.783, 360.1282, 360.1216, 411.695,
    479.3175, 479.3175, 575.9843, 718.5895, 718.5895, 822.413, 822.413, 959.9225, 1150.408, 1150.415, 1439.415,
    1643.76, 1643.76, 1918.107, 2301.974, 2301.974, 2878.024, 2878.024, 3290.307, 3838.517, 3838.517, 4606.392,
    5758.167, 5758.157, 6579.92, 7676.485, 7676.485, 9211.335, 11515.33, 11515.37, 13159.5, 13159.5, 15354.68,
    18423.75, 18423.76, 23031.47, 26320.91, 26320.65, 30708.3, 36849.63, 36849.59, 46062.16, 46062.14,
];
const LEVEL_X: [f64; 17] = [
    12., 13., 14., 15., 16., 17., 18., 19., 20., 30., 40., 50., 60., 70., 80., 90., 99.,
];
const EG_LEVEL_DB: [f64; 17] = [
    -74.03, -70.67, -69.53, -68.86, -65.8, -65.8, -63.84, -63.02, -60.31, -51.92, -44.1, -36.38, -28.79, -21.24, -13.7,
    -6.167, 0.0,
];
const CARRIER_LEVEL_DB: [f64; 17] = [
    -74.03, -70.67, -69.53, -68.86, -68.61, -65.8, -63.84, -63.38, -61.06, -52.81, -44.84, -37.14, -29.55, -21.99,
    -14.45, -6.921, 0.0,
];
const CARRIER_EG_X: [f64; 13] = [21., 25., 31., 35., 41., 45., 51., 55., 61., 71., 81., 91., 99.];
const CARRIER_EG_DB: [f64; 13] = [
    -60.17, -57.41, -51.77, -48.67, -43.95, -40.83, -36.23, -33.20, -28.64, -21.09, -13.55, -6.02, 0.0,
];
const MOD_LEVEL_X: [f64; 32] = [
    6., 9., 12., 15., 18., 21., 24., 27., 30., 33., 36., 39., 42., 45., 48., 51., 54., 57., 60., 63., 66., 69., 72.,
    75., 78., 81., 84., 87., 90., 93., 96., 99.,
];
const MOD_LEVEL_DB: [f64; 32] = [
    -83.86, -77.63, -71.98, -66.41, -63.35, -60.17, -57.84, -55.5, -53.21, -50.92, -48.63, -46.36, -44.08, -41.81,
    -39.55, -37.28, -35.02, -32.76, -30.5, -28.24, -25.98, -23.72, -21.46, -19.2, -16.94, -14.68, -12.42, -10.17,
    -7.909, -5.651, -3.393, -1.132,
];
const CARRIER_FEEDBACK: [f64; 8] = [1.0, 0.18451, 0.18445, 0.18446, 0.18446, 0.18446, 0.18453, 0.18554];
const ROUTING_GAIN_DB: [f64; 88] = [
    0.0, 6.021, 6.021, 6.021, 6.021, 3.011, 1.505, 1.505, 9.031, 9.031, 9.031, 9.031, 6.021, 6.021, 9.031, 9.031,
    9.031, 9.031, 9.031, 9.031, 9.031, 9.031, 9.031, 13.55, 13.55, 13.55, 6.021, 6.021, 3.011, 3.011, 3.011, 1.505,
    6.021, 6.021, 6.021, 3.011, 13.55, 13.55, 13.55, 13.55, 13.55, 13.55, 13.55, 13.55, 13.55, 13.55, 13.55, 13.55,
    13.55, 13.55, 13.55, 13.55, 13.55, 13.55, 9.031, 9.031, 9.031, 9.031, 9.031, 9.031, 9.031, 9.031, 9.031, 9.031,
    3.011, 13.55, 9.031, 13.55, 13.55, 13.55, 13.55, 13.55, 3.011, 13.55, 13.55, 6.021, 13.55, 9.031, 13.55, 3.011,
    13.55, 9.031, 13.55, 3.011, 13.55, 6.021, 3.011, 6.021,
];
const LEVEL_VELOCITIES: [f64; 17] = [
    1., 8., 16., 24., 32., 40., 48., 56., 64., 72., 80., 88., 96., 104., 112., 120., 127.,
];
const LEVEL_VELOCITY_DB: [[f64; 17]; 15] = [
    [
        5.27, 4.14, 2.63, 1.13, -0.38, -2.63, -4.89, -7.53, -10.54, -13.18, -16.19, -19.96, -23.73, -29.02, -35.84,
        -45.05, -120.0,
    ],
    [
        4.52, 3.39, 2.26, 1.13, -0.38, -2.26, -4.14, -6.4, -9.03, -11.29, -13.93, -16.94, -20.33, -24.86, -30.53,
        -38.5, -70.68,
    ],
    [
        3.76, 3.01, 1.88, 0.75, -0.38, -1.88, -3.39, -5.27, -7.53, -9.41, -11.67, -14.31, -16.94, -20.71, -25.62,
        -32.04, -57.07,
    ],
    [
        3.01, 2.26, 1.51, 0.75, -0.38, -1.51, -2.63, -4.14, -6.02, -7.53, -9.41, -11.29, -13.55, -16.57, -20.33,
        -25.62, -45.05,
    ],
    [
        2.26, 1.88, 1.13, 0.75, 0.0, -1.13, -1.88, -3.01, -4.52, -5.65, -6.77, -8.28, -10.16, -12.42, -15.06, -19.2,
        -33.56,
    ],
    [
        1.51, 1.13, 0.75, 0.38, 0.0, -0.75, -1.13, -1.88, -3.01, -3.76, -4.52, -5.65, -6.77, -8.28, -10.16, -12.8,
        -22.22,
    ],
    [
        0.75, 0.75, 0.38, 0.38, 0.0, -0.38, -0.38, -0.75, -1.51, -1.88, -2.26, -2.63, -3.39, -4.14, -4.89, -6.4, -10.92,
    ],
    [0.0; 17],
    [
        -8.28, -6.02, -4.89, -3.76, -3.39, -2.63, -2.26, -1.88, -1.51, -0.75, -0.38, -0.38, 0.0, 0.38, 0.38, 0.76, 0.68,
    ],
    [
        -16.57, -12.42, -9.79, -7.9, -6.77, -5.65, -4.52, -3.76, -3.01, -1.88, -1.13, -0.75, 0.0, 0.38, 0.75, 1.51,
        1.51,
    ],
    [
        -24.86, -18.45, -15.06, -12.05, -10.16, -8.28, -6.77, -5.65, -4.52, -3.01, -1.88, -1.13, 0.0, 0.75, 1.13, 1.88,
        2.26,
    ],
    [
        -33.18, -24.86, -19.96, -16.19, -13.55, -11.29, -9.41, -7.53, -6.02, -4.14, -2.63, -1.51, -0.38, 0.75, 1.51,
        2.63, 3.01,
    ],
    [
        -41.57, -31.29, -24.86, -20.33, -16.94, -14.31, -11.67, -9.41, -7.53, -5.27, -3.39, -1.88, -0.38, 1.13, 1.88,
        3.39, 3.76,
    ],
    [
        -50.18, -37.36, -30.15, -24.48, -20.33, -16.94, -13.93, -11.29, -9.03, -6.4, -4.14, -2.26, -0.38, 1.13, 2.26,
        3.76, 4.52,
    ],
    [
        -59.11, -43.88, -35.09, -28.64, -23.73, -19.96, -16.19, -13.18, -10.54, -7.53, -4.89, -2.63, -0.38, 1.51, 2.63,
        4.52, 5.27,
    ],
];
const LOW_VELOCITY: [f64; 13] = [0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1., 2., 3., 4., 5., 6., 8.];
const LOW_VELOCITY_DB: [f64; 13] = [
    -60.0, -16.26, -12.25, -10.96, -6.08, -2.88, 0.0, 2.98, 5.85, 8.62, 10.96, 12.13, 15.23,
];
const COARSE_RATIO: [f64; 32] = [
    0.4999996, 1.0, 2.0, 2.999999, 3.999999, 5.000641, 5.999998, 7.001283, 7.999998, 8.998712, 10.00128, 10.99486,
    12.0, 12.99743, 14.00257, 15.00257, 16.0, 17.00257, 17.99742, 18.99999, 20.00257, 21.00257, 21.98971, 22.99742,
    23.99999, 24.99228, 25.99485, 26.99742, 28.00513, 28.98714, 30.00513, 30.99742,
];
const FINE_FACTOR: [f64; 100] = [
    1.0, 1.010122, 1.019762, 1.030206, 1.040007, 1.049807, 1.059929, 1.069891, 1.080173, 1.089653, 1.100097, 1.109737,
    1.119537, 1.130302, 1.140263, 1.149582, 1.159704, 1.169987, 1.18027, 1.18991, 1.19955, 1.210154, 1.220116,
    1.230077, 1.240038, 1.250161, 1.25948, 1.269762, 1.280206, 1.289685, 1.300289, 1.30993, 1.31973, 1.329691,
    1.339653, 1.349615, 1.359576, 1.369859, 1.380141, 1.38946, 1.399743, 1.410347, 1.419987, 1.429627, 1.440231,
    1.449871, 1.459833, 1.469795, 1.479756, 1.489717, 1.5, 1.509961, 1.520245, 1.529563, 1.539846, 1.549486, 1.56009,
    1.569409, 1.580013, 1.589653, 1.599615, 1.610219, 1.62018, 1.630141, 1.640103, 1.650065, 1.660026, 1.670309,
    1.679306, 1.689589, 1.699871, 1.710475, 1.719794, 1.730077, 1.739396, 1.750321, 1.75964, 1.770565, 1.780206,
    1.789846, 1.799486, 1.810411, 1.820373, 1.830013, 1.839974, 1.849936, 1.860219, 1.87018, 1.880463, 1.88946,
    1.899422, 1.910026, 1.920309, 1.929306, 1.93991, 1.950514, 1.959512, 1.970437, 1.979756, 1.99036,
];
const RATIO_PAST_TOP: f64 = 6.658096;
const DETUNE_KEYS: [f64; 8] = [24., 36., 48., 60., 72., 84., 96., 108.];
#[allow(clippy::approx_constant)] // measured cents, not 2 pi
const DETUNE_CENTS: [[f64; 31]; 8] = [
    [
        -43.38, -39.16, -35.27, -29.77, -25.9, -22.05, -19.17, -17.91, -15.37, -12.43, -11.1, -8.493, -5.559, -4.243,
        -1.638, 0.0, 1.356, 3.995, 5.35, 8.303, 10.93, 12.6, 15.23, 18.02, 19.22, 21.69, 25.74, 29.79, 35.09, 39.11,
        43.13,
    ],
    [
        -37.68, -33.99, -30.59, -25.78, -22.4, -19.02, -16.49, -15.37, -13.12, -10.6, -9.484, -7.249, -4.735, -3.621,
        -1.392, 0.0, 1.111, 3.333, 4.445, 6.938, 9.154, 10.54, 12.75, 15.23, 16.33, 18.54, 22.11, 25.68, 30.33, 33.88,
        37.42,
    ],
    [
        -31.72, -28.56, -25.64, -21.54, -18.64, -15.74, -13.56, -12.55, -10.6, -8.544, -7.664, -5.848, -3.798, -2.921,
        -1.11, 0.0, 0.9104, 2.752, 3.663, 5.732, 7.569, 8.707, 10.54, 12.65, 13.59, 15.47, 18.49, 21.5, 25.43, 28.43,
        31.42,
    ],
    [
        -25.78, -23.52, -21.27, -17.61, -15.37, -13.12, -11.72, -10.6, -8.365, -7.247, -6.13, -4.735, -3.62, -2.505,
        -1.391, 0.0, 1.112, 2.224, 3.334, 4.444, 5.831, 6.94, 8.048, 10.54, 11.64, 12.75, 15.23, 17.44, 21.01, 23.21,
        25.68,
    ],
    [
        -20.14, -18.47, -16.8, -14.02, -12.35, -10.69, -9.688, -8.907, -7.247, -6.273, -5.299, -4.106, -3.133, -2.161,
        -1.19, 0.0, 0.9619, 1.923, 2.884, 3.844, 5.022, 5.981, 6.94, 8.787, 9.545, 10.3, 12.14, 13.77, 16.47, 18.08,
        19.91,
    ],
    [
        -14.24, -12.39, -12.47, -10.61, -8.279, -6.429, -6.507, -6.584, -4.735, -4.677, -2.694, -2.636, -2.579,
        -0.1152, -0.05761, 0.0, 0.08565, 0.1713, 2.18, 2.266, 2.351, 4.359, 4.445, 6.283, 6.2, 6.117, 7.954, 10.27,
        12.1, 12.02, 13.85,
    ],
    [
        -8.365, -7.247, -7.247, -6.13, -4.735, -3.62, -3.62, -3.62, -2.505, -2.505, -1.391, -1.391, -1.391, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.112, 1.112, 1.112, 2.224, 2.224, 3.335, 3.335, 3.335, 4.445, 5.831, 6.94, 6.94, 8.048,
    ],
    [
        -2.505, -2.089, -1.971, -1.556, -1.066, -0.6512, -0.533, -0.4149, 0.0, -0.09527, 0.106, 0.0107, -0.08457,
        0.1905, 0.09527, 0.0, -0.08456, -0.1691, 0.04238, -0.04219, -0.1268, 0.08456, 0.0, 0.3799, 0.4641, 0.5483,
        0.928, 1.381, 1.761, 1.845, 2.224,
    ],
];
// pitch key follow by value, percent: 1 % steps within 10 of 0, 100 and 200 (and down to 191 at the
// bottom), 5 % between
const KEY_FOLLOW_PERCENT: [i16; 128] = [
    -200, -199, -198, -197, -196, -195, -194, -193, -192, -191, -190, -185, -180, -175, -170, -165, -160, -155, -150,
    -145, -140, -135, -130, -125, -120, -115, -110, -105, -104, -103, -102, -101, -100, -99, -98, -97, -96, -95, -90,
    -85, -80, -75, -70, -65, -60, -55, -50, -45, -40, -35, -30, -25, -20, -15, -10, -9, -8, -7, -6, -5, -4, -3, -2, -1,
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95, 96, 97, 98,
    99, 100, 101, 102, 103, 104, 105, 110, 115, 120, 125, 130, 135, 140, 145, 150, 155, 160, 165, 170, 175, 180, 185,
    190, 192, 193, 194, 195, 196, 197, 198, 199, 200,
];
const PV_DEPTH: [f64; 9] = [-64., -48., -16., -1., 0., 1., 16., 48., 63.];
const PV_DEPTH_CENTS: [f64; 9] = [
    -1189.46, -891.979, -296.61, -17.6139, 0.0, 17.4349, 296.341, 891.797, 1189.4,
];
const PV_X: [f64; 10] = [0., 1., 16., 32., 48., 64., 80., 96., 112., 127.];
const PV_FACTOR: [f64; 10] = [
    0.0, 0.00750278, 0.126218, 0.25242, 0.378432, 0.50444, 0.630633, 0.756767, 0.882821, 1.0,
];
// at velocity 127, by sensitivity (4 and 6 halfway, unmeasured)
const FIXED_PV_TOP: [f64; 8] = [0.0, 73.75, 147.7, 304.05, 604.1, 904.15, 1204.1, 1504.05];
const FKF_X: [f64; 5] = [0., 25., 50., 75., 99.];
const FKF_CENTS: [f64; 5] = [0.0, 25.784, 50.78, 75.7805, 100.0];
// every eighth value from -128 (and 127)
const PART_DETUNE_CENTS: [f64; 33] = [
    -49.95, -45.94, -42.81, -39.95, -37.11, -33.99, -30.59, -27.76, -24.65, -20.71, -17.62, -14.81, -11.72, -8.93,
    -6.13, -3.06, 0.0, 2.78, 5.83, 8.6, 11.64, 14.4, 17.44, 20.46, 24.58, 27.32, 30.33, 33.88, 36.88, 39.6, 42.59,
    45.57, 49.63,
];
const PEG_CENTS: [f64; 101] = [
    -4800.74, -4200.55, -3675.3, -3225.39, -2925.42, -2775.32, -2625.35, -2475.32, -2362.98, -2250.45, -2137.97,
    -2025.35, -1912.86, -1837.98, -1762.84, -1687.88, -1612.98, -1538.0, -1462.91, -1388.13, -1312.91, -1275.32,
    -1200.39, -1162.98, -1087.96, -1050.42, -975.488, -937.97, -862.936, -825.353, -750.485, -712.856, -675.5,
    -637.974, -600.398, -562.836, -525.426, -487.876, -450.477, -412.978, -375.32, -338.001, -300.338, -262.904,
    -225.359, -188.133, -150.273, -112.909, -75.3136, -38.0961, 0.0, 37.0097, 74.4803, 112.027, 149.607, 187.028,
    224.6, 262.095, 299.615, 336.998, 374.56, 412.052, 449.567, 487.118, 524.502, 562.184, 599.592, 636.958, 674.532,
    712.034, 749.43, 824.537, 862.034, 937.205, 974.541, 1049.5, 1087.08, 1162.04, 1199.56, 1274.46, 1312.03, 1387.03,
    1462.09, 1536.99, 1612.05, 1687.11, 1762.18, 1836.96, 1912.03, 2024.54, 2137.2, 2249.41, 2362.04, 2474.46, 2624.6,
    2774.54, 2924.5, 3224.54, 3674.46, 4199.59, 4762.09,
];
const PEG_CENTS_PER_MS: [f64; 100] = [
    387.613, 336.101, 336.101, 284.374, 232.672, 232.672, 193.865, 167.996, 167.996, 142.154, 142.152, 116.297,
    96.9144, 96.9144, 83.9892, 71.0671, 71.0671, 58.1447, 48.4532, 48.4531, 41.9924, 35.5317, 35.5317, 29.0711,
    29.0711, 24.2257, 20.9956, 20.9956, 17.7654, 14.5352, 14.5352, 12.1127, 10.4976, 10.4976, 8.88256, 8.88256,
    7.26752, 6.05626, 6.05626, 5.24875, 4.44124, 4.44124, 3.63374, 3.02811, 3.02811, 2.62436, 2.22062, 2.22062,
    1.81687, 1.81687, 1.51406, 1.31218, 1.31218, 1.11031, 0.908433, 0.908433, 0.757028, 0.656093, 0.656093, 0.555156,
    0.454221, 0.454219, 0.378515, 0.378515, 0.328047, 0.277578, 0.277578, 0.227109, 0.189258, 0.189258, 0.164029,
    0.138789, 0.138789, 0.113555, 0.113555, 0.0946287, 0.0820116, 0.0820116, 0.0693965, 0.0567772, 0.0567767,
    0.0473139, 0.0410066, 0.0410066, 0.0346983, 0.0283897, 0.0283897, 0.0236579, 0.0236579, 0.0205036, 0.0173498,
    0.0173496, 0.0141948, 0.0126179, 0.0126179, 0.0126179, 0.0126179, 0.0126179, 0.0126179, 0.0126182,
];
// the common pitch envelope's level in 37.5-cent steps at depth 0
const PART_PEG_STEPS: [i16; 101] = [
    -128, -116, -104, -95, -85, -76, -68, -61, -56, -52, -49, -46, -43, -41, -39, -37, -35, -33, -32, -31, -30, -29,
    -28, -27, -26, -25, -24, -23, -22, -21, -20, -19, -18, -17, -16, -15, -14, -13, -12, -11, -10, -9, -8, -7, -6, -5,
    -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 38, 40, 43, 46, 49, 53, 58, 65, 73, 82, 92, 103, 115, 127, 128,
];
// its rate by time, in multiples of 0.05619077 cents a millisecond
const PART_PEG_RATE_STEPS: [u8; 100] = [
    0, 254, 244, 233, 212, 203, 194, 186, 179, 172, 166, 160, 154, 148, 142, 136, 131, 125, 120, 115, 110, 106, 102,
    98, 94, 91, 88, 85, 82, 79, 76, 74, 72, 70, 68, 66, 64, 62, 60, 58, 56, 54, 53, 51, 49, 47, 46, 44, 42, 41, 39, 38,
    37, 36, 34, 33, 31, 30, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 18, 17, 16, 16, 15, 14, 14, 13, 13, 12, 12, 11,
    11, 10, 10, 9, 9, 8, 8, 7, 7, 6, 6, 5, 5, 4, 4, 3, 3, 2, 1,
];
// steps of 128 lost at velocity 1, by depth velocity sensitivity
const PART_PEG_LOST: [u8; 8] = [0, 17, 35, 53, 70, 88, 106, 126];
// portamento rate 1, semitones a second, at these times
const PORTA_TIMES: [f64; 35] = [
    1., 2., 4., 6., 8., 12., 16., 20., 24., 28., 32., 36., 40., 44., 48., 52., 56., 60., 64., 68., 72., 76., 80., 84.,
    88., 92., 96., 100., 104., 108., 112., 116., 120., 124., 127.,
];
const PORTA_RATE: [f64; 35] = [
    434.1, 381.0, 378.3, 336.1, 336.2, 274.2, 248.7, 207.3, 176.3, 151.2, 131.1, 116.3, 94.93, 85.03, 71.99, 58.98,
    51.22, 43.10, 36.63, 30.40, 25.63, 21.26, 17.81, 14.66, 11.843, 9.354, 7.533, 5.943, 4.565, 3.402, 2.42, 1.770,
    1.083, 0.5208, 0.1199,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp2_and_log2() {
        for i in -400..=400 {
            let x = f64::from(i) * 0.137;
            assert!((exp2(x) / x.exp2() - 1.0).abs() < 1e-14, "exp2 {x}");
            let y = exp2(x);
            assert!((log2(y) - x).abs() < 1e-12, "log2 {y}");
        }
        assert_eq!(exp2(10.0), 1024.0);
        assert!((log2(3e-310) - 3e-310f64.log2()).abs() < 1e-9);
    }

    /// The grid's anchor points: A4 is step 13012 at 440.169 Hz; fixed coarse 16 fine 0 is A4's
    /// step; fixed coarse 21 fine 64 plays 19.92 kHz.
    #[test]
    fn pitch_grid() {
        assert!((steps_hz(A4_STEPS) - 440.169).abs() < 0.001, "{}", steps_hz(A4_STEPS));
        assert_eq!(key_steps(69), A4_STEPS);
        assert_eq!(fixed_steps(16, 0), A4_STEPS);
        assert!((steps_hz(fixed_steps(21, 64)) - 19920.0).abs() < 2.0);
        // every third key lies on the grid, the others are rounded up (1024 / 12 = 85 1/3)
        assert_eq!(key_steps(72) - key_steps(69), 512);
        assert_eq!(key_steps(70) - key_steps(69), 172);
        assert_eq!(key_steps(68) - key_steps(69), -170);
        assert_eq!(key_steps(57) - key_steps(69), -2048);
        // a step is 1/2048 octave, to the mantissa's rounding (1/8192 at worst: 0.21 cent) at either end
        for s in [0, 1, 777, 2047, 2048, 13012, 20000] {
            let c = 1200.0 * (steps_hz(s) / steps_hz(13012)).log2() - f64::from(s - 13012) * 1200.0 / 2048.0;
            assert!(c.abs() < 0.43, "step {s}: {c} cents");
        }
    }

    #[test]
    fn ratios_and_detune() {
        assert_eq!(ratio(1, 0), 1.0);
        assert_eq!(ratio(0, 0), 0.4999996);
        assert_eq!(ratio(4, 100), ratio(5, 0)); // fine 100 carries
        assert_eq!(ratio(31, 100), 0.4999996);
        assert_eq!(ratio(31, 101), 6.658096);
        assert_eq!(ratio_steps(2.0), 2048);
        assert_eq!(ratio_steps(1.5), 2 * 599); // 1024 log2 1.5 = 599.0
        assert_eq!(detune_cents(15, 24), 43.13);
        assert_eq!(detune_cents(-15, 120), -2.505);
        assert!((detune_cents(1, 66) - (1.112 + 0.9619) / 2.0).abs() < 1e-12);
        assert_eq!(detune_steps(15, 60), 2 * 22); // 25.68 cents: 21.9 steps of 1/1024 octave
    }

    #[test]
    fn levels() {
        assert_eq!(carrier_level_db(99), 0.0);
        assert_eq!(carrier_level_db(11), SILENT_DB);
        assert_eq!(carrier_level_db(55), (-37.14 - 29.55) / 2.0);
        assert!((modulator_level_db(99) + 1.132).abs() < 1e-12);
        assert_eq!(modulator_level_db(5), SILENT_DB);
        // 0.75 dB a step on a modulator
        assert!((modulator_level_db(60) - modulator_level_db(57) - 2.26).abs() < 1e-9);
        assert_eq!(carrier_eg_db(98), 0.0); // 98 plays as 99
        assert_eq!(carrier_eg_db(70), -21.09);
        assert_eq!(carrier_eg_db(15), -68.86);
        assert_eq!(modulator_eg_db(99), 0.0);
        assert_eq!(modulator_eg_db(98), 0.0);
        assert!((modulator_eg_db(92) + 5.651 - 1.132).abs() < 1e-12); // 92 plays as 93
        assert_eq!(routing_gain_db(1), 0.0);
        assert_eq!(routing_gain_db(24), 13.55);
        assert_eq!(routing_gain_db(88), 6.021);
    }

    #[test]
    fn velocity() {
        assert_eq!(velocity_db(7, 127.0), 5.27);
        assert_eq!(velocity_db(-7, 127.0), -120.0);
        assert_eq!(velocity_db(0, 13.0), 0.0);
        assert_eq!(velocity_db(3, 64.0), -4.52);
        assert!((velocity_db(-2, 68.0) - (-3.01 - 3.76) / 2.0).abs() < 1e-12);
        // below velocity 8, positive sensitivities follow +7's finer curve in proportion
        assert!((velocity_db(7, 4.0) - (-59.11 + 8.62)).abs() < 1e-12);
        assert!((velocity_db(7, 0.25) - (-59.11 - 60.0)).abs() < 1e-12);
        assert_eq!(part_velocity(64, 64, 100.0), 100.0);
        assert_eq!(part_velocity(127, 64, 100.0), 127.0);
        assert_eq!(part_velocity(0, 0, 100.0), -128.0);
        assert_eq!(part_velocity(32, 70, 50.0), 37.0);
    }

    #[test]
    fn envelope_times() {
        assert_eq!(eg_ms_per_db(50), 16.44134);
        assert_eq!(attack_ms_per_db(0), 0.01834473);
        assert_eq!(hold_ms(50), 205.439);
        assert_eq!(eg_ms_per_db(150), eg_ms_per_db(99));
        // time key follow: the measured cases
        for (s, k, want) in [
            (7, 21, -26),
            (7, 22, -24),
            (7, 63, -1),
            (7, 64, 0),
            (7, 69, 0),
            (7, 70, 1),
            (7, 112, 26),
            (7, 0, -26),
            (5, 101, 13),
        ] {
            assert_eq!(time_key_follow_steps(s, k), want, "sensitivity {s} key {k}");
        }
        assert!((time_key_follow_scale(21, 5) - 147456.0 / 491520.0).abs() < 1e-12);
        assert!((time_key_follow_scale(-21, 80) - 4.5).abs() < 1e-12);
        // time 35 is step 75; 21 steps up is 5632 / 32768 of its time
        assert!((time_key_follow_scale(21, 35) - 5632.0 / 32768.0).abs() < 1e-12);
        assert_eq!(rate_step(0), 119);
        assert_eq!(rate_step(99), 0);
    }

    #[test]
    fn feedback() {
        assert_eq!(feedback_amount(0), 0.0);
        assert_eq!(feedback_amount(7), 0.25);
        assert_eq!(feedback_amount(1), 1.0 / 256.0);
        assert_eq!(carrier_feedback_scale(5), 0.18446);
    }

    /// The pitch laws at the existing profile's measured values, and the documented bend ranges.
    #[test]
    fn pitch_laws() {
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // operator pitch envelope: levels and rates as measured
        assert_eq!((peg_cents(-50), peg_cents(0), peg_cents(50)), (-4800.74, 0.0, 4762.09));
        assert_eq!((peg_cents_per_ms(0), peg_cents_per_ms(99)), (387.613, 0.0126182));
        // common pitch envelope: 37.5-cent steps (wider past +-32) by depth; rates a multiple of
        // 0.05619077 cents/ms, time 0 instant; velocity and key scale the rates and levels
        assert_eq!(part_peg_cents(50, 0), 4800.0);
        assert_eq!((part_peg_cents(35, 0), part_peg_cents(36, 0)), (1312.5, 1425.0));
        assert_eq!((part_peg_cents(50, 2), part_peg_cents(-50, 3)), (600.0, -300.0));
        assert!(part_peg_cents_per_ms(0) == 0.0 && near(part_peg_cents_per_ms(99), 0.05619077));
        assert!(near(part_peg_cents_per_ms(1), 254.0 * 0.05619077));
        assert_eq!(part_peg_velocity_scale(3, 64.0), 102.0 / 128.0);
        assert_eq!(
            (part_peg_velocity_scale(7, 1.0), part_peg_velocity_scale(7, 127.0)),
            (2.0 / 128.0, 1.0)
        );
        assert_eq!((part_peg_key_scale(7, 36), part_peg_key_scale(5, 2)), (8.0, 1.0));
        // key follow: 96 is 100 % (no change), 64 is 0 % (every key the centre's pitch)
        assert_eq!(pitch_key_follow_cents(96, 30, 60), 0.0);
        assert_eq!(pitch_key_follow_cents(64, 72, 60), -1200.0);
        assert_eq!(pitch_key_follow_cents(127, 84, 48), 3600.0);
        // random pitch: whole 18.75-cent steps within depth / 4 either way
        assert_eq!(random_pitch_cents(127, 0), -31.0 * 18.75);
        assert_eq!(random_pitch_cents(127, u32::MAX), 31.0 * 18.75);
        // (a draw of exactly -1, r < 256, reaches one step further down)
        assert!((1..1000u32).all(|i| random_pitch_cents(4, i.wrapping_mul(0x9e37_79b9)) == 0.0));
        assert!((1..1000u32).all(|i| random_pitch_cents(8, i.wrapping_mul(0x9e37_79b9)).abs() <= 18.75));
        // velocity: the part's depth at every operator, the operator's on fixed frequencies
        assert_eq!(
            (pitch_velocity_cents(63, 127.0), pitch_velocity_cents(-64, 127.0)),
            (1189.4, -1189.46)
        );
        assert!(near(pitch_velocity_cents(16, 64.0), 296.341 * 0.50444));
        assert_eq!(fixed_pitch_velocity_cents(7, 127.0), 1504.05);
        assert!(near(fixed_pitch_velocity_cents(-7, 96.0), -1504.05 * 32.0 / 63.0));
        assert!((fixed_key_follow_cents(99, 72) - 1200.0).abs() < 1e-9 && fixed_key_follow_cents(0, 100) == 0.0);
        // the part's detune: cents, nothing within 2 of 0
        assert_eq!((part_detune_cents(-128), part_detune_cents(127)), (-49.95, 49.63));
        assert_eq!(
            (part_detune_cents(2), part_detune_cents(-2), part_detune_cents(8)),
            (0.0, 0.0, 2.78)
        );
        // bend: the documented ranges, whole cents then whole steps, cut towards 0
        assert_eq!(
            (
                bend_steps(8191, 2, -2),
                bend_steps(-8192, 2, -2),
                bend_steps(0, 24, -48)
            ),
            (341, -341, 0)
        );
        assert_eq!((bend_steps(-8192, 24, -48), bend_steps(8191, 24, -48)), (-8192, 4096));
        assert_eq!(bend_steps(-4096, 2, 12), 1024); // a range below the centre the other way
                                                    // portamento: rate 1 at the measured times, time 1 and 2 by distance
        assert_eq!((portamento_rate(64, 0, 5.0), portamento_rate(0, 0, 12.0)), (36.63, 0.0));
        assert!(near(portamento_rate(64, 1, 24.0), 2.0 * 36.63) && near(portamento_rate(64, 3, 24.0), 36.63));
        assert!(near(portamento_rate(64, 3, 6.0), 36.63 / 2.0) && portamento_rate(127, 2, 3.0) == 0.1199);
        assert!(near(portamento_rate(3, 0, 1.0), (381.0f64 * 378.3).sqrt()));
    }
}
