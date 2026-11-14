// SPDX-License-Identifier: AGPL-3.0-or-later
//! The voice-kernel spike (docs/design.md, "The spike"): eight operators on one routing, one
//! feedback path, envelopes at control rate with ramps, four voices a lane, 128-frame blocks, in
//! two layouts so their cost can be measured. It decides the architecture; it is not the engine.

use crate::simd::{F4, U4, W};
use crate::tables::{Table, BITS, SAW16, SINE, WINDOW};

pub use crate::params::OPS;
/// Frames per control step.
pub const STEP: usize = 16;
/// Frames per block.
pub const BLOCK: usize = 128;
/// Lanes of the spike's pool (32 voices).
pub const LANES: usize = 8;

/// An envelope's floor in log2 of gain (about -180 dB): the level a note starts from and released
/// operators fall to.
const FLOOR: f32 = -30.0;
/// The end rule: a released voice ends once its carriers' summed gain bound is below -96 dB.
const END_GAIN: f32 = 1.584_893e-5;

/// An operator's waveform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Sine,
    /// One single-cycle table (the placeholder band-limited form).
    Table,
    /// A window table times a sine at harmonic `centre`: the resonant forms' structure.
    Window {
        centre: u32,
    },
}

pub use crate::routing::Routing;

impl Routing {
    /// The routing with the most modulation edges in the model's table: operators 1 and 2 (here 0
    /// and 1) each modulate all six others, which are the carriers; operator 1 feeds back on itself.
    pub const MOST_EDGES: Routing = Routing {
        mods: [0, 0, 3, 3, 3, 3, 3, 3],
        carriers: 0xfc,
        fb_from: 0,
        fb_to: 0,
    };
    /// One of the two routings whose feedback runs through other operators: 3 > 4 > 5 with 5 feeding
    /// 3, 6 > 7 > 8; carriers 1, 2, 5 and 8.
    pub const LOOP: Routing = Routing {
        mods: [0, 0, 0, 1 << 2, 1 << 3, 0, 1 << 5, 1 << 6],
        carriers: 0x93,
        fb_from: 4,
        fb_to: 2,
    };
    /// A chain of eight, the deepest serial dependency.
    pub const CHAIN: Routing = Routing {
        mods: [0, 1, 1 << 1, 1 << 2, 1 << 3, 1 << 4, 1 << 5, 1 << 6],
        carriers: 0x80,
        fb_from: 0,
        fb_to: 0,
    };
}

/// The order operators are evaluated in a control step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Each operator renders a whole control step into a buffer before the next one starts; only a
    /// feedback loop through several operators is interleaved sample by sample.
    OperatorMajor,
    /// Every operator for one sample before the next sample. Any feedback path.
    SampleMajor,
}

/// The parameters the spike plays, per operator; times are log2-of-gain units a control step.
#[derive(Clone, Copy, Debug)]
pub struct Sound {
    pub routing: Routing,
    pub forms: [Form; OPS],
    pub ratio: [f32; OPS],
    /// Operator level, log2 of gain (0 is full scale; for a modulator, one cycle of deviation).
    pub level: [f32; OPS],
    pub attack: [f32; OPS],
    pub decay: [f32; OPS],
    pub sustain: [f32; OPS],
    pub release: [f32; OPS],
    /// Feedback in cycles per unit of the mean of the last two outputs.
    pub feedback: f32,
}

impl Sound {
    /// Every operator on `form`, fast attack, a decay to -12 dB and a 0.3 s release.
    pub const fn test(routing: Routing, form: Form) -> Sound {
        Sound {
            routing,
            forms: [form; OPS],
            ratio: [1.0, 2.0, 1.0, 1.0, 2.0, 3.0, 0.5, 1.01],
            level: [-1.0, -1.5, -3.0, -3.0, -3.0, -3.0, -3.0, -3.0],
            attack: [2.0; OPS],
            decay: [0.02; OPS],
            sustain: [-2.0; OPS],
            release: [0.33; OPS],
            feedback: 0.15,
        }
    }
}

