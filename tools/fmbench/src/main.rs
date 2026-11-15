// SPDX-License-Identifier: AGPL-3.0-or-later
//! fmbench: the engine's cost, timing and coverage tool.
//!
//!   fmbench time [BLOCKS]        time a block on this machine: the engine's cases at 0-64 voices
//!   fmbench check                random notes for a while: fails on a non-finite or subnormal sample
//!   fmbench coverage SOUND       the sound's parameters the engine does not play (text or binary form)
//!   fmbench engine VOICES BLOCKS ROUTING [OPS] [native|scalar] [FORMS] [SOUNDS]
//!                                VOICES held notes of a sound on ROUTING with OPS operators (default 8)
//!                                on FORMS (sine, all, odd, res, mixed: eight different tables; pitch,
//!                                mod, slfo: sines with modulators; a suffix +fT, +fx, +pan adds
//!                                a filter or pan, see `case_sound`), in
//!                                each of SOUNDS sounds (default 1); a last argument `param` sends a
//!                                parameter event every block, `ctl` a controller (CC 11); a
//!                                suffix +ctl puts every controller box to work
//!   fmbench VOICES BLOCKS [om|sm] [edges|loop|chain] [sine|table|window] [native|scalar]
//!                                the voice-kernel spike (docs/design.md, "The spike")
//!   fmbench footprint 0 x x FORM the spike's bytes a lane touches a block
//!
//! The plugin's cost through CLAP is fmhost's (src/bin/fmhost.rs), a binary of its own so that
//! linking the plugin leaves this one's code as it is.
//!
//! Under callgrind, `--toggle-collect='fmbench_render*'` counts only the rendering (tools/cost/cg.sh).
mod case;

use case::case_text;
use fm_core::coverage::not_covered;
use fm_core::engine::{Engine, Event, Kind, NoteEnd, Prepared, BLOCK, SOUNDS, VOICES};
use fm_core::simd::{scalar, Native, F4};
use fm_core::sound::Sound;
use fm_core::spike::{self, footprint, Form, Layout, Routing};
use std::time::Instant;

const RATE: f64 = 48000.0;

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmbench_render(e: &mut spike::Engine, out: &mut [f32; BLOCK], scalar: bool) {
    if scalar {
        e.render::<scalar::F>(out)
    } else {
        e.render::<Native>(out)
    }
}

