// SPDX-License-Identifier: AGPL-3.0-or-later
//! fmbench: the engine's cost, timing and coverage tool.
//!
//!   fmbench time [BLOCKS]        time a block on this machine: the engine's cases at 0-64 voices
//!   fmbench check                random notes for a while: fails on a non-finite or subnormal sample
//!   fmbench coverage SOUND       the sound's parameters the engine does not play (text or binary form)
//!   fmbench engine VOICES BLOCKS ROUTING [OPS] [native|scalar]
//!                                VOICES held notes of a sound on ROUTING with OPS operators (default 8)
//!   fmbench VOICES BLOCKS [om|sm] [edges|loop|chain] [sine|table|window] [native|scalar]
//!                                the voice-kernel spike (docs/design.md, "The spike")
//!   fmbench footprint 0 x x FORM the spike's bytes a lane touches a block
//!
//! Under callgrind, `--toggle-collect='fmbench_render*'` counts only the rendering (tools/cost/cg.sh).
use fm_core::coverage::not_covered;
use fm_core::engine::{Engine, NoteEnd, Prepared, BLOCK, VOICES};
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

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmbench_render_engine(e: &mut Engine, out: &mut [f32; BLOCK], scalar: bool) {
    let mut end = |_: NoteEnd| {};
    if scalar {
        e.render::<scalar::F>(out, &mut end)
    } else {
        e.render::<Native>(out, &mut end)
    }
}

/// The cost case: the top `ops` operators at full level, feedback 7, a slow decay 1 (so every
/// envelope is in a segment that can change: the costlier path), held notes.
fn case_sound(routing: u32, ops: u32) -> Prepared {
    let mut t = format!("fm-sound/1\nalgorithm={routing} feedback=7\n");
    for o in 1..=8 {
        let level = if o > 8 - ops { 99 } else { 0 };
        t += &format!(
            "op{o}.level={level} op{o}.coarse={} op{o}.eg_time_decay1=60 op{o}.eg_level_decay1=0\n",
            o % 4
        );
    }
    Prepared::new(&Sound::from_text(&t).expect("case sound"))
}

fn engine(routing: u32, ops: u32, voices: usize) -> Box<Engine> {
    let mut e = Box::new(Engine::new(RATE));
    e.set_sound(0, &case_sound(routing, ops));
    for v in 0..voices.min(VOICES) {
        e.note_on(0, v as u32, 36 + (v % 60) as u8, 100.0, &mut |_| {});
    }
    e
}

fn time(blocks: usize) {
    println!("time a 128-frame block at 48 kHz (wall clock, microseconds), {blocks} blocks a case");
    println!(
        "{:<28} {:>6} {:>9} {:>9} {:>9} {:>9} {:>11}",
        "case", "voices", "mean", "median", "p99", "max", "mean/voice"
    );
    let cases = [
        ("eight operators, routing 12", 12, 8),
        ("eight operators, routing 80", 80, 8),
        ("one operator, routing 1", 1, 1),
    ];
    for (name, routing, ops) in cases {
        for voices in [0usize, 1, 8, 16, 25, 32, 64] {
            let mut e = engine(routing, ops, voices);
            let mut out = [0.0f32; BLOCK];
            let mut t = Vec::with_capacity(blocks);
            for _ in 0..blocks {
                out.fill(0.0);
                let s = Instant::now();
                fmbench_render_engine(&mut e, &mut out, false);
                t.push(s.elapsed().as_secs_f64() * 1e6);
            }
            t.sort_by(f64::total_cmp);
            let mean = t.iter().sum::<f64>() / blocks as f64;
            let at = |q: f64| t[((blocks as f64 - 1.0) * q) as usize];
            println!(
                "{name:<28} {voices:>6} {mean:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>11.2}",
                at(0.5),
                at(0.99),
                t[blocks - 1],
                mean / voices.max(1) as f64
            );
        }
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
    for (s, (routing, ops)) in [(12, 8), (80, 5), (5, 2)].into_iter().enumerate() {
        e.set_sound(s, &case_sound(routing, ops));
    }
    let (mut notes, mut ended, mut bad) = (0u32, 0u32, 0usize);
    for _ in 0..20_000 {
        for _ in 0..r() % 4 {
            if r() % 3 == 0 && notes > 0 {
                e.note_off((r() % 3) as usize, r() % notes);
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
        let mut out = [0.0f32; BLOCK];
        e.render::<Native>(&mut out, &mut |_| ended += 1);
        bad += out
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
            let mut e = engine(num(3, 12) as u32, num(4, 8) as u32, voices);
            let scalar = arg(5, "native") == "scalar";
            let mut out = [0.0f32; BLOCK];
            let mut sum = 0.0f64;
            let t = Instant::now();
            for _ in 0..blocks {
                out.fill(0.0);
                fmbench_render_engine(&mut e, &mut out, scalar);
                sum += out.iter().map(|x| f64::from(x.abs())).sum::<f64>();
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