/// Four voices of one sound. Plain arrays, the same on every back end.
#[derive(Clone, Copy)]
struct Lane {
    ph: [[u32; W]; OPS],
    inc: [[u32; W]; OPS],
    gain: [[f32; W]; OPS],
    /// Envelope level, log2 of gain.
    eg: [[f32; W]; OPS],
    /// 1 while an operator's envelope is in its attack, else 0.
    att: [[f32; W]; OPS],
    /// All ones for a held key.
    gate: [u32; W],
    /// The feedback source's last two outputs.
    fb: [[f32; W]; 2],
    /// Voices sounding, one bit each.
    active: u8,
}

const LANE0: Lane = Lane {
    ph: [[0; W]; OPS],
    inc: [[0; W]; OPS],
    gain: [[0.0; W]; OPS],
    eg: [[FLOOR; W]; OPS],
    att: [[0.0; W]; OPS],
    gate: [0; W],
    fb: [[0.0; W]; 2],
    active: 0,
};

/// The spike's engine: one sound, `LANES` lanes. Fixed size; no allocation.
pub struct Engine {
    sound: Sound,
    layout: Layout,
    lanes: [Lane; LANES],
}

/// Index helpers that keep every index provably in range (the block path has no panic).
#[inline(always)]
fn slot(v: usize) -> usize {
    v % W
}

impl Engine {
    pub const fn new(sound: Sound, layout: Layout) -> Engine {
        Engine {
            sound,
            layout,
            lanes: [LANE0; LANES],
        }
    }

    /// Starts voice `v` (0 to 4 LANES - 1; others are ignored) at `hz`, at sample rate `rate`.
    pub fn note_on(&mut self, v: usize, hz: f32, rate: f32) {
        let Some(lane) = self.lanes.get_mut(v / W) else { return };
        let s = slot(v);
        let ratio = self.sound.ratio;
        // cycles a sample in 32-bit phase units; `as` saturates
        put(&mut lane.inc, s, |i| {
            (hz * ratio.get(i).copied().unwrap_or(0.0) / rate * 4_294_967_296.0) as u32
        });
        put(&mut lane.ph, s, |_| 0);
        put(&mut lane.gain, s, |_| 0.0);
        put(&mut lane.eg, s, |_| FLOOR);
        put(&mut lane.att, s, |_| 1.0);
        set(&mut lane.gate, s, u32::MAX);
        setf(&mut lane.fb[0], s, 0.0);
        setf(&mut lane.fb[1], s, 0.0);
        lane.active |= [1u8, 2, 4, 8].get(s).copied().unwrap_or(0);
    }

    pub fn note_off(&mut self, v: usize) {
        if let Some(lane) = self.lanes.get_mut(v / W) {
            set(&mut lane.gate, slot(v), 0);
        }
    }

    /// Voices sounding.
    pub fn active(&self) -> u32 {
        self.lanes.iter().map(|l| l.active.count_ones()).sum()
    }

    /// Adds one block of every sounding voice, mono, to `out`.
    pub fn render<V: F4>(&mut self, out: &mut [f32; BLOCK]) {
        let sound = &self.sound;
        let layout = self.layout;
        for lane in self.lanes.iter_mut().filter(|l| l.active != 0) {
            let mut vout = [[0.0f32; W]; BLOCK];
            for chunk in vout.chunks_exact_mut(STEP) {
                let Ok(chunk) = <&mut [[f32; W]; STEP]>::try_from(chunk) else {
                    continue;
                };
                let dg = envelopes::<V>(lane, sound);
                step::<V>(lane, sound, layout, &dg, chunk);
            }
            for (o, v) in out.iter_mut().zip(vout.iter()) {
                *o += (v[0] + v[1]) + (v[2] + v[3]);
            }
        }
    }
}