// The engine's cases render as a host shell does (`render_buses`): a channel no voice wrote is left
// as it was, a right channel that is the left is not copied (the shell copies its output anyway).
#[no_mangle]
#[inline(never)]
pub extern "C" fn fmbench_render_engine(e: &mut Engine, out: &mut [[f32; BLOCK]; 2], scalar: bool) {
    let (mut end, out) = (|_: NoteEnd| {}, core::slice::from_mut(out));
    if scalar {
        e.render_buses::<scalar::F>(out, &[0; SOUNDS], BLOCK, &[], &mut end);
    } else {
        e.render_buses::<Native>(out, &[0; SOUNDS], BLOCK, &[], &mut end);
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmbench_render_event(e: &mut Engine, out: &mut [[f32; BLOCK]; 2], ev: &Event) {
    let out = core::slice::from_mut(out);
    e.render_buses::<Native>(
        out,
        &[0; SOUNDS],
        BLOCK,
        core::slice::from_ref(ev),
        &mut |_: NoteEnd| {},
    );
}

/// The cost case: the top `ops` operators at full level on `forms`, feedback 7, a slow decay 1 (so
/// every envelope is in a segment that can change: the costlier path), held notes, no portamento;
/// `pitch`: sines with both pitch envelopes moving and portamento gliding on every voice; `mod`:
/// sines with the voice LFO on pitch and amplitude (sine, delay, every operator) and level scaling
/// on every operator; `slfo`: sines with the sound LFO (triangle, delay) on every operator's
/// frequency, level and detune.
fn case_sound(routing: u32, ops: u32, forms: &str) -> Prepared {
    Prepared::new(&Sound::from_text(&case_text(routing, ops, forms)).expect("case sound"))
}

/// `voices` held notes in each of `sounds` sounds (all the same case).
fn engine(routing: u32, ops: u32, voices: usize, forms: &str, sounds: usize) -> Box<Engine> {
    let mut e = Box::new(Engine::new(RATE));
    let p = case_sound(routing, ops, forms);
    for s in 0..sounds.min(fm_core::engine::SOUNDS) {
        e.set_sound(s, &p);
        if forms.ends_with("ctl") {
            // the boxes' sources away from rest, and expression
            for (cc, v) in [(1, 90), (11, 100)] {
                e.control(s, cc, v);
            }
        }
        for v in 0..voices.min(VOICES / sounds) {
            e.note_on(s, v as u32, 36 + ((v + 7 * s) % 60) as u8, 100.0, &mut |_| {});
        }
    }
    e
}

fn time(blocks: usize) {
    println!("time a 128-frame block at 48 kHz (wall clock, microseconds), {blocks} blocks a case");
    println!(
        "{:<30} {:>6} {:>9} {:>9} {:>9} {:>9} {:>11}",
        "case", "voices", "mean", "median", "p99", "max", "mean/voice"
    );
    let cases = [
        ("eight operators, routing 12", 12, 8, "sine"),
        ("eight operators, routing 80", 80, 8, "sine"),
        ("one operator, routing 1", 1, 1, "sine"),
        ("eight resonant, routing 12", 12, 8, "res"),
        ("eight mixed forms, routing 12", 12, 8, "mixed"),
        ("pitch envelopes, portamento", 12, 8, "pitch"),
        ("voice LFO, level scaling", 12, 8, "mod"),
        ("sound LFO", 12, 8, "slfo"),
        ("filter, low-pass 24 dB", 12, 8, "sine+f0"),
        ("filter, every modulator", 12, 8, "odd+fx"),
        ("pan", 12, 8, "sine+pan"),
        ("controllers, every box", 12, 8, "odd+fx+ctl"),
    ];
    let mut rows: Vec<(String, Box<Engine>, usize)> = Vec::new();
    for (name, routing, ops, forms) in cases {
        for voices in [0usize, 1, 8, 16, 25, 32, 64] {
            rows.push((name.to_string(), engine(routing, ops, voices, forms, 1), voices));
        }
    }
    // few voices a sound (the lane gate's cases, docs/design.md): one and two voices in each of 1, 5
    // and 8 sounds, and five in one sound
    for (sounds, voices) in [(1usize, 1usize), (1, 2), (5, 1), (5, 2), (8, 1), (8, 2), (1, 5)] {
        let name = format!("routing 12, {sounds} sounds");
        rows.push((name, engine(12, 8, voices, "sine", sounds), voices * sounds));
    }
    // one voice in each of three sounds like a shared instance's parts, controller boxes at rest
    // values (case.rs's `shared_text`; the hosted gate's `shared` case without the shell)
    let mut e = Box::new(Engine::new(RATE));
    for s in 0..3 {
        let text = case::shared_text(s + 1);
        e.set_sound(s, &Prepared::new(&Sound::from_text(&text).expect("case sound")));
        e.note_on(s, s as u32, 60, 100.0, &mut |_| {});
    }
    rows.push(("shared, 3 sounds with boxes".into(), e, 3));
    for (name, mut e, voices) in rows {
        let mut t = Vec::with_capacity(blocks);
        for _ in 0..blocks {
            let mut out = [[0.0f32; BLOCK]; 2];
            let s = Instant::now();
            fmbench_render_engine(&mut e, &mut out, false);
            t.push(s.elapsed().as_secs_f64() * 1e6);
        }
        t.sort_by(f64::total_cmp);
        let mean = t.iter().sum::<f64>() / blocks as f64;
        let at = |q: f64| t[((blocks as f64 - 1.0) * q) as usize];
        println!(
            "{name:<30} {voices:>6} {mean:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>11.2}",
            at(0.5),
            at(0.99),
            t[blocks - 1],
            mean / voices.max(1) as f64
        );
    }
}

fn check() -> bool {
    let mut seed = 0x1234_5678u32;
    let mut r = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut e = Box::new(Engine::new(RATE));
    for (s, (routing, ops, forms)) in [(12, 8, "mixed"), (80, 5, "sine+f0+ctl"), (5, 2, "res")]
        .into_iter()
        .enumerate()
    {
        e.set_sound(s, &case_sound(routing, ops, forms));
    }
    let (mut notes, mut ended, mut bad) = (0u32, 0u32, 0usize);
    for _ in 0..20_000 {
        for _ in 0..r() % 4 {
            if r() % 3 == 0 && notes > 0 {
                e.note_off((r() % 3) as usize, r() % notes, &mut |_| ended += 1);
            } else {
                notes += 1;
                e.note_on(
                    (r() % 3) as usize,
                    notes - 1,
                    (r() % 128) as u8,
                    (1 + r() % 127) as f32,
                    &mut |_| ended += 1,
                );
            }
        }
        // the controllers, the sostenuto pedal among them, at any value
        if r() % 4 == 0 {
            let cc = [1, 2, 4, 11, 66, fm_core::engine::PRESSURE][r() as usize % 6];
            e.control((r() % 3) as usize, cc, (r() % 128) as u8);
        }
        let mut out = [[0.0f32; BLOCK]; 2];
        e.render::<Native>(&mut out, &mut |_| ended += 1);
        bad += out
            .as_flattened()
            .iter()
            .filter(|x| !x.is_finite() || (x.abs() < f32::MIN_POSITIVE && **x != 0.0))
            .count();
    }
    println!(
        "check: {notes} notes, {ended} ended, {} sounding, {bad} non-finite or subnormal samples",
        e.active()
    );
    bad == 0
}

fn coverage(path: &str) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            return false;
        }
    };
    let sound = match Sound::from_blob(&bytes) {
        Ok(s) => s,
        Err(_) => match Sound::from_text(&String::from_utf8_lossy(&bytes)) {
            Ok(s) => s,
            Err((line, e)) => {
                eprintln!("{path}: line {line}: {e:?}");
                return false;
            }
        },
    };
    let mut n = 0;
    not_covered(&sound, |p, v, why| {
        let op = p.op.map(|o| format!("op{}.", o + 1)).unwrap_or_default();
        println!("not covered: {}={v}: {op}{}, {why}", p.id, p.name);
        n += 1;
    });
    println!("{n} not covered");
    true
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize, d: &str| a.get(i).cloned().unwrap_or_else(|| d.to_string());
    let num = |i: usize, d: usize| arg(i, "").parse().unwrap_or(d);
    match arg(0, "").as_str() {
        "time" => return time(num(1, 2000)),
        "check" => std::process::exit(if check() { 0 } else { 1 }),
        "coverage" => std::process::exit(if coverage(&arg(1, "")) { 0 } else { 1 }),
        "engine" => {
            let (voices, blocks) = (num(1, 8), num(2, 100));
            let sounds = num(7, 1).clamp(1, fm_core::engine::SOUNDS);
            let mut e = engine(num(3, 12) as u32, num(4, 8) as u32, voices, &arg(6, "sine"), sounds);
            let scalar = arg(5, "native") == "scalar";
            let voices = voices * sounds;
            let mut sum = 0.0f64;
            let t = Instant::now();
            // "param": a parameter event every block (the voice LFO's speed, so the sound is prepared
            // again and its plan stays)
            // "ctl": foot controller 1 and expression moving every block (the boxes evaluated again)
            let (param, ctl) = (arg(8, "") == "param", arg(8, "") == "ctl");
            for b in 0..blocks {
                let mut out = [[0.0f32; BLOCK]; 2];
                if param || ctl {
                    let kind = if ctl {
                        Kind::Control {
                            cc: 11,
                            value: 100 + (b % 2) as u8,
                        }
                    } else {
                        Kind::Param {
                            id: 23,
                            value: 30 + (b % 2) as i16,
                        }
                    };
                    fmbench_render_event(
                        &mut e,
                        &mut out,
                        &Event {
                            frame: 0,
                            sound: 0,
                            kind,
                        },
                    );
                } else {
                    fmbench_render_engine(&mut e, &mut out, scalar);
                }
                sum += out.as_flattened().iter().map(|x| f64::from(x.abs())).sum::<f64>();
            }
            let us = t.elapsed().as_secs_f64() * 1e6 / (blocks * voices.max(1)) as f64;
            println!(
                "engine voices {voices} blocks {blocks}: {us:.2} us a voice-block, {} sounding, checksum {sum:.6}",
                e.active()
            );
            return;
        }
        _ => {}
    }
    let form = match arg(4, "table").as_str() {
        "sine" => Form::Sine,
        "window" => Form::Window { centre: 7 },
        _ => Form::Table,
    };
    if arg(0, "") == "footprint" {
        let f = footprint(form);
        println!(
            "lane state {} B, scratch {} B, tables {} B, per voice {} B",
            f.lane,
            f.scratch,
            f.tables,
            f.lane / 4
        );
        return;
    }
    let (voices, blocks) = (num(0, 8), num(1, 100));
    let layout = if arg(2, "om") == "sm" {
        Layout::SampleMajor
    } else {
        Layout::OperatorMajor
    };
    let routing = match arg(3, "edges").as_str() {
        "loop" => Routing::LOOP,
        "chain" => Routing::CHAIN,
        _ => Routing::MOST_EDGES,
    };
    let scalar = arg(5, "native") == "scalar";
    let mut sound = spike::Sound::test(routing, form);
    sound.release = [0.0; 8]; // held for the whole run
    let mut e = Box::new(spike::Engine::new(sound, layout));
    for v in 0..voices {
        e.note_on(v, 60.0 + 23.0 * v as f32, 48000.0);
    }
    let mut out = [0.0f32; BLOCK];
    let t = Instant::now();
    let mut sum = 0.0f64;
    for _ in 0..blocks {
        out.fill(0.0);
        fmbench_render(&mut e, &mut out, scalar);
        sum += out.iter().map(|x| f64::from(x.abs())).sum::<f64>();
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / (blocks * voices.max(1)) as f64;
    println!(
        "voices {voices} blocks {blocks} {:?} {} {:?} {}: {us:.2} us a voice-block, checksum {sum:.6}",
        layout,
        arg(3, "edges"),
        form,
        if scalar { "scalar" } else { "native" }
    );
    let _ = Native::splat(0.0); // the native type is linked whatever the arguments
}