/// Whether bit `i` of `mask` is set (false past bit 7; no shift can overflow).
#[inline(always)]
fn has(mask: u8, i: usize) -> bool {
    u32::try_from(i)
        .ok()
        .and_then(|i| mask.checked_shr(i))
        .is_some_and(|m| m & 1 != 0)
}

/// Sets voice slot `s` of every operator's entry to `f(operator)`.
fn put<T: Copy>(a: &mut [[T; W]; OPS], s: usize, f: impl Fn(usize) -> T) {
    for (i, row) in a.iter_mut().enumerate() {
        if let Some(e) = row.get_mut(s) {
            *e = f(i);
        }
    }
}

#[inline(always)]
fn set(a: &mut [u32; W], s: usize, x: u32) {
    if let Some(e) = a.get_mut(s) {
        *e = x;
    }
}

#[inline(always)]
fn setf(a: &mut [f32; W], s: usize, x: f32) {
    if let Some(e) = a.get_mut(s) {
        *e = x;
    }
}

/// One control step of every operator's envelope; returns each operator's gain increment a frame
/// (the ramp to the new gain) and ends released voices that fell below the end rule's bound.
#[inline(always)]
fn envelopes<V: F4>(lane: &mut Lane, sound: &Sound) -> [[f32; W]; OPS] {
    let gate = V::U::load(lane.gate);
    let zero = V::splat(0.0);
    let mut dg = [[0.0; W]; OPS];
    let mut bound = zero;
    for i in 0..OPS {
        let (Some(eg), Some(att), Some(gain), Some(d)) =
            (lane.eg.get_mut(i), lane.att.get_mut(i), lane.gain.get(i), dg.get_mut(i))
        else {
            continue;
        };
        let p = |a: &[f32; OPS]| V::splat(a.get(i).copied().unwrap_or(0.0));
        let l = V::load(*eg);
        let a = V::load(*att);
        let up = (l + p(&sound.attack)).min(zero);
        let down = (l - p(&sound.decay)).max(p(&sound.sustain));
        let rel = (l - p(&sound.release)).max(V::splat(FLOOR));
        let l2 = V::select(gate, V::select(a.ge(V::splat(0.5)), up, down), rel);
        *att = V::select(up.ge(zero), zero, a).store();
        *eg = l2.store();
        let target = (l2 + p(&sound.level)).exp2();
        *d = ((target - V::load(*gain)) * V::splat(1.0 / STEP as f32)).store();
        if has(sound.routing.carriers, i) {
            bound = bound + target;
        }
    }
    // a released voice ends below the bound; its slot is free in this step
    let ended = V::select(bound.ge(V::splat(END_GAIN)), zero, V::splat(1.0)).store();
    for (bit, (e, g)) in [1u8, 2, 4, 8].iter().zip(ended.iter().zip(lane.gate.iter())) {
        if *e != 0.0 && *g == 0 {
            lane.active &= !bit;
        }
    }
    dg
}

/// Linear interpolation in `t` at 32-bit phase `p`: the top `BITS` bits index, the next 16 are the
/// fraction.
#[inline(always)]
fn read<V: F4>(t: &Table, p: V::U) -> V {
    let f = p.shr::<{ 16 - BITS as i32 }>().and(V::U::splat(0xffff)).small_to_f32() * V::splat(1.0 / 65536.0);
    let (a, b) = V::gather2(t, p.shr::<{ 32 - BITS as i32 }>());
    (b - a).mul_add(f, a)
}

/// A modulation input in cycles as a 32-bit phase offset (2^-24 cycle resolution; inputs clamped to
/// +-127 cycles so the conversion is exact on every back end).
#[inline(always)]
fn offset<V: F4>(m: V) -> V::U {
    (m.min(V::splat(127.0)).max(V::splat(-127.0)) * V::splat(16_777_216.0))
        .to_i32()
        .shl::<8>()
}

#[inline(always)]
fn wave<V: F4>(form: Form, ph: V::U, off: V::U) -> V {
    match form {
        Form::Sine => read(&SINE, ph.add(off)),
        Form::Table => read(&SAW16, ph.add(off)),
        Form::Window { centre } => {
            read::<V>(&WINDOW, ph.add(off)) * read::<V>(&SINE, ph.mul(V::U::splat(centre)).add(off))
        }
    }
}

/// One operator over one control step into `y`, its form decided once.
#[inline(always)]
fn run_op<V: F4>(
    form: Form,
    ph: &mut [u32; W],
    inc: [u32; W],
    gain: &mut [f32; W],
    dg: [f32; W],
    input: &[[f32; W]; STEP],
    y: &mut [[f32; W]; STEP],
) {
    let inc = V::U::load(inc);
    let dg = V::load(dg);
    let mut p = V::U::load(*ph);
    let mut g = V::load(*gain);
    for (y, m) in y.iter_mut().zip(input.iter()) {
        *y = (wave::<V>(form, p, offset(V::load(*m))) * g).store();
        g = g + dg;
        p = p.add(inc);
    }
    *ph = p.store();
    *gain = g.store();
}

/// One control step of a lane: operators in ascending order (every modulation edge runs upwards),
/// each a whole step at a time, except a feedback loop through several operators, whose operators
/// are interleaved sample by sample. `Layout::SampleMajor` interleaves all eight (measured only).
fn step<V: F4>(lane: &mut Lane, sound: &Sound, layout: Layout, dg: &[[f32; W]; OPS], vout: &mut [[f32; W]; STEP]) {
    let r = &sound.routing;
    let (to, from) = (usize::from(r.fb_to), usize::from(r.fb_from).min(OPS - 1));
    let mut y = [[[0.0f32; W]; STEP]; OPS];
    let mut next = 0;
    for i in 0..OPS {
        if i < next {
            continue;
        }
        let hi = match layout {
            Layout::SampleMajor => OPS - 1,
            Layout::OperatorMajor if i == to && from > to => from,
            Layout::OperatorMajor => i,
        };
        if hi == i {
            single::<V>(lane, sound, dg, &mut y, i);
        } else {
            interleaved::<V>(lane, sound, dg, &mut y, i, hi);
        }
        for k in i..=hi {
            if let (true, Some(out)) = (has(r.carriers, k), y.get(k)) {
                for (o, s) in vout.iter_mut().zip(out.iter()) {
                    *o = (V::load(*o) + V::load(*s)).store();
                }
            }
        }
        next = hi.saturating_add(1);
    }
}

/// Operator `i` over a whole step: its modulation input summed first, then its samples.
fn single<V: F4>(lane: &mut Lane, sound: &Sound, dg: &[[f32; W]; OPS], y: &mut [[[f32; W]; STEP]; OPS], i: usize) {
    let r = &sound.routing;
    let Some((done, rest)) = y.split_at_mut_checked(i) else {
        return;
    };
    let Some((cur, _)) = rest.split_first_mut() else { return };
    let (Some(ph), Some(&inc), Some(gain), Some(&dg), Some(&form), Some(&mods)) = (
        lane.ph.get_mut(i),
        lane.inc.get(i),
        lane.gain.get_mut(i),
        dg.get(i),
        sound.forms.get(i),
        r.mods.get(i),
    ) else {
        return;
    };
    let mut input = [[0.0f32; W]; STEP];
    for (j, src) in done.iter().enumerate() {
        if has(mods, j) {
            for (m, s) in input.iter_mut().zip(src.iter()) {
                *m = (V::load(*m) + V::load(*s)).store();
            }
        }
    }
    if usize::from(r.fb_to) == i && r.fb_from == r.fb_to {
        // self-feedback: sample by sample within this operator
        let fbk = V::splat(sound.feedback * 0.5);
        let (inc, dgv) = (V::U::load(inc), V::load(dg));
        let (mut p, mut g) = (V::U::load(*ph), V::load(*gain));
        let (mut y1, mut y2) = (V::load(lane.fb[0]), V::load(lane.fb[1]));
        for (out, m) in cur.iter_mut().zip(input.iter()) {
            let v = wave::<V>(form, p, offset((y1 + y2).mul_add(fbk, V::load(*m)))) * g;
            *out = v.store();
            (y2, y1) = (y1, v);
            g = g + dgv;
            p = p.add(inc);
        }
        (*ph, *gain, lane.fb) = (p.store(), g.store(), [y1.store(), y2.store()]);
    } else {
        run_op::<V>(form, ph, inc, gain, dg, &input, cur);
    }
}

/// Operators `lo..=hi` sample by sample: each sample's input reads the outputs already written for
/// that sample (sources below `lo` were written for the whole step before).
fn interleaved<V: F4>(
    lane: &mut Lane,
    sound: &Sound,
    dg: &[[f32; W]; OPS],
    y: &mut [[[f32; W]; STEP]; OPS],
    lo: usize,
    hi: usize,
) {
    let r = &sound.routing;
    let mut p = lane.ph.map(V::U::load);
    let inc = lane.inc.map(V::U::load);
    let mut g = lane.gain.map(V::load);
    let dg = dg.map(V::load);
    let fbk = V::splat(sound.feedback * 0.5);
    let (mut y1, mut y2) = (V::load(lane.fb[0]), V::load(lane.fb[1]));
    let (to, from) = (usize::from(r.fb_to), usize::from(r.fb_from));
    for s in 0..STEP {
        for k in lo..=hi {
            let mods = r.mods.get(k).copied().unwrap_or(0);
            let mut m = V::splat(0.0);
            for (j, yj) in y.iter().enumerate().take(k) {
                if has(mods, j) {
                    m = m + V::load(yj.get(s).copied().unwrap_or_default());
                }
            }
            if k == to {
                m = (y1 + y2).mul_add(fbk, m);
            }
            let (Some(pk), Some(&inck), Some(gk), Some(&dgk), Some(&form)) =
                (p.get_mut(k), inc.get(k), g.get_mut(k), dg.get(k), sound.forms.get(k))
            else {
                continue;
            };
            let v = wave::<V>(form, *pk, offset(m)) * *gk;
            *gk = *gk + dgk;
            *pk = pk.add(inck);
            if k == from {
                (y2, y1) = (y1, v);
            }
            if let Some(out) = y.get_mut(k).and_then(|o| o.get_mut(s)) {
                *out = v.store();
            }
        }
    }
    // only the interleaved operators' state changed
    for k in lo..=hi {
        if let (Some(d), Some(v)) = (lane.ph.get_mut(k), p.get(k)) {
            *d = v.store();
        }
        if let (Some(d), Some(v)) = (lane.gain.get_mut(k), g.get(k)) {
            *d = v.store();
        }
    }
    lane.fb = [y1.store(), y2.store()];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simd::{scalar, Native};

    fn play<V: F4>(r: Routing, form: Form, layout: Layout, blocks: usize) -> Vec<f32> {
        let mut e = Engine::new(Sound::test(r, form), layout);
        for v in 0..12 {
            e.note_on(v, 55.0 * (1.0 + v as f32 * 0.37), 48000.0);
        }
        let mut all = Vec::new();
        for b in 0..blocks {
            if b == blocks / 2 {
                (0..12).for_each(|v| e.note_off(v));
            }
            let mut out = [0.0; BLOCK];
            e.render::<V>(&mut out);
            all.extend_from_slice(&out);
        }
        all
    }

    const FORMS: [Form; 3] = [Form::Sine, Form::Table, Form::Window { centre: 7 }];

    #[test]
    fn native_and_scalar_are_bit_identical() {
        for form in FORMS {
            for (r, layout) in [
                (Routing::MOST_EDGES, Layout::OperatorMajor),
                (Routing::MOST_EDGES, Layout::SampleMajor),
                (Routing::LOOP, Layout::SampleMajor),
                (Routing::LOOP, Layout::OperatorMajor),
                (Routing::CHAIN, Layout::OperatorMajor),
            ] {
                let a = play::<Native>(r, form, layout, 40);
                let b = play::<scalar::F>(r, form, layout, 40);
                assert!(
                    a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
                    "{form:?} {layout:?}"
                );
            }
        }
    }

    #[test]
    fn layouts_are_bit_identical() {
        for form in FORMS {
            for r in [Routing::MOST_EDGES, Routing::CHAIN, Routing::LOOP] {
                let a = play::<Native>(r, form, Layout::OperatorMajor, 20);
                let b = play::<Native>(r, form, Layout::SampleMajor, 20);
                assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()), "{form:?}");
            }
        }
    }

    #[test]
    fn finite_bounded_and_ends() {
        for form in FORMS {
            let mut e = Engine::new(Sound::test(Routing::MOST_EDGES, form), Layout::OperatorMajor);
            for v in 0..32 {
                e.note_on(v, 30.0 + v as f32 * 150.0, 48000.0);
            }
            let mut peak = 0.0f32;
            let mut blocks = 0;
            while e.active() > 0 && blocks < 4000 {
                if blocks == 100 {
                    (0..32).for_each(|v| e.note_off(v));
                }
                let mut out = [0.0; BLOCK];
                e.render::<Native>(&mut out);
                assert!(out.iter().all(|x| x.is_finite()));
                peak = out.iter().fold(peak, |m, x| m.max(x.abs()));
                blocks += 1;
            }
            // 32 voices of six carriers at -18 dB each: at most 32 * 6 / 8
            assert!(peak > 0.1 && peak <= 24.0, "{form:?} peak {peak}");
            assert_eq!(e.active(), 0, "{form:?}: voices did not end after their release");
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = Engine::new(Sound::test(Routing::LOOP, Form::Table), Layout::SampleMajor);
        let mut out = [0.0; BLOCK];
        e.render::<Native>(&mut out);
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn golden_render() {
        // The same bits on every target and back end: this sum of the output's bit patterns is
        // checked on all four CI targets.
        let h = FORMS.iter().fold(0u64, |h, &f| {
            play::<Native>(Routing::MOST_EDGES, f, Layout::OperatorMajor, 16)
                .iter()
                .fold(h, |h, x| h.rotate_left(5) ^ u64::from(x.to_bits()))
        });
        assert_eq!(h, GOLDEN, "render hash {h:#x}");
    }

    const GOLDEN: u64 = 0x8f2f_3f67_b77d_e16e;
}

/// Bytes one lane (four voices) touches in a block, by part.
pub struct Footprint {
    /// The lane's own state, read and written every block.
    pub lane: usize,
    /// Per-step scratch (operator outputs, modulation input, the lane's voice output), reused by
    /// every lane, so hot in the first-level cache.
    pub scratch: usize,
    /// Tables read when every operator is on `form` (shared by all lanes and instances).
    pub tables: usize,
}

pub const fn footprint(form: Form) -> Footprint {
    let table = core::mem::size_of::<Table>();
    Footprint {
        lane: core::mem::size_of::<Lane>(),
        scratch: core::mem::size_of::<[[[f32; W]; STEP]; OPS]>()
            + core::mem::size_of::<[[f32; W]; STEP]>()
            + core::mem::size_of::<[[f32; W]; BLOCK]>(),
        tables: match form {
            Form::Sine | Form::Table => table,
            Form::Window { .. } => 2 * table,
        },
    }
}

#[cfg(test)]
mod budget {
    use super::*;

    /// The first-level data cache is 32 KB: the hot set of a lane at work (its state, the scratch,
    /// and the tables of the costliest form) must leave half of it for the rest of the block path.
    #[test]
    fn hot_set_fits_half_the_l1() {
        let f = footprint(Form::Window { centre: 1 });
        assert!(
            f.lane + f.scratch + f.tables <= 16 * 1024,
            "{} B",
            f.lane + f.scratch + f.tables
        );
    }
}
